//! Click'n'Load: link sites (file hosts' folders, link protectors) hand a
//! whole package of links to a download manager listening on
//! `127.0.0.1:9666`, the port JDownloader made the convention.
//!
//! What a page does, and what the app answers (the public protocol):
//! - `GET /jdcheck.js` → `jdownloader=true;` (the page shows its button).
//! - `GET /`, `GET /flash` → `JDownloader` (older pages check this).
//! - `POST /flash/add` with `urls` (one per line), and optionally `package`,
//!   `passwords`, `source`.
//! - `POST /flash/addcrypted2` with `crypted` (base64 of AES-128-CBC, key
//!   and IV the same 16 bytes) and `jk`, a tiny JavaScript function that
//!   returns that key in hex.
//!
//! Nothing downloads by itself: the links open in the "Download all links"
//! window, as a package (its own folder, its archive passwords saved for
//! extracting), for the user to check and start. Only this computer can
//! reach the port, posts are capped in size and in number per minute, and a
//! page cannot name another site: the site shown is the browser's `Origin` /
//! `Referer`. Written from the protocol's public description.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PORT: u16 = 9666;
/// Posts allowed per minute (all sites together).
const POSTS_PER_MINUTE: usize = 10;
/// The biggest post read (a package of a few thousand links fits).
const MAX_BODY: usize = 1 << 20;

/// A package of links a page handed over.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Package {
    pub name: String,
    pub urls: Vec<String>,
    pub passwords: Vec<String>,
}

/// `function f(){ return '31323334…'; }` → the 16 key bytes (the first run
/// of 32 hex digits the function returns).
pub fn key_of(jk: &str) -> Option<[u8; 16]> {
    let b = jk.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_hexdigit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let run = b[i..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
            if run == 32 {
                let mut key = [0u8; 16];
                for (k, pair) in key.iter_mut().zip(jk[i..i + 32].as_bytes().chunks(2)) {
                    *k = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
                }
                return Some(key);
            }
            i += run.max(1);
        } else {
            i += 1;
        }
    }
    None
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    }
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len().saturating_sub(1)]);
    }
    Some(out)
}

/// The links in an `addcrypted2` post.
pub fn decrypt(crypted: &str, jk: &str) -> Option<Vec<String>> {
    let key = key_of(jk)?;
    let data = b64_decode(crypted)?;
    let plain = crate::aes128::cbc_decrypt(&key, &key, &data).ok()?;
    // Padded with zero bytes (or PKCS#7 by some pages): both are cut.
    let text = String::from_utf8_lossy(&plain).into_owned();
    Some(lines(text.trim_end_matches(|c: char| c == '\0' || (c as u32) < 0x11)))
}

/// The http(s) links in a list, one per line, no repeats.
fn lines(s: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    s.split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| crate::model::is_download_url(l) && l.len() <= 4096)
        .filter(|l| seen.insert(l.to_string()))
        .map(str::to_string)
        .collect()
}

fn form(body: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

/// A package from a post to `/flash/<what>`.
pub fn read(what: &str, body: &[u8]) -> Result<Package, &'static str> {
    let f = form(body);
    let urls = match what {
        "add" => lines(f.get("urls").map(String::as_str).unwrap_or("")),
        "addcrypted2" => {
            let (Some(c), Some(jk)) = (f.get("crypted"), f.get("jk")) else { return Err("crypted and jk are needed") };
            decrypt(c, jk).ok_or("the links could not be decrypted")?
        }
        _ => return Err("unknown request"),
    };
    if urls.is_empty() {
        return Err("no links");
    }
    let passwords = f
        .get("passwords")
        .map(|p| p.split(['\r', '\n']).map(str::trim).filter(|p| !p.is_empty() && p.len() <= 200).map(str::to_string).collect())
        .unwrap_or_default();
    let name = f
        .get("package")
        .map(|n| n.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string())
        .unwrap_or_default();
    Ok(Package { name, urls, passwords })
}

/// The site a post came from, as the browser names it.
fn site(headers: &axum::http::HeaderMap) -> String {
    for h in ["origin", "referer"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            if let Ok(u) = url::Url::parse(v) {
                if let Some(host) = u.host_str() {
                    return format!("{}://{host}/", u.scheme());
                }
            }
        }
    }
    String::new()
}

/// Posts in the last minute.
static RECENT: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

fn allow_post(now: Instant) -> bool {
    let Ok(mut r) = RECENT.lock() else { return false };
    r.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if r.len() >= POSTS_PER_MINUTE {
        return false;
    }
    r.push(now);
    true
}

/// Open the package in the "Download all links" window.
fn hand_over(site: &str, pkg: Package) -> Result<(), String> {
    let links: Vec<crate::native_host::PageLink> =
        pkg.urls.iter().map(|u| crate::native_host::PageLink { url: u.clone(), text: String::new() }).collect();
    let mut batch = crate::native_host::links_batch(site, &pkg.name, "", &links);
    batch.package = pkg.name;
    batch.passwords = pkg.passwords;
    crate::native_host::open_links_batch(&batch)
}

fn text(body: &'static str, kind: &'static str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [
            (axum::http::header::CONTENT_TYPE, kind),
            // Pages ask with scripts too (fetch / XHR).
            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type"),
        ],
        body,
    )
        .into_response()
}

fn router() -> axum::Router {
    use axum::routing::{get, post};
    let posted = |what: &'static str| {
        move |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
            if body.len() > MAX_BODY {
                return text("failed too big\r\n", "text/plain");
            }
            if !allow_post(Instant::now()) {
                return text("failed too many\r\n", "text/plain");
            }
            let from = site(&headers);
            match read(what, &body) {
                Ok(pkg) => match tokio::task::spawn_blocking(move || hand_over(&from, pkg)).await {
                    Ok(Ok(())) => text("success\r\n", "text/plain"),
                    _ => text("failed\r\n", "text/plain"),
                },
                Err(_) => text("failed\r\n", "text/plain"),
            }
        }
    };
    let options = || async { text("", "text/plain") };
    axum::Router::new()
        .route("/", get(|| async { text("JDownloader\r\n", "text/plain") }))
        .route("/flash", get(|| async { text("JDownloader\r\n", "text/plain") }))
        .route("/jdcheck.js", get(|| async { text("jdownloader=true;\r\nvar version='download-craft';\r\n", "text/javascript") }))
        .route(
            "/crossdomain.xml",
            get(|| async {
                text(
                    "<?xml version=\"1.0\"?>\r\n<!DOCTYPE cross-domain-policy SYSTEM \"http://www.macromedia.com/xml/dtds/cross-domain-policy.dtd\">\r\n<cross-domain-policy>\r\n<allow-access-from domain=\"*\" />\r\n</cross-domain-policy>\r\n",
                    "text/xml",
                )
            }),
        )
        .route("/flash/add", post(posted("add")).options(options))
        .route("/flash/addcrypted2", post(posted("addcrypted2")).options(options))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
}

/// Whether Click'n'Load is on (Settings → Browser Integration; on by default).
pub fn enabled() -> bool {
    crate::model::ui_pref_bool("click_n_load", true)
}

/// Start listening, on a thread of its own, for the life of the app. The
/// port may be taken (JDownloader runs): then it says so and stays off.
pub fn start() -> Result<(), String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", PORT)).map_err(|e| format!("port {PORT} is in use ({e})"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("click-n-load".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async move {
                let Ok(l) = tokio::net::TcpListener::from_std(listener) else { return };
                let _ = axum::serve(l, router()).await;
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in data.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    fn crypted(key: &[u8; 16], text: &str) -> String {
        let mut data = text.as_bytes().to_vec();
        while data.len() % 16 != 0 {
            data.push(0);
        }
        b64(&crate::aes128::cbc_encrypt(key, key, &data))
    }

    #[test]
    fn the_key_comes_out_of_the_function() {
        let k = key_of("function f(){ return '31323334353637383930616263646566';}").unwrap();
        assert_eq!(&k, b"1234567890abcdef");
        assert!(key_of("function f(){ return 'abc';}").is_none());
        // A longer hex run is not a key.
        assert!(key_of("return '3132333435363738393061626364656666';").is_none());
    }

    #[test]
    fn crypted_links_open() {
        let key = *b"1234567890abcdef";
        let c = crypted(&key, "https://files.example/a.part1.rar\r\nhttps://files.example/a.part2.rar\r\n");
        let jk = "function f(){ return '31323334353637383930616263646566';}";
        assert_eq!(decrypt(&c, jk).unwrap(), vec!["https://files.example/a.part1.rar", "https://files.example/a.part2.rar"]);
        // The wrong key gives no links, not garbage.
        let wrong = "function f(){ return '00000000000000000000000000000000';}";
        assert!(decrypt(&c, wrong).map(|v| v.is_empty()).unwrap_or(true));
    }

    #[test]
    fn a_plain_post_is_a_package() {
        let body = b"urls=https%3A%2F%2Fa.example%2F1.zip%0D%0Ajavascript%3Aalert(1)%0D%0Ahttps%3A%2F%2Fa.example%2F1.zip%0D%0Ahttps%3A%2F%2Fa.example%2F2.zip&package=My+Film&passwords=secret%0D%0Aother";
        let p = read("add", body).unwrap();
        assert_eq!(p.urls, vec!["https://a.example/1.zip", "https://a.example/2.zip"]);
        assert_eq!(p.name, "My Film");
        assert_eq!(p.passwords, vec!["secret", "other"]);
        assert!(read("add", b"urls=").is_err());
        assert!(read("addcrypted2", b"crypted=abc").is_err());
    }

    #[test]
    fn posts_are_limited_per_minute() {
        let t = Instant::now() + Duration::from_secs(3600);
        RECENT.lock().unwrap().clear();
        for _ in 0..POSTS_PER_MINUTE {
            assert!(allow_post(t));
        }
        assert!(!allow_post(t));
        assert!(allow_post(t + Duration::from_secs(61)));
        RECENT.lock().unwrap().clear();
    }
}
