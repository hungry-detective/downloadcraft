//! Native-messaging host for the browser extension (`--native-host`).
//!
//! Protocol (Chrome/Edge/Firefox native messaging = 4-byte LE length + UTF-8 JSON):
//!   -> {"type":"ping"}                                  <- {"type":"pong","version":1}
//!   -> {"type":"download","url":"https://...","filename":"optional",
//!       "pageUrl":"https://page...","referer":"...","cookie":"...","mime":"...",
//!       "headers":{"origin":"...","authorization":"...","x-...":"..."}}
//!                                                      <- {"type":"ok","queued":true}
//!
//! Download requests are appended to an inbox file that the GUI polls and
//! imports as queued (auto-started) downloads.
//!
//! Persistent port (`runtime.connectNative`): every message may carry a
//! `rid`, echoed on its reply, so several requests can be in flight. After
//! `{"type":"hello"}` the host also *pushes* to the extension:
//!   <- {"type":"settings","settings":{...},"rules":{...}}   capture settings + site rules
//!   <- {"type":"event","event":"done"|"failed",...}        downloads the extension sent
//!   <- {"type":"pageFetch","id":"...","url":"...","page":"..."}  fetch inside the page
//! and accepts `{"type":"pageFetchChunk","fetchId":..,"chunk":b64,"done":bool}`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct Incoming {
    #[serde(rename = "type")]
    kind: String,
    /// "report": one line for the app's log (a site's layout changed, ...).
    #[serde(default)]
    note: Option<String>,
    url: Option<String>,
    filename: Option<String>,
    /// Page the download was captured from (IDM-style referer fallback).
    #[serde(default, alias = "pageUrl")]
    page_url: Option<String>,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    cookie: Option<String>,
    /// Origin / Authorization / x-* headers the player sent with this media.
    #[serde(default)]
    headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    mime: Option<String>,
    /// Browser-measured size bytes (HEAD/Range from the page session).
    /// The engine can't always re-probe media URLs (403 off-session), so
    /// the dialog prefers this over probing.
    #[serde(default)]
    size: Option<u64>,
    /// Adaptive pair audio URL (video-only + audio merge). The extension
    /// sends these in camelCase (`pairUrl`/`pairMime`) like `pageUrl`, so
    /// without the aliases the pair was silently dropped and the merge never
    /// happened.
    #[serde(default, alias = "pairUrl")]
    pub pair_url: Option<String>,
    #[serde(default, alias = "pairMime")]
    pub pair_mime: Option<String>,
    /// Batch presence check (`{"type":"query"}`): which of these URLs are
    /// already queued. Answered from queue.json without side effects.
    /// (Only read on Incoming; stored nowhere.)
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    /// Preferred YouTube quality from the extension quality menu.
    #[serde(default)]
    pub quality: Option<String>,
    /// Media duration in seconds (page player / JSON, best-effort).
    /// Shown in the confirm dialog and checked against the completed file.
    #[serde(default)]
    pub dur: Option<u64>,
    #[serde(default, alias = "audioExpected")]
    pub audio_expected: bool,
    /// "Download all links" (`{"type":"links"}`): every link on a page.
    #[serde(default)]
    pub links: Option<Vec<PageLink>>,
    /// Page title for the links window.
    #[serde(default)]
    pub title: Option<String>,
    /// `{"type":"control"}`: which queue row, and what to do with it.
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub action: Option<String>,
    /// `{"type":"readFile"}`: where the next piece of the file starts.
    #[serde(default)]
    pub offset: Option<u64>,
    /// Request id on a persistent port; echoed on the reply.
    #[serde(default)]
    pub rid: Option<u64>,
    /// The browser's side of the capture (method/body, request headers,
    /// server IP, proxy). Sanitized before it is stored.
    #[serde(default)]
    pub browser: Option<crate::model::BrowserContext>,
    /// `pageFetchChunk`: which request, a base64 piece, the last one, and
    /// the outcome.
    #[serde(default, alias = "fetchId")]
    pub fetch_id: Option<String>,
    #[serde(default)]
    pub chunk: Option<String>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
    /// `setPrefs`: capture settings changed in the extension's popup.
    #[serde(default)]
    pub prefs: Option<serde_json::Value>,
}

/// One link from a page ("Download all links").
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PageLink {
    pub url: String,
    #[serde(default)]
    pub text: String,
}

/// What `--links <file>` reads: written by the host, deleted by the window.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LinksBatch {
    pub page_url: String,
    #[serde(default)]
    pub title: String,
    /// The page's cookies; the window applies them to same-host links only.
    #[serde(default)]
    pub cookie: String,
    pub links: Vec<PageLink>,
}

/// Validate a page's links and keep the downloadable ones (http/https,
/// sane lengths, no duplicates), capped like the extension's collector.
pub fn links_batch(page_url: &str, title: &str, cookie: &str, links: &[PageLink]) -> LinksBatch {
    let mut seen = std::collections::HashSet::new();
    let kept = links
        .iter()
        .filter(|l| crate::model::is_download_url(&l.url) && l.url.len() <= 4096)
        .filter(|l| seen.insert(l.url.clone()))
        .take(5000)
        .map(|l| PageLink {
            url: l.url.clone(),
            text: l.text.chars().filter(|c| !c.is_control()).take(200).collect(),
        })
        .collect();
    LinksBatch {
        page_url: page_url.to_string(),
        title: title.chars().filter(|c| !c.is_control()).take(200).collect(),
        cookie: cookie.to_string(),
        links: kept,
    }
}

fn spawn_links(msg: &Incoming) -> Result<(), String> {
    open_links_window(
        &msg.page_url.clone().unwrap_or_default(),
        msg.title.as_deref().unwrap_or(""),
        msg.cookie.as_deref().unwrap_or(""),
        msg.links.as_deref().unwrap_or(&[]),
    )
}

/// The "pick links" window for any list of links (download all links, batch
/// patterns, dropped lists).
pub fn open_links_window(page: &str, title: &str, cookie: &str, links: &[PageLink]) -> Result<(), String> {
    let batch = links_batch(page, title, cookie, links);
    if batch.links.is_empty() {
        return Err("no downloadable links on this page".into());
    }
    let dir = crate::model::config_dir().ok_or_else(|| "no config dir".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("links_{}.json", chrono::Local::now().timestamp_millis()));
    let json = serde_json::to_string(&batch).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string())?;
    let exe = crate::platform::launch_path().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--links").arg(&path);
    spawn_detached(cmd);
    Ok(())
}

/// Started by the browser straight from the manifest, without the launcher
/// script (`--native-host`). Chromium passes the caller's origin first,
/// Firefox the manifest path and then the extension's id. Used when the
/// program sits under a non-ASCII path, which a `.bat` cannot name (issue #5).
pub fn launched_directly(args: &[String]) -> bool {
    let ours = |a: &String| {
        a.strip_prefix("chrome-extension://").is_some_and(|rest| {
            let id = rest.trim_end_matches('/');
            id == crate::app::EXTENSION_ID || crate::app::STORE_EXTENSION_IDS.contains(&id)
        }) || a == "send-to-download-craft@example"
    };
    args.iter().skip(1).take(2).any(ours)
}

/// Spawn a window that outlives this short-lived host.
fn spawn_detached(mut cmd: std::process::Command) {
    // Never hand our stdio to the window: an inherited stdout keeps the
    // browser's pipe open after this host exits.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // DETACHED_PROCESS so the window outlives this short-lived host, and
        // BREAKAWAY_FROM_JOB because Firefox runs native hosts inside a job
        // object it kills when the host exits: children inherit that job, so a
        // plain detached dialog was being torn down the moment Firefox closed
        // our stdin (verified live — Chrome does not use such a job, which is
        // why only Firefox looked like it did nothing). A job that forbids
        // breaking away makes the spawn fail, and a plain detached window
        // still beats none at all.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return;
        }
        cmd.creation_flags(DETACHED_PROCESS);
    }
    // Linux/macOS: out of the host's process group, which the browser may
    // signal as a whole when it closes the host.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let _ = cmd.spawn();
}

#[derive(Debug, Serialize)]
struct Reply {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    urls: Option<Vec<String>>,
    /// The app's theme ("light" | "dark" | "system") on pong, so the
    /// extension's in-page controls match it.
    #[serde(skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    /// `{"type":"list"}`: the app's active and recent downloads.
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Vec<serde_json::Value>>,
}

impl Reply {
    fn ok() -> Self {
        Reply { kind: "ok".into(), version: None, queued: Some(true), error: None, urls: None, theme: None, items: None }
    }
    fn err(e: impl Into<String>) -> Self {
        Reply { kind: "error".into(), version: None, queued: None, error: Some(e.into()), urls: None, theme: None, items: None }
    }
}

/// The popup's Downloads tab: every active row, then the newest finished or
/// failed ones, with live bytes and speed from the app's progress snapshot.
/// Largest file the extension may drag into a page (issue #15): it is read
/// through this channel piece by piece and held in the popup's memory.
const DRAG_MOST: u64 = 100 << 20;
/// One piece: 512 KiB, under native messaging's 1 MB reply limit once base64.
const DRAG_PIECE: u64 = 512 << 10;

/// A piece of a finished download, for dragging it out of the extension's
/// list onto a page's upload box. Only a row of the app's own queue that is
/// complete, and only its own file: the extension cannot name any other path.
fn read_finished_file(id: Option<u64>, offset: u64) -> Result<serde_json::Value, String> {
    use crate::model::{DownloadItem, Status};
    let id = id.ok_or("no download named")?;
    let dir = crate::model::config_dir().ok_or("no config dir")?;
    let items: Vec<DownloadItem> = std::fs::read_to_string(dir.join("queue.json"))
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default();
    let d = items.iter().find(|d| d.id == id).ok_or("no such download")?;
    if !matches!(d.status, Status::Completed) {
        return Err("the download is not finished".into());
    }
    let path = d.dest_path();
    let size = std::fs::metadata(&path).map_err(|_| "the file was moved or deleted")?.len();
    if size > DRAG_MOST {
        return Err(format!("too large to drag ({} MB, the most is {} MB)", size >> 20, DRAG_MOST >> 20));
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    let start = offset.min(size);
    f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(DRAG_PIECE.min(size - start) as usize);
    f.take(DRAG_PIECE).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let next = start + buf.len() as u64;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| d.filename.clone());
    let mime = std::path::Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .map(crate::model::mime_for_ext)
        .unwrap_or("application/octet-stream");
    Ok(serde_json::json!({
        "type": "file",
        "name": name,
        "mime": mime,
        "size": size,
        "offset": start,
        "data": crate::model::base64_encode(&buf),
        "done": next >= size,
    }))
}

fn app_downloads() -> Vec<serde_json::Value> {
    use crate::model::{DownloadItem, Status};
    let dir = match crate::model::config_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let items: Vec<DownloadItem> = std::fs::read_to_string(dir.join("queue.json"))
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default();
    let live: serde_json::Value = std::fs::read_to_string(dir.join("progress.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut rows: Vec<&DownloadItem> = items.iter().collect();
    rows.sort_by(|a, b| b.id.cmp(&a.id));
    let active = |d: &&DownloadItem| matches!(d.status, Status::Downloading | Status::Queued | Status::Paused);
    let mut out: Vec<&DownloadItem> = rows.iter().copied().filter(active).collect();
    out.extend(rows.iter().copied().filter(|d| !active(d)).take(15));
    out.into_iter()
        .map(|d| {
            let snap = live.get("items").and_then(|m| m.get(d.id.to_string()));
            let num = |k: &str| snap.and_then(|s| s.get(k)).and_then(|x| x.as_f64());
            let (state, error) = match &d.status {
                Status::Downloading => ("downloading", String::new()),
                Status::Queued => ("queued", String::new()),
                Status::Paused => ("paused", String::new()),
                Status::Completed => ("done", String::new()),
                Status::Failed(e) => ("failed", e.clone()),
            };
            serde_json::json!({
                "id": d.id,
                "name": d.filename,
                "state": state,
                "error": error,
                "total": num("total").map(|t| t as u64).or(d.total_bytes),
                "done": num("downloaded").map(|t| t as u64).unwrap_or(d.downloaded_bytes),
                "speed": num("speed").unwrap_or(0.0),
                "url": d.url,
            })
        })
        .collect()
}

/// A file the extension saved itself ("Download in browser"): list it in the
/// app as finished. `filename` is the saved file's full path; it must exist.
fn record_browser_download(msg: &Incoming) -> Result<(), String> {
    let url = msg.url.clone().filter(|u| u.starts_with("http")).unwrap_or_default();
    let path = std::path::PathBuf::from(msg.filename.clone().unwrap_or_default());
    if !path.is_absolute() || !path.is_file() {
        return Err("the saved file was not found".into());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).ok_or("no file name")?;
    let dir = path.parent().map(|d| d.to_string_lossy().to_string()).ok_or("no folder")?;
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut item = crate::model::DownloadItem::new(0, if url.is_empty() { format!("file:///{}", path.display()) } else { url }, name, dir, 1);
    item.total_bytes = Some(len);
    item.downloaded_bytes = len;
    item.referer = msg.page_url.clone().unwrap_or_default();
    item.description = "Downloaded in the browser".into();
    match crate::model::record_finished(&mut item) {
        crate::model::AppendResult::Error(e) => Err(e),
        _ => Ok(()),
    }
}

/// Queue a pause/resume/cancel order for the app (the same cmd.json the
/// progress windows use).
fn app_control(id: Option<u64>, action: &str) -> Result<(), String> {
    let allowed = ["pause", "resume", "cancel", "pause_all", "resume_all"];
    if !allowed.contains(&action) {
        return Err("unknown action".into());
    }
    let p = crate::model::config_dir().ok_or("no config dir")?.join("cmd.json");
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() {
        return Err("the app is busy; retry".into());
    }
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let arr = v.as_array_mut().ok_or("cmd file unreadable")?;
    let mut o = serde_json::json!({ "action": action });
    if let Some(id) = id {
        o["id"] = serde_json::json!(id);
    }
    arr.push(o);
    let json = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&p, json.as_bytes()).map_err(|e| e.to_string())
}

/// The app theme as the extension understands it.
fn app_theme() -> String {
    match crate::model::ui_pref_str("theme", "system").as_str() {
        "light" => "light".to_string(),
        "dark" | "black" => "dark".to_string(),
        _ => "system".to_string(),
    }
}

/// Which of `urls` are already in the shared queue (read-only, locked).
fn queued_urls(urls: &[String]) -> Vec<String> {
    if urls.is_empty() {
        return Vec::new();
    }
    let _guard = crate::model::fs_lock("queue");
    if !_guard.owned() { return Vec::new(); }
    let items: Vec<crate::model::DownloadItem> = crate::model::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("queue.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    urls.iter()
        .filter(|u| items.iter().any(|d| &d.url == *u))
        .cloned()
        .collect()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InboxItem {
    pub url: String,
    pub filename: Option<String>,
    pub received_at: String,
    /// Page URL the media was captured from (used as Referer fallback).
    #[serde(default)]
    pub page_url: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    /// Captured request headers (credentials: kept in the private inbox,
    /// never on a command line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub pair_url: Option<String>,
    #[serde(default)]
    pub pair_mime: Option<String>,
    /// Batch presence check (`{"type":"query"}`): which of these URLs are
    /// already queued. Answered from queue.json without side effects.
    /// (Only read on Incoming; stored nowhere.)
    #[serde(default)]
    pub urls: Option<Vec<String>>,
    /// Preferred YouTube quality from the extension quality menu.
    #[serde(default)]
    pub quality: Option<String>,
    /// Media duration in seconds (page player / JSON, best-effort).
    /// Shown in the confirm dialog and checked against the completed file.
    #[serde(default)]
    pub dur: Option<u64>,
    #[serde(default, alias = "audioExpected")]
    pub audio_expected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<crate::model::BrowserContext>,
}

/// Lines the browser extension asked to show in the app's log.
pub fn notes_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("extension_notes.txt"))
}

/// Append one note (single line, bounded); the file is capped so a page
/// that reports in a loop cannot grow it.
fn append_extension_note(text: &str) {
    let Some(path) = notes_path() else { return };
    let line: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
    if line.trim().is_empty() {
        return;
    }
    let _guard = crate::model::fs_lock("extension_notes");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    if lines.iter().any(|l| l == &line) {
        return;
    }
    lines.push(line);
    let keep = lines.len().saturating_sub(50);
    let _ = std::fs::write(&path, lines[keep..].join("
") + "
");
}

/// Take every pending note (the app shows them in its log).
pub fn take_extension_notes() -> Vec<String> {
    let Some(path) = notes_path() else { return Vec::new() };
    let _guard = crate::model::fs_lock("extension_notes");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    let _ = std::fs::remove_file(&path);
    text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

pub fn inbox_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("native_inbox.json"))
}

/// One stdin frame: clean EOF (browser gone → exit the host) vs a broken
/// frame (reply with an error and keep serving — one bad message must not
/// kill the host and abandon everything behind it).
enum HostFrame {
    Eof,
    Bad(String),
    Msg(Incoming),
}

fn read_message() -> HostFrame {
    use std::io::Read;
    let mut len_buf = [0u8; 4];
    match std::io::stdin().read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return HostFrame::Eof,
        Err(_) => return HostFrame::Eof, // pipe broken: nothing more to serve
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return HostFrame::Eof; // clean close
    }
    if len > 4 * 1024 * 1024 {
        // Bigger than any real request. The length itself is valid framing,
        // so skip exactly that many bytes and keep serving: exiting dropped
        // every later capture on this port. Only a stream that ends early
        // (a corrupt prefix) closes the host.
        let skipped = std::io::copy(&mut std::io::stdin().take(len as u64), &mut std::io::sink());
        return match skipped {
            Ok(n) if n == len as u64 => HostFrame::Bad(format!("message too large ({len} bytes)")),
            _ => HostFrame::Eof,
        };
    }
    let mut buf = vec![0u8; len];
    if std::io::stdin().read_exact(&mut buf).is_err() {
        return HostFrame::Eof;
    }
    match serde_json::from_slice::<Incoming>(&buf) {
        Ok(m) => HostFrame::Msg(m),
        Err(e) => HostFrame::Bad(format!("bad message: {e}")),
    }
}

/// Request id of the message being answered (one at a time: replies are
/// written by the reading loop).
static CURRENT_RID: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);
/// Replies and pushes share stdout: one frame at a time.
static OUT: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn write_message(reply: &Reply) {
    let Ok(mut v) = serde_json::to_value(reply) else { return };
    // A pong also says which app this is and which extension it keeps in
    // its fixed folder: an older running extension reloads onto those files.
    if reply.kind == "pong" {
        v["app_version"] = serde_json::json!(env!("CARGO_PKG_VERSION"));
        if let Some(ver) = crate::extsync::kept_version() {
            v["ext_version"] = serde_json::json!(ver);
        }
        if let Some(dir) = crate::extsync::stable_dir() {
            v["ext_dir"] = serde_json::json!(dir.display().to_string());
        }
    }
    if let Some(rid) = *CURRENT_RID.lock().unwrap_or_else(|e| e.into_inner()) {
        v["rid"] = serde_json::json!(rid);
    }
    write_value(&v);
}

fn write_value(v: &serde_json::Value) {
    if let Ok(data) = serde_json::to_vec(v) {
        let len = (data.len() as u32).to_le_bytes();
        let _g = OUT.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = std::io::stdout();
        use std::io::Write;
        let _ = out.write_all(&len);
        let _ = out.write_all(&data);
        let _ = out.flush();
    }
}

// ---- push channel (persistent port) ----------------------------------------

/// Capture settings the user set in the app (only keys present in ui.json,
/// so the extension keeps its own choices for the rest).
pub fn capture_settings() -> serde_json::Value {
    let ui: serde_json::Value = crate::model::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut out = serde_json::Map::new();
    for (key, ext) in [
        ("capture_auto", "autoCapture"),
        ("capture_types", "captureTypes"),
        ("capture_skip_sites", "blockHosts"),
        ("capture_min_kb", "minSizeKB"),
        ("notify_done", "notify"),
        ("browser_fallback", "browserFallback"),
        ("capture_skip_key", "skipKey"),
    ] {
        if let Some(v) = ui.get(key) {
            out.insert(ext.to_string(), v.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Capture settings changed in the extension popup land in the app's
/// settings too, so the two never disagree.
fn set_prefs(prefs: &serde_json::Value) -> Result<(), String> {
    let mut changes: Vec<(&str, serde_json::Value)> = Vec::new();
    if let Some(b) = prefs.get("autoCapture").and_then(|v| v.as_bool()) {
        changes.push(("capture_auto", b.into()));
    }
    if let Some(n) = prefs.get("minSizeKB").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))) {
        changes.push(("capture_min_kb", n.min(10_000_000).into()));
    }
    if let Some(h) = prefs.get("blockHosts").and_then(|v| v.as_str()) {
        let h: String = h.chars().filter(|c| !c.is_control()).take(2000).collect();
        changes.push(("capture_skip_sites", h.into()));
    }
    if changes.is_empty() {
        return Ok(());
    }
    let path = crate::model::config_dir().ok_or("no config dir")?.join("ui.json");
    let _guard = crate::model::fs_lock("ui-settings");
    if !_guard.owned() {
        return Err("settings are busy; retry".into());
    }
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = v.as_object_mut().ok_or("settings unreadable")?;
    for (k, val) in changes {
        obj.insert(k.to_string(), val);
    }
    let json = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string())
}

/// Built-in site rules (also published at the repository root, where the
/// app looks for newer ones).
pub const DEFAULT_SITE_RULES: &str = include_str!("../site_rules.json");

/// Where the app fetches newer site rules (at most once a day).
const SITE_RULES_URL: &str = "https://raw.githubusercontent.com/hungry-detective/downloadcraft/master/site_rules.json";

/// Refresh `site_rules.json` from the project when the copy is a day old.
/// A rules file only replaces ours when it parses and is newer.
pub fn refresh_site_rules() {
    let Some(dir) = crate::model::config_dir() else { return };
    let path = dir.join("site_rules.json");
    let version = |v: &serde_json::Value| v.get("version").and_then(|x| x.as_u64()).unwrap_or(0);
    let builtin: serde_json::Value = serde_json::from_str(DEFAULT_SITE_RULES).unwrap_or_default();
    let current: Option<serde_json::Value> = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok());
    if current.as_ref().map_or(true, |c| version(c) < version(&builtin)) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = crate::model::atomic_write(&path, DEFAULT_SITE_RULES.as_bytes());
    }
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(24 * 3600));
    if fresh && current.is_some() {
        return;
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
    let got = rt.block_on(async {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().ok()?;
        let r = client.get(SITE_RULES_URL).send().await.ok()?.error_for_status().ok()?;
        r.text().await.ok()
    });
    let mine = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).unwrap_or_default();
    match got.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok().map(|v| (t, v))) {
        Some((text, v)) if v.is_object() && version(&v) > version(&mine) => {
            let _ = crate::model::atomic_write(&path, text.as_bytes());
        }
        // Nothing newer: touch the file so the next check waits a day.
        _ => {
            let _ = crate::model::atomic_write(&path, serde_json::to_string_pretty(&mine).unwrap_or_default().as_bytes());
        }
    }
}

/// Site rules the app keeps (`site_rules.json`, refreshed by the app).
pub fn site_rules() -> serde_json::Value {
    crate::model::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("site_rules.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        // Before the app has written its copy: the rules it was built with.
        .or_else(|| serde_json::from_str(DEFAULT_SITE_RULES).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Folder for page-fetch requests (app -> extension) and their results.
pub fn page_fetch_dir() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|d| d.join("pagefetch"))
}

fn safe_fetch_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// One queue row as an event for the extension.
fn row_event(d: &crate::model::DownloadItem, event: &str, error: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "event", "event": event, "id": d.id, "url": d.url, "name": d.filename,
        "error": error, "page": d.referer, "quality": d.quality_pref,
        "path": d.dest_path().to_string_lossy(),
    })
}

/// Pushes settings, finished/failed downloads and page-fetch requests to the
/// extension until the browser closes the port (the process then exits).
fn push_loop() {
    use crate::model::{DownloadItem, Status};
    let mut last_settings = serde_json::Value::Null;
    let mut known: std::collections::HashMap<u64, u8> = std::collections::HashMap::new();
    let mut first = true;
    let mut asked: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let settings = serde_json::json!({ "type": "settings", "settings": capture_settings(), "rules": site_rules() });
        if settings != last_settings {
            write_value(&settings);
            last_settings = settings;
        }
        // Rows the extension sent: tell it when they finish or fail.
        let items: Vec<DownloadItem> = crate::model::config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("queue.json")).ok())
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default();
        for d in &items {
            if !d.browser.as_ref().is_some_and(|b| b.from_extension) {
                continue;
            }
            let state = match &d.status {
                Status::Completed => 2,
                Status::Failed(_) => 3,
                _ => 1,
            };
            let before = known.insert(d.id, state);
            if first || before == Some(state) || state == 1 {
                continue;
            }
            match &d.status {
                Status::Completed => write_value(&row_event(d, "done", "")),
                Status::Failed(e) => write_value(&row_event(d, "failed", e)),
                _ => {}
            }
        }
        first = false;
        // Page-fetch requests the app is waiting on.
        if let Some(dir) = page_fetch_dir() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let Some(id) = name.strip_suffix(".req.json") else { continue };
                    if !safe_fetch_id(id) || asked.contains(id) {
                        continue;
                    }
                    asked.insert(id.to_string());
                    if let Some(req) = std::fs::read_to_string(e.path()).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
                        write_value(&serde_json::json!({
                            "type": "pageFetch", "id": id,
                            "url": req.get("url"), "page": req.get("page"),
                        }));
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}

/// Store one piece of a page-fetch answer; the last piece publishes it.
fn page_fetch_chunk(msg: &Incoming) -> Result<(), String> {
    let id = msg.fetch_id.clone().unwrap_or_default();
    if !safe_fetch_id(&id) {
        return Err("bad fetch id".into());
    }
    let dir = page_fetch_dir().ok_or("no config dir")?;
    if !dir.join(format!("{id}.req.json")).exists() {
        return Err("no such fetch request".into());
    }
    let part = dir.join(format!("{id}.part"));
    if let Some(err) = msg.error.clone().filter(|e| !e.is_empty()) {
        let _ = std::fs::remove_file(&part);
        let body = serde_json::json!({ "error": err, "status": msg.status });
        return std::fs::write(dir.join(format!("{id}.err")), body.to_string()).map_err(|e| e.to_string());
    }
    if let Some(c) = msg.chunk.as_deref().filter(|c| !c.is_empty()) {
        let bytes = crate::model::base64_decode(c).ok_or("bad chunk")?;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&part).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    if msg.done {
        if !part.exists() {
            std::fs::write(&part, b"").map_err(|e| e.to_string())?;
        }
        std::fs::rename(&part, dir.join(format!("{id}.res"))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Just the name of a suggested filename. Browsers report a download's whole
/// save path (Linux: "/home/me/Downloads/x.zip"), and cleaning that as one
/// name gave "_home_me_Downloads_x.zip" (issue #6).
fn file_part(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    (!last.is_empty()).then(|| last.to_string())
}

/// Keep only headers the engine may replay, with sane sizes. The extension
/// already filters; this is the trust boundary, so filter again.
fn replayable_headers(
    headers: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<std::collections::BTreeMap<String, String>> {
    let kept: std::collections::BTreeMap<String, String> = headers?
        .iter()
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .filter(|(k, v)| {
            crate::engine::replayable_header(k)
                && k.len() <= 64
                && !v.is_empty()
                && v.len() <= 4096
                && !v.contains(['\r', '\n'])
        })
        .take(20)
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// One capture's inbox identity: the resource URL plus the requested quality.
/// The queue and `poll_inbox` match captures the same way, so two quality
/// choices of one page stay two entries end to end.
fn same_capture(item: &InboxItem, url: &str, quality: &str) -> bool {
    item.url == url
        && crate::model::same_quality(item.quality.as_deref().unwrap_or(""), quality)
}

fn queue_download(msg: &Incoming) -> Result<bool, String> {
    const MAX_INBOX: usize = 200;
    let url = msg.url.clone().unwrap_or_default();
    let path = inbox_path().ok_or_else(|| "no config dir".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _guard = crate::model::fs_lock("inbox");
    if !_guard.owned() { return Err("Download inbox is busy; retry shortly".into()); }
    // Never destroy pending captures on an unreadable inbox.
    let data = std::fs::read_to_string(&path).ok();
    let mut items: Vec<InboxItem> = match data {
        None => Vec::new(), // no file yet
        Some(s) => serde_json::from_str(&s)
            .map_err(|_| "inbox unreadable — kept for the next poll".to_string())?,
    };
    let referer = msg
        .referer
        .clone()
        .or_else(|| msg.page_url.clone())
        .filter(|s| !s.trim().is_empty());
    let fresh = InboxItem {
        url: url.clone(),
        // A name with no extension takes the one its MIME type names (an
        // image saved from an address without one).
        filename: msg.filename.as_deref().and_then(file_part).map(|n| {
            crate::model::with_mime_ext(&n, msg.mime.as_deref().unwrap_or(""))
        }),
        received_at: chrono::Local::now().to_rfc3339(),
        page_url: msg.page_url.clone(),
        referer,
        cookie: msg.cookie.clone().filter(|s| !s.trim().is_empty()),
        headers: replayable_headers(msg.headers.as_ref()),
        mime: msg.mime.clone().filter(|s| !s.trim().is_empty()),
        size: msg.size.filter(|s| *s > 0),
        pair_url: msg.pair_url.clone().filter(|s| s.starts_with("http")),
        pair_mime: msg.pair_mime.clone().filter(|s| !s.trim().is_empty()),
        urls: None,
        quality: msg.quality.clone().filter(|s| !s.trim().is_empty()),
        dur: msg.dur.filter(|s| *s > 0),
        audio_expected: msg.audio_expected,
        browser: msg.browser.clone().and_then(|b| b.sanitized()),
    };
    // Same capture re-captured: refresh metadata instead of ignoring it (the
    // filename/referer/cookie may be corrected this time). Quality is part of
    // the identity, exactly as in the queue and `poll_inbox`: one page at
    // 1080p and at 720p is two downloads, so neither may overwrite the other
    // here before the manager reads the inbox.
    let quality_key = fresh.quality.clone().unwrap_or_default();
    let is_new = match items
        .iter_mut()
        .find(|i| same_capture(i, &url, &quality_key))
    {
        Some(existing) => {
            *existing = fresh;
            false
        }
        None => {
            items.push(fresh);
            true
        }
    };
    if items.len() > MAX_INBOX {
        items.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        items.truncate(MAX_INBOX);
    }
    let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
    crate::model::atomic_write(&path,json.as_bytes()).map_err(|e|e.to_string())?;
    Ok(is_new)
}

/// Fire-and-forget the standalone Download File Info dialog (detached, so
/// it outlives this short-lived native-host process).
fn spawn_confirm(msg: &Incoming) {
    let url = msg.url.clone().unwrap_or_default();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let Ok(exe) = crate::platform::launch_path() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--confirm-download").arg("--url").arg(&url);
    if let Some(f) = msg.filename.as_deref().and_then(file_part) {
        cmd.arg("--filename").arg(f);
    }
    // The engine reads this as both the Referer and "the page the media came
    // from" (YouTube fresh-link retry, social page fallback), so the page
    // wins over an origin-only Referer ("https://www.youtube.com/"). A
    // Referer with a real path is an embedded player's page, which the CDN
    // checks (sending the article URL instead got 403s): that one wins.
    let page = msg.page_url.clone().filter(|s| !s.trim().is_empty());
    let captured = msg.referer.clone().filter(|s| !s.trim().is_empty());
    let has_path = |u: &str| reqwest::Url::parse(u).map(|x| x.path() != "/" && !x.path().is_empty()).unwrap_or(false);
    let pick = match (page, captured) {
        (Some(p), Some(r)) if r != p && has_path(&r) => Some(r),
        (Some(p), _) => Some(p),
        (None, r) => r,
    };
    if let Some(r) = pick {
        cmd.arg("--referer").arg(r);
    }
    // The dialog claims credentials from the owner-only inbox.
    if let Some(m) = msg.mime.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--mime").arg(m);
    }
    if let Some(s) = msg.size.filter(|s| *s > 0) {
        cmd.arg("--size").arg(s.to_string());
    }
    if let Some(p) = msg.pair_url.clone().filter(|s| s.starts_with("http")) {
        cmd.arg("--pair-url").arg(p);
        if let Some(m) = msg.pair_mime.clone().filter(|s| !s.trim().is_empty()) {
            cmd.arg("--pair-mime").arg(m);
        }
    }
    if let Some(q) = msg.quality.clone().filter(|s| !s.trim().is_empty()) {
        cmd.arg("--quality").arg(q);
    }
    if msg.audio_expected { cmd.arg("--expect-audio"); }
    if let Some(d) = msg.dur.filter(|s| *s > 0) {
        cmd.arg("--dur").arg(d.to_string());
    }
    spawn_detached(cmd);
}

/// The extension just talked to the app: the "Get ready" window shows it as
/// connected from now on.
fn note_extension_seen() {
    if let Some(p) = crate::model::config_dir().map(|d| d.join("extension_seen.json")) {
        let origin = std::env::args().find(|a| a.contains("-extension://")).unwrap_or_default();
        let v = serde_json::json!({ "at": chrono::Local::now().to_rfc3339(), "origin": origin });
        let _ = std::fs::write(p, v.to_string());
    }
}

/// Entry point when launched with `--native-host`. Never returns normally.
pub fn run() -> eframe::Result<()> {
    // Keep stderr quiet; any panic text would corrupt the pipe (stdout stays clean).
    loop {
        match read_message() {
            HostFrame::Eof => break,
            HostFrame::Bad(e) => {
                write_message(&Reply::err(e));
                continue;
            }
            HostFrame::Msg(msg) => {
            *CURRENT_RID.lock().unwrap_or_else(|e| e.into_inner()) = msg.rid;
            match msg.kind.as_str() {
            "hello" => {
                note_extension_seen();
                if !PUSHING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    std::thread::spawn(push_loop);
                }
                write_message(&Reply {
                    kind: "pong".into(),
                    version: Some(2),
                    queued: None,
                    error: None,
                    urls: None,
                    theme: Some(app_theme()),
                    items: None,
                });
            }
            "setPrefs" => match msg.prefs.as_ref().map(set_prefs).unwrap_or(Ok(())) {
                Ok(()) => write_message(&Reply::ok()),
                Err(e) => write_message(&Reply::err(e)),
            },
            "pageFetchChunk" => match page_fetch_chunk(&msg) {
                Ok(()) => write_message(&Reply::ok()),
                Err(e) => write_message(&Reply::err(e)),
            },
            "report" => {
                if let Some(text) = msg.note.as_deref() {
                    append_extension_note(text);
                }
                write_message(&Reply::ok());
            }
            "ping" => write_message(&{ note_extension_seen(); Reply {
                kind: "pong".into(),
                version: Some(1),
                queued: None,
                error: None,
                urls: None,
                theme: Some(app_theme()),
                items: None,
            } }),
            "query" => {
                // Presence check only: no queueing, no dialogs, no locks held
                // beyond the read.
                let urls = msg.urls.clone().unwrap_or_default();
                let hits = queued_urls(&urls);
                write_message(&Reply {
                    kind: "queued".into(),
                    version: None,
                    queued: None,
                    error: None,
                    urls: Some(hits),
                    theme: None,
                    items: None,
                });
            }
            "download" => {
                let url = msg.url.clone().unwrap_or_default();
                let ok = url.starts_with("http://") || url.starts_with("https://");
                if !ok {
                    write_message(&Reply::err("need http/https url"));
                    continue;
                }
                // Blob/media-source URLs are not directly downloadable by the
                // engine — reject loudly so the extension can fall back to the
                // page URL instead of queueing a dead link.
                if url.starts_with("blob:") {
                    write_message(&Reply::err(
                        "blob: URLs need the page URL — send the page link instead",
                    ));
                    continue;
                }
                match queue_download(&msg) {
                    // A row is waiting for a fresh link ("Refresh download
                    // address"): the app takes this capture, no dialog.
                    Ok(_) if crate::model::refresh_wait().is_some() => write_message(&Reply::ok()),
                    Ok(_is_new) => {
                        write_message(&Reply::ok());
                        // IDM-style: pop the standalone Download File Info
                        // window in front. The inbox entry doubles as a
                        // fallback — the dialog claims it on open, and the
                        // main window only prompts for entries the dialog
                        // never claimed (see poll_inbox age gate).
                        spawn_confirm(&msg);
                    }
                    Err(e) => write_message(&Reply::err(e)),
                }
            }
            "list" => write_message(&Reply {
                kind: "list".into(),
                version: None,
                queued: None,
                error: None,
                urls: None,
                theme: None,
                items: Some(app_downloads()),
            }),
            "record" => match record_browser_download(&msg) {
                Ok(()) => write_message(&Reply::ok()),
                Err(e) => write_message(&Reply::err(e)),
            },
            "control" => match app_control(msg.id, msg.action.as_deref().unwrap_or("")) {
                Ok(()) => write_message(&Reply::ok()),
                Err(e) => write_message(&Reply::err(e)),
            },
            "readFile" => match read_finished_file(msg.id, msg.offset.unwrap_or(0)) {
                Ok(v) => {
                    let mut v = v;
                    if let Some(rid) = msg.rid {
                        v["rid"] = serde_json::json!(rid);
                    }
                    write_value(&v);
                }
                Err(e) => write_message(&Reply::err(e)),
            },
            "links" => match spawn_links(&msg) {
                Ok(()) => write_message(&Reply::ok()),
                Err(e) => write_message(&Reply::err(e)),
            },
            _ => write_message(&Reply::err("unknown message")),
            }
            }
        }
    }
    Ok(())
}

static PUSHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
mod media_tests {
    use super::*;
    #[test]
    fn player_expectations_survive_browser_and_inbox_formats() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/video.mp4","dur":2700,"audioExpected":true}"#).unwrap();
        assert_eq!(msg.dur, Some(2700));
        assert!(msg.audio_expected);
        let stored: InboxItem = serde_json::from_str(r#"{"url":"https://cdn.example.org/video.mp4","filename":null,"received_at":"now","dur":2700,"audio_expected":true}"#).unwrap();
        assert_eq!(stored.dur, Some(2700));
        assert!(stored.audio_expected);
        let old: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/video.mp4"}"#).unwrap();
        assert!(!old.audio_expected);
        assert!(old.headers.is_none());
    }

    #[test]
    fn captured_headers_keep_only_replayable_tokens() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://cdn.example.org/v",
            "headers":{"Origin":"https://site.example","authorization":"Bearer t","x-token":"1",
                       "x-client-data":"variations","Range":"bytes=0-","host":"evil","x-bad":"a\r\nb: c"}}"#).unwrap();
        let kept = replayable_headers(msg.headers.as_ref()).unwrap();
        assert_eq!(kept.keys().cloned().collect::<Vec<_>>(), ["authorization", "origin", "x-token"]);
        assert!(replayable_headers(None).is_none());
        let none = std::collections::BTreeMap::from([("cookie".to_string(), "a=b".to_string())]);
        assert!(replayable_headers(Some(&none)).is_none());
    }

    #[test]
    fn browser_launches_without_the_flag_are_recognised() {
        let ext = format!("chrome-extension://{}/", crate::app::EXTENSION_ID);
        assert!(launched_directly(&["dc.exe".into(), ext, "--parent-window=0".into()]));
        assert!(launched_directly(&["dc.exe".into(), r"C:\m.json".into(), "send-to-download-craft@example".into()]));
        assert!(!launched_directly(&["dc.exe".into(), "chrome-extension://someoneelse/".into()]));
        assert!(!launched_directly(&["dc.exe".into(), "--settings".into()]));
    }

    #[test]
    fn suggested_paths_keep_only_the_file_name() {
        assert_eq!(file_part("/home/me/Downloads/a.zip").as_deref(), Some("a.zip"));
        assert_eq!(file_part(r"C:\Users\me\Downloads\b.exe").as_deref(), Some("b.exe"));
        assert_eq!(file_part("plain.txt").as_deref(), Some("plain.txt"));
        assert_eq!(file_part("/home/me/"), None);
    }

    #[test]
    fn browser_context_is_sanitized_at_the_inbox() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"download","url":"https://site.example/r","rid":7,
            "browser":{"method":"post","body":"YT0xJmI9Mg==","fromExtension":true,"serverIp":"203.0.113.9",
              "proxy":"socks5://127.0.0.1:9050","headers":{"User-Agent":"UA/1","Cookie":"a=b","Accept":"*/*","Range":"bytes=0-"}}}"#).unwrap();
        assert_eq!(msg.rid, Some(7));
        let b = msg.browser.clone().unwrap().sanitized().unwrap();
        assert!(b.is_post());
        assert_eq!(crate::model::base64_decode(&b.body).unwrap(), b"a=1&b=2");
        assert!(b.from_extension);
        assert_eq!(b.server_ip, "203.0.113.9");
        assert_eq!(b.proxy, "socks5://127.0.0.1:9050");
        assert_eq!(b.headers.keys().cloned().collect::<Vec<_>>(), ["accept", "user-agent"]);
        let bad: crate::model::BrowserContext = serde_json::from_str(r#"{"method":"DELETE","serverIp":"x","proxy":"file:///etc"}"#).unwrap();
        assert!(bad.sanitized().is_none());
        assert_eq!(crate::model::base64_encode(b"a=1&b=2"), "YT0xJmI9Mg==");
        assert!(safe_fetch_id("f1a2-3") && !safe_fetch_id("../x") && !safe_fetch_id(""));
    }

    #[test]
    fn built_in_site_rules_parse() {
        let v: serde_json::Value = serde_json::from_str(DEFAULT_SITE_RULES).unwrap();
        assert!(v.get("version").and_then(|x| x.as_u64()).unwrap() >= 1);
        assert!(v.get("youtube").and_then(|y| y.get("itagQuality")).is_some());
    }

    #[test]
    fn page_links_keep_only_unique_web_urls() {
        let msg: Incoming = serde_json::from_str(r#"{"type":"links","pageUrl":"https://site.example/dir/",
            "links":[{"url":"https://site.example/a.mp4","text":"A\u0007 video"},
                     {"url":"https://site.example/a.mp4","text":"again"},
                     {"url":"javascript:alert(1)"},{"url":"file:///c:/x"},
                     {"url":"http://other.example/b.zip"}]}"#).unwrap();
        let b = links_batch("https://site.example/dir/", "", "", msg.links.as_deref().unwrap());
        assert_eq!(b.links.len(), 2);
        assert_eq!(b.links[0].text, "A video");
        assert_eq!(b.links[1].url, "http://other.example/b.zip");
    }

    #[test]
    fn inbox_identity_keeps_each_quality_of_one_page() {
        // One page captured at two qualities must stay two inbox entries: they
        // are two downloads (the queue and poll_inbox match them the same way).
        let hd: InboxItem = serde_json::from_str(
            r#"{"url":"https://www.youtube.com/watch?v=ABC","received_at":"2026-11-05","quality":"1080p"}"#,
        )
        .unwrap();
        assert!(same_capture(&hd, "https://www.youtube.com/watch?v=ABC", "1080p"));
        assert!(same_capture(&hd, "https://www.youtube.com/watch?v=ABC", " 1080P "));
        assert!(!same_capture(&hd, "https://www.youtube.com/watch?v=ABC", "720p"));
        assert!(!same_capture(&hd, "https://www.youtube.com/watch?v=XYZ", "1080p"));
        // No quality choice: an empty capture only matches another empty one.
        let plain: InboxItem = serde_json::from_str(
            r#"{"url":"https://example.com/f.zip","received_at":"2026-11-05"}"#,
        )
        .unwrap();
        assert!(same_capture(&plain, "https://example.com/f.zip", ""));
        assert!(!same_capture(&plain, "https://example.com/f.zip", "720p"));
    }
}
