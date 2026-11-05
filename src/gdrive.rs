//! Google Drive and Google Docs links.
//!
//! A Drive share link (`drive.google.com/file/d/<ID>/view`) is a web page,
//! not the file. The file comes from Drive's download endpoint:
//!
//! * a small file is answered straight away;
//! * a big one first gets a "can't scan this file for viruses" page with a
//!   form, and the form's fields (a confirm value and a per-session uuid)
//!   lead to the bytes.
//!
//! Those fields are read off the page Drive sent; nothing is made up. Google
//! Docs, Sheets and Slides links are turned into their export (Word, Excel,
//! PowerPoint). Folders are refused with a plain reason.

use anyhow::{anyhow, Result};

const DOWNLOAD: &str = "https://drive.usercontent.google.com/download";

/// What a Google link is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// A Drive file, by id (plus a resource key some shared files need).
    File { id: String, resource_key: Option<String> },
    /// A Docs editor file, exported as `format`.
    Export { url: String },
    /// A Drive folder: not downloadable as one file.
    Folder,
}

fn host_of(u: &url::Url) -> String {
    u.host_str().unwrap_or("").to_ascii_lowercase()
}

fn query(u: &url::Url, key: &str) -> Option<String> {
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.to_string()).filter(|v| !v.is_empty())
}

fn valid_id(id: &str) -> bool {
    id.len() >= 10 && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Is `url` served by Google Drive or Docs (share pages, downloads, exports)?
pub fn is_google_host(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        matches!(host_of(&u).as_str(), "drive.google.com" | "drive.usercontent.google.com" | "docs.google.com")
    })
}

/// Is this a Google Drive / Docs link this module handles?
pub fn is_drive_url(url: &str) -> bool {
    classify(url).is_some()
}

/// Read a Drive or Docs link. `None` for anything else.
pub fn classify(raw: &str) -> Option<Link> {
    let u = url::Url::parse(raw.trim()).ok()?;
    let host = host_of(&u);
    let segs: Vec<&str> = u.path_segments().map(|s| s.filter(|p| !p.is_empty()).collect()).unwrap_or_default();
    let resource_key = query(&u, "resourcekey");
    let file = |id: &str| valid_id(id).then(|| Link::File { id: id.to_string(), resource_key: resource_key.clone() });
    match host.as_str() {
        "drive.google.com" => {
            // /file/d/<ID>/view, /u/0/file/d/<ID>, /drive/folders/<ID>,
            // /open?id=<ID>, /uc?id=<ID>
            if let Some(i) = segs.iter().position(|s| *s == "folders") {
                return segs.get(i + 1).map(|_| Link::Folder);
            }
            if let Some(i) = segs.windows(2).position(|w| w[0] == "file" && w[1] == "d") {
                return segs.get(i + 2).and_then(|id| file(id));
            }
            if matches!(segs.last(), Some(&"open") | Some(&"uc")) {
                return query(&u, "id").and_then(|id| file(&id));
            }
            None
        }
        "drive.usercontent.google.com" => query(&u, "id").and_then(|id| file(&id)),
        "docs.google.com" => {
            // /document/d/<ID>/edit -> export as docx, and so on. Also the
            // old /uc?id= form, which is Drive.
            if segs.last() == Some(&"uc") {
                return query(&u, "id").and_then(|id| file(&id));
            }
            let base = segs.windows(3).find_map(|w| {
                (w[1] == "d" && valid_id(w[2])).then(|| (w[0], w[2]))
            })?;
            let (kind, id) = base;
            let format = match kind {
                "document" => "docx",
                "spreadsheets" => "xlsx",
                "presentation" => "pptx",
                "drawings" => "png",
                _ => return None,
            };
            // An export URL a user pasted keeps its own format.
            if segs.last() == Some(&"export") {
                return Some(Link::Export { url: raw.trim().to_string() });
            }
            let path = if kind == "drawings" { "export/png".to_string() } else { format!("export?format={format}") };
            Some(Link::Export { url: format!("https://docs.google.com/{kind}/d/{id}/{path}") })
        }
        _ => None,
    }
}

/// The plain download address of a Drive file id.
pub fn download_url(id: &str, resource_key: Option<&str>) -> String {
    download_url_at(DOWNLOAD, id, resource_key)
}

/// `download_url` against a given endpoint. Tests point this at a local
/// server, so the resolve path runs without the network.
fn download_url_at(base: &str, id: &str, resource_key: Option<&str>) -> String {
    let mut u = url::Url::parse(base).expect("download endpoint");
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("id", id).append_pair("export", "download");
        if let Some(k) = resource_key {
            q.append_pair("resourcekey", k);
        }
    }
    u.to_string()
}

/// The value of `name="..."` style attribute `attr` inside one tag.
fn attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(attr) {
        let at = from + i;
        from = at + attr.len();
        // A whole attribute name: `data-id=` is not `id=`.
        let before = lower[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let rest = tag[at + attr.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(html_unescape(value));
    }
    None
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// The download form on Drive's virus-scan page, as the URL it submits to.
/// `None` when the page has no such form.
pub fn form_target(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<form")?;
    let end = lower[start..].find("</form").map(|e| start + e).unwrap_or(html.len());
    let form = &html[start..end];
    let open_end = form.find('>')?;
    let action = attr(&form[..open_end], "action")?;
    let mut target = url::Url::parse(&action).ok()?;
    let host = host_of(&target);
    if target.scheme() != "https" || !(host == "drive.usercontent.google.com" || host == "drive.google.com") {
        return None;
    }
    let mut fields = Vec::new();
    let flower = form.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = flower[from..].find("<input") {
        let at = from + i;
        let close = flower[at..].find('>').map(|e| at + e).unwrap_or(form.len());
        let tag = &form[at..close];
        from = close;
        if let (Some(name), Some(value)) = (attr(tag, "name"), attr(tag, "value")) {
            fields.push((name, value));
        }
    }
    if !fields.iter().any(|(k, _)| k == "id") {
        return None;
    }
    target.set_query(None);
    {
        let mut q = target.query_pairs_mut();
        for (k, v) in &fields {
            q.append_pair(k, v);
        }
    }
    Some(target.to_string())
}

/// Turn a Drive / Docs link into the address that serves the file itself.
/// Anything that is not a Google link comes back unchanged.
pub async fn resolve(client: &reqwest::Client, opts: &crate::engine::EngineOpts, url: &str) -> Result<String> {
    resolve_at(client, opts, url, DOWNLOAD).await
}

/// `resolve` with the download endpoint injected: the page/file decision is
/// the part worth covering, and a local server can stand in for Drive.
async fn resolve_at(
    client: &reqwest::Client,
    opts: &crate::engine::EngineOpts,
    url: &str,
    download_base: &str,
) -> Result<String> {
    let link = match classify(url) {
        Some(l) => l,
        None => return Ok(url.to_string()),
    };
    let first = match link {
        Link::Folder => {
            return Err(anyhow!(
                "this is a Google Drive folder; open it and download the files inside (or use Download all links)"
            ))
        }
        Link::Export { url } => return Ok(url),
        Link::File { id, resource_key } => {
            // A download link that already carries Drive's confirm/uuid is
            // the file; keep it as the browser handed it over.
            let given = url::Url::parse(url).ok();
            if given.as_ref().is_some_and(|u| {
                host_of(u) == "drive.usercontent.google.com" && query(u, "uuid").is_some()
            }) {
                return Ok(url.to_string());
            }
            download_url_at(download_base, &id, resource_key.as_deref())
        }
    };
    let resp = opts.get(client, &first).send().await?;
    let status = resp.status();
    let final_host = host_of(resp.url());
    if final_host == "accounts.google.com" || status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(anyhow!("this Google Drive file is not shared publicly (ask for \"Anyone with the link\" access)"));
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(anyhow!("Google Drive has no file with this link (deleted, or the link is wrong)"));
    }
    let html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|c| c.contains("text/html"));
    if !html {
        // Small files: the first answer is already the file.
        return Ok(resp.url().to_string());
    }
    // The virus-scan page: read it (it is small) and follow its form.
    let mut body = Vec::new();
    let mut stream = resp;
    while let Some(chunk) = stream.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > 512 * 1024 {
            break;
        }
    }
    let page = String::from_utf8_lossy(&body);
    if let Some(target) = form_target(&page) {
        return Ok(target);
    }
    let lower = page.to_ascii_lowercase();
    if lower.contains("quota") || lower.contains("too many users") {
        return Err(anyhow!("Google Drive's download limit for this file is reached for now; try again in 24 hours"));
    }
    Err(anyhow!("Google Drive answered with a page instead of the file (it may need a sign-in)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1AbCdEfGhIjKlMnOpQrStUvWxYz012345";

    #[test]
    fn share_links_become_file_ids() {
        for u in [
            format!("https://drive.google.com/file/d/{ID}/view?usp=sharing"),
            format!("https://drive.google.com/u/0/file/d/{ID}/edit"),
            format!("https://drive.google.com/open?id={ID}"),
            format!("https://drive.google.com/uc?export=download&id={ID}"),
            format!("https://docs.google.com/uc?id={ID}"),
            format!("https://drive.usercontent.google.com/download?id={ID}&export=download"),
        ] {
            assert_eq!(classify(&u), Some(Link::File { id: ID.into(), resource_key: None }), "{u}");
        }
        assert_eq!(
            classify(&format!("https://drive.google.com/file/d/{ID}/view?resourcekey=0-abc")),
            Some(Link::File { id: ID.into(), resource_key: Some("0-abc".into()) })
        );
    }

    #[test]
    fn docs_export_and_folders() {
        assert_eq!(
            classify(&format!("https://docs.google.com/document/d/{ID}/edit")),
            Some(Link::Export { url: format!("https://docs.google.com/document/d/{ID}/export?format=docx") })
        );
        assert_eq!(
            classify(&format!("https://docs.google.com/spreadsheets/d/{ID}/edit#gid=0")),
            Some(Link::Export { url: format!("https://docs.google.com/spreadsheets/d/{ID}/export?format=xlsx") })
        );
        assert_eq!(
            classify(&format!("https://docs.google.com/presentation/d/{ID}/edit")),
            Some(Link::Export { url: format!("https://docs.google.com/presentation/d/{ID}/export?format=pptx") })
        );
        assert_eq!(classify(&format!("https://drive.google.com/drive/folders/{ID}")), Some(Link::Folder));
    }

    #[test]
    fn other_links_are_left_alone() {
        for u in [
            "https://example.com/file/d/abcdefghijkl/view",
            "https://drive.google.com/",
            "https://drive.google.com/file/d/short/view",
            "https://www.google.com/search?q=drive",
            "not a url",
        ] {
            assert_eq!(classify(u), None, "{u}");
        }
    }

    #[test]
    fn download_address() {
        assert_eq!(
            download_url(ID, None),
            format!("https://drive.usercontent.google.com/download?id={ID}&export=download")
        );
    }

    #[test]
    fn reads_the_virus_scan_form() {
        let page = format!(
            r#"<html><body><p>Google Drive can't scan this file for viruses.</p>
            <form id="download-form" action="https://drive.usercontent.google.com/download" method="get">
            <input type="submit" id="uc-download-link" class="goog-inline-block jfk-button" value="Download anyway"/>
            <input type="hidden" name="id" value="{ID}">
            <input type="hidden" name="export" value="download">
            <input type="hidden" name="confirm" value="t">
            <input type="hidden" name="uuid" value="5f3b-aa&amp;b"></form></body></html>"#
        );
        assert_eq!(
            form_target(&page).as_deref(),
            Some(format!("https://drive.usercontent.google.com/download?id={ID}&export=download&confirm=t&uuid=5f3b-aa%26b").as_str())
        );
    }

    #[test]
    fn a_form_elsewhere_is_ignored() {
        let page = format!(r#"<form action="https://evil.example/x"><input name="id" value="{ID}"></form>"#);
        assert_eq!(form_target(&page), None);
        assert_eq!(form_target("<html>no form</html>"), None);
        // `data-name` is not `name`.
        let page = r#"<form action="https://drive.usercontent.google.com/download"><input data-name="id" value="x"></form>"#;
        assert_eq!(form_target(page), None);
    }

    /// The whole resolve path — small file, virus-scan page, and the honest
    /// refusals — against a local server standing in for Drive. No network and
    /// no shared file that can vanish, so it belongs in the default suite (the
    /// old version was `#[ignore]`d for needing both).
    #[tokio::test]
    async fn scan_page_error_and_file_paths_offline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = s.read(&mut buf).await.unwrap_or(0);
                        if n == 0 { return; }
                        raw.extend_from_slice(&buf[..n]);
                        if raw.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                    }
                    let request = String::from_utf8_lossy(&raw).to_string();
                    let line = request.lines().next().unwrap_or("");
                    let (status, mime, body) = if line.contains("id=goodfile0001") {
                        ("200 OK", "application/octet-stream", b"npz-bytes".to_vec())
                    } else if line.contains("id=missingfile1") {
                        ("404 Not Found", "text/plain", b"gone".to_vec())
                    } else if line.contains("id=forbidden001") {
                        ("403 Forbidden", "text/plain", b"no".to_vec())
                    } else if line.contains("id=scanfolder01") {
                        let page = r#"<html><body><p>Google Drive can't scan this file for viruses.</p>
                            <form id="download-form" action="https://drive.usercontent.google.com/download" method="get">
                            <input type="hidden" name="id" value="scanfolder01">
                            <input type="hidden" name="export" value="download">
                            <input type="hidden" name="confirm" value="t">
                            <input type="hidden" name="uuid" value="u-1"></form></body></html>"#;
                        ("200 OK", "text/html; charset=utf-8", page.as_bytes().to_vec())
                    } else if line.contains("id=quotafile001") {
                        let page = b"<html>too many users have viewed this file</html>".to_vec();
                        ("200 OK", "text/html; charset=utf-8", page)
                    } else {
                        let page = b"<html><p>sign in to continue</p></html>".to_vec();
                        ("200 OK", "text/html; charset=utf-8", page)
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        let opts = crate::engine::EngineOpts::default();
        let client = opts.build_client().unwrap();
        let base = format!("http://{addr}/download");
        let link = |id: &str| format!("https://drive.google.com/file/d/{id}/view");
        // A small file: the first answer is already the bytes.
        let got = resolve_at(&client, &opts, &link("goodfile0001"), &base).await.unwrap();
        assert!(got.contains("id=goodfile0001"), "{got}");
        // The virus-scan page: its form fields become the download address.
        let got = resolve_at(&client, &opts, &link("scanfolder01"), &base).await.unwrap();
        assert!(got.starts_with("https://drive.usercontent.google.com/download?"), "{got}");
        assert!(got.contains("id=scanfolder01") && got.contains("confirm=t") && got.contains("uuid=u-1"), "{got}");
        // Refusals say why, instead of downloading the page as the file.
        let err = resolve_at(&client, &opts, &link("missingfile1"), &base).await.unwrap_err().to_string();
        assert!(err.contains("no file"), "{err}");
        let err = resolve_at(&client, &opts, &link("forbidden001"), &base).await.unwrap_err().to_string();
        assert!(err.contains("not shared publicly"), "{err}");
        let err = resolve_at(&client, &opts, &link("quotafile001"), &base).await.unwrap_err().to_string();
        assert!(err.contains("download limit"), "{err}");
        let err = resolve_at(&client, &opts, &link("unknowfile01"), &base).await.unwrap_err().to_string();
        assert!(err.contains("page instead of the file"), "{err}");
    }
}
