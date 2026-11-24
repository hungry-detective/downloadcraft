//! File hosts whose links are web pages, with the real file behind them:
//! Mediafire, Pixeldrain and Mega (one file each), and folders on Google
//! Drive, Pixeldrain and Mediafire (many files, opened in Download all
//! links). Written from each site's public pages and public API.
//!
//! - Mediafire: the file page's Download button holds the file's address
//!   (sometimes base64-scrambled in `data-scrambled-url`). Folders: the
//!   public `folder/get_content` API.
//! - Pixeldrain: `/u/<id>` is served by `/api/file/<id>?download`, its name
//!   by `/api/file/<id>/info`; lists `/l/<id>` by `/api/list/<id>`.
//! - Mega: the link names the file and its key (`/file/<handle>#<key>`).
//!   The public API (`a: g`) gives a download address and the file's
//!   encrypted name; the file is stored AES-128-CTR encrypted and is
//!   decrypted once downloaded.
//! - Google Drive folders: the folder's embedded view lists its files.

use anyhow::{anyhow, Context, Result};

const PAGE_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

fn host(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default()
}

fn on(url: &str, site: &str) -> bool {
    let h = host(url);
    h == site || h.ends_with(&format!(".{site}"))
}

fn path_segs(url: &str) -> Vec<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.path_segments().map(|s| s.filter(|x| !x.is_empty()).map(str::to_string).collect()))
        .unwrap_or_default()
}

/// What a link is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    MediafireFile,
    MediafireFolder(String),
    PixeldrainFile(String),
    PixeldrainList(String),
    MegaFile { handle: String, key: String },
    DriveFolder(String),
}

pub fn kind(url: &str) -> Option<Kind> {
    let segs = path_segs(url);
    let seg = |i: usize| segs.get(i).cloned().unwrap_or_default();
    let id_ok = |s: &str| !s.is_empty() && s.len() <= 100 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if on(url, "mediafire.com") {
        return match seg(0).as_str() {
            "file" | "file_premium" | "view" if id_ok(&seg(1)) => Some(Kind::MediafireFile),
            "folder" if id_ok(&seg(1)) => Some(Kind::MediafireFolder(seg(1))),
            _ => None,
        };
    }
    if on(url, "pixeldrain.com") {
        return match seg(0).as_str() {
            "u" if id_ok(&seg(1)) => Some(Kind::PixeldrainFile(seg(1))),
            "l" if id_ok(&seg(1)) => Some(Kind::PixeldrainList(seg(1))),
            _ => None,
        };
    }
    if on(url, "mega.nz") || on(url, "mega.co.nz") || on(url, "mega.io") {
        let u = url::Url::parse(url).ok()?;
        let frag = u.fragment().unwrap_or("");
        // /file/<handle>#<key>, or the old #!<handle>!<key>.
        if seg(0) == "file" && id_ok(&seg(1)) && !frag.is_empty() {
            return Some(Kind::MegaFile { handle: seg(1), key: frag.to_string() });
        }
        if let Some(rest) = frag.strip_prefix('!') {
            let (h, k) = rest.split_once('!')?;
            return Some(Kind::MegaFile { handle: h.to_string(), key: k.to_string() });
        }
        return None;
    }
    if on(url, "drive.google.com") {
        if let Some(i) = segs.iter().position(|s| s == "folders") {
            let id = seg(i + 1);
            if id_ok(&id) {
                return Some(Kind::DriveFolder(id));
            }
        }
    }
    None
}

/// One file behind the link (not a folder).
pub fn is_file_page(url: &str) -> bool {
    matches!(kind(url), Some(Kind::MediafireFile | Kind::PixeldrainFile(_) | Kind::MegaFile { .. }))
}

/// A folder or list: many files.
pub fn is_folder(url: &str) -> bool {
    matches!(kind(url), Some(Kind::MediafireFolder(_) | Kind::PixeldrainList(_) | Kind::DriveFolder(_)))
}

/// A file behind a page: where to download it, its name and size when the
/// site says, and Mega's key (the data arrives encrypted).
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub url: String,
    pub name: Option<String>,
    pub size: Option<u64>,
    pub mega_key: Option<[u8; 32]>,
}

pub async fn resolve(client: &reqwest::Client, url: &str) -> Result<Resolved> {
    match kind(url) {
        Some(Kind::MediafireFile) => mediafire_file(client, url).await,
        Some(Kind::PixeldrainFile(id)) => pixeldrain_file(client, &id).await,
        Some(Kind::MegaFile { handle, key }) => mega_file(client, &handle, &key).await,
        _ => Err(anyhow!("not a file page this app knows")),
    }
}

/// The files in a folder link: (address, name).
pub async fn list(client: &reqwest::Client, url: &str) -> Result<Vec<(String, String)>> {
    match kind(url) {
        Some(Kind::DriveFolder(id)) => drive_folder(client, &id).await,
        Some(Kind::PixeldrainList(id)) => pixeldrain_list(client, &id).await,
        Some(Kind::MediafireFolder(key)) => mediafire_folder(client, &key).await,
        _ => Err(anyhow!("not a folder this app knows")),
    }
}

async fn page(client: &reqwest::Client, url: &str) -> Result<String> {
    let r = client.get(url).header(reqwest::header::USER_AGENT, PAGE_AGENT).send().await?;
    if !r.status().is_success() {
        return Err(anyhow!("the site answered HTTP {}", r.status().as_u16()));
    }
    Ok(r.text().await?)
}

// ---------------- Mediafire ----------------

/// The Download button's address on a Mediafire file page.
pub fn mediafire_button(html: &str) -> Option<String> {
    let at = html.find("id=\"downloadButton\"")?;
    let start = html[..at].rfind('<')?;
    let end = at + html[at..].find('>')?;
    let tag = &html[start..end];
    let attr = |name: &str| {
        let k = format!("{name}=\"");
        let i = tag.find(&k)? + k.len();
        Some(tag[i..i + tag[i..].find('"')?].to_string())
    };
    if let Some(h) = attr("href").filter(|h| h.starts_with("http")) {
        return Some(h.replace("&amp;", "&"));
    }
    let scrambled = attr("data-scrambled-url")?;
    let plain = crate::cnl::b64_decode(&scrambled)?;
    String::from_utf8(plain).ok().filter(|h| h.starts_with("http"))
}

async fn mediafire_file(client: &reqwest::Client, url: &str) -> Result<Resolved> {
    let html = page(client, url).await.context("Mediafire")?;
    let direct = mediafire_button(&html).ok_or_else(|| anyhow!("Mediafire: the page has no download button (removed, or private)"))?;
    let name = direct.rsplit('/').next().map(|n| crate::engine::sanitize_filename(&percent(n)));
    Ok(Resolved { url: direct, name, ..Default::default() })
}

async fn mediafire_folder(client: &reqwest::Client, key: &str) -> Result<Vec<(String, String)>> {
    let api = format!(
        "https://www.mediafire.com/api/1.5/folder/get_content.php?folder_key={key}&content_type=files&response_format=json&chunk_size=1000"
    );
    let v: serde_json::Value = client.get(api).send().await?.json().await.context("Mediafire folder")?;
    let files = v.pointer("/response/folder_content/files").and_then(|f| f.as_array()).cloned().unwrap_or_default();
    Ok(files
        .iter()
        .filter_map(|f| {
            let name = f.get("filename")?.as_str()?.to_string();
            let q = f.get("quickkey")?.as_str()?;
            Some((format!("https://www.mediafire.com/file/{q}/{}/file", urlish(&name)), name))
        })
        .collect())
}

// ---------------- Pixeldrain ----------------

async fn pixeldrain_file(client: &reqwest::Client, id: &str) -> Result<Resolved> {
    let info: serde_json::Value = client
        .get(format!("https://pixeldrain.com/api/file/{id}/info"))
        .send()
        .await?
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);
    Ok(Resolved {
        url: format!("https://pixeldrain.com/api/file/{id}?download"),
        name: info.get("name").and_then(|n| n.as_str()).map(crate::engine::sanitize_filename),
        size: info.get("size").and_then(|n| n.as_u64()),
        mega_key: None,
    })
}

async fn pixeldrain_list(client: &reqwest::Client, id: &str) -> Result<Vec<(String, String)>> {
    let v: serde_json::Value = client.get(format!("https://pixeldrain.com/api/list/{id}")).send().await?.json().await.context("Pixeldrain list")?;
    Ok(v.get("files")
        .and_then(|f| f.as_array())
        .into_iter()
        .flatten()
        .filter_map(|f| Some((format!("https://pixeldrain.com/u/{}", f.get("id")?.as_str()?), f.get("name")?.as_str()?.to_string())))
        .collect())
}

// ---------------- Google Drive folders ----------------

/// The files in a Drive folder's embedded view: (file id, name).
pub fn drive_entries(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("id=\"entry-") {
        rest = &rest[at + "id=\"entry-".len()..];
        let Some(end) = rest.find('"') else { break };
        let id = rest[..end].to_string();
        let block_end = rest.find("id=\"entry-").unwrap_or(rest.len());
        let block = &rest[..block_end];
        // Sub-folders link to another folder view: not a file.
        if block.contains("/drive/folders/") || block.contains("folderview?id=") {
            continue;
        }
        let name = block
            .find("flip-entry-title\">")
            .map(|i| &block[i + "flip-entry-title\">".len()..])
            .and_then(|t| t.find('<').map(|j| t[..j].trim().to_string()))
            .map(|n| html_unescape(&n))
            .unwrap_or_default();
        if !id.is_empty() && !name.is_empty() {
            out.push((id, name));
        }
    }
    out
}

async fn drive_folder(client: &reqwest::Client, id: &str) -> Result<Vec<(String, String)>> {
    let html = page(client, &format!("https://drive.google.com/embeddedfolderview?id={id}")).await.context("Google Drive folder")?;
    let files = drive_entries(&html);
    if files.is_empty() {
        return Err(anyhow!("Google Drive: the folder is empty or not shared with everyone who has the link"));
    }
    Ok(files.into_iter().map(|(fid, name)| (format!("https://drive.google.com/file/d/{fid}/view"), name)).collect())
}

// ---------------- Mega ----------------

/// A Mega key from the link (base64url, 32 bytes for a file).
fn mega_key(key: &str) -> Option<[u8; 32]> {
    let bytes = crate::cnl::b64_decode(key)?;
    bytes.try_into().ok()
}

/// The file's AES key and the counter's start: the 32-byte link key holds
/// both (key = first half XOR second half; nonce = bytes 16..24).
pub fn mega_aes(k: &[u8; 32]) -> ([u8; 16], [u8; 8]) {
    let mut key = [0u8; 16];
    for i in 0..16 {
        key[i] = k[i] ^ k[i + 16];
    }
    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&k[16..24]);
    (key, nonce)
}

/// The file name in Mega's encrypted attributes (`MEGA{"n":"…"}`).
pub fn mega_name(at: &str, key: &[u8; 16]) -> Option<String> {
    let data = crate::cnl::b64_decode(at)?;
    let plain = crate::aes128::cbc_decrypt(key, &[0u8; 16], &data).ok()?;
    let text = String::from_utf8_lossy(&plain);
    let json = text.strip_prefix("MEGA")?.trim_end_matches('\0');
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("n").and_then(|n| n.as_str()).map(crate::engine::sanitize_filename)
}

async fn mega_file(client: &reqwest::Client, handle: &str, key: &str) -> Result<Resolved> {
    let k = mega_key(key).ok_or_else(|| anyhow!("Mega: the link's key is incomplete (copy the whole link, with the part after #)"))?;
    let seq: u32 = rand::random();
    let answer: serde_json::Value = client
        .post(format!("https://g.api.mega.co.nz/cs?id={seq}"))
        .body(serde_json::json!([{ "a": "g", "g": 1, "ssl": 2, "p": handle }]).to_string())
        .send()
        .await?
        .json()
        .await
        .context("Mega")?;
    let first = answer.get(0).cloned().unwrap_or(answer.clone());
    if let Some(code) = first.as_i64() {
        return Err(anyhow!(match code {
            -9 | -2 => "Mega: the file is not there any more".to_string(),
            -16 => "Mega: the file was taken down".to_string(),
            -18 => "Mega: busy, try again in a moment".to_string(),
            c => format!("Mega refused the link (error {c})"),
        }));
    }
    let direct = first.get("g").and_then(|g| g.as_str()).ok_or_else(|| anyhow!("Mega: no download address (over the transfer quota?)"))?;
    let (aes, _) = mega_aes(&k);
    Ok(Resolved {
        url: direct.to_string(),
        name: first.get("at").and_then(|a| a.as_str()).and_then(|a| mega_name(a, &aes)),
        size: first.get("s").and_then(|s| s.as_u64()),
        mega_key: Some(k),
    })
}

/// Decrypt a downloaded Mega file in place (AES-128-CTR).
pub fn mega_decrypt_file(path: &std::path::Path, k: &[u8; 32]) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let (key, nonce) = mega_aes(k);
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut pos: u64 = 0;
    loop {
        f.seek(SeekFrom::Start(pos))?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctr_apply(&key, &nonce, pos, &mut buf[..n]);
        f.seek(SeekFrom::Start(pos))?;
        f.write_all(&buf[..n])?;
        pos += n as u64;
    }
    f.flush()?;
    Ok(())
}

/// XOR `data` (which starts at byte `offset` of the file) with the CTR
/// key stream: counter block = nonce (8 bytes) + block number (8, big endian).
pub fn ctr_apply(key: &[u8; 16], nonce: &[u8; 8], offset: u64, data: &mut [u8]) {
    let mut block_no = offset / 16;
    let mut skip = (offset % 16) as usize;
    let mut i = 0;
    while i < data.len() {
        let mut ctr = [0u8; 16];
        ctr[..8].copy_from_slice(nonce);
        ctr[8..].copy_from_slice(&block_no.to_be_bytes());
        let stream = crate::aes128::encrypt_block(key, &ctr);
        while skip < 16 && i < data.len() {
            data[i] ^= stream[skip];
            i += 1;
            skip += 1;
        }
        skip = 0;
        block_no += 1;
    }
}

fn percent(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

fn urlish(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_told_apart() {
        assert_eq!(kind("https://www.mediafire.com/file/abc123xyz/song.mp3/file"), Some(Kind::MediafireFile));
        assert_eq!(kind("https://www.mediafire.com/folder/k3y9/Music"), Some(Kind::MediafireFolder("k3y9".into())));
        assert_eq!(kind("https://pixeldrain.com/u/AbCd1234"), Some(Kind::PixeldrainFile("AbCd1234".into())));
        assert_eq!(kind("https://pixeldrain.com/l/List5678"), Some(Kind::PixeldrainList("List5678".into())));
        assert_eq!(kind("https://drive.google.com/drive/folders/1AbCdEfGhIjKlMn"), Some(Kind::DriveFolder("1AbCdEfGhIjKlMn".into())));
        assert_eq!(
            kind("https://mega.nz/file/HnBQ3Zz#Kj0x-1abc_DEF"),
            Some(Kind::MegaFile { handle: "HnBQ3Zz".into(), key: "Kj0x-1abc_DEF".into() })
        );
        assert_eq!(kind("https://mega.nz/#!HnBQ3Zz!Kj0x"), Some(Kind::MegaFile { handle: "HnBQ3Zz".into(), key: "Kj0x".into() }));
        assert_eq!(kind("https://mega.nz/file/HnBQ3Zz"), None, "no key, no file");
        assert_eq!(kind("https://example.com/file/x"), None);
        assert!(is_folder("https://pixeldrain.com/l/List5678") && !is_folder("https://pixeldrain.com/u/x1"));
    }

    #[test]
    fn mediafire_button_plain_and_scrambled() {
        let plain = r#"<a class="input popsok" aria-label="Download file" href="https://download1.mediafire.com/abc/song.mp3" id="downloadButton">"#;
        assert_eq!(mediafire_button(plain).as_deref(), Some("https://download1.mediafire.com/abc/song.mp3"));
        // base64 of "https://download2.mediafire.com/x/y.zip"
        let scrambled = r##"<a href="#" data-scrambled-url="aHR0cHM6Ly9kb3dubG9hZDIubWVkaWFmaXJlLmNvbS94L3kuemlw" id="downloadButton">"##;
        assert_eq!(mediafire_button(scrambled).as_deref(), Some("https://download2.mediafire.com/x/y.zip"));
    }

    #[test]
    fn drive_folder_entries() {
        let html = r#"<div class="flip-entry" id="entry-1aaaaaaaaaaa"><a href="https://drive.google.com/file/d/1aaaaaaaaaaa/view"><div class="flip-entry-title">Song &amp; Dance.mp3</div></a></div>
        <div class="flip-entry" id="entry-1fffffffffff"><a href="https://drive.google.com/drive/folders/1fffffffffff"><div class="flip-entry-title">Sub</div></a></div>
        <div class="flip-entry" id="entry-1bbbbbbbbbbb"><a href="https://drive.google.com/file/d/1bbbbbbbbbbb/view"><div class="flip-entry-title">Notes.pdf</div></a></div>"#;
        assert_eq!(
            drive_entries(html),
            vec![("1aaaaaaaaaaa".to_string(), "Song & Dance.mp3".to_string()), ("1bbbbbbbbbbb".to_string(), "Notes.pdf".to_string())]
        );
    }

    #[test]
    fn mega_ctr_and_name_round_trip() {
        // Any 32-byte key: encrypting twice with CTR gives the original back,
        // at any offset, and the name attribute opens with the derived key.
        let k: [u8; 32] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let (key, nonce) = mega_aes(&k);
        let plain: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut data = plain.clone();
        ctr_apply(&key, &nonce, 0, &mut data);
        assert_ne!(data, plain);
        // Decrypting from the middle (a resumed write) lines up too.
        let (a, b) = data.split_at_mut(37);
        ctr_apply(&key, &nonce, 0, a);
        ctr_apply(&key, &nonce, 37, b);
        assert_eq!(data, plain);
        // A file on disk.
        let p = std::env::temp_dir().join(format!("dc-mega-{}", std::process::id()));
        let mut enc = plain.clone();
        ctr_apply(&key, &nonce, 0, &mut enc);
        std::fs::write(&p, &enc).unwrap();
        mega_decrypt_file(&p, &k).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), plain);
        let _ = std::fs::remove_file(&p);
        // The name attribute: "MEGA" + JSON, zero-padded, AES-CBC with a zero IV.
        let mut attr = br#"MEGA{"n":"film.mkv"}"#.to_vec();
        while attr.len() % 16 != 0 {
            attr.push(0);
        }
        let enc = crate::aes128::cbc_encrypt(&key, &[0u8; 16], &attr);
        let b64: String = {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut s = String::new();
            for c in enc.chunks(3) {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                for i in 0..=c.len() {
                    s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                }
            }
            s
        };
        assert_eq!(mega_name(&b64, &key).as_deref(), Some("film.mkv"));
    }
}
