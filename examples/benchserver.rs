//! A fast local test server for speed benchmarks (the Python one in
//! tools/bench tops out well below what a download manager can take on
//! loopback, so it measured the server, not the app).
//!
//!     cargo run --release --example benchserver -- 8766
//!
//! `/<name>` serves generated content: the size comes from the name
//! (`file1g`, `file300m`, `file1m`, `file256k`; `__runN` is ignored, so every
//! run can use a new name). Range requests, `?rate=<bytes/s per connection>`
//! and `?drop=<bytes before the connection is cut>` like the Python server.
//! The first request for each name is logged to stdout with its time, for
//! timing "from the first request".

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// The content repeats this block (random bytes, made once).
const BLOCK: usize = 4 << 20;

fn block() -> &'static Bytes {
    static B: OnceLock<Bytes> = OnceLock::new();
    B.get_or_init(|| {
        // xorshift: fast, and not all zeros.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut v = Vec::with_capacity(BLOCK);
        while v.len() < BLOCK {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.extend_from_slice(&x.to_le_bytes());
        }
        Bytes::from(v)
    })
}

fn size_of(name: &str) -> Option<u64> {
    let stem = name.split("__").next()?.split('.').next()?;
    let n = stem.strip_prefix("file")?;
    let (num, unit) = n.split_at(n.find(|c: char| !c.is_ascii_digit())?);
    let num: u64 = num.parse().ok()?;
    Some(match unit {
        "g" => num << 30,
        "m" => num << 20,
        "k" => num << 10,
        _ => return None,
    })
}

fn log_first(name: &str) {
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap();
    if g.get_or_insert_with(HashSet::new).insert(name.to_string()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        println!("{name} {t:.4}");
    }
}

async fn serve(Path(name): Path<String>, RawQuery(raw): RawQuery, headers: HeaderMap, head: bool) -> Response {
    let q: HashMap<String, String> = url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()).into_owned().collect();
    log_first(&name);
    let Some(size) = size_of(&name) else {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    };
    let rate: u64 = q.get("rate").and_then(|v| v.parse().ok()).unwrap_or(0);
    let drop: u64 = q.get("drop").and_then(|v| v.parse().ok()).unwrap_or(0);
    let (mut start, mut end, mut status) = (0u64, size - 1, StatusCode::OK);
    if let Some(r) = headers.get("range").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("bytes=")) {
        let (a, b) = r.split(',').next().unwrap_or("").split_once('-').unwrap_or(("", ""));
        if a.is_empty() {
            start = size.saturating_sub(b.parse().unwrap_or(0));
        } else {
            start = a.parse().unwrap_or(0);
            end = b.parse().unwrap_or(size - 1).min(size - 1);
        }
        status = StatusCode::PARTIAL_CONTENT;
    }
    let len = end + 1 - start;
    let mut res = Response::builder()
        .status(status)
        .header("content-type", "application/octet-stream")
        .header("accept-ranges", "bytes")
        .header("content-length", len)
        .header("last-modified", "Mon, 02 Nov 2026 10:00:00 GMT")
        .header("etag", HeaderValue::from_str(&format!("\"{name}\"")).unwrap());
    if status == StatusCode::PARTIAL_CONTENT {
        res = res.header("content-range", format!("bytes {start}-{end}/{size}"));
    }
    if head {
        return res.body(Body::empty()).unwrap();
    }
    let stream = futures_util::stream::unfold((start, 0u64, std::time::Instant::now()), move |(pos, sent, t0)| async move {
        if pos > end {
            return None;
        }
        if drop > 0 && sent >= drop {
            return Some((Err(std::io::Error::other("cut")), (end + 1, sent, t0)));
        }
        let off = (pos % BLOCK as u64) as usize;
        let mut n = ((end + 1 - pos) as usize).min(BLOCK - off).min(if rate > 0 { 64 << 10 } else { 1 << 20 });
        if drop > 0 {
            n = n.min((drop - sent) as usize);
        }
        if rate > 0 {
            let due = std::time::Duration::from_secs_f64(sent as f64 / rate as f64);
            let el = t0.elapsed();
            if due > el {
                tokio::time::sleep(due - el).await;
            }
        }
        let chunk = block().slice(off..off + n);
        Some((Ok::<Bytes, std::io::Error>(chunk), (pos + n as u64, sent + n as u64, t0)))
    });
    res.body(Body::from_stream(stream)).unwrap()
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(8766);
    block();
    let app = axum::Router::new().route(
        "/{name}",
        axum::routing::get(|p, q, h| serve(p, q, h, false)).head(|p, q, h| serve(p, q, h, true)),
    );
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    eprintln!("benchserver on 127.0.0.1:{port}");
    axum::serve(l, app).await.unwrap();
}
