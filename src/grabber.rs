//! Site grabber (P3.1): collect downloadable file links from a gallery/page.
//!
//! Clean-room single-page + same-host depth-1 crawl. All parsing is pure and
//! fixture-tested; only `grab_site` touches the network.

use crate::engine::EngineOpts;
use crate::model::is_download_url;

pub const MAX_PAGES_DEFAULT: usize = 15;
pub const MAX_LINKS_DEFAULT: usize = 200;

/// One discovered file link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabLink {
    pub url: String,
    pub name: String,
}

/// Built-in extension presets (comma lists, matched case-insensitively).
pub fn preset_exts(preset: &str) -> String {
    match preset {
        "Video" => "mp4,webm,mkv,mov,avi,m4v".to_string(),
        "Audio" => "mp3,m4a,flac,ogg,wav,aac".to_string(),
        "Images" => "png,jpg,jpeg,gif,webp,svg,bmp".to_string(),
        "Documents" => "pdf,doc,docx,txt,epub".to_string(),
        "Archives" => "zip,rar,7z,tar,gz".to_string(),
        _ => "mp4,webm,mkv,mp3,png,jpg,pdf,zip".to_string(), // Mixed
    }
}

pub fn parse_ext_list(s: &str) -> Vec<String> {
    s.split([',', ';', ' ', '\n', '\t'])
        .map(|x| x.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|x| !x.is_empty())
        .collect()
}

fn path_ext(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    base.rsplit('/')
        .next()
        .and_then(|seg| seg.rsplit('.').next())
        .filter(|e| *e != base.rsplit('/').next().unwrap_or(""))
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn file_name_of(url: &str) -> String {
    if let Ok(u) = url::Url::parse(url) {
        if let Some(seg) = u.path_segments().and_then(|mut s| s.next_back()) {
            let clean = seg.split('?').next().unwrap_or(seg);
            if !clean.is_empty() {
                return percent_decode(clean);
            }
        }
    }
    url.to_string()
}

fn percent_decode(s: &str) -> String {
    let mut raw: Vec<u8> = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                raw.push(hex);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

fn abs_url(base: &str, href: &str) -> Option<String> {
    let h = href.trim().replace("&amp;", "&");
    if h.is_empty() || h.starts_with('#') || h.starts_with("javascript:") || h.starts_with("mailto:") {
        return None;
    }
    if h.starts_with("http://") || h.starts_with("https://") {
        return Some(h);
    }
    url::Url::parse(base)
        .ok()
        .and_then(|b| b.join(&h).ok())
        .map(|u| u.to_string())
}

/// Extract file links of wanted extensions from one HTML page.
/// Scans `<a href>`, media `src` (`img/video/audio/source`), and
/// `og:video`-style meta content. Same-URL dupes collapse.
pub fn extract_file_links(html: &str, base_url: &str, exts: &[String]) -> Vec<GrabLink> {
    if exts.is_empty() {
        return Vec::new();
    }
    let wanted = |url: &str| {
        let e = path_ext(url);
        !e.is_empty() && exts.iter().any(|w| *w == e)
    };
    let mut out: Vec<GrabLink> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let push = |url: String, out: &mut Vec<GrabLink>, seen: &mut std::collections::HashSet<String>| {
        if !is_download_url(&url) || !wanted(&url) || !seen.insert(url.clone()) {
            return;
        }
        // Cap total length (proxy-safe, log-safe).
        if url.len() > 2000 {
            return;
        }
        out.push(GrabLink { name: file_name_of(&url), url });
    };
    // Tag attributes: href/src in document order.
    let mut rest = html;
    loop {
        // next href="..." or src="..." (double or single quotes)
        let mut best: Option<(usize, usize, usize)> = None; // (match pos, value_start, value_end)
        for pat in ["href=\"", "href='", "src=\"", "src='"] {
            if let Some(p) = rest.find(pat) {
                let q = pat.chars().last().unwrap();
                let start = p + pat.len();
                if let Some(e) = rest[start..].find(q) {
                    if best.map(|(bp, _, _)| p < bp).unwrap_or(true) {
                        best = Some((p, start, start + e));
                    }
                }
            }
        }
        let Some((_, vs, end)) = best else { break };
        let val = rest[vs..end].to_string();
        if let Some(abs) = abs_url(base_url, &val) {
            push(abs, &mut out, &mut seen);
        }
        rest = &rest[end + 1..];
        if out.len() >= MAX_LINKS_DEFAULT {
            break;
        }
    }
    // Meta content (og:video etc.) — may carry URLs the tags miss.
    let mut mrest = html;
    while let Some(p) = mrest.find("content=\"") {
        let start = p + 9;
        if let Some(e) = mrest[start..].find('"') {
            let val = mrest[start..start + e].replace("&amp;", "&");
            if let Some(abs) = abs_url(base_url, &val) {
                push(abs, &mut out, &mut seen);
            }
            mrest = &mrest[start + e + 1..];
        } else {
            break;
        }
        if out.len() >= MAX_LINKS_DEFAULT * 2 {
            break;
        }
    }
    out.truncate(MAX_LINKS_DEFAULT);
    out
}

/// Same-host page links (for depth-1 crawl): `<a href>` without a file
/// extension, http(s) only.
pub fn extract_page_links(html: &str, base_url: &str) -> Vec<String> {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .unwrap_or_default();
    if host.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = html;
    while let Some(p) = rest.find("href=\"") {
        let start = p + 6;
        let Some(e) = rest[start..].find('"') else { break };
        let val = rest[start..start + e].to_string();
        rest = &rest[start + e + 1..];
        let Some(abs) = abs_url(base_url, &val) else { continue };
        if !is_download_url(&abs) || !path_ext(&abs).is_empty() {
            continue;
        }
        let same = url::Url::parse(&abs)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
            .map(|h| h == host)
            .unwrap_or(false);
        if same && seen.insert(abs.clone()) {
            out.push(abs);
        }
        if out.len() >= 60 {
            break;
        }
    }
    out
}

async fn fetch_html(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
) -> Option<String> {
    const MAX_PAGE: usize = 4 * 1024 * 1024;
    let resp = opts.get(client, url).send().await.ok()?.error_for_status().ok()?;
    // Crawled links without an extension are often the files themselves
    // (/download?id=5): only pages are read, and never past the cap. The
    // whole body used to be loaded first, so a multi-GB file filled RAM.
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ctype.is_empty() && !(ctype.contains("html") || ctype.contains("xml") || ctype.starts_with("text/")) {
        return None;
    }
    if resp.content_length().is_some_and(|n| n > MAX_PAGE as u64) {
        return None;
    }
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.ok()?);
        if body.len() > MAX_PAGE {
            return None;
        }
    }
    Some(String::from_utf8_lossy(&body).into_owned())
}

/// The folder part of a URL ("https://h/a/b/page.html" -> "https://h/a/b/").
fn folder_of(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    match base.rfind('/') {
        Some(i) if i > base.find("://").map_or(0, |p| p + 2) => base[..=i].to_string(),
        _ => format!("{base}/"),
    }
}

/// Crawl the start page and same-site pages up to `depth` links away (IDM's
/// Site Grabber), breadth first, at most `max_pages` pages. `stay_under` keeps
/// the crawl inside the start page's folder. Returns deduped file links.
pub async fn grab_site(
    client: &reqwest::Client,
    opts: &EngineOpts,
    start_url: &str,
    exts: &[String],
    depth: usize,
    stay_under: bool,
    max_pages: usize,
) -> Result<Vec<GrabLink>, String> {
    let first = fetch_html(client, opts, start_url)
        .await
        .ok_or_else(|| "could not fetch that page (network or not HTML)".to_string())?;
    let mut all = extract_file_links(&first, start_url, exts);
    let folder = folder_of(start_url);
    let allowed = |p: &str| !stay_under || p.starts_with(&folder);
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::from([start_url.to_string()]);
    let mut frontier: Vec<String> = extract_page_links(&first, start_url).into_iter().filter(|p| allowed(p)).collect();
    let max_pages = max_pages.max(1);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    for _level in 0..depth.min(5) {
        frontier.retain(|p| visited.insert(p.clone()));
        let room = max_pages.saturating_sub(visited.len() - frontier.len());
        frontier.truncate(room);
        if frontier.is_empty() || all.len() >= MAX_LINKS_DEFAULT * 5 {
            break;
        }
        let mut handles = Vec::new();
        for p in std::mem::take(&mut frontier) {
            let client = client.clone();
            let opts = opts.clone();
            let exts = exts.to_vec();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _g = sem.acquire_owned().await;
                let html = fetch_html(&client, &opts, &p).await?;
                Some((extract_file_links(&html, &p, &exts), extract_page_links(&html, &p)))
            }));
        }
        for h in handles {
            if let Ok(Some((mut links, pages))) = h.await {
                all.append(&mut links);
                frontier.extend(pages.into_iter().filter(|p| allowed(p) && !visited.contains(p)));
            }
        }
    }
    // Dedupe across pages, keep first-seen order.
    let mut seen = std::collections::HashSet::new();
    all.retain(|l| seen.insert(l.url.clone()));
    all.truncate(MAX_LINKS_DEFAULT * 5);
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head>
<meta property="og:video" content="https://cdn.example/v.mp4">
</head><body>
<a href="/files/a.mp4">A</a>
<a href="https://cdn.example/files/a.mp4">A dup</a>
<a href="https://other.example/x.mp3">song</a>
<a href="/gallery/2">next</a>
<a href="mailto:x@y.z">mail</a>
<img src="pic.JPG">
<video src="https://cdn.example/mov_bbb.mp4"></video>
<source src="/v/clip.webm">
</body></html>"#;

    #[test]
    fn extract_filters_and_resolves() {
        let exts = parse_ext_list("mp4, jpg");
        assert_eq!(exts, vec!["mp4".to_string(), "jpg".to_string()]);
        let links = extract_file_links(PAGE, "https://site.example/g/1", &exts);
        let urls: Vec<&str> = links.iter().map(|l| l.url.as_str()).collect();
        assert!(urls.contains(&"https://site.example/files/a.mp4"));
        assert!(urls.contains(&"https://cdn.example/v.mp4"));
        assert!(urls.contains(&"https://site.example/g/pic.JPG"));
        // same filename on two hosts = two links; exact dupes collapse
        assert_eq!(urls.iter().filter(|u| u.contains("a.mp4")).count(), 2);
        let uniq: std::collections::HashSet<&&str> = urls.iter().collect();
        assert_eq!(uniq.len(), urls.len());
        assert!(!urls.iter().any(|u| u.contains(".mp3")));
        assert!(!urls.iter().any(|u| u.contains("gallery")));
    }

    #[test]
    fn page_links_same_host_only() {
        let pages = extract_page_links(PAGE, "https://site.example/g/1");
        assert_eq!(pages, vec!["https://site.example/gallery/2".to_string()]);
    }
}
