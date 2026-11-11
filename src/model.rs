use chrono::{DateTime, Local, Timelike};

/// Public application version. Keep this aligned with Cargo and extension manifests.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Queued => "Queued".to_string(),
            Status::Downloading => "Downloading".to_string(),
            Status::Paused => "Paused".to_string(),
            Status::Completed => "Completed".to_string(),
            Status::Failed(e) => format!("Failed: {}", shorten(e, 40)),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Status::Downloading)
    }
}

fn shorten(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    // Byte slicing panics on multibyte boundaries (called every frame via
    // Status::label with network-controlled error text) — cut at a char edge.
    let cut = s
        .char_indices()
        .take_while(|(i, _)| *i < n)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}…", &s[..cut])
}

/// OS-held interprocess lock. Lock files persist; the handle owns the lock,
/// which the OS releases on close/crash. A long download must never lose its
/// destination lock merely because a timestamp is old.
pub struct FsGuard {
    _file: Option<std::fs::File>,
}
impl FsGuard {
    pub fn owned(&self) -> bool {
        self._file.is_some()
    }
}
pub fn fs_lock(name: &str) -> FsGuard {
    fn acquire(name: &str) -> Option<std::fs::File> {
        let path = config_dir()?.join(format!("{name}.lock"));
        std::fs::create_dir_all(path.parent()?).ok()?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .ok()?;
        for _ in 0..100 {
            match file.try_lock() {
                Ok(()) => return Some(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                Err(_) => return None,
            }
        }
        None
    }
    FsGuard {
        _file: acquire(name),
    }
}

/// Liveness heartbeat for the main window (`main.lock` timestamp, refreshed
/// every few seconds). Standalone dialogs use this instead of counting
/// process names (two dialogs used to look like "main is running").
pub fn touch_heartbeat() {
    if let Some(p) = config_dir().map(|d| d.join("main.lock")) {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, chrono::Local::now().to_rfc3339());
    }
}

/// The main window is closing: say so at once, so a capture dialog opened
/// right after does not take the app for running.
pub fn clear_heartbeat() {
    if let Some(p) = config_dir().map(|d| d.join("main.lock")) {
        let _ = std::fs::remove_file(p);
    }
}

/// Whether the main instance still holds its `win/main` slot. The OS drops
/// that lock when the owning process ends and only then, so it is the
/// authoritative answer to "is the app running" — unlike a timestamp, which
/// goes stale whenever the GUI stops drawing frames (minimized, covered, or
/// on a desktop that simply stops scheduling the window).
fn main_instance_held() -> bool {
    let Some(dir) = config_dir().map(|d| d.join("win")) else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // The same no-sharing open the owner uses: it fails while the slot
        // is taken and succeeds (for this probe) when it is free.
        opts.share_mode(0);
    }
    let Ok(file) = opts.open(dir.join("main")) else {
        // Windows refuses the open outright while the owner holds the slot.
        return cfg!(windows);
    };
    #[cfg(unix)]
    {
        if let Err(std::fs::TryLockError::WouldBlock) = file.try_lock() {
            return true;
        }
        let _ = file.unlock();
    }
    let _ = &file;
    false
}

pub fn is_main_live() -> bool {
    // The instance lock first: the OS holds it for exactly as long as the
    // process lives, so a minimized window can never look like a dead app.
    if main_instance_held() {
        return true;
    }
    // No slot claimed (an older build, or a launcher that skipped it): a
    // fresh heartbeat still counts.
    let Some(p) = config_dir().map(|d| d.join("main.lock")) else {
        return false;
    };
    let Ok(data) = std::fs::read_to_string(&p) else {
        return false;
    };
    chrono::DateTime::parse_from_rfc3339(data.trim())
        // Refreshed every 2 s while the app runs; 30 s here kept a just-closed
        // app "running" long enough to queue captures that then sat there.
        .map(|dt| chrono::Local::now().timestamp() - dt.timestamp() < 10)
        .unwrap_or(false)
}

static HEARTBEAT_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Refresh the heartbeat from a thread of its own. The GUI loop only runs
/// while frames are being drawn, and a minimized window can be skipped by the
/// compositor for minutes: when the heartbeat came from that loop, `main.lock`
/// went stale and a capture dialog declared the running app stopped.
pub fn start_heartbeat() {
    if HEARTBEAT_ON.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        while HEARTBEAT_ON.load(std::sync::atomic::Ordering::SeqCst) {
            touch_heartbeat();
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

/// Shutdown: stop refreshing and remove the timestamp together, so a dialog
/// opened right after does not take the closing app for running.
pub fn stop_heartbeat() {
    HEARTBEAT_ON.store(false, std::sync::atomic::Ordering::SeqCst);
    clear_heartbeat();
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub save_dir: String,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub status: Status,
    /// This download's own speed limit (KB/s, 0 = none), set from its
    /// progress window; kept for later runs.
    #[serde(default)]
    pub limit_kbps: u64,
    #[serde(default)]
    pub speed_bps: f64,
    pub created_at: DateTime<Local>,
    pub connections: usize,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub referer: String,
    #[serde(default)]
    pub cookie: String,
    /// Origin / Authorization / x-* headers the browser player sent with the
    /// captured media (see engine::CapturedHeaders). Cleared on completion
    /// like the cookie.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extra_headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub resumable: bool,
    /// Optional expected SHA-256 hex digest for integrity verification.
    /// Empty = no verification. Clean-room feature inspired by
    /// multi-protocol managers (no reference code copied).
    #[serde(default)]
    pub checksum_sha256: String,
    /// Free-text note from the IDM-style Download File Info dialog.
    /// Empty = none. Never used for engine behavior.
    #[serde(default)]
    pub description: String,
    /// Adaptive pair: companion audio URL for a video-only stream, merged
    /// with ffmpeg on completion (P1.3). Empty = single-file download.
    #[serde(default)]
    pub pair_url: String,
    #[serde(default)]
    pub pair_mime: String,
    /// Replace an existing destination file instead of auto-renaming.
    #[serde(default)]
    pub overwrite_existing: bool,
    /// Preferred YouTube quality picked in the extension menu (e.g. "720p",
    /// "1080p60"). Empty = best available. The engine picks the highest
    /// stream at or below it (ciphered pages expose no direct URLs, so the
    /// choice can only be honored server-side at resolve time).
    #[serde(default)]
    pub quality_pref: String,
    /// The resolution that actually landed on disk ("1280×720"), read from the
    /// helper's chosen format or the finished file with ffprobe. Empty =
    /// unknown. Display only — never decides status.
    #[serde(default)]
    pub resolution: String,
    /// Media duration in seconds from the extension (best-effort, P4).
    /// Used to check the completed file when ffprobe is available.
    #[serde(default)]
    pub duration_secs: u64,
    /// Positive audio evidence from the captured player; unknown stays false.
    #[serde(default)]
    pub expects_audio: bool,
    /// Per-download retry budget (P5). None = use the global setting.
    /// Some(n) overrides it for this item only (clamped 1–20 at use).
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Per-download stall timeout in seconds (P5). None = global.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Checksum-mismatch auto-repair latch (P5): set once the app spends
    /// its single automatic from-scratch redownload after a mismatch, so
    /// a persistently corrupt source fails loudly instead of looping.
    #[serde(default)]
    pub checksum_repaired: bool,
    /// Queue v2 ordering: higher runs first; ties keep creation order.
    #[serde(default)]
    pub priority: i64,
    /// Queue v2 scheduled start, "HH:MM" local. Empty = as soon as the queue
    /// can take it. A time already past starts immediately.
    #[serde(default)]
    pub start_at: String,
    /// What the browser knew about this download when the extension caught
    /// it (method/body, its request headers, server IP, proxy). None for
    /// downloads added any other way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserContext>,
    /// Other URLs for the same file (mirrors): connections spread over them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mirrors: Vec<String>,
    /// Named queue this row belongs to; empty = the Main queue.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub queue: String,
}

/// A named queue besides Main (IDM: several queues, each with its own
/// schedule and number of simultaneous downloads).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct QueueDef {
    pub name: String,
    /// Downloads at the same time.
    pub max: usize,
    /// Start / stop by itself at this local time ("HH:MM"; empty = never).
    #[serde(default)]
    pub start_at: String,
    #[serde(default)]
    pub stop_at: String,
    /// Check finished files for a newer version on the server every N hours
    /// (0 = never) and download the changed ones again (IDM's queue sync).
    #[serde(default)]
    pub sync_hours: u64,
    #[serde(skip)]
    pub running: bool,
    /// Date the start / stop time last fired (once a day each).
    #[serde(skip)]
    pub fired_start: String,
    #[serde(skip)]
    pub fired_stop: String,
    #[serde(skip)]
    pub last_sync: Option<std::time::Instant>,
}

/// "https://a/f.iso | https://b/f.iso": the first is the download, the rest
/// are mirrors of it (IDM's alternative addresses).
pub fn split_mirrors(text: &str) -> (String, Vec<String>) {
    let mut parts = text.split(['|', '\n']).map(str::trim).filter(|s| !s.is_empty());
    let main = parts.next().unwrap_or("").to_string();
    let mirrors = parts.filter(|u| is_download_url(u) && *u != main).map(str::to_string).collect();
    (main, mirrors)
}

/// The browser's side of a captured download, so the app can repeat the
/// request the way the browser made it (IDM sends the same).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BrowserContext {
    /// "POST" for form downloads; empty = GET.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// POST body, base64.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// The browser's own request headers (User-Agent, Accept, Accept-Language,
    /// client hints, Content-Type of a POST). Never cookies or credentials:
    /// those travel in their own, site-bound fields.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    /// The server address the browser was connected to.
    #[serde(default, alias = "serverIp", skip_serializing_if = "String::is_empty")]
    pub server_ip: String,
    /// The browser's proxy ("http://host:port", "socks5://host:port").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy: String,
    /// YouTube's visitor id from the page (helps yt-dlp past bot checks).
    #[serde(default, alias = "visitorData", skip_serializing_if = "String::is_empty")]
    pub visitor_data: String,
    /// Sent by the browser extension (it can take the download back).
    #[serde(default, alias = "fromExtension")]
    pub from_extension: bool,
}

/// Request headers from the browser that the app may repeat. Hop-by-hop,
/// range, cookie and credential headers are never among them.
pub fn browser_header_allowed(name: &str) -> bool {
    matches!(
        name,
        "user-agent" | "accept" | "accept-language" | "content-type" | "upgrade-insecure-requests" | "dnt"
    ) || name.starts_with("sec-ch-ua")
        || name.starts_with("sec-fetch-")
}

impl BrowserContext {
    /// Clean what the extension sent (this is the trust boundary).
    pub fn sanitized(mut self) -> Option<Self> {
        self.method = self.method.trim().to_ascii_uppercase();
        if self.method != "POST" {
            self.method.clear();
            self.body.clear();
        }
        if self.body.len() > 2 * 1024 * 1024 || base64_decode(&self.body).is_none() {
            self.body.clear();
            if self.method == "POST" {
                // A POST whose body could not be kept cannot be repeated.
                self.method.clear();
            }
        }
        self.headers = std::mem::take(&mut self.headers)
            .into_iter()
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .filter(|(k, v)| browser_header_allowed(k) && !v.is_empty() && v.len() <= 2048 && !v.contains(['\r', '\n']))
            .take(24)
            .collect();
        if self.server_ip.parse::<std::net::IpAddr>().is_err() {
            self.server_ip.clear();
        }
        let p = self.proxy.trim().to_ascii_lowercase();
        if !(p.starts_with("http://") || p.starts_with("https://") || p.starts_with("socks5://") || p.starts_with("socks5h://") || p.starts_with("socks4://"))
            || self.proxy.len() > 300
            || self.proxy.contains(['\r', '\n', ' '])
        {
            self.proxy.clear();
        }
        if self.visitor_data.len() > 200 || !self.visitor_data.chars().all(|c| c.is_ascii_alphanumeric() || "-_%=.".contains(c)) {
            self.visitor_data.clear();
        }
        let empty = self.method.is_empty() && self.headers.is_empty() && self.server_ip.is_empty()
            && self.proxy.is_empty() && self.visitor_data.is_empty() && !self.from_extension;
        (!empty).then_some(self)
    }

    pub fn is_post(&self) -> bool {
        self.method == "POST"
    }
}

/// Standard base64 (with or without padding). None on bad input.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'=').collect();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= val(c)? << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

/// Standard base64 with padding.
pub fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

impl DownloadItem {
    pub fn new(id: u64, url: String, filename: String, save_dir: String, connections: usize) -> Self {
        Self {
            id,
            url,
            filename,
            save_dir,
            total_bytes: None,
            downloaded_bytes: 0,
            status: Status::Queued,
            limit_kbps: 0,
            speed_bps: 0.0,
            created_at: Local::now(),
            connections: connections.clamp(1, 32),
            selected: false,
            referer: String::new(),
            cookie: String::new(),
            extra_headers: std::collections::BTreeMap::new(),
            username: String::new(),
            password: String::new(),
            resumable: false,
            checksum_sha256: String::new(),
            description: String::new(),
            pair_url: String::new(),
            pair_mime: String::new(),
            overwrite_existing: false,
            quality_pref: String::new(),
            resolution: String::new(),
            duration_secs: 0,
            expects_audio: false,
            max_retries: None,
            timeout_secs: None,
            checksum_repaired: false,
            priority: 0,
            start_at: String::new(),
            browser: None,
            mirrors: Vec::new(),
            queue: String::new(),
        }
    }

    pub fn dest_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.save_dir).join(&self.filename)
    }

    /// Effective retry budget: per-download override or the global setting.
    pub fn eff_retries(&self, global: u32) -> u32 {
        self.max_retries.unwrap_or(global).clamp(1, 20).max(1)
    }

    /// Effective stall timeout: per-download override or the global setting.
    pub fn eff_timeout(&self, global: u64) -> u64 {
        self.timeout_secs.unwrap_or(global).clamp(5, 600).max(5)
    }

    pub fn progress(&self) -> f32 {
        match self.total_bytes {
            Some(t) if t > 0 => (self.downloaded_bytes as f64 / t as f64).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        }
    }

    pub fn category(&self) -> &'static str {
        category_of(&self.filename)
    }
}

/// Queue v2: whether a row's "start at" time has arrived. Blank = now.
/// A time already past also means now (set it and it runs), and an
/// unparsable value never blocks a download forever.
pub fn start_at_due(s: &str, now: DateTime<Local>) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    match parse_hhmm(t) {
        Some(mins) => {
            let now_secs = now.hour() * 3600 + now.minute() * 60 + now.second();
            mins as u32 * 60 <= now_secs
        }
        None => true,
    }
}

pub fn category_of(filename: &str) -> &'static str {    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" => "Image",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "Music",
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "ts" | "m4s" => "Video",
        "pdf" | "doc" | "docx" | "txt" | "epub" => "Document",
        "zip" | "rar" | "7z" | "tar" | "gz" => "Compressed",
        "exe" | "msi" => "Apps",
        _ => "Other",
    }
}

pub fn format_bytes(b: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", b, UNITS[u])
    } else {
        format!("{:.2} {}", v, UNITS[u])
    }
}

pub fn format_speed(bps: f64) -> String {
    if bps <= 0.0 {
        return "-".to_string();
    }
    format!("{}/s", format_bytes(bps as u64))
}

pub fn format_eta(item: &DownloadItem) -> String {
    if item.speed_bps <= 1.0 {
        return "-".to_string();
    }
    if let Some(total) = item.total_bytes {
        let left = total.saturating_sub(item.downloaded_bytes) as f64;
        let secs = (left / item.speed_bps) as u64;
        if secs >= 3600 {
            format!("{} h {} m left", secs / 3600, (secs % 3600) / 60)
        } else if secs >= 60 {
            format!("{} m {} s left", secs / 60, secs % 60)
        } else {
            format!("{} s left", secs)
        }
    } else {
        "-".to_string()
    }
}

/// Last-resort filename from a URL. Placeholder segments that carry no
/// real name (`videoplayback`, `watch`, `play`, `reel`, API paths…),
/// all-digit IDs (`/reel/113054…`), and raw hash-looking segments are
/// skipped in favor of an earlier meaningful segment; otherwise a
/// timestamped default.
pub fn filename_from_url(raw: &str) -> String {
    if crate::gallery::is_gallery(raw) {
        return crate::gallery::folder_name(raw);
    }
    // Torrents: the magnet's own name (dn=), or the .torrent file's name
    // without ".torrent"; the real name arrives with the metadata.
    if crate::torrent::is_torrent(raw) {
        if let Some(n) = crate::torrent::magnet_name(raw) {
            return crate::engine::sanitize_filename(&n);
        }
        let last = raw.trim().split(['?', '#']).next().unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
        let stem = last.strip_suffix(".torrent").or_else(|| last.strip_suffix(".TORRENT")).unwrap_or(last);
        return if stem.is_empty() || stem.starts_with("magnet:") {
            "Torrent".to_string()
        } else {
            crate::engine::sanitize_filename(stem)
        };
    }
    filename_from_url_plain(raw)
}

fn filename_from_url_plain(raw: &str) -> String {
    const PLACEHOLDERS: &[&str] = &[
        "videoplayback", "watch", "play", "player", "download", "media",
        "api", "v", "video", "audio", "file", "get", "stream", "content",
        "reel", "reels", "embed", "post", "posts", "share",
    ];
    if let Ok(u) = url::Url::parse(raw) {
        if let Some(segs) = u.path_segments() {
            let all: Vec<&str> = segs.filter(|s| !s.is_empty()).collect();
            for seg in all.iter().rev() {
                let clean = seg.split('?').next().unwrap_or(seg);
                let low = clean.to_ascii_lowercase();
                if clean.is_empty() || PLACEHOLDERS.contains(&low.as_str()) {
                    continue;
                }
                // Numeric IDs (`/reel/113054…`, `/posts/123`) are not names.
                if !clean.is_empty() && clean.bytes().all(|b| b.is_ascii_digit()) {
                    continue;
                }
                // Raw hash/blob ids (32+ hex chars, no extension) are not names.
                let is_hash = clean.len() >= 32
                    && clean.bytes().all(|b| b.is_ascii_hexdigit())
                    && !clean.contains('.');
                if is_hash {
                    continue;
                }
                return percent_decode(clean);
            }
        }
    }
    format!("download_{}.bin", chrono::Local::now().timestamp())
}

/// Extension for a `Content-Type` (with parameters tolerated): the correct
/// suffix when the URL itself carries no usable name (P1). Unknown types
/// yield None so callers keep the safe `.bin` default.
pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match m.as_str() {
        "video/mp4" => Some(".mp4"),
        "video/webm" => Some(".webm"),
        "video/x-matroska" | "video/mkv" => Some(".mkv"),
        "video/quicktime" => Some(".mov"),
        "video/x-msvideo" => Some(".avi"),
        "audio/mpeg" | "audio/mp3" => Some(".mp3"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some(".m4a"),
        "audio/webm" | "audio/opus" => Some(".webm"),
        "audio/wav" | "audio/x-wav" => Some(".wav"),
        "audio/flac" => Some(".flac"),
        "audio/ogg" => Some(".ogg"),
        "image/png" => Some(".png"),
        "image/jpeg" => Some(".jpg"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "image/avif" => Some(".avif"),
        "image/heic" | "image/heif" => Some(".heic"),
        "image/bmp" | "image/x-ms-bmp" => Some(".bmp"),
        "image/tiff" => Some(".tiff"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(".ico"),
        "image/svg+xml" => Some(".svg"),
        "application/pdf" => Some(".pdf"),
        "application/zip" => Some(".zip"),
        "application/x-rar-compressed" | "application/vnd.rar" => Some(".rar"),
        "application/x-7z-compressed" => Some(".7z"),
        "application/gzip" | "application/x-gzip" => Some(".gz"),
        "application/x-msdownload" | "application/octet-stream" => None,
        _ => None,
    }
}

/// The MIME type of a file extension (`"jpg"`), for a file handed to the
/// browser (dragging a download onto a page's upload box).
pub fn mime_for_ext(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// `name` with the extension `mime` names when it has none of its own: a
/// file served from an address without one (Civitai's original images come
/// from `…/blobs-managed-public/7DKFBPEAVXB9VHPBCH8B5AGB30`) would otherwise
/// be saved with no extension at all. A name that has one keeps it.
pub fn with_mime_ext(name: &str, mime: &str) -> String {
    let has_ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| (1..=5).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()));
    match ext_for_mime(mime) {
        Some(ext) if !has_ext && !name.trim().is_empty() => format!("{}{ext}", name.trim_end_matches('.')),
        _ => name.to_string(),
    }
}

/// True for `download_<ts>.bin` last-resort names (no real name survived).
pub fn is_fallback_filename(name: &str) -> bool {
    let n = name.trim();
    n.starts_with("download_") && n.ends_with(".bin")
}

/// URL fallback name with the extension corrected from a known MIME type:
/// `download_123.bin` + `video/mp4` → `download_123.mp4`, and extensionless
/// `song` + `audio/mpeg` → `song.mp3`. Anything already suffixed is
/// returned untouched.
pub fn filename_from_url_with_mime(raw: &str, mime: Option<&str>) -> String {
    let ext = mime.and_then(ext_for_mime);
    let base = filename_from_url(raw);
    if is_fallback_filename(&base) {
        if let Some(ext) = ext {
            return base.trim_end_matches(".bin").to_string() + ext;
        }
        return base;
    }
    if !base.contains('.') {
        if let Some(ext) = ext {
            return base + ext;
        }
    }
    base
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

pub fn is_download_url(s: &str) -> bool {
    let t = s.trim();
    // Magnet links and .torrent files/links download through BitTorrent.
    if crate::torrent::is_torrent(t) {
        return t.len() < 8192;
    }
    // "gallery:<address>" asks for gallery-dl on any site it knows.
    if let Some(rest) = t.strip_prefix("gallery:") {
        return (rest.starts_with("http://") || rest.starts_with("https://")) && rest.len() < 2048;
    }
    // FTPS: `ftps://` (implicit on 990, else explicit) and `ftpes://`.
    (t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("ftp://")
        || t.starts_with("ftps://")
        || t.starts_with("ftpes://"))
        && t.len() < 2048
        && !t.contains(' ')
}

/// Bytes at `path`: a file's length, or every file under a folder (a
/// torrent of several files is saved as one).
pub fn size_on_disk(path: &std::path::Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m.len(),
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| size_on_disk(&e.path())).sum())
            .unwrap_or(0),
        _ => 0,
    }
}

/// Public product name shown everywhere in the UI.
pub const APP_NAME: &str = "Download Craft";
/// Config/storage slug: `%APPDATA%/download_craft` (v0.5+).
pub const APP_SLUG: &str = "download_craft";
/// Previous slug, kept only for one-time migration of existing installs.
const LEGACY_SLUG: &str = "idm_rust";

/// App config directory, e.g. `%APPDATA%/download_craft`.
pub fn config_dir() -> Option<std::path::PathBuf> {
    // Server mode (Docker) keeps its settings, queue and pairings in a
    // folder of its own.
    if let Some(d) = std::env::var_os("DOWNLOAD_CRAFT_DATA").filter(|v| !v.is_empty()) {
        return Some(std::path::PathBuf::from(d));
    }
    dirs::config_dir().map(|p| p.join(APP_SLUG))
}

/// Default destination when the OS has no configured Downloads directory.
pub fn default_download_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("DOWNLOAD_CRAFT_DOWNLOADS").filter(|v| !v.is_empty()) {
        return std::path::PathBuf::from(d);
    }
    dirs::download_dir().or_else(|| dirs::home_dir().map(|p| p.join("Downloads")))
        .unwrap_or_else(|| std::path::PathBuf::from("Downloads"))
}

/// Claim the single-instance slot for a standalone window (`name` values look
/// like `progress_12` / `complete_12`). Windows enforces the exclusivity and
/// releases it when the owning process ends, so a window the user closed frees
/// its slot immediately — that is what lets a later Retry pop a fresh one
/// instead of a duplicate. `None` means another process already holds it.
pub fn claim_window(name: &str) -> Option<std::fs::File> {
    let dir = config_dir()?.join("win");
    let _ = std::fs::create_dir_all(&dir);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // No other process may even open it while this handle lives.
        opts.share_mode(0);
    }
    let file = opts.open(dir.join(name)).ok()?;
    #[cfg(unix)]
    file.try_lock().ok()?;
    Some(file)
}

/// The monitor's work area in points: the desktop minus the taskbar (and any
/// other docked bar), so a window centred inside it can never end up half
/// behind the taskbar. Falls back to the whole monitor when the work area is
/// not known yet.
pub fn work_area_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let monitor = ctx.input(|i| i.viewport().monitor_size);
    #[cfg(windows)]
    {
        let scale = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .filter(|s| *s > 0.05);
        if let (Some(scale), Some((l, t, r, b))) = (scale, primary_work_area_px()) {
            let rect = egui::Rect::from_min_max(
                egui::pos2(l / scale, t / scale),
                egui::pos2(r / scale, b / scale),
            );
            // egui reports monitor_size in points; if the scaled work area
            // disagrees with it wildly the scale factor was wrong (mixed DPI),
            // so prefer the monitor rectangle in that case.
            let plausible = match monitor {
                Some(m) => {
                    rect.width() > m.x * 0.25
                        && rect.width() < m.x * 2.5
                        && rect.height() > m.y * 0.25
                        && rect.height() < m.y * 2.5
                }
                None => rect.width() > 200.0 && rect.height() > 200.0,
            };
            if plausible {
                return Some(rect);
            }
        }
    }
    monitor.map(|m| egui::Rect::from_min_size(egui::Pos2::ZERO, m))
}

/// `SPI_GETWORKAREA` of the primary monitor, in physical pixels.
#[cfg(windows)]
fn primary_work_area_px() -> Option<(f32, f32, f32, f32)> {
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    const SPI_GETWORKAREA: u32 = 0x0030;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SystemParametersInfoW(
            action: u32,
            param: u32,
            data: *mut std::ffi::c_void,
            win_ini: u32,
        ) -> i32;
    }
    let mut r = Rect { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) };
    (ok != 0).then(|| (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32))
}

/// Put a fixed-size window in the middle of the work area. Returns `false`
/// while the monitor geometry is still unknown so callers retry next frame
/// instead of latching a bad position (Windows cascades otherwise).
pub fn center_window(ctx: &egui::Context, size: egui::Vec2) -> bool {
    let Some(work) = work_area_rect(ctx) else {
        return false;
    };
    if work.width() < 120.0 || work.height() < 120.0 {
        return false;
    }
    // Clamp to the top-left corner: a window taller than the work area should
    // hug the top edge rather than run off the bottom.
    let x = (work.center().x - size.x / 2.0).max(work.min.x);
    let y = (work.center().y - size.y / 2.0).max(work.min.y);
    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
    true
}

/// Read a boolean preference out of ui.json (the main app's settings file)
/// with a default, so standalone windows honour app settings without pulling
/// in the app module.
pub fn ui_pref_bool(key: &str, default: bool) -> bool {
    config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(key).and_then(|x| x.as_bool()))
        .unwrap_or(default)
}

/// Read a string preference out of ui.json with a default, for the helper
/// logic that runs outside the app window (which helper executable to use).
pub fn ui_pref_str(key: &str, default: &str) -> String {
    config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get(key).and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_else(|| default.to_string())
}

/// Merge one boolean into ui.json, leaving every other key alone. The main app
/// watches that file's mtime and reloads it, so the change lands live.
pub fn ui_pref_set_bool(key: &str, value: bool) {
    let _guard=fs_lock("ui-settings");
    if !_guard.owned() { return; }
    let Some(p) = config_dir().map(|d| d.join("ui.json")) else {
        return;
    };
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    obj.insert(key.to_string(), serde_json::Value::Bool(value));
    if let Ok(json) = serde_json::to_string_pretty(&v) {
        let _ = atomic_write(&p, json.as_bytes());
    }
}

/// Merge one number into ui.json (the interface size from Ctrl +/-).
pub fn ui_pref_set_u64(key: &str, value: u64) {
    let _guard = fs_lock("ui-settings");
    if !_guard.owned() {
        return;
    }
    let Some(p) = config_dir().map(|d| d.join("ui.json")) else { return };
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(key.to_string(), serde_json::json!(value));
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            let _ = atomic_write(&p, json.as_bytes());
        }
    }
}

/// Merge one string into ui.json (used when an action commits a helper choice
/// right away, e.g. "Re-download" switching to the app's own ffmpeg).
pub fn ui_pref_set_str(key: &str, value: &str) {
    let Some(p) = config_dir().map(|d| d.join("ui.json")) else {
        return;
    };
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    obj.insert(key.to_string(), serde_json::Value::String(value.to_string()));
    if let Ok(json) = serde_json::to_string_pretty(&v) {
        let _ = atomic_write(&p, json.as_bytes());
    }
}

/// One-time migration from the legacy `idm_rust` config dir: moves
/// queue/settings/inbox files so existing installs keep their history.
/// Safe to call on every startup (no-op once migrated).
pub fn migrate_config() {
    let base = match dirs::config_dir() {
        Some(p) => p,
        None => return,
    };
    let (old, new) = (base.join(LEGACY_SLUG), base.join(APP_SLUG));
    if !new.exists() && old.exists() {
        if std::fs::create_dir_all(&new).is_ok() {
            if let Ok(entries) = std::fs::read_dir(&old) {
                for e in entries.flatten() {
                    let dest = new.join(e.file_name());
                    if !dest.exists() {
                        let _ = std::fs::rename(e.path(), &dest);
                    }
                }
            }
            // Remove the legacy dir only when fully drained.
            if std::fs::read_dir(&old).map(|mut e| e.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(&old);
            }
        }
    }
}

pub fn relative_time(dt: &chrono::DateTime<chrono::Local>) -> String {
    let secs = (chrono::Local::now() - *dt).num_seconds().max(0);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} minutes ago", secs / 60)
    } else if secs < 86400 {
        format!("{} hours ago", secs / 3600)
    } else {
        format!("{} days ago", secs / 86400)
    }
}

/// Normalize a user-supplied SHA-256 hex string: trim, lowercase, strip
/// optional `sha256:` prefix. Returns "" when empty.
pub fn normalize_sha256(s: &str) -> String {
    let mut t = s.trim().to_lowercase();
    if let Some(rest) = t.strip_prefix("sha256:") {
        t = rest.trim().to_string();
    }
    t
}

/// True when `s` looks like a 64-char hex SHA-256 digest.
pub fn is_valid_sha256(s: &str) -> bool {
    let t = normalize_sha256(s);
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Compute the SHA-256 hex digest of a file (streamed, low memory).
pub fn sha256_of_file(path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    use std::io::Read;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Pick a non-colliding destination: if `dest` exists (and is not a
/// leftover `.dcpart` situation handled by the engine), append ` (1)`,
/// ` (2)`, ... before the extension. Pure clean-room implementation.
pub fn conflict_free_dest(dest: &std::path::Path) -> std::path::PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let ext = dest
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for i in 1..10000 {
        let cand = parent.join(format!("{stem} ({i}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    dest.to_path_buf()
}

/// Atomically write `data` to `path` (temp file + rename) so a crash or
/// power loss never leaves a half-written queue/settings file.
pub fn atomic_write(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
    let name=path.file_name().ok_or_else(||std::io::Error::other("No state filename"))?.to_string_lossy();
    for _ in 0..100 {
        let id=NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let temp=path.with_file_name(format!(".{name}.{}.{id}.tmp",std::process::id()));
        let mut options=std::fs::OpenOptions::new();options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temp) {
            Ok(mut file) => {
                let cleanup=Temp(temp);
                std::io::Write::write_all(&mut file,data)?;
                drop(file);
                std::fs::rename(&cleanup.0,path)?;
                return Ok(());
            }
            Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("Cannot create a private state file"))
}

/// The queue and inbox can contain captured session credentials. Also protect
/// existing state files before any app, dialog, CLI or native-host mode starts.
pub fn secure_config_directory() -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(dir)=config_dir() {
        use std::os::unix::fs::{DirBuilderExt,PermissionsExt};
        let mut builder=std::fs::DirBuilder::new();builder.recursive(true).mode(0o700).create(&dir)?;
        std::fs::set_permissions(dir,std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// One type→folder routing rule (P3.2): comma-separated extensions map to a
/// subfolder under the save directory. First match wins (ordered in UI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderRule {
    #[serde(default)]
    pub exts: String,
    #[serde(default)]
    pub folder: String,
}

impl FolderRule {
    pub fn matches(&self, filename: &str) -> bool {
        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext.is_empty() || ext == filename.to_ascii_lowercase() {
            return false;
        }
        self.exts
            .split([',', ';', ' '])
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .any(|e| !e.is_empty() && e == ext)
    }
}

pub fn default_folder_rules() -> Vec<FolderRule> {
    vec![
        FolderRule { exts: "mp4,mkv,webm,mov,avi".into(), folder: "Video".into() },
        FolderRule { exts: "mp3,m4a,flac,ogg,wav".into(), folder: "Music".into() },
        FolderRule { exts: "png,jpg,jpeg,gif,webp,svg".into(), folder: "Image".into() },
        FolderRule { exts: "pdf,doc,docx,txt,epub".into(), folder: "Document".into() },
        FolderRule { exts: "zip,rar,7z,tar,gz".into(), folder: "Compressed".into() },
        FolderRule { exts: "exe,msi".into(), folder: "Apps".into() },
    ]
}

/// Render a rename template: {name} stem, {ext} extension with dot,
/// {date} YYYYMMDD, {host} source host. Empty/blank results fall back to
/// the original stem; the extension is preserved when missing.
pub fn apply_rename_template(tpl: &str, filename: &str, url: &str) -> String {
    let t = tpl.trim();
    if t.is_empty() {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rfind('.') {
        Some(i) if i > 0 => (filename[..i].to_string(), filename[i..].to_string()),
        _ => (filename.to_string(), String::new()),
    };
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let date = chrono::Local::now().format("%Y%m%d").to_string();
    let mut out = t
        .replace("{name}", &stem)
        .replace("{ext}", &ext)
        .replace("{date}", &date)
        .replace("{host}", &host);
    out = out.trim().to_string();
    if out.is_empty() || out == ext {
        return filename.to_string();
    }
    if !ext.is_empty() && !out.to_ascii_lowercase().ends_with(&ext.to_ascii_lowercase()) {
        out.push_str(&ext);
    }
    crate::engine::sanitize_filename(&out)
}

/// Position of a non-completed item writing the same destination file.
/// A fresh capture of the same video (new signed URL, same filename) must
/// refresh that entry instead of queueing a colliding duplicate: two tasks
/// on one destination fail with "already downloading elsewhere", and stale
/// part files from a different URL would corrupt resume.
pub fn find_dest_clash(items: &[DownloadItem], dest: &std::path::Path) -> Option<usize> {
    items.iter().position(|d| {
        !matches!(d.status, Status::Completed) && d.dest_path() == dest
    })
}

/// Drop resume state that belongs to a previous URL for `dest` (sidecar
/// `.dcpart`/segment files + HLS maps). Called when an entry's URL is
/// replaced: offsets from another (possibly sliced) URL must never be
/// resumed into the new file.
pub fn drop_resume_state(dest: &std::path::Path) {
    let _ = std::fs::remove_file(dest.with_extension("dcpart"));
    let _ = std::fs::remove_file(dest.with_extension("dcmeta"));
    for i in 0..16 {
        let _ = std::fs::remove_file(dest.with_extension(format!("dcpart{i}")));
    }
    let _ = std::fs::remove_file(dest.with_extension("hlsmap"));
    crate::engine::remove_partial(dest);
    let _ = std::fs::remove_file(dest.with_extension("dcftp"));
    let _ = std::fs::remove_file(dest.with_extension("dcftpparts"));
}

/// Whether `new` is a fresh link to the very file `old` points at, so what is
/// already downloaded can continue: the same address, or the same server and
/// path with a new token or signature in the query (an expired link captured
/// again, "Refresh download address"). YouTube-style stream addresses share
/// one path for every quality and slice, so they never count. The engine
/// still compares the server's ETag / Last-Modified and size before it
/// continues, and starts over by itself if the file changed.
pub fn same_file_link(old: &str, new: &str) -> bool {
    if old == new {
        return true;
    }
    let (Ok(a), Ok(b)) = (url::Url::parse(old), url::Url::parse(new)) else { return false };
    let host = a.host_str().unwrap_or("").to_ascii_lowercase();
    if host.ends_with("googlevideo.com") || a.path().ends_with("/videoplayback") || a.path() == "/" {
        return false;
    }
    a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port() == b.port() && a.path() == b.path()
}

/// A row is being refreshed from `old_url` to `new_url` (or restarted after
/// failing). Keep the bytes on disk when it is the same file; drop them when
/// it is not (another file's bytes must never be resumed into this one).
/// Returns whether the partial was kept.
pub fn keep_or_drop_resume(dest: &std::path::Path, old_url: &str, new_url: &str) -> bool {
    if same_file_link(old_url, new_url) && crate::engine::has_partial(dest) {
        return true;
    }
    drop_resume_state(dest);
    false
}

/// Bytes of `dest` already on disk in an unfinished download (from its
/// resume journal), or 0.
pub fn partial_bytes(dest: &std::path::Path) -> u64 {
    let mut js = vec![downloadcraft_engine::journal_path(dest)];
    if let Some(w) = crate::engine::work_dir() {
        js.push(downloadcraft_engine::journal_path_in(dest, Some(&w)));
    }
    js.iter().filter_map(|j| downloadcraft_engine::journal_progress(j)).map(|(_, d)| d).max().unwrap_or(0)
}

// ---- saved site logins (IDM "Site logins") ----------------------------------

/// A login used automatically for one site.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteLogin {
    /// "example.com" (also its subdomains) or "files.example.com".
    pub host: String,
    pub user: String,
    pub pass: String,
}

#[derive(Serialize, Deserialize)]
struct StoredLogin {
    host: String,
    user: String,
    /// The password, protected for this Windows user (DPAPI) and base64'd;
    /// elsewhere base64 in a file only the user can read.
    pass: String,
}

fn logins_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("logins.json"))
}

#[cfg(windows)]
fn protect(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Some(v)
}

#[cfg(not(windows))]
fn protect(data: &[u8], _encrypt: bool) -> Option<Vec<u8>> {
    Some(data.to_vec())
}

/// The saved logins (passwords decrypted).
/// A saved Site Grabber setup (IDM's grabber projects): run it again later
/// to pick up what the site added.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrabProject {
    pub name: String,
    pub url: String,
    pub exts: String,
    pub follow: bool,
    pub levels: usize,
    pub stay: bool,
}

fn grab_projects_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("grab_projects.json"))
}

pub fn grab_projects() -> Vec<GrabProject> {
    grab_projects_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_grab_projects(list: &[GrabProject]) -> bool {
    let Some(p) = grab_projects_path() else { return false };
    serde_json::to_string_pretty(list).ok().is_some_and(|j| std::fs::write(p, j).is_ok())
}

pub fn site_logins() -> Vec<SiteLogin> {
    let stored: Vec<StoredLogin> = logins_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    stored
        .into_iter()
        .filter_map(|s| {
            let pass = protect(&base64_decode(&s.pass)?, false)?;
            Some(SiteLogin { host: s.host, user: s.user, pass: String::from_utf8(pass).ok()? })
        })
        .collect()
}

/// Save the logins (passwords protected for this user).
pub fn save_site_logins(list: &[SiteLogin]) -> bool {
    let Some(p) = logins_path() else { return false };
    let stored: Vec<StoredLogin> = list
        .iter()
        .filter(|l| !l.host.trim().is_empty() && !l.user.trim().is_empty())
        .filter_map(|l| {
            Some(StoredLogin {
                host: l.host.trim().trim_start_matches("*.").to_ascii_lowercase(),
                user: l.user.trim().to_string(),
                pass: base64_encode(&protect(l.pass.as_bytes(), true)?),
            })
        })
        .collect();
    let ok = serde_json::to_string_pretty(&stored).ok().is_some_and(|j| atomic_write(&p, j.as_bytes()).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    ok
}

/// The saved login for `url`'s host (or a parent domain), if any.
pub fn login_for(url: &str) -> Option<(String, String)> {
    let host = url::Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    site_logins()
        .into_iter()
        .filter(|l| host == l.host || host.ends_with(&format!(".{}", l.host)))
        .max_by_key(|l| l.host.len())
        .map(|l| (l.user, l.pass))
}

// ---- refresh download address (IDM) ----------------------------------------

/// "Refresh download address": row `id` waits (until `until`, unix seconds)
/// for the next link the browser extension sends, which then replaces its
/// expired link instead of opening a new download.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefreshWait {
    pub id: u64,
    pub until: i64,
}

fn refresh_wait_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("refresh_wait.json"))
}

/// The row waiting for a fresh link, if the wait has not run out.
pub fn refresh_wait() -> Option<RefreshWait> {
    let w: RefreshWait = serde_json::from_str(&std::fs::read_to_string(refresh_wait_path()?).ok()?).ok()?;
    (w.until > Local::now().timestamp()).then_some(w)
}

/// Start (Some) or end (None) a wait for a fresh link.
pub fn set_refresh_wait(w: Option<RefreshWait>) {
    let Some(p) = refresh_wait_path() else { return };
    match w {
        Some(w) => {
            let _ = atomic_write(&p, serde_json::to_string(&w).unwrap_or_default().as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Bind `dest`'s single-stream resume state to a new URL for the same file
/// (a refreshed link), so the bytes already on disk are kept.
pub fn rebind_resume(dest: &std::path::Path, new_url: &str) {
    let meta = dest.with_extension("dcmeta");
    if let Some(mut v) = std::fs::read_to_string(&meta).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        v["url"] = serde_json::Value::from(new_url);
        let _ = std::fs::write(&meta, v.to_string());
    }
}

/// Outcome of `append_to_queue`.
#[derive(Debug, Clone, PartialEq)]
pub enum AppendResult {
    /// Entry added, or an existing dead entry refreshed in place — start it.
    Added(u64),
    /// The URL is already queued and still usable (id of the existing row).
    AlreadyQueued(u64),
    /// The queue file could not be read or written. Callers must NOT fall
    /// back to "write a fresh file": that silently deletes every entry we
    /// failed to parse. Surface the message instead.
    Error(String),
}

/// Read the shared queue, tolerating a UTF-8 BOM (editors and PowerShell
/// snapshots add one and `serde_json` rejects it). A file that exists but
/// does not parse is an error — never an empty queue, or the next write
/// would destroy it.
pub fn read_queue_items(path: &std::path::Path) -> Result<Vec<DownloadItem>, String> {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let data = data.trim_start_matches('\u{feff}');
    if data.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(data).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

fn write_queue(path: &std::path::Path, items: &[DownloadItem]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(items)
        .map_err(|e| format!("cannot serialize the queue: {e}"))?;
    atomic_write(path, json.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Is this row effectively gone — a failure, or a "Completed" row whose file
/// no longer exists? Such an entry must never block a fresh capture.
fn entry_is_dead(d: &DownloadItem) -> bool {
    match &d.status {
        Status::Failed(_) => true,
        Status::Completed => !d.dest_path().exists(),
        _ => false,
    }
}

/// Look up a queued URL. Returns `(id, dead)`; `dead` mirrors what
/// `append_to_queue` would do with a fresh capture of the same URL.
pub fn queued_entry(url: &str, quality: &str) -> Option<(u64, bool)> {
    let path = config_dir()?.join("queue.json");
    let items = read_queue_items(&path).ok()?;
    let d = items.into_iter().find(|d| d.url == url && same_quality(&d.quality_pref, quality))?;
    Some((d.id, entry_is_dead(&d)))
}

/// Status of queue row `id`, as the queue file has it.
pub fn queued_status(id: u64) -> Option<Status> {
    let path = config_dir()?.join("queue.json");
    read_queue_items(&path).ok()?.into_iter().find(|d| d.id == id).map(|d| d.status)
}

/// Two requests for one page URL are the same download only at the same
/// quality: a YouTube watch page queued at 1080p and again at 720p is two
/// files.
pub fn same_quality(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// `dest`, or "name (n).ext" when another queued row (or a file on disk)
/// already owns that path.
fn dest_free_of_queue(items: &[DownloadItem], dest: &std::path::Path) -> std::path::PathBuf {
    let taken = |p: &std::path::Path| p.exists() || items.iter().any(|d| d.dest_path() == p);
    if !taken(dest) {
        return dest.to_path_buf();
    }
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "download".to_string());
    let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..1000)
        .map(|n| parent.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or_else(|| dest.to_path_buf())
}

/// Append an item to the shared queue file with a fresh id (used by the
/// standalone dialogs). Holds the queue lock across read-assign-write so
/// concurrent writers can no longer collide on ids. A non-completed entry for
/// the SAME destination file (fresh capture, new signed URL) is refreshed in
/// place — new URL/meta, progress reset — and its id is returned, so a
/// re-click never creates a colliding duplicate.
pub fn append_to_queue(item: &mut DownloadItem) -> AppendResult {
    let _guard = fs_lock("queue");
    if !_guard.owned() { return AppendResult::Error("Queue is busy; retry shortly".into()); }
    let path = match config_dir() {
        Some(d) => d.join("queue.json"),
        None => return AppendResult::Error("no config directory".to_string()),
    };
    append_to_queue_at(&path, item)
}

/// Add a download that already finished elsewhere (the browser extension's
/// "Download in browser") as a Completed row, so it shows in the app's list
/// with Open file / Open folder. A row for the same file is not added twice.
pub fn record_finished(item: &mut DownloadItem) -> AppendResult {
    let _guard = fs_lock("queue");
    if !_guard.owned() { return AppendResult::Error("Queue is busy; retry shortly".into()); }
    let path = match config_dir() {
        Some(d) => d.join("queue.json"),
        None => return AppendResult::Error("no config directory".to_string()),
    };
    let mut items = match read_queue_items(&path) {
        Ok(v) => v,
        Err(e) => return AppendResult::Error(e),
    };
    if let Some(d) = items.iter().find(|d| d.dest_path() == item.dest_path()) {
        return AppendResult::AlreadyQueued(d.id);
    }
    item.id = next_queue_id(&path, &items);
    item.status = Status::Completed;
    items.push(item.clone());
    match write_queue(&path, &items) {
        Ok(()) => AppendResult::Added(item.id),
        Err(e) => AppendResult::Error(e),
    }
}

/// Next queue id. Ids are never reused, even after the newest row is
/// removed: the main window treats an id it has seen disappear as deleted
/// and silently drops a later row that reuses it (a dialog's "starting it"
/// then started nothing). A high-water mark beside the queue file keeps the
/// counter moving. Callers must hold the queue lock.
pub fn next_queue_id(queue_path: &std::path::Path, items: &[DownloadItem]) -> u64 {
    let mark = queue_path.with_extension("lastid");
    let last = std::fs::read_to_string(&mark)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let id = items.iter().map(|d| d.id).max().unwrap_or(0).max(last) + 1;
    let _ = std::fs::write(&mark, id.to_string());
    id
}

/// Path-injected core of `append_to_queue` (unit-tested without touching the
/// real queue). Callers must hold the queue lock.
fn append_to_queue_at(path: &std::path::Path, item: &mut DownloadItem) -> AppendResult {
    let mut items = match read_queue_items(path) {
        Ok(v) => v,
        Err(e) => return AppendResult::Error(e),
    };
    if let Some(pos) = items.iter().position(|d| d.url == item.url && same_quality(&d.quality_pref, &item.quality_pref)) {
        // Same URL again. Reuse the row unless it is dead (failed, or finished
        // with the file gone) — then refresh it so "Start Download" always
        // works instead of dead-ending on "already in queue".
        if !entry_is_dead(&items[pos]) {
            return AppendResult::AlreadyQueued(items[pos].id);
        }
        // The fresh capture's data replaces the dead row's (only identity and
        // queue settings stay): keeping the old cookie/headers/referer — the
        // very ones that expired — made the retry fail the same way.
        let old = &items[pos];
        // A failed download keeps what it already has: the same link (or a
        // fresh one to the same file) continues from there.
        let kept = !matches!(old.status, Status::Completed)
            && old.dest_path() == item.dest_path()
            && keep_or_drop_resume(&old.dest_path(), &old.url, &item.url);
        if !kept {
            drop_resume_state(&old.dest_path());
        }
        let mut fresh = item.clone();
        fresh.id = old.id;
        fresh.created_at = old.created_at;
        fresh.priority = old.priority;
        fresh.start_at = old.start_at.clone();
        fresh.downloaded_bytes = if kept { partial_bytes(&old.dest_path()) } else { 0 };
        fresh.speed_bps = 0.0;
        fresh.status = Status::Queued;
        fresh.selected = false;
        let id = fresh.id;
        item.id = id;
        items[pos] = fresh;
        return match write_queue(path, &items) {
            Ok(()) => AppendResult::Added(id),
            Err(e) => AppendResult::Error(e),
        };
    }
    // Another quality of the same page wanting the same file name gets its
    // own name instead of taking over that row.
    if let Some(pos) = find_dest_clash(&items, &item.dest_path()) {
        // A different download (another quality, or another URL while that
        // row is running or paused) gets its own name. Taking over an active
        // row swapped its URL and deleted its partial files mid-transfer.
        let active = matches!(items[pos].status, Status::Downloading | Status::Paused);
        if (items[pos].url == item.url && !same_quality(&items[pos].quality_pref, &item.quality_pref))
            || (active && items[pos].url != item.url)
        {
            let free = dest_free_of_queue(&items, &item.dest_path());
            if let Some(name) = free.file_name() {
                item.filename = name.to_string_lossy().to_string();
            }
        }
    }
    if let Some(pos) = find_dest_clash(&items, &item.dest_path()) {
        let id = items[pos].id;
        let created = items[pos].created_at;
        let fresh_url = item.url.clone();
        let same_url = items[pos].url == fresh_url;
        let old_url = std::mem::replace(&mut items[pos].url, fresh_url);
        items[pos].filename = item.filename.clone();
        items[pos].save_dir = item.save_dir.clone();
        items[pos].referer = std::mem::take(&mut item.referer);
        items[pos].cookie = std::mem::take(&mut item.cookie);
        items[pos].extra_headers = std::mem::take(&mut item.extra_headers);
        items[pos].username = std::mem::take(&mut item.username);
        items[pos].password = std::mem::take(&mut item.password);
        let checksum_changed = items[pos].checksum_sha256 != item.checksum_sha256;
        items[pos].checksum_sha256 = std::mem::take(&mut item.checksum_sha256);
        items[pos].pair_url = std::mem::take(&mut item.pair_url);
        items[pos].pair_mime = std::mem::take(&mut item.pair_mime);
        items[pos].quality_pref = std::mem::take(&mut item.quality_pref);
        items[pos].duration_secs = item.duration_secs;
        items[pos].expects_audio = item.expects_audio;
        items[pos].max_retries = item.max_retries;
        items[pos].timeout_secs = item.timeout_secs;
        if checksum_changed {
            // New checksum, new latch: the repair retry applies per digest.
            items[pos].checksum_repaired = false;
        }
        items[pos].overwrite_existing = item.overwrite_existing;
        items[pos].total_bytes = item.total_bytes;
        items[pos].resolution = String::new();
        if !same_url {
            // A fresh link to the same file continues; different bytes (or
            // a slice of them) never resume old parts.
            let dp = items[pos].dest_path();
            items[pos].downloaded_bytes =
                if keep_or_drop_resume(&dp, &old_url, &items[pos].url) { partial_bytes(&dp) } else { 0 };
        }
        items[pos].speed_bps = 0.0;
        items[pos].status = Status::Queued;
        items[pos].created_at = created;
        return match write_queue(path, &items) {
            Ok(()) => AppendResult::Added(id),
            Err(e) => AppendResult::Error(e),
        };
    }
    let id = next_queue_id(path, &items);
    item.id = id;
    item.status = Status::Queued;
    items.push(item.clone());
    match write_queue(path, &items) {
        Ok(()) => AppendResult::Added(id),
        Err(e) => AppendResult::Error(e),
    }
}

/// A daily speed window (P3.4): HH:MM–HH:MM at `limit_kbps` (0 = full speed).
/// Overnight ranges (22:00–06:00) wrap midnight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeedWindow {
    #[serde(default = "SpeedWindow::d02")]
    pub start: String,
    #[serde(default = "SpeedWindow::d06")]
    pub end: String,
    #[serde(default)]
    pub limit_kbps: u64,
}

impl SpeedWindow {
    fn d02() -> String {
        "02:00".to_string()
    }
    fn d06() -> String {
        "06:00".to_string()
    }
    pub fn start_min(&self) -> Option<u16> {
        parse_hhmm(&self.start)
    }
    pub fn end_min(&self) -> Option<u16> {
        parse_hhmm(&self.end)
    }
    pub fn active_at(&self, now_min: u16) -> bool {
        match (self.start_min(), self.end_min()) {
            (Some(s), Some(e)) => {
                if s <= e {
                    now_min >= s && now_min < e
                } else {
                    now_min >= s || now_min < e
                }
            }
            _ => false,
        }
    }
}

pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// True when `latest` is newer than `current` (`v` prefix + whitespace OK).
pub fn is_newer_version(current: &str, latest: &str) -> bool {
    fn parts(s: &str) -> Vec<u64> {
        s.trim()
            .trim_start_matches(['v', 'V'])
            .split(|c| c == '.' || c == '-' || c == '+')
            .filter_map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .ok()
            })
            .collect()
    }
    let (a, b) = (parts(current), parts(latest));
    let n = a.len().max(b.len()).max(1);
    for i in 0..n {
        let (x, y) = (*a.get(i).unwrap_or(&0), *b.get(i).unwrap_or(&0));
        if x != y {
            return y > x;
        }
    }
    false
}

/// Pick the portable Windows asset from a GitHub `releases/latest` payload
/// (P5): a `.zip` whose name mentions windows (+64-bit when several).
/// Pure helper, fixture-tested below.
pub fn pick_windows_asset(v: &serde_json::Value) -> Option<String> {
    let assets = v.get("assets")?.as_array()?;
    let names: Vec<(&str, &str)> = assets
        .iter()
        .filter_map(|a| {
            let name = a.get("name")?.as_str()?;
            let url = a.get("browser_download_url")?.as_str()?;
            Some((name, url))
        })
        .collect();
    let is_zip = |n: &str| n.to_ascii_lowercase().ends_with(".zip");
    let is_win = |n: &str| n.to_ascii_lowercase().contains("windows");
    let is_64 = |n: &str| {
        let l = n.to_ascii_lowercase();
        l.contains("x86_64") || l.contains("x64")
    };
    // Portable build first, then any 64-bit windows zip, then any windows
    // zip, then any zip at all.
    let portable = names
        .iter()
        .find(|(n, _)| is_zip(n) && is_win(n) && n.to_ascii_lowercase().contains("portable"));
    let win64 = names.iter().find(|(n, _)| is_zip(n) && is_win(n) && is_64(n));
    let win = names.iter().find(|(n, _)| is_zip(n) && is_win(n));
    let any = names.iter().find(|(n, _)| is_zip(n));
    portable
        .or(win64)
        .or(win)
        .or(any)
        .map(|(_, u)| u.to_string())
}

#[cfg(test)]
mod mime_ext_tests {
    use super::with_mime_ext;

    #[test]
    fn a_name_without_an_extension_takes_the_mime_one() {
        assert_eq!(with_mime_ext("7DKFBPEAVXB9VHPBCH8B5AGB30", "image/jpeg"), "7DKFBPEAVXB9VHPBCH8B5AGB30.jpg");
        assert_eq!(with_mime_ext("photo", "image/avif; q=1"), "photo.avif");
        assert_eq!(with_mime_ext("photo.webp", "image/jpeg"), "photo.webp");
        assert_eq!(with_mime_ext("report.final.pdf", "application/pdf"), "report.final.pdf");
        // "v1.2 release" has no real extension (a space and a long tail).
        assert_eq!(with_mime_ext("v1.2 release notes", "application/pdf"), "v1.2 release notes.pdf");
        assert_eq!(with_mime_ext("blob", "application/octet-stream"), "blob");
        assert_eq!(with_mime_ext("", "image/png"), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_start_times() {
        use chrono::TimeZone;
        let at = |h, m| Local.with_ymd_and_hms(2026, 10, 21, h, m, 0).unwrap();
        // Blank means now; a time already past also means now.
        assert!(start_at_due("", at(10, 0)));
        assert!(start_at_due("08:30", at(10, 0)));
        assert!(start_at_due("10:00", at(10, 0)));
        // A later clock time holds the row back.
        assert!(!start_at_due("23:30", at(10, 0)));
        // Garbage never blocks a download forever.
        assert!(start_at_due("later", at(10, 0)));
    }

    #[test]
    fn categories() {
        assert_eq!(category_of("a.mp3"), "Music");
        assert_eq!(category_of("a.MP4"), "Video");
        assert_eq!(category_of("a.png"), "Image");
        assert_eq!(category_of("a.pdf"), "Document");
        assert_eq!(category_of("a.zip"), "Compressed");
        assert_eq!(category_of("a.exe"), "Apps");
        assert_eq!(category_of("a.xyz"), "Other");
        assert_eq!(category_of("noext"), "Other");
    }

    #[test]
    fn queue_read_tolerates_bom_and_flags_corruption() {
        let dir = std::env::temp_dir().join("dc_queue_read_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("queue.json");

        // Missing file = empty queue, not an error.
        let _ = std::fs::remove_file(&p);
        assert_eq!(read_queue_items(&p).unwrap().len(), 0);

        // A UTF-8 BOM (editors/PowerShell snapshots add one) must not make the
        // queue look empty — treating it as empty is how a rewrite silently
        // deletes every existing row.
        let item = DownloadItem::new(
            1,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        let json = serde_json::to_string_pretty(&vec![item]).unwrap();
        std::fs::write(&p, format!("\u{feff}{json}")).unwrap();
        let items = read_queue_items(&p).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].url, "https://e.com/a.zip");

        // Genuine corruption is an error, never an empty list: callers must
        // refuse to write rather than wipe the queue.
        std::fs::write(&p, "{ not json").unwrap();
        assert!(read_queue_items(&p).is_err());

        // A blank file is an empty queue.
        std::fs::write(&p, "  \n").unwrap();
        assert_eq!(read_queue_items(&p).unwrap().len(), 0);
    }

    #[test]
    fn dead_entries_do_not_block_requeue() {
        let dir = std::env::temp_dir().join("dc_dead_entry_test");
        let _ = std::fs::create_dir_all(&dir);
        let mut item = DownloadItem::new(
            1,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );

        // Finished with the file present = live (must not be re-downloaded).
        item.status = Status::Completed;
        std::fs::write(item.dest_path(), b"x").unwrap();
        assert!(!entry_is_dead(&item));

        // Finished with the file GONE = dead: a re-capture must be allowed.
        std::fs::remove_file(item.dest_path()).unwrap();
        assert!(entry_is_dead(&item));

        // Failed is dead even when a partial file lingers.
        std::fs::write(item.dest_path(), b"y").unwrap();
        item.status = Status::Failed("early end".into());
        assert!(entry_is_dead(&item));

        // Live states are never dead.
        for s in [Status::Queued, Status::Downloading, Status::Paused] {
            item.status = s;
            assert!(!entry_is_dead(&item));
        }
    }

    #[test]
    fn dead_row_is_refreshed_not_blocked() {
        let dir = std::env::temp_dir().join("dc_append_dead_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("queue.json");

        // A finished row whose file is gone must not block a re-download.
        let mut old = DownloadItem::new(
            1,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        old.status = Status::Completed;
        old.downloaded_bytes = 999;
        std::fs::write(&p, serde_json::to_string_pretty(&vec![old]).unwrap()).unwrap();

        let mut fresh = DownloadItem::new(
            0,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        fresh.total_bytes = Some(1024);
        assert_eq!(append_to_queue_at(&p, &mut fresh), AppendResult::Added(1));

        let items = read_queue_items(&p).unwrap();
        assert_eq!(items.len(), 1, "must refresh in place, not duplicate");
        assert_eq!(items[0].id, 1);
        assert!(matches!(items[0].status, Status::Queued));
        assert_eq!(items[0].downloaded_bytes, 0);
        assert_eq!(items[0].total_bytes, Some(1024));

        // A live row is reused, not duplicated.
        let mut again = DownloadItem::new(
            0,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        assert_eq!(append_to_queue_at(&p, &mut again), AppendResult::AlreadyQueued(1));
        assert_eq!(read_queue_items(&p).unwrap().len(), 1);
    }

    #[test]
    fn a_failed_row_captured_again_keeps_its_bytes() {
        let dir = std::env::temp_dir().join("dc_append_failed_resume_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("queue.json");
        let dest = dir.join("big.zip");
        // A partial with 600 of 1000 bytes on disk.
        std::fs::write(downloadcraft_engine::data_path(&dest), vec![0u8; 1000]).unwrap();
        std::fs::write(
            downloadcraft_engine::journal_path(&dest),
            format!(
                r#"{{"v":1,"key":{},"total":1000,"etag":null,"last_modified":null,"remaining":[[600,1000]]}}"#,
                serde_json::to_string(&dest.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();
        let row = |url: &str| DownloadItem::new(0, url.into(), "big.zip".into(), dir.to_string_lossy().into(), 8);
        let mut old = row("https://e.com/big.zip?token=old");
        old.id = 1;
        old.status = Status::Failed("connection lost".into());
        old.downloaded_bytes = 600;
        std::fs::write(&p, serde_json::to_string_pretty(&vec![old]).unwrap()).unwrap();

        // The same link again: continues from 600.
        assert_eq!(append_to_queue_at(&p, &mut row("https://e.com/big.zip?token=old")), AppendResult::Added(1));
        let items = read_queue_items(&p).unwrap();
        assert_eq!(items[0].downloaded_bytes, 600);
        assert!(crate::engine::has_partial(&dest));

        // A fresh token for the same file (a refreshed link): still kept.
        let mut items = read_queue_items(&p).unwrap();
        items[0].status = Status::Failed("link expired".into());
        std::fs::write(&p, serde_json::to_string_pretty(&items).unwrap()).unwrap();
        assert_eq!(append_to_queue_at(&p, &mut row("https://e.com/big.zip?token=new")), AppendResult::Added(1));
        assert!(crate::engine::has_partial(&dest));

        // Another file into the same name: the old bytes go.
        let mut items = read_queue_items(&p).unwrap();
        items[0].status = Status::Paused;
        std::fs::write(&p, serde_json::to_string_pretty(&items).unwrap()).unwrap();
        let _ = append_to_queue_at(&p, &mut row("https://other.org/big.zip"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_links_to_the_same_file() {
        assert!(same_file_link("https://e.com/a.zip?t=1", "https://e.com/a.zip?t=2"));
        assert!(same_file_link("https://e.com/a.zip", "https://e.com/a.zip"));
        assert!(!same_file_link("https://e.com/a.zip", "https://e.com/b.zip"));
        assert!(!same_file_link("https://e.com/a.zip", "https://f.com/a.zip"));
        assert!(!same_file_link(
            "https://r1.googlevideo.com/videoplayback?itag=137",
            "https://r1.googlevideo.com/videoplayback?itag=22"
        ));
    }

    #[test]
    fn other_quality_of_one_page_is_its_own_download() {
        let dir = std::env::temp_dir().join("dc_append_quality_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("queue.json");
        std::fs::write(&p, "[]").unwrap();
        let page = "https://www.youtube.com/watch?v=x";
        let item = |q: &str| {
            let mut d = DownloadItem::new(0, page.into(), "Video.mp4".into(), dir.to_string_lossy().into(), 8);
            d.quality_pref = q.into();
            d
        };
        assert_eq!(append_to_queue_at(&p, &mut item("1080p")), AppendResult::Added(1));
        // Same page, other quality, same suggested name: a second row with
        // its own file, never "already queued" or a takeover of row 1.
        assert_eq!(append_to_queue_at(&p, &mut item("720p")), AppendResult::Added(2));
        let items = read_queue_items(&p).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].quality_pref, "1080p");
        assert_eq!(items[0].filename, "Video.mp4");
        assert_eq!(items[1].filename, "Video (1).mp4");
        // The same quality again is still a duplicate.
        assert_eq!(append_to_queue_at(&p, &mut item("720P")), AppendResult::AlreadyQueued(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removed_ids_are_never_reused() {
        let dir = std::env::temp_dir().join("dc_append_ids_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("queue.json");
        std::fs::write(&p, "[]").unwrap();
        let item = |n: &str| DownloadItem::new(0, format!("https://e.com/{n}"), n.into(), dir.to_string_lossy().into(), 8);
        assert_eq!(append_to_queue_at(&p, &mut item("a.zip")), AppendResult::Added(1));
        assert_eq!(append_to_queue_at(&p, &mut item("b.zip")), AppendResult::Added(2));
        // The newest row is removed (CLI `remove`, main window delete)…
        let mut items = read_queue_items(&p).unwrap();
        items.retain(|d| d.id != 2);
        write_queue(&p, &items).unwrap();
        // …and the next capture must not inherit its id.
        assert_eq!(append_to_queue_at(&p, &mut item("c.zip")), AppendResult::Added(3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_queue_is_never_wiped() {
        let dir = std::env::temp_dir().join("dc_append_corrupt_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("queue.json");
        std::fs::write(&p, "!!! not json !!!").unwrap();
        let before = std::fs::read_to_string(&p).unwrap();

        let mut item = DownloadItem::new(
            0,
            "https://e.com/a.zip".into(),
            "a.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        assert!(matches!(
            append_to_queue_at(&p, &mut item),
            AppendResult::Error(_)
        ));
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            before,
            "an unreadable queue must be left untouched, never overwritten"
        );
    }

    #[test]
    fn bom_queue_rows_survive_append() {
        let dir = std::env::temp_dir().join("dc_append_bom_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("queue.json");
        let keep = DownloadItem::new(
            7,
            "https://e.com/keep.zip".into(),
            "keep.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        std::fs::write(
            &p,
            format!("\u{feff}{}", serde_json::to_string_pretty(&vec![keep]).unwrap()),
        )
        .unwrap();

        let mut item = DownloadItem::new(
            0,
            "https://e.com/new.zip".into(),
            "new.zip".into(),
            dir.to_string_lossy().into(),
            8,
        );
        assert!(matches!(
            append_to_queue_at(&p, &mut item),
            AppendResult::Added(_)
        ));
        let urls: Vec<String> = read_queue_items(&p)
            .unwrap()
            .into_iter()
            .map(|d| d.url)
            .collect();
        assert!(
            urls.contains(&"https://e.com/keep.zip".to_string()),
            "existing rows must survive a BOM"
        );
        assert!(urls.contains(&"https://e.com/new.zip".to_string()));
    }

    #[test]
    fn byte_formats() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1024 * 1024 * 927 + 1024 * 1024 / 2), "927.50 MB");
        assert_eq!(format_speed(0.0), "-");
        assert_eq!(format_speed(1024.0 * 1024.0 * 4.91), "4.91 MB/s");
    }

    #[test]
    fn url_checks() {
        assert!(is_download_url("https://example.com/f.zip"));
        assert!(is_download_url("ftp://example.com/f.zip"));
        assert!(is_download_url("ftps://example.com/f.zip"));
        assert!(!is_download_url("not a url"));
        assert_eq!(filename_from_url("https://h/x/my%20file.zip?a=b"), "my file.zip");
        assert_eq!(filename_from_url("https://h/x/%D9%81%D8%A7%D8%B1%D8%B3%DB%8C.mp3"), "فارسی.mp3");
        // placeholders and raw hashes fall back instead of becoming the name
        let fb = filename_from_url("https://h/watch?v=x");
        assert!(fb.starts_with("download_") && fb.ends_with(".bin"), "{fb}");
        assert_eq!(filename_from_url("https://h/v/abc123/videoplayback?x=1"), "abc123");
        // numeric IDs and reel paths are not names either (P1)
        let reel = filename_from_url("https://www.facebook.com/reel/1130540689426081");
        assert!(reel.starts_with("download_"), "{reel}");
        let posts = filename_from_url("https://www.facebook.com/posts/12345");
        assert!(posts.starts_with("download_"), "{posts}");
        // real file tails still win over placeholder parents
        assert_eq!(
            filename_from_url("https://scontent.xx.fbcdn.net/o1/v/t2/AQabc123.mp4?oh=1&oe=2"),
            "AQabc123.mp4"
        );
    }

    #[test]
    fn mime_naming_matrix() {
        // P1: extension-from-MIME only rescues last-resort names.
        assert_eq!(ext_for_mime("video/mp4"), Some(".mp4"));
        assert_eq!(ext_for_mime("video/mp4; charset=binary"), Some(".mp4"));
        assert_eq!(ext_for_mime("VIDEO/WEBM"), Some(".webm"));
        assert_eq!(ext_for_mime("audio/mp4"), Some(".m4a"));
        assert_eq!(ext_for_mime("audio/mpeg"), Some(".mp3"));
        assert_eq!(ext_for_mime("image/jpeg"), Some(".jpg"));
        assert_eq!(ext_for_mime("application/pdf"), Some(".pdf"));
        assert_eq!(ext_for_mime("application/octet-stream"), None);
        assert_eq!(ext_for_mime("text/html"), None);
        assert_eq!(ext_for_mime(""), None);
        assert!(is_fallback_filename("download_123.bin"));
        assert!(!is_fallback_filename("clip.mp4"));
        // 10-URL acceptance: meaningful names untouched, fallbacks fixed.
        assert_eq!(
            filename_from_url_with_mime("https://cdn.example/a/my%20clip.mp4", Some("video/mp4")),
            "my clip.mp4"
        );
        let song = filename_from_url_with_mime("https://cdn.example/a/song", Some("audio/mpeg"));
        assert_eq!(song, "song.mp3");
        assert_eq!(
            filename_from_url_with_mime("https://h/x/file.unknown", Some("video/webm")),
            "file.unknown"
        );
        let reel = filename_from_url_with_mime(
            "https://www.facebook.com/reel/1130540689426081",
            Some("video/mp4"),
        );
        assert!(reel.starts_with("download_") && reel.ends_with(".mp4"), "{reel}");
        let ytid = filename_from_url_with_mime(
            "https://youtu.be/aqz-KE-bpKQ",
            Some("video/mp4"),
        );
        assert_eq!(ytid, "aqz-KE-bpKQ.mp4");
        let disp = filename_from_url_with_mime("https://h/dl?id=7", Some("application/pdf"));
        assert_eq!(disp, "dl.pdf");
        // unknown mime: usable bare names stay, true fallbacks keep .bin
        let keep = filename_from_url_with_mime("https://h/nameless", Some("text/html"));
        assert_eq!(keep, "nameless");
        let keep2 = filename_from_url_with_mime("https://h/nameless", None);
        assert_eq!(keep2, "nameless");
    }

    #[test]
    fn relative_shapes() {
        let now = chrono::Local::now();
        assert_eq!(relative_time(&now), "just now");
        assert!(relative_time(&(now - chrono::Duration::hours(14))).contains("14 hours ago"));
        assert!(relative_time(&(now - chrono::Duration::days(2))).contains("2 days ago"));
    }

    #[test]
    fn dest_clash_refresh() {
        let dir = std::env::temp_dir().join("dc_clash_test");
        let _ = std::fs::create_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let mut a = DownloadItem::new(1, "https://h/v1.mp4".into(), "v.mp4".into(), dir_s.clone(), 4);
        a.status = Status::Failed("old".into());
        let items = vec![a];
        let dest = std::path::Path::new(&dir_s).join("v.mp4");
        assert_eq!(find_dest_clash(&items, &dest), Some(0));
        let other = std::path::Path::new(&dir_s).join("other.mp4");
        assert_eq!(find_dest_clash(&items, &other), None);
        let mut done = DownloadItem::new(2, "https://h/v2.mp4".into(), "v.mp4".into(), dir_s, 4);
        done.status = Status::Completed;
        assert_eq!(find_dest_clash(&vec![done], &dest), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn p5_overrides_and_compat() {
        // P5: per-download overrides fall back to globals, clamped.
        let mut d = DownloadItem::new(1, "https://h/v.mp4".into(), "v.mp4".into(), "C:\\T".into(), 4);
        assert_eq!(d.eff_retries(5), 5);
        assert_eq!(d.eff_timeout(60), 60);
        d.max_retries = Some(99);
        d.timeout_secs = Some(1);
        assert_eq!(d.eff_retries(5), 20);
        assert_eq!(d.eff_timeout(60), 5);
        d.max_retries = Some(0);
        assert_eq!(d.eff_retries(5), 1);
        // Old queue.json entries (no new keys) still parse with safe
        // defaults: no override, no repair spent, no duration.
        let old = r#"{"id":7,"url":"https://h/v.mp4","filename":"v.mp4","save_dir":"C:\\T","total_bytes":100,"downloaded_bytes":10,"status":"Queued","created_at":"2026-10-15T00:00:00+06:00","connections":8}"#;
        let back: DownloadItem = serde_json::from_str(old).expect("old queue parses");
        assert_eq!(back.max_retries, None);
        assert_eq!(back.timeout_secs, None);
        assert!(!back.checksum_repaired);
        assert_eq!(back.duration_secs, 0);
        assert_eq!(back.eff_retries(5), 5);
    }

    #[test]
    fn sha256_helpers() {
        assert_eq!(normalize_sha256("  SHA256:ABC "), "abc");
        assert!(is_valid_sha256(&"a".repeat(64)));
        assert!(!is_valid_sha256("xyz"));
        assert!(!is_valid_sha256(""));
        // conflict_free_dest returns input when nothing exists
        let p = std::path::Path::new("no_such_dir_xyz/no_such_file.bin");
        assert_eq!(conflict_free_dest(p), p.to_path_buf());
    }

    #[test]
    fn folder_rules_and_templates() {
        let rules = default_folder_rules();
        assert!(rules.iter().find(|r| r.folder == "Video").unwrap().matches("a.MP4"));
        assert!(!rules.iter().find(|r| r.folder == "Video").unwrap().matches("a.mp3"));
        assert!(!rules.iter().find(|r| r.folder == "Video").unwrap().matches("noext"));
        assert_eq!(
            apply_rename_template("{name} [{date}]{ext}", "Vid.mp4", "https://h/x"),
            format!("Vid [{}].mp4", chrono::Local::now().format("%Y%m%d"))
        );
        assert_eq!(
            apply_rename_template("{name} from {host}{ext}", "a.mkv", "https://cdn.example/x"),
            "a from cdn.example.mkv"
        );
        assert_eq!(apply_rename_template("prefix_{name}", "a.mp4", "https://h"), "prefix_a.mp4");
        assert_eq!(apply_rename_template("", "a.mp4", "https://h"), "a.mp4");
        assert_eq!(apply_rename_template("   ", "a.mp4", "https://h"), "a.mp4");
    }

    #[test]
    fn windows_and_versions() {
        assert_eq!(parse_hhmm("02:00"), Some(120));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("abc"), None);
        let w = SpeedWindow { start: "22:00".into(), end: "06:00".into(), limit_kbps: 0 };
        assert!(w.active_at(23 * 60));
        assert!(w.active_at(5 * 60));
        assert!(!w.active_at(12 * 60));
        let d = SpeedWindow { start: "02:00".into(), end: "06:00".into(), limit_kbps: 0 };
        assert!(d.active_at(3 * 60));
        assert!(!d.active_at(7 * 60));
        assert!(is_newer_version("0.6.0", "v0.7.0"));
        assert!(!is_newer_version("0.7.0", "0.6.0"));
        assert!(!is_newer_version("0.6.0", "0.6.0"));
        assert!(is_newer_version("0.6", "0.6.1"));
        assert!(!is_newer_version("  v0.6.0 ", "0.6.0"));
    }

    #[test]
    fn update_asset_pick() {
        // P5: portable windows zip wins; then 64-bit; then any windows
        // zip; then any zip; nothing zip-like gives None.
        let v: serde_json::Value = serde_json::json!({
            "tag_name": "v0.8.0",
            "assets": [
                {"name": "DownloadCraft-0.8.0-linux-x64.tar.gz", "browser_download_url": "https://h/l.tgz"},
                {"name": "DownloadCraft-0.8.0-windows-x64.zip", "browser_download_url": "https://h/w64.zip"},
                {"name": "DownloadCraft-0.8.0-windows-x64-portable.zip", "browser_download_url": "https://h/port.zip"},
                {"name": "notes.txt", "browser_download_url": "https://h/n.txt"}
            ]
        });
        assert_eq!(pick_windows_asset(&v).as_deref(), Some("https://h/port.zip"));
        let v2: serde_json::Value = serde_json::json!({
            "assets": [
                {"name": "app-windows-x64.zip", "browser_download_url": "https://h/a.zip"},
                {"name": "app-linux.zip", "browser_download_url": "https://h/b.zip"}
            ]
        });
        assert_eq!(pick_windows_asset(&v2).as_deref(), Some("https://h/a.zip"));
        let v3: serde_json::Value = serde_json::json!({"assets": []});
        assert_eq!(pick_windows_asset(&v3), None);
        assert_eq!(pick_windows_asset(&serde_json::json!({})), None);
    }
}

/// Overlay only fields owned by this writer, preserving plugin preferences.
pub fn merge_ui_values(mut current:serde_json::Value,patch:serde_json::Value)->serde_json::Value {
    if !current.is_object(){current=serde_json::json!({});}
    if let (Some(target),Some(changes))=(current.as_object_mut(),patch.as_object()) {
        for (key,value) in changes {target.insert(key.clone(),value.clone());}
    }
    current
}
#[cfg(test)]
mod preference_merge_tests {
    #[test]fn main_window_save_keeps_helper_locations(){
        let saved=super::merge_ui_values(serde_json::json!({"ytdlp_source":"custom","ytdlp_custom_path":"/home/user/yt-dlp","ffmpeg_custom_path":"/home/user/ffmpeg","theme":"dark"}),serde_json::json!({"columns":["name"],"theme":"light"}));
        assert_eq!(saved["ytdlp_source"],"custom");assert_eq!(saved["ffmpeg_custom_path"],"/home/user/ffmpeg");assert_eq!(saved["theme"],"light");
    }
}

#[cfg(test)]
mod audit_lock_tests {
    use super::*;
    #[test]
    fn audit_active_lock_is_not_stolen_after_twenty_seconds() {
        let name=format!("audit-held-{}",std::process::id());
        let first=fs_lock(&name);
        assert!(first.owned());
        let path=config_dir().unwrap().join(format!("{name}.lock"));
        let file=std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()-std::time::Duration::from_secs(60))).unwrap();
        let second=fs_lock(&name);
        assert!(!second.owned(),"a live owner's lock must never expire");
        drop(second);drop(first);
        assert!(fs_lock(&name).owned());
    }
}

#[cfg(all(test,unix))]
mod audit_private_state_tests {
    #[test]
    fn replacing_state_makes_credentials_private_and_cleans_temporary_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir=std::env::temp_dir().join(format!("dc-private-state-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();let p=dir.join("state.json");
        std::fs::write(&p,b"old").unwrap();
        std::fs::set_permissions(&p,std::fs::Permissions::from_mode(0o644)).unwrap();
        super::atomic_write(&p,b"private").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(),b"private");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,0o600);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(),1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// ---- batch download patterns (IDM "Add batch download") --------------------

/// Expand `https://site/img[001-120].jpg` (or `(001-120)`, `[a-z]`) into
/// every URL it names. Several groups multiply. Numbers keep the width of the
/// first bound (001, 002, …). None when the text holds no range or would
/// produce more than 10,000 URLs.
pub fn expand_batch(pattern: &str) -> Option<Vec<String>> {
    // One range group: (start index, end index, values).
    fn group(s: &str, open: usize) -> Option<(usize, Vec<String>)> {
        let close_ch = if s[open..].starts_with('[') { ']' } else { ')' };
        let rel = s[open + 1..].find(close_ch)?;
        let inner = &s[open + 1..open + 1 + rel];
        let (a, b) = inner.split_once('-')?;
        let (a, b) = (a.trim(), b.trim());
        let vals: Vec<String> = if !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() {
            let (x, y): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
            if y < x || y - x > 10_000 {
                return None;
            }
            let width = if a.len() > 1 && a.starts_with('0') { a.len() } else { 0 };
            (x..=y).map(|n| format!("{n:0width$}")).collect()
        } else if a.len() == 1 && b.len() == 1 {
            let (x, y) = (a.chars().next()?, b.chars().next()?);
            if !(x.is_ascii_alphabetic() && y.is_ascii_alphabetic()) || y < x {
                return None;
            }
            (x..=y).map(|c| c.to_string()).collect()
        } else {
            return None;
        };
        Some((open + 1 + rel + 1, vals))
    }
    let mut parts: Vec<Vec<String>> = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    let mut groups = 0;
    while i < pattern.len() {
        let c = pattern[i..].chars().next()?;
        if c == '[' || c == '(' {
            if let Some((next, vals)) = group(pattern, i) {
                parts.push(vec![std::mem::take(&mut lit)]);
                parts.push(vals);
                groups += 1;
                i = next;
                continue;
            }
        }
        lit.push(c);
        i += c.len_utf8();
    }
    parts.push(vec![lit]);
    if groups == 0 {
        return None;
    }
    let total: usize = parts.iter().map(|p| p.len()).try_fold(1usize, |acc, n| acc.checked_mul(n))?;
    if total > 10_000 {
        return None;
    }
    let mut out = vec![String::new()];
    for p in parts {
        out = out.iter().flat_map(|pre| p.iter().map(move |v| format!("{pre}{v}"))).collect();
    }
    Some(out.into_iter().filter(|u| is_download_url(u)).collect())
}

// ---- URL lists, including IDM's export (.ef2) ------------------------------

/// One link from an imported list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportedLink {
    pub url: String,
    pub referer: String,
    pub filename: String,
    pub cookie: String,
}

/// Read a URL list: one URL per line, or IDM's export format (`.ef2`):
/// `<` / URL / `referer: …` / `User-Agent: …` / `filename: …` / `>` blocks.
pub fn parse_url_list(text: &str) -> Vec<ImportedLink> {
    let mut out: Vec<ImportedLink> = Vec::new();
    let mut cur: Option<ImportedLink> = None;
    for line in text.lines() {
        let t = line.trim().trim_start_matches('\u{feff}');
        if t == "<" {
            cur = Some(ImportedLink::default());
            continue;
        }
        if t == ">" {
            if let Some(l) = cur.take().filter(|l| is_download_url(&l.url)) {
                out.push(l);
            }
            continue;
        }
        if let Some(l) = cur.as_mut() {
            let low = t.to_ascii_lowercase();
            if l.url.is_empty() && is_download_url(t) {
                l.url = t.to_string();
            } else if let Some(v) = low.strip_prefix("referer:").map(|_| t[8..].trim()) {
                l.referer = v.to_string();
            } else if let Some(v) = low.strip_prefix("filename:").map(|_| t[9..].trim()) {
                l.filename = v.to_string();
            } else if let Some(v) = low.strip_prefix("cookie:").map(|_| t[7..].trim()) {
                l.cookie = v.to_string();
            }
            continue;
        }
        let u = t.trim_matches(|c| c == '"' || c == '\'');
        if is_download_url(u) {
            out.push(ImportedLink { url: u.to_string(), ..Default::default() });
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|l| seen.insert(l.url.clone()));
    out
}

// ---- statistics -------------------------------------------------------------

/// Bytes and files finished today, this month and ever.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub day: String,
    pub day_bytes: u64,
    pub month: String,
    pub month_bytes: u64,
    pub total_bytes: u64,
    pub files: u64,
}

fn stats_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("stats.json"))
}

/// The statistics, rolled over to today's day and month.
pub fn stats() -> Stats {
    let mut s: Stats = stats_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let now = Local::now();
    let (day, month) = (now.format("%Y-%m-%d").to_string(), now.format("%Y-%m").to_string());
    if s.day != day {
        s.day = day;
        s.day_bytes = 0;
    }
    if s.month != month {
        s.month = month;
        s.month_bytes = 0;
    }
    s
}

/// Count one finished download.
pub fn record_finished_bytes(bytes: u64) {
    let _guard = fs_lock("stats");
    if !_guard.owned() {
        return;
    }
    let mut s = stats();
    s.day_bytes += bytes;
    s.month_bytes += bytes;
    s.total_bytes += bytes;
    s.files += 1;
    if let (Some(p), Ok(j)) = (stats_path(), serde_json::to_string(&s)) {
        let _ = atomic_write(&p, j.as_bytes());
    }
}

#[cfg(test)]
mod batch_import_tests {
    use super::*;

    #[test]
    fn batch_patterns_expand() {
        let v = expand_batch("https://s.example/img[008-011].jpg").unwrap();
        assert_eq!(v, ["https://s.example/img008.jpg", "https://s.example/img009.jpg", "https://s.example/img010.jpg", "https://s.example/img011.jpg"]);
        let v = expand_batch("https://s.example/(a-c)/p(1-2).zip").unwrap();
        assert_eq!(v.len(), 6);
        assert_eq!(v[0], "https://s.example/a/p1.zip");
        assert_eq!(v[5], "https://s.example/c/p2.zip");
        assert!(expand_batch("https://s.example/plain.zip").is_none());
        assert!(expand_batch("https://s.example/x[1-99999].zip").is_none());
        // Parentheses that are not a range stay as they are.
        assert!(expand_batch("https://s.example/Movie (2024).mp4").is_none());
    }

    #[test]
    fn idm_export_and_plain_lists_import() {
        let ef2 = "<\r\nhttps://cdn.example/a.zip\r\nreferer: https://site.example/page\r\nUser-Agent: Mozilla/5.0\r\nfilename: a (1).zip\r\n>\r\n<\r\nhttps://cdn.example/b.iso\r\n>\r\n";
        let v = parse_url_list(ef2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].url, "https://cdn.example/a.zip");
        assert_eq!(v[0].referer, "https://site.example/page");
        assert_eq!(v[0].filename, "a (1).zip");
        assert_eq!(v[1].referer, "");
        let plain = parse_url_list("\"https://x.example/1.zip\"\nnot a url\nhttps://x.example/1.zip\nhttps://x.example/2.zip");
        assert_eq!(plain.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(), ["https://x.example/1.zip", "https://x.example/2.zip"]);
    }
}
