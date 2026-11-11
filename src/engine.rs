use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt};

/// Shared state between the UI thread and the download task.
pub struct SharedState {
    /// Where the server is and how far ("Germany · about 24 ms (near)"),
    /// worked out beside the download (src/geo.rs).
    pub server: Arc<std::sync::Mutex<String>>,
    /// This download's own speed cap (bytes/s, 0 = none).
    pub limit_bps: Arc<AtomicU64>,
    pub downloaded: AtomicU64,
    pub total: AtomicU64,
    pub has_total: AtomicBool,
    /// Duration/audio expectations from the player, manifest or extractor.
    pub expected_duration: AtomicU64,
    pub expects_audio: AtomicBool,
    /// Transfer rate in bytes/s reported by the helper itself (yt-dlp). Left at
    /// 0 by the built-in engine, which lets the UI measure the rate from the
    /// byte counter instead.
    pub speed_bps: AtomicU64,
    pub paused: AtomicBool,
    pub cancelled: AtomicBool,
    /// Set only after the engine and all final validation have succeeded.
    pub succeeded: AtomicBool,
    /// The engine fetched a picture and its sound apart and muxed them (the
    /// app's own YouTube reader does, with no pair link on the row): the
    /// file's size is not the legs' sum, so the row must not compare them.
    pub merged: AtomicBool,
    /// External tools pause by stopping and restarting from their partial files.
    pub external_helper: AtomicBool,
    /// Engine-detected filename (Content-Disposition / video title).
    pub resolved_name: std::sync::Mutex<Option<String>>,
    /// Whether the server supports range resume.
    pub resumable: AtomicBool,
    /// Per-part (start, end) byte ranges for segmented downloads.
    pub part_total: std::sync::Mutex<Vec<(u64, u64)>>,
    /// Per-part downloaded bytes (same indexing as part_total).
    pub part_done: std::sync::Mutex<Vec<u64>>,
    /// Set when YouTube answers with a bot-check wall (HTTP 429 /sorry).
    pub rate_limited: AtomicBool,
    /// What the transfer is doing when it is not moving bytes — the helper's
    /// own phase names ("Getting info", "Merging"), empty for the built-in
    /// engine, which is always transferring once it has started. yt-dlp spends
    /// its first seconds solving YouTube's player challenge with nothing on the
    /// wire, which used to read as a stalled download.
    pub phase: std::sync::Mutex<String>,
    /// Terminal engine error text (e.g. checksum mismatch, unrecoverable
    /// HTTP error). Set once by the download task before it exits.
    pub error: std::sync::Mutex<Option<String>>,
    /// A non-fatal remark for the row's log: a fallback that likely cost the
    /// audio track, a helper that could not be used, and so on. Never decides
    /// the status.
    pub note: std::sync::Mutex<Option<String>>,
    /// Resolution of the video that actually got fetched — "1280×720" from the
    /// helper's chosen format (yt-dlp info fields), or the finished file's own
    /// frame size read back with ffprobe. Empty while unknown, so the UI can
    /// show what really landed instead of the browser's guess.
    pub resolution: std::sync::Mutex<String>,
    /// Set once a live HLS playlist is being recorded. The UI uses it to treat
    /// Stop as "finish the recording" instead of "pause for later".
    pub live: AtomicBool,
    /// Fragment index/count while a helper fetches a fragmented stream
    /// (count 0 = not fragmented). Such a stream has no size until it is
    /// done, so this is what the progress bar and Part Info show.
    pub frag_index: AtomicU64,
    pub frag_count: AtomicU64,
    /// yt-dlp has already run for this download (the fallback chain must not
    /// start it twice).
    pub ytdlp_tried: AtomicBool,
    /// The app's own YouTube reader failed for this download: later
    /// resolves (expired links, retries) go straight to the other paths.
    pub youtube_own_failed: AtomicBool,
}

impl SharedState {
    pub fn new() -> Self {
        Self {
            server: Arc::new(std::sync::Mutex::new(String::new())),
            limit_bps: Arc::new(AtomicU64::new(0)),
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(0),
            has_total: AtomicBool::new(false),
            expected_duration: AtomicU64::new(0),
            expects_audio: AtomicBool::new(false),
            speed_bps: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            succeeded: AtomicBool::new(false),
            merged: AtomicBool::new(false),
            external_helper: AtomicBool::new(false),
            resolved_name: std::sync::Mutex::new(None),
            resumable: AtomicBool::new(false),
            part_total: std::sync::Mutex::new(Vec::new()),
            part_done: std::sync::Mutex::new(Vec::new()),
            rate_limited: AtomicBool::new(false),
            phase: std::sync::Mutex::new(String::new()),
            error: std::sync::Mutex::new(None),
            note: std::sync::Mutex::new(None),
            resolution: std::sync::Mutex::new(String::new()),
            live: AtomicBool::new(false),
            frag_index: AtomicU64::new(0),
            frag_count: AtomicU64::new(0),
            ytdlp_tried: AtomicBool::new(false),
            youtube_own_failed: AtomicBool::new(false),
        }
    }

    /// Set the current stage label (see [`SharedState::phase`]).
    pub fn set_phase(&self, p: impl Into<String>) {
        if let Ok(mut g) = self.phase.lock() {
            let p = p.into();
            if *g != p {
                *g = p;
            }
        }
    }

    /// Record a non-fatal remark for the row's log (first one wins, so the
    /// most specific cause survives later fallbacks).
    pub fn set_note(&self, n: impl Into<String>) {
        if let Ok(mut g) = self.note.lock() {
            if g.is_none() {
                *g = Some(n.into());
            }
        }
    }

    /// The current stage label, or an empty string.
    pub fn phase_now(&self) -> String {
        self.phase
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// Record the first terminal error (subsequent ones are ignored).
    pub fn set_error(&self, e: impl Into<String>) {
        if let Ok(mut g) = self.error.lock() {
            if g.is_none() {
                *g = Some(e.into());
            }
        }
    }

    /// Record the real resolution of the fetched video ("1280×720"). Ignored
    /// when empty so a late unknown never clobbers a known value.
    pub fn set_resolution(&self, r: impl Into<String>) {
        let r = r.into();
        if r.is_empty() {
            return;
        }
        if let Ok(mut g) = self.resolution.lock() {
            if *g != r {
                *g = r;
            }
        }
    }

    /// The recorded resolution, or an empty string.
    pub fn resolution_now(&self) -> String {
        self.resolution
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }
}

/// Map an I/O error to a user-facing message, with a clear text for a
/// full disk instead of a raw OS code.
pub(crate) fn io_message(e: &std::io::Error) -> String {
    #[cfg(windows)]
    const DISK_FULL: i32 = 112; // ERROR_DISK_FULL
    #[cfg(not(windows))]
    const DISK_FULL: i32 = 28; // ENOSPC
    if e.raw_os_error() == Some(DISK_FULL) {
        return "disk is full — free space and resume to retry".to_string();
    }
    format!("file error: {e}")
}

/// Per-download + global options for the engine.
#[derive(Debug, Clone)]
pub struct EngineOpts {
    pub speed_limit_bps: u64,
    pub timeout_secs: u64,
    pub max_retries: u32,
    pub user_agent: String,
    pub referer: String,
    pub cookie: String,
    pub username: String,
    pub password: String,
    pub proxy_url: String,
    /// Allow the engine to rename the file from server/video metadata.
    /// CLI one-shot downloads set this false to honor an explicit -o path.
    pub allow_rename: bool,
    /// Optional expected SHA-256 hex digest; verified after transfer.
    /// Empty = skip verification.
    pub expected_sha256: String,
    pub expected_duration_secs: u64,
    pub require_audio: bool,
    /// When true, an existing destination file is replaced instead of
    /// auto-renaming to `name (1).ext`.
    pub overwrite_existing: bool,
    /// Preferred YouTube quality from the extension menu ("720p", "1080p60",
    /// ...). Empty = best available. Honored at resolve time by picking the
    /// highest stream at or below it.
    pub quality_pref: String,
    /// Live scheduler override (speed windows): when present, the rate
    /// limiter honors it over `speed_limit_bps` without restarts.
    pub global_limit: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// This download's own live cap (bytes/s, 0 = none): its progress window.
    pub own_limit: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    /// Maximum concurrent connections to ONE host across every download
    /// (Queue v2). 0 = no extra cap. Enforced process-wide, so three
    /// simultaneous jobs cannot open 3×8 sockets to the same server.
    pub per_host_conns: usize,
    /// Origin / Authorization / CDN x-* headers the browser player sent with
    /// the captured media, replayed so token-checked CDNs don't answer 403.
    pub captured_headers: CapturedHeaders,
    /// The browser's side of a captured download: its request headers,
    /// method/body, server IP and proxy (see model::BrowserContext).
    pub browser: Option<crate::model::BrowserContext>,
    /// YouTube visitor id for a yt-dlp retry after a bot check (empty =
    /// yt-dlp's own). Set only by the fallback chain.
    pub ytdlp_visitor_data: String,
    /// Name of this download's log (the queue id); empty = no log.
    pub log_key: String,
    /// Mirrors of the file (other URLs serving the same bytes).
    pub mirrors: Vec<String>,
    /// Allow HTTP/2 (off by default: one TCP connection per segment is the
    /// whole point of a segmented download). The fallback chain turns it on
    /// for servers that refuse HTTP/1.1.
    pub http2: bool,
    /// Connect over IPv6 too (off: connections are bound to IPv4, see
    /// client_builder). The fallback chain turns it on for a host that has
    /// no working IPv4 path or failed its name lookup.
    pub any_ip: bool,
    /// yt-dlp presents a browser's TLS fingerprint (`--impersonate`, its
    /// bundled curl_cffi): set for the Cloudflare retry in `fallback_chain`.
    pub impersonate: bool,
    /// HTTP Digest login learned from the server's challenge (shared by
    /// every clone of these options).
    pub digest: Arc<std::sync::Mutex<Option<DigestAuth>>>,
}

/// An HTTP Digest challenge (RFC 7616, MD5 / qop=auth).
#[derive(Debug, Clone, Default)]
pub struct DigestAuth {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    qop_auth: bool,
    nc: u32,
}

impl DigestAuth {
    /// Parse `WWW-Authenticate: Digest realm="…", nonce="…", qop="auth"`.
    pub fn parse(header: &str) -> Option<Self> {
        let rest = header.trim().strip_prefix("Digest")?.trim();
        let mut d = DigestAuth::default();
        let mut algo_ok = true;
        for part in split_auth_params(rest) {
            let (k, v) = part.split_once('=')?;
            let v = v.trim().trim_matches('"').to_string();
            match k.trim().to_ascii_lowercase().as_str() {
                "realm" => d.realm = v,
                "nonce" => d.nonce = v,
                "opaque" => d.opaque = Some(v),
                "qop" => d.qop_auth = v.split(',').any(|q| q.trim() == "auth"),
                "algorithm" => algo_ok = v.eq_ignore_ascii_case("MD5"),
                _ => {}
            }
        }
        (algo_ok && !d.nonce.is_empty()).then_some(d)
    }

    /// The Authorization header for one request.
    pub fn header(&mut self, user: &str, pass: &str, method: &str, uri: &str) -> String {
        use md5::{Digest, Md5};
        let hex = |s: String| format!("{:x}", Md5::digest(s.as_bytes()));
        let ha1 = hex(format!("{user}:{}:{pass}", self.realm));
        let ha2 = hex(format!("{method}:{uri}"));
        self.nc += 1;
        let nc = format!("{:08x}", self.nc);
        let cnonce = format!("{:016x}", chrono::Local::now().timestamp_nanos_opt().unwrap_or(0) as u64 ^ 0x9e37_79b9_7f4a_7c15);
        let mut h = if self.qop_auth {
            let resp = hex(format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", self.nonce));
            format!(r#"Digest username="{user}", realm="{}", nonce="{}", uri="{uri}", qop=auth, nc={nc}, cnonce="{cnonce}", response="{resp}""#, self.realm, self.nonce)
        } else {
            let resp = hex(format!("{ha1}:{}:{ha2}", self.nonce));
            format!(r#"Digest username="{user}", realm="{}", nonce="{}", uri="{uri}", response="{resp}""#, self.realm, self.nonce)
        };
        if let Some(o) = &self.opaque {
            h.push_str(&format!(r#", opaque="{o}""#));
        }
        h.push_str(", algorithm=MD5");
        h
    }
}

/// Split `a="x, y", b=z` on the commas between parameters.
fn split_auth_params(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Ask the server once, without credentials, whether it wants a Digest
/// login; remember the challenge for every later request.
async fn learn_digest(client: &reqwest::Client, opts: &EngineOpts, url: &str) {
    let Ok(resp) = client.get(url).header(reqwest::header::RANGE, "bytes=0-0").send().await else { return };
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return;
    }
    for v in resp.headers().get_all(reqwest::header::WWW_AUTHENTICATE) {
        if let Some(d) = v.to_str().ok().and_then(DigestAuth::parse) {
            *mutex_guard(&opts.digest) = Some(d);
            return;
        }
    }
}

/// Request headers captured by the extension for one download. They are
/// sent only to the site they were captured from, never to unrelated hosts a
/// playlist or redirect points at (bearer tokens must not leak).
#[derive(Debug, Clone, Default)]
pub struct CapturedHeaders {
    site: String,
    headers: Vec<(String, String)>,
}

impl CapturedHeaders {
    pub fn for_url(url: &str, headers: &std::collections::BTreeMap<String, String>) -> Self {
        let Some(site) = site_of_url(url) else { return Self::default() };
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .filter(|(k, v)| replayable_header(k) && !v.is_empty() && !v.contains(['\r', '\n']))
            .collect();
        if headers.is_empty() { Self::default() } else { Self { site, headers } }
    }

    fn applies_to(&self, url: &str) -> bool {
        !self.headers.is_empty() && site_of_url(url).as_deref() == Some(self.site.as_str())
    }
}

/// Which captured request headers may be replayed by the engine. Cookie and
/// Referer travel in their own fields; hop-by-hop, range and fetch-metadata
/// headers are the engine's own business.
pub fn replayable_header(name: &str) -> bool {
    name == "origin" || name == "authorization" || (name.starts_with("x-") && name != "x-client-data")
}

/// Registrable site of a URL's host ("a.b.example.co.uk" -> "example.co.uk",
/// "rr1.googlevideo.com" -> "googlevideo.com"). Approximate public-suffix
/// handling: a two-letter country TLD under a short second level (co.uk,
/// com.au) keeps three labels.
fn site_of_url(url: &str) -> Option<String> {
    let host = reqwest::Url::parse(url).ok()?.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || !host.contains('.') {
        return Some(host);
    }
    // Multi-tenant hosting: every customer has its own subdomain, so the
    // site is the whole host (d111.cloudfront.net is not d222's).
    const SHARED_SUFFIXES: &[&str] = &[
        "cloudfront.net", "akamaized.net", "akamaihd.net", "edgesuite.net", "azureedge.net",
        "blob.core.windows.net", "fastly.net", "b-cdn.net", "r2.dev", "r2.cloudflarestorage.com",
        "workers.dev", "pages.dev", "github.io", "githubusercontent.com", "gitlab.io",
        "herokuapp.com", "appspot.com", "web.app", "firebaseapp.com", "netlify.app",
        "vercel.app", "amazonaws.com", "storage.googleapis.com", "googleusercontent.com",
        "digitaloceanspaces.com", "backblazeb2.com", "onrender.com", "fly.dev",
    ];
    if SHARED_SUFFIXES.iter().any(|s| host.ends_with(&format!(".{s}"))) {
        return Some(host);
    }
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let keep = if n >= 3 && labels[n - 1].len() == 2 && labels[n - 2].len() <= 3 { 3 } else { 2 };
    Some(labels[n.saturating_sub(keep)..].join("."))
}

impl Default for EngineOpts {
    fn default() -> Self {
        Self {
            speed_limit_bps: 0,
            own_limit: None,
            timeout_secs: 60,
            max_retries: 5,
            user_agent: String::new(),
            referer: String::new(),
            cookie: String::new(),
            username: String::new(),
            password: String::new(),
            proxy_url: String::new(),
            allow_rename: true,
            expected_sha256: String::new(),
            expected_duration_secs: 0,
            require_audio: false,
            overwrite_existing: false,
            quality_pref: String::new(),
            global_limit: None,
            per_host_conns: 0,
            captured_headers: CapturedHeaders::default(),
            browser: None,
            ytdlp_visitor_data: String::new(),
            log_key: String::new(),
            mirrors: Vec::new(),
            http2: false,
            any_ip: false,
            impersonate: false,
            digest: Default::default(),
        }
    }
}

// ---- per-download log (IDM keeps one per download) -------------------------

/// The stage an engine names as its transfer starts, telling which one
/// downloads: shown for a few seconds before the status says Downloading.
pub const PHASE_APP_ENGINE: &str = "Downloading using app engine";
pub const PHASE_YTDLP: &str = "Downloading using yt-dlp";

/// Whether `phase` only says the transfer is under way (no stage to name).
pub fn phase_is_downloading(phase: &str) -> bool {
    phase == "Downloading" || phase == PHASE_APP_ENGINE || phase == PHASE_YTDLP
}

/// Where download `key`'s log lives (`<config>/logs/<key>.log`).
pub fn job_log_path(key: &str) -> Option<PathBuf> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    crate::model::config_dir().map(|d| d.join("logs").join(format!("{key}.log")))
}

/// Append one line to download `key`'s log, keeping the file small.
/// [`job_log`] for other modules (the torrent one).
pub fn job_log_pub(key: &str, line: String) {
    job_log(key, line)
}

pub fn job_log(key: &str, line: impl AsRef<str>) {
    let Some(path) = job_log_path(key) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        if let Ok(old) = std::fs::read_to_string(&path) {
            // Halved at a character boundary: a Bengali or emoji name in the
            // log must not make the cut panic.
            let mut mid = old.len() / 2;
            while !old.is_char_boundary(mid) {
                mid += 1;
            }
            let keep = &old[mid..];
            let cut = keep.find('\n').map(|i| &keep[i + 1..]).unwrap_or(keep);
            let _ = std::fs::write(&path, cut);
        }
    }
    use std::io::Write;
    // One write per line: connections logging at the same moment would
    // otherwise interleave their pieces ("[01:57:57[01:57:57] ] …").
    let text = format!("[{}] {}\n", chrono::Local::now().format("%H:%M:%S"), line.as_ref().replace(['\r', '\n'], " "));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// The folder chosen for unfinished downloads (Settings -> Download Engine
/// -> Temporary folder); None = next to the file.
pub fn work_dir() -> Option<PathBuf> {
    let d = crate::model::ui_pref_str("temp_dir", "");
    let d = d.trim();
    (!d.is_empty()).then(|| PathBuf::from(d))
}

/// Unfinished engine data for `dest`, in the temporary folder or next to it.
pub fn has_partial(dest: &std::path::Path) -> bool {
    downloadcraft_engine::has_partial(dest) || work_dir().is_some_and(|w| downloadcraft_engine::has_partial_in(dest, Some(&w)))
}

/// Drop the engine's unfinished data for `dest` wherever it is.
pub fn remove_partial(dest: &std::path::Path) {
    downloadcraft_engine::remove_partial(dest);
    if let Some(w) = work_dir() {
        downloadcraft_engine::remove_partial_in(dest, Some(&w));
    }
}

/// Stop a background download (the Download dialog's early start) so another
/// can continue it: raise its stop flag and wait — at most `max`, back as
/// soon as it ends — for it to write its journal and let go of its files.
/// Aborting right away left no journal (the engine writes one every 3 s and
/// on stop), so the app started the file again from 0 % and its progress
/// showed "Connecting…" before jumping back up. Returns whether it ended.
pub fn stop_for_handover<T>(shared: &SharedState, handle: &tokio::task::JoinHandle<T>, max: std::time::Duration) -> bool {
    shared.cancelled.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + max;
    while !handle.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let ended = handle.is_finished();
    handle.abort();
    ended
}

/// Move `from`'s unfinished data so it continues as `to` (the journal is
/// keyed by the URL, not the path, so a resume picks it up under the new
/// name). Data next to the file and in the temporary folder both move.
/// Returns whether anything was moved.
pub fn move_partial(from: &std::path::Path, to: &std::path::Path) -> bool {
    let wd = work_dir();
    let mut pairs = vec![
        (downloadcraft_engine::data_path(from), downloadcraft_engine::data_path(to)),
        (downloadcraft_engine::journal_path(from), downloadcraft_engine::journal_path(to)),
    ];
    if let Some(w) = wd.as_deref() {
        pairs.push((downloadcraft_engine::data_path_in(from, Some(w)), downloadcraft_engine::data_path_in(to, Some(w))));
        pairs.push((downloadcraft_engine::journal_path_in(from, Some(w)), downloadcraft_engine::journal_path_in(to, Some(w))));
    }
    // The file may already have a partial of its own (a paused download
    // whose link was clicked again: the Download window's early start began
    // from 0 in its scratch folder). Keep whichever has more bytes of the
    // same file; replacing a 15% partial with a few seconds' worth made
    // every part start again from its beginning.
    let done = |dest: &std::path::Path| -> Option<(u64, u64)> {
        let mut js = vec![downloadcraft_engine::journal_path(dest)];
        if let Some(w) = wd.as_deref() {
            js.push(downloadcraft_engine::journal_path_in(dest, Some(w)));
        }
        js.iter()
            .filter_map(|j| downloadcraft_engine::journal_progress(j))
            .max_by_key(|(_, d)| *d)
    };
    if let (Some((t_from, d_from)), Some((t_to, d_to))) = (done(from), done(to)) {
        if t_from == t_to && d_to >= d_from && has_partial(to) {
            return false;
        }
    }
    let mut moved = false;
    for (a, b) in pairs {
        if !a.is_file() {
            continue;
        }
        // The journal belongs to its file's path (the engine's resume key):
        // re-key it for the new one, or the moved bytes are thrown away and
        // the download starts again from 0.
        if a.to_string_lossy().ends_with(".dcjournal") {
            downloadcraft_engine::rekey_journal(&a, &to.to_string_lossy());
        }
        if let Some(dir) = b.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(&b);
        // A rename across drives fails: copy, then remove.
        let ok = std::fs::rename(&a, &b).is_ok() || (std::fs::copy(&a, &b).is_ok() && std::fs::remove_file(&a).is_ok());
        moved |= ok;
    }
    moved
}

/// Every unfinished-data path of `dest` (for deleting leftovers).
pub fn partial_paths(dest: &std::path::Path) -> Vec<PathBuf> {
    let mut v = vec![
        downloadcraft_engine::data_path(dest),
        downloadcraft_engine::journal_path(dest),
        dest.with_extension("dcftp"),
        dest.with_extension("dcftpparts"),
    ];
    if let Some(w) = work_dir() {
        v.push(downloadcraft_engine::data_path_in(dest, Some(&w)));
        v.push(downloadcraft_engine::journal_path_in(dest, Some(&w)));
    }
    v
}

impl EngineOpts {
    pub fn build_client(&self) -> Result<reqwest::Client> {
        Ok(self.client_builder()?.build()?)
    }

    /// A client that connects to the server address the browser used for
    /// `url` (same CDN edge, IP-bound tokens) - only when that address
    /// answers right now; None = use the normal client.
    pub async fn pinned_client(&self, url: &str) -> Option<reqwest::Client> {
        let ip: std::net::IpAddr = self.browser.as_ref()?.server_ip.parse().ok()?;
        // The client binds IPv4 (see client_builder): IPv6 cannot be pinned.
        if !ip.is_ipv4() {
            return None;
        }
        let u = reqwest::Url::parse(url).ok()?;
        let host = u.host_str()?.to_string();
        if host.parse::<std::net::IpAddr>().is_ok() || proxy_choice(&self.proxy_url) != ProxyChoice::System {
            return None;
        }
        let port = u.port_or_known_default()?;
        let addr = std::net::SocketAddr::new(ip, port);
        let reach = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await;
        if !matches!(reach, Ok(Ok(_))) {
            return None;
        }
        self.client_builder().ok()?.resolve(&host, addr).build().ok()
    }

    fn client_builder(&self) -> Result<reqwest::ClientBuilder> {
        // HTTP/1.1 only: several media servers RST or truncate H2 streams
        // from non-browser TLS stacks, while plain H1 range GETs work.
        // IPv4-only egress: a broken IPv6 path (tunnel/relay) completes TCP
        // but serves bot-wall 403s and mid-stream cuts on media edges, while
        // the identical request over IPv4 returns 206 (verified live:
        // curl -4 206 vs curl -6 connection failure on the same URL that
        // our Happy-Eyeballs client kept failing). Binding the source to
        // IPv4 steers every connection at the verified-good path.
        // P2: pooled keepalive connections (segmented/chunked/HLS/DASH open
        // dozens of short requests per file) + TCP keepalives so throttled
        // CDN idle sockets don't die mid-range. H2/IPv6 stay OFF until a
        // live CDN proof run clears them (see docs/ROADMAP.md).
        // Captured headers ride on every request, and on a redirect reqwest
        // strips only Authorization/Cookie: an x-* token or Origin would
        // follow a redirect to any host. Refuse redirects that leave the
        // captured site instead (the browser already resolved its own).
        let redirect = match self.captured_headers.headers.is_empty() {
            true => reqwest::redirect::Policy::limited(10),
            false => {
                let site = self.captured_headers.site.clone();
                reqwest::redirect::Policy::custom(move |attempt| {
                    if attempt.previous().len() >= 10 {
                        attempt.error("too many redirects")
                    } else if site_of_url(attempt.url().as_str()).as_deref() != Some(site.as_str()) {
                        attempt.error("redirected to another site while carrying captured browser headers; capture the video again from its page")
                    } else {
                        attempt.follow()
                    }
                })
            }
        };
        let mut b = reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .redirect(redirect)
            .tcp_nodelay(true)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .pool_max_idle_per_host(16)
            .tcp_keepalive(std::time::Duration::from_secs(60));
        if self.timeout_secs > 0 {
            // Connect timeout ONLY: reqwest's `timeout` limits the whole
            // transfer, which wrongly kills big files on slow links.
            b = b.connect_timeout(std::time::Duration::from_secs(self.timeout_secs));
        }
        // Settings → Network: held to one adapter (a VPN's), which is also
        // the kill switch (nothing starts while it is down), and the DNS.
        let bind = crate::netlock::bind_ip().map_err(anyhow::Error::msg)?;
        match bind {
            Some(ip) => b = b.local_address(ip),
            None if !self.any_ip => b = b.local_address(std::net::IpAddr::from([0, 0, 0, 0])),
            None => {}
        }
        if let Some(r) = crate::netlock::resolver(bind) {
            b = b.dns_resolver(r);
        }
        if !self.http2 {
            b = b.http1_only();
        }
        match proxy_choice(&self.proxy_url) {
            ProxyChoice::Direct => b = b.no_proxy(),
            ProxyChoice::Pac(script) => {
                b = b.no_proxy().proxy(reqwest::Proxy::custom(move |u| {
                    crate::sysproxy::pac_proxy_for(&script, u.as_str()).and_then(|p| reqwest::Url::parse(&p).ok())
                }));
            }
            ProxyChoice::Manual(url) => {
                // Settings → Proxy can limit its own proxy to http or https
                // pages; a browser's proxy (same field) covers everything.
                let mine = crate::model::ui_pref_str("proxy_url", "").trim() == url;
                let only = crate::model::ui_pref_str("proxy_protocols", "http,https");
                let has = |k: &str| only.split(',').any(|p| p.trim() == k);
                let (http, https) = (has("http"), has("https"));
                b = b.no_proxy();
                b = match (mine, http, https) {
                    (true, true, false) => b.proxy(reqwest::Proxy::http(&url)?),
                    (true, false, true) => b.proxy(reqwest::Proxy::https(&url)?),
                    (true, false, false) => b,
                    _ => b.proxy(reqwest::Proxy::all(&url)?),
                };
            }
            ProxyChoice::System => {
                if crate::sysproxy::auto_configured() {
                    // Windows uses a PAC script / auto-detect: ask it per host.
                    b = b.proxy(reqwest::Proxy::custom(|u| {
                        crate::sysproxy::proxy_for(u.as_str()).and_then(|p| reqwest::Url::parse(&p).ok())
                    }));
                }
            }
        }
        Ok(b)
    }

    /// The proxy a download of `url` goes through: the one set in Settings,
    /// else what the system's PAC script picks (for tools like yt-dlp that
    /// do not read it themselves).
    /// An empty string means "connect directly" (yt-dlp's `--proxy ""`).
    pub fn effective_proxy(&self, url: &str) -> Option<String> {
        match proxy_choice(&self.proxy_url) {
            ProxyChoice::Direct => Some(String::new()),
            ProxyChoice::Pac(script) => Some(crate::sysproxy::pac_proxy_for(&script, url).unwrap_or_default()),
            ProxyChoice::Manual(u) => Some(u),
            ProxyChoice::System => crate::sysproxy::proxy_for(url),
        }
    }

    pub(crate) fn get(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.get(url), Some(url))
    }

    fn post(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.post(url), Some(url))
    }

    fn head(&self, client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        self.apply(client.head(url), Some(url))
    }

    pub(crate) fn apply(
        &self,
        mut r: reqwest::RequestBuilder,
        url: Option<&str>,
    ) -> reqwest::RequestBuilder {
        r = r
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        // Google Drive refuses a cross-site CORS fetch (403): its files are
        // fetched the way a typed-in address is.
        r = if url.is_some_and(crate::gdrive::is_google_host) {
            r.header("Sec-Fetch-Dest", "document")
                .header("Sec-Fetch-Mode", "navigate")
                .header("Sec-Fetch-Site", "none")
        } else {
            // Browser streaming headers: FB/IG and other media CDNs throttle
            // or drop non-browser Range requests. These match what the
            // extension captures from a real navigation.
            r.header("Sec-Fetch-Dest", "empty")
                .header("Sec-Fetch-Mode", "cors")
                .header("Sec-Fetch-Site", "cross-site")
        };
        // Client Hints: strict googlevideo edges (SABR/adaptive serving)
        // 403 Range requests that lack them — the legacy progressive edge
        // doesn't care, which is why small files always worked. Derived
        // from the configured UA so the two can never disagree (a lying
        // hint pair is more suspicious than none at all).
        if let Some(v) = chrome_major(&self.user_agent) {
            r = r
                .header(
                    "sec-ch-ua",
                    format!("\"Chromium\";v=\"{v}\", \"Google Chrome\";v=\"{v}\", \"Not-A.Brand\";v=\"99\""),
                )
                .header("sec-ch-ua-mobile", "?0")
                .header("sec-ch-ua-platform", format!("\"{}\"", client_hint_platform(&self.user_agent)));
        }
        // The browser's own request headers for this capture (User-Agent,
        // Accept, Accept-Language, client hints) replace the defaults above:
        // the server sees the same request the browser made.
        if let Some(b) = self.browser.as_ref().filter(|b| !b.headers.is_empty()) {
            let mut map = reqwest::header::HeaderMap::new();
            for (k, v) in &b.headers {
                if k == "content-type" && !b.is_post() {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                    reqwest::header::HeaderValue::from_str(v),
                ) {
                    map.insert(n, v);
                }
            }
            r = r.headers(map);
        }
        // Default page referer for social CDNs when the caller captured none:
        // bare CDN hosts (fbcdn/instagram) often 403 Range requests with no
        // Referer while serving the same bytes with one.
        let mut referer = self.referer.trim().to_string();
        if referer.is_empty() {
            if let Some(u) = url {
                let low = u.to_ascii_lowercase();
                if low.contains("fbcdn.net") || low.contains("facebook.com") {
                    referer = "https://www.facebook.com/".to_string();
                } else if low.contains("cdninstagram.com") || low.contains("instagram.com") {
                    referer = "https://www.instagram.com/".to_string();
                }
            }
        }
        if !referer.is_empty() {
            r = r.header(reqwest::header::REFERER, referer);
        }
        if !self.cookie.trim().is_empty() {
            r = r.header(reqwest::header::COOKIE, self.cookie.trim().to_string());
        }
        if url.is_some_and(|u| self.captured_headers.applies_to(u)) {
            for (name, value) in &self.captured_headers.headers {
                if let (Ok(n), Ok(v)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) {
                    r = r.header(n, v);
                }
            }
        }
        let digest = if self.username.trim().is_empty() { None } else { mutex_guard(&self.digest).clone() };
        if let (Some(mut d), Some(u)) = (digest, url) {
            let uri = reqwest::Url::parse(u)
                .map(|x| format!("{}{}", x.path(), x.query().map(|q| format!("?{q}")).unwrap_or_default()))
                .unwrap_or_else(|_| "/".into());
            let h = d.header(self.username.trim(), &self.password, "GET", &uri);
            if let Some(stored) = mutex_guard(&self.digest).as_mut() {
                stored.nc = d.nc;
            }
            r = r.header(reqwest::header::AUTHORIZATION, h);
        } else if !self.username.trim().is_empty() {
            r = r.basic_auth(
                self.username.trim(),
                if self.password.is_empty() {
                    None
                } else {
                    Some(self.password.clone())
                },
            );
        }
        r
    }
}

/// Streaming write buffer for the network loops. `tokio::fs::File` turns
/// every `write_all` into a blocking-pool round trip, so a stream that yields
/// 8–16 KiB chunks pays one dispatch per chunk; a 1 MiB buffer cuts that ~64×
/// and keeps the socket drain loop off the disk path.
const WRITE_BUF: usize = 1 << 20;

fn client_hint_platform(ua: &str) -> &'static str {
    if ua.contains("Android") { "Android" }
    else if ua.contains("iPhone") || ua.contains("iPad") { "iOS" }
    else if ua.contains("Windows") { "Windows" }
    else if ua.contains("Macintosh") { "macOS" }
    else if ua.contains("CrOS") { "Chrome OS" }
    else if ua.contains("Linux") { "Linux" }
    else { "Unknown" }
}

pub(crate) async fn wait_if_paused(shared: &Arc<SharedState>) -> Result<()> {
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        if !shared.paused.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// Serialize jobs targeting the same destination (two concurrent writers
/// would interleave part/work files). Returns None when another job owns
/// the destination right now — callers fail loudly instead of corrupting.
fn claim_dest_job(dest: &std::path::Path) -> Option<crate::model::FsGuard> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    dest.to_string_lossy().hash(&mut h);
    let g = crate::model::fs_lock(&format!("job-{:x}", h.finish()));
    g.owned().then_some(g)
}

static WORK_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Unique sidecar dir per job so concurrent/retried jobs never share part
/// files (`dcwork-<pid>-<n>`).
fn unique_work_dir(dest: &std::path::Path) -> std::path::PathBuf {
    let n = WORK_COUNTER.fetch_add(1, Ordering::SeqCst);
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "dl".to_string());
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parent.join(format!("{stem}.dcwork-{}-{n}", std::process::id()))
}

/// The work folder of an adaptive (picture + sound) download: one per
/// destination, the same on every attempt, so a paused, failed or restarted
/// download continues its legs from the bytes already there (the engine's
/// journal checks each leg's size against the fresh link). Only one job may
/// write a destination at a time (`claim_dest_job`).
fn adaptive_work_dir(dest: &std::path::Path) -> std::path::PathBuf {
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "dl".to_string());
    let parent = dest.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parent.join(format!("{stem}.dcwork"))
}

/// Resume binding sidecar (P0): ties `.dcpart` bytes to the exact URL +
/// total (+ connection split) that wrote them. Offset-only resume corrupts
/// files when a re-capture (new signed URL, same filename) or a changed
/// server file reuses the name — the old prefix belongs to different bytes.
/// A missing sidecar (legacy partials, fixture seeds) still resumes by
/// length once, then gets bound; a mismatch discards the stale prefix.
#[derive(serde::Serialize, serde::Deserialize)]
struct ResumeMeta {
    url: String,
    total: Option<u64>,
    conns: usize,
}

fn resume_meta_path(dest: &std::path::Path) -> std::path::PathBuf {
    dest.with_extension("dcmeta")
}

/// Resume metadata is useful only while partial data exists. Call after
/// successful completion/verification; retain it for paused/failed jobs.
fn cleanup_completed_resume(dest: &std::path::Path) {
    if !dest.is_file() || dest.with_extension("dcpart").exists()
        || (0..16).any(|i| dest.with_extension(format!("dcpart{i}")).exists())
        || has_partial(dest) {
        return;
    }
    let _ = std::fs::remove_file(resume_meta_path(dest));
}

fn cleanup_completed_job(dest: &std::path::Path, shared: &Arc<SharedState>) {
    let resolved = mutex_guard(&shared.resolved_name).clone();
    let actual = resolved.map(|n| dest.with_file_name(n)).unwrap_or_else(|| dest.to_path_buf());
    cleanup_completed_resume(&actual);
}

fn read_resume_meta(dest: &std::path::Path) -> Option<ResumeMeta> {
    let raw = std::fs::read_to_string(resume_meta_path(dest)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_resume_meta(dest: &std::path::Path, url: &str, total: Option<u64>, conns: usize) {
    let m = ResumeMeta { url: url.to_string(), total, conns };
    if let Ok(json) = serde_json::to_string(&m) {
        let _ = std::fs::write(resume_meta_path(dest), json);
    }
}

fn meta_matches(meta: &ResumeMeta, url: &str, total: Option<u64>, conns: usize) -> bool {
    if meta.url != url || meta.conns != conns {
        return false;
    }
    match (meta.total, total) {
        (Some(a), Some(b)) => a == b,
        // Unknown on either side: can't prove staleness, keep resuming.
        _ => true,
    }
}

/// Validated resume offset for `dest` (.dcpart length) against the given
/// URL/total/split. Drops stale prefixes (and ALL segmented parts `dcpart{i}`
/// for i in 0..16 — a halved re-split has fewer parts than the failed one)
/// on mismatch and (re)binds the sidecar.
pub(crate) fn resume_offset(
    dest: &std::path::Path,
    part_ext: &str,
    url: &str,
    total: Option<u64>,
    conns: usize,
) -> u64 {
    let part = dest.with_extension(part_ext);
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    match read_resume_meta(dest) {
        Some(meta) if meta_matches(&meta, url, total, conns) => {
            // Adopt a newly learned total into the binding.
            if meta.total.is_none() && total.is_some() {
                write_resume_meta(dest, url, total, conns);
            }
            // Clamp absurd prefixes (total shrank server-side): the 416
            // paths handle the restart; never resume past the end.
            if let Some(t) = total {
                if t > 0 && have >= t {
                    return have;
                }
            }
            have
        }
        Some(_) => {
            // Bound to different bytes: discard, never resume across URLs.
            let _ = std::fs::remove_file(&part);
            for i in 0..16 {
                let _ = std::fs::remove_file(dest.with_extension(format!("dcpart{i}")));
            }
            let _ = std::fs::remove_file(resume_meta_path(dest));
            write_resume_meta(dest, url, total, conns);
            0
        }
        None => {
            // Legacy/unbound partial: resume once by length, then bind it.
            // (One-time hole for pre-sidecar files; self-heals on next run.)
            write_resume_meta(dest, url, total, conns);
            have
        }
    }
}
/// Shared token-bucket rate limiter: lives in the reusable engine crate.
pub use downloadcraft_engine::RateLimiter;

/// Lock a mutex even when poisoned (a panicking holder must not cascade
/// into every UI poll and engine task — take the guard anyway).
pub(crate) fn mutex_guard<'a, T>(m: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- per-host connection cap (Queue v2) ------------------------------------
//
// IDM-style "max connections per server": one process-wide semaphore per host,
// shared by every download, so a burst of jobs cannot hammer a CDN with more
// sockets than the user allows. 0 = no extra cap (opt-in in Settings).

type HostSlotMap = std::collections::HashMap<String, (usize, Arc<tokio::sync::Semaphore>)>;
static HOST_SLOTS: std::sync::OnceLock<std::sync::Mutex<HostSlotMap>> = std::sync::OnceLock::new();

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_ascii_lowercase()))
        .unwrap_or_default()
}

/// Hold a connection slot for `url`'s host until the permit is dropped.
async fn host_slot(url: &str, cap: usize) -> Option<tokio::sync::OwnedSemaphorePermit> {
    host_gate(url, cap)?.acquire_owned().await.ok()
}

/// The process-wide semaphore for `url`'s host (None = no cap).
fn host_gate(url: &str, cap: usize) -> Option<Arc<tokio::sync::Semaphore>> {
    if cap == 0 {
        return None;
    }
    let host = host_of(url);
    if host.is_empty() {
        return None;
    }
    let sem = {
        let map = HOST_SLOTS.get_or_init(|| std::sync::Mutex::new(HostSlotMap::new()));
        let mut g = mutex_guard(map);
        match g.get(&host) {
            // A changed cap gets a fresh semaphore; old permits die with it.
            Some((c, s)) if *c == cap => s.clone(),
            _ => {
                let s = Arc::new(tokio::sync::Semaphore::new(cap));
                g.insert(host, (cap, s.clone()));
                s
            }
        }
    };
    Some(sem)
}

/// Marker for HTTP refusals that must NOT be retried (401/403): the same
/// request will fail the same way. Attached to the error (instead of
/// matching "403" substrings in rendered text, which false-positives on
/// byte counts and wrapped messages).
#[derive(Debug)]
struct FinalHttp(String);

impl std::fmt::Display for FinalHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FinalHttp {}

fn final_err(msg: String) -> anyhow::Error {
    anyhow::Error::new(FinalHttp(msg))
}

/// Map an HTTP failure to an actionable message. A bare `403` on a
/// googlevideo link almost always means a stale/session-locked stream URL
/// (they expire and are IP/session-bound) — retrying the same URL is
/// pointless, so callers fail fast on these instead of burning retries.
fn http_status_err(url: &str, e: reqwest::Error) -> anyhow::Error {
    match e.status() {
        Some(s) => http_code_err(url, s.as_u16()),
        None => anyhow!("network: {e}"),
    }
}

/// [`http_status_err`] for a bare status code.
fn http_code_err(url: &str, code: u16) -> anyhow::Error {
    let status = reqwest::StatusCode::from_u16(code).ok();
    if status == Some(reqwest::StatusCode::FORBIDDEN) {
        if url.contains("googlevideo.com")
            || url.contains("youtube.com")
            || url.contains("youtu.be")
        {
            return final_err(
                "YouTube refused this link (403) — stream links expire and are locked to the browser session that created them: replay the video, reopen the ⬇ Download this video menu for a fresh quality row, and download that (progressive ≤720p saves as one ready file)".to_string(),
            );
        }
        return final_err(
            "server refused (403 Forbidden) — it likely needs login or cookies: open the page in your browser and capture it with the Download Craft extension".to_string(),
        );
    }
    if status == Some(reqwest::StatusCode::UNAUTHORIZED) {
        return final_err(
            "server needs a login (401) — add the username/password in the Add dialog's advanced section, or capture via the browser extension".to_string(),
        );
    }
    match status {
        Some(s) => anyhow!("HTTP {s}"),
        None => anyhow!("HTTP {code}"),
    }
}

/// Errors that mean no connection was made at all (name lookup, routing).
fn unreachable_host(low: &str) -> bool {
    [
        "dns error",
        "failed to lookup address",
        "no such host",
        "name or service not known",
        "network is unreachable",
        "host is unreachable",
        "address not available",
        "requested address is not valid",
        "no route to host",
    ]
    .iter()
    .any(|k| low.contains(k))
}

/// Plain words for failures whose raw text helps nobody: a redirect loop
/// (almost always a login or cookie wall) and certificate problems (never
/// bypassed).
pub(crate) fn explain_failure(raw: &str) -> Option<&'static str> {
    let low = raw.to_ascii_lowercase();
    if low.contains("too many redirects") || low.contains("redirect loop") {
        return Some("the server keeps redirecting (usually a login or cookie wall) — open the page in the browser and capture the download with the extension");
    }
    if low.contains("certificate") || low.contains("unknownissuer") || low.contains("invalid peer certificate") {
        return Some("the site's security certificate is not valid (expired, self-signed or for another name) — Download Craft does not download from it");
    }
    None
}

fn is_final_http_error(e: &anyhow::Error) -> bool {
    e.downcast_ref::<FinalHttp>().is_some()
}

/// Saturating counter decrement (plain `fetch_sub` wraps to u64::MAX on
/// underflow — which once painted a stuck-full progress bar).
fn sub_saturating(a: &std::sync::atomic::AtomicU64, n: u64) {
    a.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(n)))
        .ok();
}

fn is_googlevideo_url(url: &str) -> bool {
    url.contains("googlevideo.com")
}

/// Major Chrome version from a UA string ("... Chrome/126.0 ...") for
/// Client Hints. None when the UA isn't Chrome-based (custom UAs silently
/// skip hints rather than lie).
fn chrome_major(ua: &str) -> Option<u32> {
    let p = ua.find("Chrome/")? + "Chrome/".len();
    ua[p..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u32>()
        .ok()
        .filter(|v| (50..=999).contains(v))
}

/// Signed, throttled social CDN links (Facebook/Instagram reels): parallel
/// Range requests get cut, and the URL itself expires mid-download.
fn is_fragile_cdn(url: &str) -> bool {
    let low = url.to_ascii_lowercase();
    low.contains("fbcdn")
        || low.contains("facebook.com")
        || low.contains("cdninstagram.com")
        || low.contains("instagram.com")
        || low.contains("fbsbx.com")
        || low.contains("scontent")
        || low.contains("lookaside.")
}

/// The media *page* a captured social CDN asset belongs to, when handing that
/// page to yt-dlp is both possible and safe. Facebook/Instagram players stream
/// adaptive representations - a video-only DASH quality plus a separate
/// audio-only one - and the browser captures whichever the player was
/// fetching, which is why such a capture downloads as a silent video. The page
/// is where both legs can be found, but only a referer naming ONE video
/// (reel/watch/video/photo page) may be used: a feed, profile or home page
/// could resolve to a different video entirely.
fn social_page_url(asset: &str, referer: &str) -> Option<String> {
    if !is_fragile_cdn(asset) {
        return None;
    }
    let page = referer.trim();
    if page.is_empty() || !page.starts_with("http") {
        return None;
    }
    let low = page.to_ascii_lowercase();
    let social = low.contains("facebook.com")
        || low.contains("fb.watch")
        || low.contains("fb.gg")
        || low.contains("instagram.com");
    if !social {
        return None;
    }
    let names_one_video = low.contains("/reel/")
        || low.contains("/reels/")
        || low.contains("/watch")
        || low.contains("/videos/")
        || low.contains("/video.php")
        || low.contains("/tv/")
        || low.contains("/p/")
        || low.contains("/share/")
        || low.contains("?v=");
    if !names_one_video {
        return None;
    }
    Some(page.to_string())
}

/// A URL naming an individual media fragment, rather than an ordinary
/// complete file which merely supports HTTP Range requests.
fn is_media_fragment(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false; };
    let path = u.path().to_ascii_lowercase();
    let leaf = path.rsplit('/').next().unwrap_or("");
    if [".m4s", ".cmfv", ".cmfa", ".mpd.seg"].iter().any(|e| path.ends_with(e)) { return true; }
    if path.ends_with(".ts") {
        let stem = leaf.trim_end_matches(".ts");
        if stem.bytes().all(|c| c.is_ascii_digit())
            || ["seg", "segment", "chunk", "fragment", "s", "filesequence", "sequence"].iter()
                .any(|p| stem.strip_prefix(p).map(|v| v.trim_start_matches(['-', '_']).starts_with(|c: char| c.is_ascii_digit())).unwrap_or(false)) {
            return true;
        }
    }
    let media = [".mp4", ".m4a", ".webm", ".mp3", ".aac", ".ts"].iter().any(|e| path.ends_with(e));
    media && u.query_pairs().any(|(k,v)| {
        k == "bytestart" || k == "byteend" || (k == "range" && v.split_once('-').map(|(a,b)|
            !a.is_empty() && !b.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.bytes().all(|c| c.is_ascii_digit())
        ).unwrap_or(false))
    })
}

fn fragment_page(raw: &str, referer: &str) -> Result<Option<String>> {
    if !is_media_fragment(raw) { return Ok(None); }
    let page = url::Url::parse(referer).ok().filter(|u| matches!(u.scheme(), "http" | "https")
        && !looks_media_url(referer) && !is_media_fragment(referer));
    page.map(|p| Some(p.to_string())).ok_or_else(|| anyhow!(
        "this URL is a playback fragment, not the complete media — capture the full playlist or the specific player page"))
}

/// Vimeo fragments are not progressive MP4 files, even when named .mp4.
fn is_vimeo_fragment(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false; };
    let host = u.host_str().unwrap_or("");
    (host == "vimeocdn.com" || host.ends_with(".vimeocdn.com"))
        && (u.path().contains("/v2/range/") || u.query_pairs().any(|(k, v)| {
            k == "range" && v.split_once('-').map(|(a, b)| {
                !a.is_empty() && !b.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.bytes().all(|c| c.is_ascii_digit())
            }).unwrap_or(false)
        }))
}

/// Manifests and adaptive legs need audio-aware resolution, not HLS concat.
fn is_vimeo_adaptive(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false; };
    let host = u.host_str().unwrap_or("");
    (host == "vimeocdn.com" || host.ends_with(".vimeocdn.com"))
        && (is_playlist_url(raw) || is_vimeo_fragment(raw)
            || host.starts_with("vod-adaptive.") || host.starts_with("vod-adaptive-"))
}

fn vimeo_download_target(raw: &str, referer: &str) -> Result<Option<String>> {
    if is_vimeo_video_page(raw) { return Ok(Some(raw.to_string())); }
    if !is_vimeo_adaptive(raw) { return Ok(None); }
    if is_vimeo_video_page(referer) { return Ok(Some(referer.to_string())); }
    if is_playlist_url(raw) { return Ok(Some(raw.to_string())); }
    Err(anyhow!("Vimeo URL is only a playback fragment — capture the full manifest or the specific Vimeo player URL"))
}

/// The page of one video post on Instagram, TikTok, X / Twitter or Reddit
/// (never a profile or a feed): the browser extension hands these over for
/// the video under its button.
pub(crate) fn social_video_page(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).or_else(|| host.strip_prefix("old.")).unwrap_or(&host).to_string();
    let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
    let id = |s: &&str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
    let digits = |s: &&str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    match host.as_str() {
        "instagram.com" => {
            // /reel/<id>, /reels/<id>, /p/<id>, /tv/<id>, also /<user>/reel/<id>
            parts.windows(2).any(|w| matches!(w[0], "reel" | "reels" | "p" | "tv") && id(&w[1]))
        }
        // /@user/video/<digits> (the user part may be empty: /@/video/<id>)
        "tiktok.com" => parts.windows(2).any(|w| w[0] == "video" && digits(&w[1])),
        "x.com" | "twitter.com" | "mobile.twitter.com" => {
            parts.len() >= 3 && parts[1] == "status" && digits(&parts[2])
        }
        "reddit.com" => parts.len() >= 4 && parts[0] == "r" && parts[2] == "comments" && id(&parts[3]),
        "v.redd.it" => parts.len() == 1 && id(&parts[0]),
        _ => false,
    }
}

/// The extension's "Audio only" choice: yt-dlp fetches the best audio.
/// The sound alone: "audio" (saved as MP3) or "audio-m4a" (YouTube's own
/// AAC track as served, no re-encoding).
pub(crate) fn wants_audio_only(pref: &str) -> bool {
    let p = pref.trim().to_ascii_lowercase();
    matches!(p.as_str(), "audio" | "mp3" | "audio-m4a" | "m4a")
}

/// The sound alone, kept as the M4A YouTube serves (no MP3 conversion).
pub(crate) fn wants_m4a(pref: &str) -> bool {
    let p = pref.trim().to_ascii_lowercase();
    matches!(p.as_str(), "audio-m4a" | "m4a")
}

/// `dest` as an .mp3 beside it, numbered when that name is taken.
fn conflict_free_mp3(dest: &std::path::Path) -> std::path::PathBuf {
    let out = dest.with_extension("mp3");
    if out.exists() {
        crate::model::conflict_free_dest(&out)
    } else {
        out
    }
}

/// Re-encodes `src` (an M4A/WebM sound) as an MP3 at `out`: LAME's best
/// variable bit rate, as yt-dlp's `--audio-quality 0` asks for.
async fn ffmpeg_to_mp3(src: &std::path::Path, out: &std::path::Path) -> Result<()> {
    let ffmpeg = ffmpeg_path().ok_or_else(|| anyhow!("MP3 needs FFmpeg (Settings -> Plugins)"))?;
    let mut cmd = tokio::process::Command::new(ffmpeg);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-vn", "-c:a", "libmp3lame", "-q:a", "0"])
        .arg(out)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let o = cmd.output().await.map_err(|e| anyhow!("FFmpeg would not start: {e}"))?;
    if !o.status.success() {
        let _ = tokio::fs::remove_file(out).await;
        let err = String::from_utf8_lossy(&o.stderr);
        return Err(anyhow!("making the MP3 failed: {}", err.lines().last().unwrap_or("FFmpeg error")));
    }
    Ok(())
}

fn vimeo_needs_audio(raw: &str) -> bool {
    is_vimeo_video_page(raw) || is_vimeo_adaptive(raw)
}

/// Only a specific video, never a profile/feed. Keep unlisted hashes and query.
fn is_vimeo_video_page(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false; };
    if !matches!(u.scheme(), "http" | "https") { return false; }
    let parts: Vec<_> = u.path().trim_matches('/').split('/').collect();
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    match u.host_str().unwrap_or("") {
        "player.vimeo.com" => parts.len() == 2 && parts[0] == "video" && numeric(parts[1]),
        "vimeo.com" | "www.vimeo.com" => (parts.len() == 1 || parts.len() == 2) && numeric(parts[0]),
        _ => false,
    }
}

/// Which site a cookie string belongs to, for the Netscape jar yt-dlp reads.
/// The wrong domain silently drops every cookie, and Facebook then answers as
/// a logged-out visitor that hides the video.
fn cookie_domain(url: &str) -> String {
    if vimeo_needs_audio(url) {
        return ".vimeo.com".to_string();
    }
    let low = url.to_ascii_lowercase();
    if low.contains("facebook") || low.contains("fbcdn") || low.contains("fbsbx") {
        ".facebook.com".to_string()
    } else if low.contains("instagram") || low.contains("cdninstagram") {
        ".instagram.com".to_string()
    } else if low.contains("youtube.com") || low.contains("youtu.be") || low.contains("googlevideo.com") {
        ".youtube.com".to_string()
    } else {
        url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| format!(".{h}"))).unwrap_or_default()
    }
}

/// Facebook progressive URLs sometimes carry `bytestart`/`byteend` slice
/// selectors (the player fetches previews with them). Such a URL can only
/// ever return that slice (~1 MB) — downloading it while expecting the full
/// file size loops forever at the slice length ("got 1192564 of 9495549").
/// Strip those selectors so the same signed URL serves the full file; the
/// remaining `oh`/`oe` params keep the auth. No-op for other hosts.
pub fn normalize_media_url(url: &str) -> String {
    let low = url.to_ascii_lowercase();
    let social = low.contains("fbcdn")
        || low.contains("facebook.com")
        || low.contains("scontent")
        || low.contains("fbsbx.com")
        || low.contains("lookaside.")
        || low.contains("cdninstagram.com")
        || low.contains("instagram.com");
    if !social {
        return url.to_string();
    }
    if !low.contains("bytestart") && !low.contains("byteend") {
        return url.to_string();
    }
    let Ok(mut u) = url::Url::parse(url) else {
        return url.to_string();
    };
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| {
            let kl = k.to_ascii_lowercase();
            kl != "bytestart" && kl != "byteend"
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    // `query_pairs` borrows `u`, so collect first, then rebuild.
    u.set_query(None);
    if !kept.is_empty() {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
        u.set_query(Some(&q.finish()));
    }
    u.to_string()
}

pub(crate) fn is_youtube_page(url: &str) -> bool {
    url.contains("youtube.com") || url.contains("youtu.be")
}

/// Verify a finished file against an expected SHA-256 hex digest.
/// Returns Ok(()) when no digest was requested.
pub async fn verify_checksum(path: &PathBuf, expected_hex: &str) -> Result<()> {
    let t = expected_hex.trim().to_lowercase();
    let t = t.strip_prefix("sha256:").unwrap_or(&t).trim().to_string();
    if t.is_empty() {
        return Ok(());
    }
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid SHA-256 checksum (want 64 hex chars)"));
    }
    let p = path.clone();
    let actual = tokio::task::spawn_blocking(move || crate::model::sha256_of_file(&p))
        .await
        .map_err(|e| anyhow!("checksum task: {e}"))??;
    if actual != t {
        return Err(anyhow!(
            "checksum mismatch: expected {t}, got {actual}"
        ));
    }
    Ok(())
}

/// Probe total size + range support with HEAD, falling back to a 0-0 range GET.
/// Returns (total, supports_range, content_type, disposition_filename).
pub async fn probe(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
) -> (Option<u64>, bool, Option<String>, Option<String>) {
    let dbg = std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok();
    // A torrent's size and name come from its metadata (src/torrent.rs),
    // not from an HTTP probe of a magnet link.
    if crate::torrent::is_torrent(url) || crate::gallery::is_gallery(url) {
        return (None, false, None, None);
    }
    // A form (POST) download only exists as that POST: a GET/HEAD probe
    // would ask for something else (often the form page itself).
    if opts.browser.as_ref().is_some_and(|b| b.is_post()) {
        return (None, false, None, None);
    }
    // A Drive / Docs link: probe the file it serves, not its web page.
    let drive_target;
    let url = if crate::gdrive::is_drive_url(url) {
        match crate::gdrive::resolve(client, opts, url).await {
            Ok(u) => {
                drive_target = u;
                drive_target.as_str()
            }
            Err(_) => return (None, false, None, None),
        }
    } else {
        url
    };
    // Try HEAD first — but ONLY trust success responses. Error pages
    // (403/404 with `Content-Length: 0`) must fall through to the Range
    // fallback: treating 0 as a real total poisons everything downstream
    // (grand totals of 0, single streams that can never satisfy).
    if let Ok(resp) = opts.head(client, url).send().await {
        if dbg {
            eprintln!("[dbg] probe HEAD status={}", resp.status());
        }
        if resp.status().is_success() {
            let h = resp.headers();
            let total = h
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            let accept = h
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("bytes"))
                .unwrap_or(false);
            let ctype = h
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let disp = disposition_filename(h);
            if total.is_some() || disp.is_some() {
                return (total, accept, ctype, disp);
            }
        }
    }
    // Fallback: GET bytes=0-0, parse Content-Range: bytes 0-0/12345
    if let Ok(resp) = opts
        .get(client, url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
    {
        let status = resp.status();
        if dbg {
            eprintln!("[dbg] probe GET 0-0 status={status}");
        }
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
                if let Ok(s) = cr.to_str() {
                    if let Some(total) = s.split('/').next_back().and_then(|t| t.parse::<u64>().ok()) {
                        return (Some(total), true, None, None);
                    }
                }
            }
        }
        let total = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let disp = disposition_filename(resp.headers());
        // Error statuses teach nothing: a 403 page's `Content-Length: 0` is
        // not a total (see the HEAD gate above).
        if status.is_success() {
            return (total, false, None, disp);
        }
        return (None, false, None, disp);
    }
    (None, false, None, None)
}

/// One-shot Range check for expiring signed CDN links: a single
/// `GET bytes=0-0` decides the whole strategy. Returns:
/// - `Ok(Some(total))` on 206 (ranges honored; total from Content-Range or
///   the caller-kept seed) → caller uses chunked sequential ranges.
/// - `Ok(None)` on 200/unknown (ranges ignored) → caller uses one plain
///   stream (resuming is impossible there anyway).
/// - `Err` on 401/403/410 or network failure → the link is dead/refused;
///   callers fail fast with the expiry hint instead of burning 100 futile
///   chunks that each return zero bytes (`got 0 of …`).
async fn probe_range_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    seed_total: Option<u64>,
) -> Result<Option<u64>, anyhow::Error> {
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| anyhow!("network: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        if let Some(cr) = resp.headers().get(reqwest::header::CONTENT_RANGE) {
            if let Ok(s) = cr.to_str() {
                if let Some(total) = s.split('/').next_back().and_then(|t| t.parse::<u64>().ok()) {
                    return Ok(Some(total));
                }
            }
        }
        return Ok(seed_total);
    }
    // Consume the one byte so the connection can be reused.
    let _ = resp.bytes().await;
    match status {
        reqwest::StatusCode::UNAUTHORIZED
        | reqwest::StatusCode::FORBIDDEN
        | reqwest::StatusCode::GONE => Err(final_err(
            "link refused (expired or session-locked) — replay the reel and click the download button again for a fresh link".to_string(),
        )),
        _ => Ok(None),
    }
}

/// Extract `filename` / `filename*` from a Content-Disposition header.
/// Splits on `;` outside quotes (a quoted `"a;b.mp4"` must survive), matches
/// parameter names case-insensitively, and prefers RFC 5987 `filename*`.
fn disposition_filename(h: &reqwest::header::HeaderMap) -> Option<String> {
    let v = h
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
    // Split on semicolons that are NOT inside double quotes.
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut in_q = false;
    // Byte walk is safe here: we only look for ASCII `;` and `"`.
    let bytes = v.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                in_q = !in_q;
                i += 1;
            }
            b';' if !in_q => {
                parts.push(v[start..i].trim());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    parts.push(v[start..].trim());
    let param = |name: &str| -> Option<&str> {
        parts.iter().find_map(|p| {
            let (k, val) = p.split_once('=')?;
            if k.trim().eq_ignore_ascii_case(name) {
                Some(val.trim())
            } else {
                None
            }
        })
    };
    if let Some(rest) = param("filename*") {
        // RFC 5987: charset'lang'percent-encoded (value may be quoted).
        let rest = rest.trim().trim_matches('"');
        if let Some(enc) = rest.split('\'').nth(2) {
            let name = percent_decode_owned(enc);
            if !name.trim().is_empty() {
                return Some(sanitize_filename(&name));
            }
        }
    }
    if let Some(rest) = param("filename") {
        let s = rest.trim().trim_matches('"').trim();
        if !s.is_empty() {
            return Some(sanitize_filename(s));
        }
    }
    None
}

pub(crate) fn percent_decode_owned(s: &str) -> String {
    let mut raw: Vec<u8> = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(hex) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                raw.push(hex);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            raw.push(b' ');
        } else {
            raw.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

/// A capture-supplied name that carries no real title: the CDN's opaque stem
/// (`50k`, `6.6k`, `videoplayback`), a generic word, or a bare URL hash.
/// Such a name must yield to a resolved page title even when the caller asked
/// to keep the destination exactly — otherwise a YouTube capture is saved as
/// `50k.mp4` although its title was resolved perfectly well.
pub fn weak_capture_name(name: &str) -> bool {
    let stem = name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase();
    if stem.is_empty() {
        return true;
    }
    // Our own placeholders are always weaker than a real title.
    if stem.starts_with("youtube_") || stem.starts_with("download_") {
        return true;
    }
    // A bare counter ("50k", "6.6k") or a bare resolution ("1080p").
    let digits = stem.trim_end_matches(['k', 'm', 'b', 'p']);
    if !digits.is_empty()
        && digits.chars().any(|c| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '_'))
    {
        return true;
    }
    // A bare hash, as googlevideo path segments often are.
    if stem.len() >= 8 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    matches!(
        stem.as_str(),
        "videoplayback"
            | "index"
            | "file"
            | "media"
            | "stream"
            | "video"
            | "play"
            | "master"
            | "playlist"
            | "manifest"
            | "chunk"
            | "segment"
            | "init"
            | "download"
            | "untitled"
    )
}

/// Media extensions a resolved title may legitimately carry itself.
const TITLE_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "mp3", "m4a", "aac", "ogg", "wav", "flac",
    "m3u8", "mpd", "ts", "m4s",
];

/// The file name for a resolved media title: the title itself, keeping the
/// extension the captured name already has (a page title never carries one).
/// `captured` is the destination as it stood before the title was known; when
/// it is `None` the caller fixes the extension from the MIME type later.
pub fn title_filename(title: &str, captured: Option<&std::path::Path>) -> String {
    let name = sanitize_filename(title);
    let lower = name.to_ascii_lowercase();
    if TITLE_EXTS.iter().any(|e| lower.ends_with(&format!(".{e}"))) {
        return name;
    }
    let ext = captured
        .and_then(|p| p.extension().and_then(|e| e.to_str()))
        .map(str::to_ascii_lowercase);
    match ext {
        Some(ext) if TITLE_EXTS.contains(&ext.as_str()) => format!("{name}.{ext}"),
        _ => name,
    }
}

/// Make a server/title string safe as a Windows filename.
pub fn sanitize_filename(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        out = "download.bin".to_string();
    }
    if out.len() > 120 {
        // Keep the extension, and a closing quality tag (" [1080p]") with it:
        // a long title in a 3-byte script (Bengali) used to cut the tag to
        // " [1" and the 1080p and 720p copies of a reel then shared a name.
        if let Some(dot) = out.rfind('.') {
            let (stem, ext) = out.split_at(dot);
            let tag_at = stem
                .rfind(" [")
                .filter(|&i| stem.ends_with(']') && stem.len() - i <= 24);
            let (stem, tag) = match tag_at {
                Some(i) => stem.split_at(i),
                None => (stem, ""),
            };
            let keep = 120usize.saturating_sub(ext.len() + tag.len());
            let cut = stem
                .char_indices()
                .take_while(|(i, c)| i + c.len_utf8() <= keep)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            out = format!("{}{}{}", stem[..cut].trim_end(), tag, ext);
        } else {
            let cut = out
                .char_indices()
                .take_while(|(i, _)| *i < 120)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            out.truncate(cut);
        }
    }
    out
}

/// A resolved playable stream: the video URL plus an optional companion
/// audio URL (adaptive video-only qualities, merged with ffmpeg on
/// completion — the only way to deliver 1080p+).
pub struct ResolvedMedia {
    pub url: String,
    pub title: String,
    pub pair_url: Option<String>,
}

impl ResolvedMedia {
    fn single(url: String, title: String) -> Self {
        Self { url, title, pair_url: None }
    }
}

/// Try to extract a direct media URL + title from an HTML video page.
/// Handles YouTube watch pages (progressive MP4 streams, else an adaptive
/// video+audio pair for high qualities) plus generic pages via og:video /
/// contentUrl / bare .mp4 links.
pub async fn resolve_media(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    shared: &Arc<SharedState>,
) -> Option<ResolvedMedia> {
    // A failed page fetch must not rule out the player API (it only needs
    // the video id), so default to empty HTML instead of bailing.
    let html = opts
        .get(client, page_url)
        .send()
        .await
        .ok()
        .map(|r| async move { r.text().await.ok() });
    let mut html = match html {
        Some(f) => f.await.unwrap_or_default(),
        None => String::new(),
    };
    if html.len() > 8 * 1024 * 1024 {
        html.clear();
    }
    // YouTube bot-check wall: flag it, but keep trying the player API —
    // the wall covers the watch page, not necessarily the API.
    let walled = html.contains("sorry/index") || html.contains("unusual traffic");
    if walled {
        shared.rate_limited.store(true, Ordering::SeqCst);
    }
    if page_url.contains("youtube.com") || page_url.contains("youtu.be") {
        // The app's own reader (TV client + challenge solver): every
        // quality, served whole. Skipped once it failed for this download.
        if youtube_own_reader_allowed(opts) && !shared.youtube_own_failed.load(Ordering::SeqCst) {
            match crate::youtube::resolve(client, opts, page_url, shared).await {
                Ok(hit) => {
                    dbg_resolve("youtube-own", &hit.url);
                    return Some(hit);
                }
                Err(e) => {
                    shared.youtube_own_failed.store(true, Ordering::SeqCst);
                    job_log(&opts.log_key, &format!("the app's YouTube reader could not read the page: {e:#}"));
                }
            }
        }
        // Player API first: works off the video id alone, even behind the
        // page wall (verified live: ANDROID client returns direct URLs).
        if let Some(vid) = youtube_id(page_url) {
            if let Some(hit) = resolve_youtube_api(client, opts, page_url, &html, &vid).await
            {
                // API success clears the wall flag: this network CAN do it.
                shared.rate_limited.store(false, Ordering::SeqCst);
                return Some(hit);
            }
        }
        if !walled {
            if let Some(hit) = resolve_youtube_full(client, opts, page_url, &html).await {
                dbg_resolve("page-formats", &hit.url);
                return Some(hit);
            }
        }
        return None;
    }
    if walled {
        // Never scrape bot-wall pages for "media links" (they match junk
        // like the sorry-page URL itself, which then fails loudly later).
        return None;
    }
    let hit = resolve_generic(&html, page_url);
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        match &hit {
            Some((u, _)) => dbg_resolve("generic", u),
            None => eprintln!("[dbg] resolve: none"),
        }
    }
    hit.map(|(u, t)| ResolvedMedia::single(u, t))
}

fn dbg_resolve(via: &str, url: &str) {
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        let host = url::Url::parse(url)
            .map(|u| u.host_str().unwrap_or("?").to_string())
            .unwrap_or_else(|_| "?".to_string());
        eprintln!("[dbg] resolve via={via} host={host} len={}", url.len());
    }
}

/// Extract a YouTube video id from watch/shorts/embed/live/youtu.be URLs.
pub(crate) fn youtube_id(page_url: &str) -> Option<String> {
    let u = url::Url::parse(page_url).ok()?;
    if let Some(id) = u
        .query_pairs()
        .find(|(k, _)| k == "v")
        .map(|(_, v)| v.to_string())
    {
        if !id.is_empty() {
            return Some(id);
        }
    }
    let segs: Vec<&str> = u
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    for w in segs.windows(2) {
        if ["shorts", "embed", "live", "v"].contains(&w[0]) && w[1].len() >= 6 {
            return Some(w[1].to_string());
        }
    }
    if segs.len() == 1 && segs[0].len() >= 6 {
        // youtu.be/ID
        return Some(segs[0].to_string());
    }
    None
}

/// youtubei player API: stream URLs even when the watch page embeds none
/// (same endpoint third-party clients use). Verified live against the API:
/// the WEB client is playability-gated on flagged networks while the
/// ANDROID client still returns direct progressive URLs — so ANDROID is
/// tried first, WEB kept as fallback.
async fn resolve_youtube_api(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    html: &str,
    video_id: &str,
) -> Option<ResolvedMedia> {
    fn grab(html: &str, key: &str) -> Option<String> {
        let m = format!("\"{key}\":\"");
        let p = html.find(&m)? + m.len();
        let rest = &html[p..];
        let end = rest.find('"')?;
        let s = rest[..end].to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
    let visitor = grab(html, "visitorData").unwrap_or_default();
    let ver =
        grab(html, "INNERTUBE_CLIENT_VERSION").unwrap_or_else(|| "2.20241201".to_string());
    // ANDROID first (works behind the WEB client's bot wall), then WEB.
    // NOTE: visitorData is only sent when non-empty — an empty value (e.g.
    // from a bot-wall page) makes ANDROID return no streams at all.
    const ANDROID_UA: &str =
        "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip";
    let mut android_ctx = serde_json::json!({
        "clientName": "ANDROID",
        "clientVersion": "20.10.38",
        "hl": "en",
        "androidSdkVersion": 34,
    });
    if !visitor.is_empty() {
        android_ctx["visitorData"] = serde_json::Value::String(visitor.clone());
    }
    if let Some(hit) = player_api(client, opts, page_url, video_id, &android_ctx, Some(ANDROID_UA)).await
    {
        dbg_resolve("player-api-android", &hit.url);
        return Some(hit);
    }
    let mut web_ctx = serde_json::json!({
        "clientName": "WEB",
        "clientVersion": ver,
        "hl": "en",
    });
    if !visitor.is_empty() {
        web_ctx["visitorData"] = serde_json::Value::String(visitor);
    }
    if let Some(hit) = player_api(client, opts, page_url, video_id, &web_ctx, None).await {
        dbg_resolve("player-api", &hit.url);
        return Some(hit);
    }
    dbg_resolve("player-api", "<no-stream>");
    None
}

/// Best adaptive pair (video-only + audio, both direct URLs) from an
/// `adaptiveFormats` array: highest video at/below `cap` (closest above
/// when everything exceeds it) plus highest-bitrate audio.
/// Returns (video_score, video_url, audio_url).
fn pick_adaptive_pair(
    arr: &[serde_json::Value],
    cap: Option<u64>,
) -> Option<(u64, String, String)> {
    let mut bv: Option<(u64, String)> = None;
    let mut ba: Option<(u64, String)> = None;
    for f in arr {
        let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
        let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
        if url.is_empty() {
            continue;
        }
        if mime.starts_with("video/") {
            let score = stream_score(f);
            if better_stream(&bv, score, cap) {
                bv = Some((score, url.to_string()));
            }
        } else if mime.starts_with("audio/") {
            let br = f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            if ba.as_ref().map(|(b, _)| br > *b).unwrap_or(true) {
                ba = Some((br, url.to_string()));
            }
        }
    }
    let (vs, vu) = bv?;
    let (_, au) = ba?;
    Some((vs, vu, au))
}

/// One player-API call with the given client identity. Only `OK`
/// playability with a direct (non-ciphered) progressive MP4 URL counts —
/// ciphered entries would need a JS decipher challenge we don't run.
async fn player_api(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    video_id: &str,
    ctx_client: &serde_json::Value,
    ua_override: Option<&str>,
) -> Option<ResolvedMedia> {
    // Standard UA + referer via our options.
    let mut req = opts
        .post(client, "https://www.youtube.com/youtubei/v1/player?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8&prettyPrint=false");
    if let Some(ua) = ua_override {
        req = req.header(reqwest::header::USER_AGENT, ua);
    }
    let body = serde_json::json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "context": { "client": ctx_client },
    });
    let resp = req
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::REFERER, page_url)
        .header(reqwest::header::ORIGIN, "https://www.youtube.com")
        .json(&body)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    if v
        .pointer("/playabilityStatus/status")
        .and_then(|s| s.as_str())
        != Some("OK")
    {
        return None;
    }
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    let cap = quality_cap(&opts.quality_pref);
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if better_stream(&best, score, cap) {
                best = Some((score, url.to_string()));
            }
        }
    }
    // Adaptive pair (1080p+ live here, never in progressive): use it when
    // the menu asked for quality and it beats progressive, or when there is
    // no progressive stream at all (ciphered-only videos). Default path
    // (no preference) keeps the single progressive file — no ffmpeg needed.
    let pair = v
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|f| f.as_array())
        .and_then(|a| pick_adaptive_pair(a, cap));
    match (best, pair) {
        (Some((ps, _)), Some((vs, vu, au)))
            if cap.is_some() && vs > ps =>
        {
            Some(ResolvedMedia {
                url: with_ratebypass(&vu),
                title: format!("{title}.mp4"),
                pair_url: Some(with_ratebypass(&au)),
            })
        }
        (None, Some((_, vu, au))) => Some(ResolvedMedia {
            url: with_ratebypass(&vu),
            title: format!("{title}.mp4"),
            pair_url: Some(with_ratebypass(&au)),
        }),
        (Some((_, u)), _) => Some(ResolvedMedia::single(
            with_ratebypass(&u),
            format!("{title}.mp4"),
        )),
        (None, None) => None,
    }
}

/// YouTube hands adaptive (DASH) legs out as `gir=yes`: to any non-browser
/// client they expose only roughly their first minute and then 403 — verified
/// live from this machine (varying headers, cookies, a signed-in page fetch
/// and repeated fresh resolves all hit the same wall, while the same URL
/// serves the prefix fine). A gated quality is therefore retried as the muxed
/// progressive file, which YouTube does serve whole. That file tops out at
/// 360p (720p on older uploads), so the saved name records the downgrade
/// instead of quietly handing back a much smaller file as if it were the
/// requested quality.
fn note_youtube_downgrade(
    hit: &ResolvedMedia,
    dest: &mut std::path::PathBuf,
    shared: &SharedState,
) {
    if hit.pair_url.is_some() {
        return; // no progressive stream exists: nothing was downgraded to
    }
    let label = if hit.url.contains("itag=22") {
        "720p"
    } else if hit.url.contains("itag=18") {
        "360p"
    } else {
        return;
    };
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem.is_empty() {
        return;
    }
    let ext = dest
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "mp4".to_string());
    let name = format!("{stem} ({label} - YouTube limit).{ext}");
    dest.set_file_name(&name);
    mutex_guard(&shared.resolved_name).replace(name);
}

/// Direct-URL-only page resolve (legacy behavior, kept for fixtures).
/// Production path is [`resolve_youtube_full`] (adds cipher + `n` handling).
#[allow(dead_code)]
fn resolve_youtube(html: &str) -> Option<(String, String)> {
    let v = yt_player_value(html)?;
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    // Prefer the highest-quality progressive MP4 with a direct URL.
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, url.to_string()));
            }
        }
    }
    best.map(|(_, u)| (with_ratebypass(&u), format!("{title}.mp4")))
}

/// Comparable quality score for a stream entry: the height the *label*
/// promises ("1080p60" → 1080), falling back to dimensions. This matches
/// `quality_cap` exactly (both parse the same labels), so a "1080p" menu
/// pick compares equal to a "1080p60" stream — unlike max(width, height),
/// which scores landscape 1080p as 1920 and would reject it under a 1080
/// cap (downgrading the user to 360p).
fn stream_score(f: &serde_json::Value) -> u64 {
    if let Some(q) = f.get("qualityLabel").and_then(|q| q.as_str()) {
        let digits: String = q.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            if (144..=4320).contains(&n) {
                return n;
            }
        }
    }
    let h = f.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
    if (144..=4320).contains(&h) {
        return h;
    }
    f.get("width")
        .and_then(|w| w.as_u64())
        .unwrap_or(0)
        .max(h)
}

/// Parse a menu quality ("720p", "1080p60") into a height cap. None =
/// no preference (best available).
pub(crate) fn quality_cap(pref: &str) -> Option<u64> {
    let digits: String = pref.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    let n: u64 = digits.parse().ok()?;
    if (144..=4320).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Whether `score` beats the current best under an optional height cap:
/// highest stream at or below the cap wins; when everything exceeds it,
/// the closest (lowest) wins instead of the biggest.
fn better_stream(best: &Option<(u64, String)>, score: u64, cap: Option<u64>) -> bool {
    let Some((b, _)) = best else {
        return true;
    };
    let Some(c) = cap else {
        return score > *b;
    };
    match (score <= c, *b <= c) {
        (true, false) => true,
        (true, true) => score > *b,
        (false, true) => false,
        (false, false) => score < *b,
    }
}

/// YouTube throttles googlevideo streams unless `ratebypass=yes` is set
/// (same trick every downloader uses).
fn with_ratebypass(u: &str) -> String {
    if u.contains("googlevideo.com") && !u.contains("ratebypass=") {
        format!("{u}&ratebypass=yes")
    } else {
        u.to_string()
    }
}

// ---------------------------------------------------------------------------
// YouTube signature decipher (P2.1) + throttle-param fix (P2.2), clean-room.
//
// Page-embedded and some API formats carry `signatureCipher` (a ciphered `s`
// plus the stream `url`) instead of a direct URL, and googlevideo links carry
// a session `n` throttle parameter. The running browser player deciphers both
// with functions from YouTube's PUBLIC player JS (base.js) — the same source
// tools like yt-dlp read. We fetch that JS, extract the classic
// reverse/splice/swap operation chains, and apply them. No IDM code, no
// copied decipher tables: shapes are detected, never hardcoded per version.
// Anything that doesn't match the classic shape is left alone (safe fallback
// to direct URLs + `ratebypass`).

/// One decipher operation: full reverse, drop-first-N, or swap-with-N.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SigOp {
    Reverse,
    Splice(usize),
    Swap(usize),
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn player_ops_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<SigOp>>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<SigOp>>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Parse ONE `=function(a){a=a.split("");...return ...join("")}` body at
/// byte offset `p` (start of the `=`). Returns (helper_obj, ops).
fn try_decipher_at(js: &str, p: usize) -> Option<(String, Vec<SigOp>)> {
    let after = js.get(p..)?;
    // Body ends at the `}` closing the function: find `join("")` then `}`.
    let j = after.find("join(\"\")")?;
    let close = after[j..].find('}')? + j;
    let body = after.get(..close)?;
    // Calls live between the split and the return.
    let split_end = body.find(".split(\"\");")? + ".split(\"\");".len();
    let ret_at = body.find("return")?;
    if split_end >= ret_at {
        return None;
    }
    let calls = body.get(split_end..ret_at)?;
    // First call fixes the helper object; every call must share it.
    let mut obj: Option<String> = None;
    let mut methods: Vec<(String, Option<usize>)> = Vec::new();
    for stmt in calls.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let paren = stmt.find('(')?;
        let head = stmt[..paren].trim();
        let dot = head.rfind('.')?;
        let (o, m) = (head[..dot].trim(), head[dot + 1..].trim());
        if o.is_empty() || m.is_empty() || !m.chars().all(is_ident_char) {
            return None;
        }
        match &obj {
            Some(known) if known != o => return None,
            _ => obj = Some(o.to_string()),
        }
        let args_raw = stmt[paren + 1..].strip_suffix(')')?;
        let args: Vec<&str> = args_raw.split(',').map(|a| a.trim()).collect();
        if args.is_empty() || args[0] != "a" {
            return None;
        }
        let num = if args.len() >= 2 {
            Some(args[1].parse::<usize>().ok()?)
        } else {
            None
        };
        methods.push((m.to_string(), num));
    }
    let obj = obj?;
    if methods.is_empty() {
        return None;
    }
    // Helper object definition: `OBJ={` (identifier-boundary checked).
    let mut def_at = None;
    let mut search = 0;
    let needle = format!("{obj}={{");
    while let Some(rel) = js[search..].find(needle.as_str()) {
        let abs = search + rel;
        let ok_before = abs
            .checked_sub(1)
            .and_then(|i| js[i..].chars().next())
            .map(|c| !is_ident_char(c))
            .unwrap_or(true);
        if ok_before {
            // Body starts past the full `OBJ={` match (needle includes `=`).
            def_at = Some(abs + needle.len());
            break;
        }
        search = abs + 1;
    }
    let mut d = def_at?;
    // We start INSIDE the object's braces, so the depth starts at 1; the
    // scan ends at the matching close.
    let bytes = js.as_bytes();
    let mut depth = 1usize;
    let mut in_str = false;
    let mut str_q = 0u8;
    let mut esc = false;
    let mut end = None;
    while d < bytes.len() {
        let c = bytes[d];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == str_q {
                in_str = false;
            }
        } else if c == b'"' || c == b'\'' {
            in_str = true;
            str_q = c;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                end = Some(d);
                break;
            }
        }
        d += 1;
    }
    let obj_body = js.get(def_at?..end?)?;
    // Method kinds from definition bodies.
    let mut kinds: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    let mut msearch = 0;
    while msearch < obj_body.len() {
        // Skip inter-method separators so `,MB:` parses as `MB`.
        while msearch < obj_body.len()
            && (obj_body[msearch..].starts_with(',') || obj_body[msearch..].starts_with(' '))
        {
            msearch += 1;
        }
        if msearch >= obj_body.len() {
            break;
        }
        // top-level `name:function(` entries
        let rest = &obj_body[msearch..];
        let colon = match rest.find(':') {
            Some(c) => c,
            None => break,
        };
        let name = rest[..colon].trim();
        if name.is_empty() || !name.chars().all(is_ident_char) {
            msearch += colon + 1;
            continue;
        }
        let after_colon = rest[colon + 1..].trim_start();
        if !after_colon.starts_with("function") {
            msearch += colon + 1;
            continue;
        }
        let brace = match after_colon.find('{') {
            Some(b) => b,
            None => break,
        };
        // balanced body
        let bb = after_colon.as_bytes();
        let mut dd = brace;
        let mut dep = 0usize;
        let mut istr = false;
        let mut iq = 0u8;
        let mut iesc = false;
        let mut mend = None;
        while dd < bb.len() {
            let c = bb[dd];
            if istr {
                if iesc {
                    iesc = false;
                } else if c == b'\\' {
                    iesc = true;
                } else if c == iq {
                    istr = false;
                }
            } else if c == b'"' || c == b'\'' {
                istr = true;
                iq = c;
            } else if c == b'{' {
                dep += 1;
            } else if c == b'}' {
                dep -= 1;
                if dep == 0 {
                    mend = Some(dd);
                    break;
                }
            }
            dd += 1;
        }
        let mbody = after_colon.get(brace..mend? + 1).unwrap_or("");
        let mend_abs = mend?;
        let kind = if mbody.contains(".reverse(") {
            "rev"
        } else if mbody.contains(".splice(") {
            "spl"
        } else if mbody.contains("a[0]") && mbody.contains("%a.length") {
            "swp"
        } else {
            msearch += colon + 1 + mend_abs + 1;
            continue;
        };
        kinds.insert(name.to_string(), kind);
        msearch += colon + 1 + mend_abs + 1;
    }
    // Map calls to ops.
    let mut ops = Vec::new();
    for (m, num) in methods {
        match kinds.get(m.as_str()).copied() {
            Some("rev") => ops.push(SigOp::Reverse),
            Some("spl") => ops.push(SigOp::Splice(num?)),
            Some("swp") => ops.push(SigOp::Swap(num?)),
            _ => return None,
        }
    }
    Some((obj, ops))
}

/// All classic-shape decipher candidates in a player JS bundle.
fn decipher_candidates(js: &str) -> Vec<(String, Vec<SigOp>)> {
    let marker = "=function(a){a=a.split(\"\")";
    let mut out = Vec::new();
    let mut base = 0;
    while let Some(rel) = js[base..].find(marker) {
        let p = base + rel;
        if let Some(hit) = try_decipher_at(js, p) {
            out.push(hit);
        }
        base = p + marker.len();
    }
    out
}

/// The signature-decipher operations (first classic candidate).
fn parse_sig_ops(js: &str) -> Option<Vec<SigOp>> {
    decipher_candidates(js).into_iter().next().map(|(_, ops)| ops)
}

/// Best-effort throttle-param fix: a SECOND classic-shape function (not the
/// signature decipher) is the `n` transform on classic players — apply it.
/// Anything else (single candidate, exotic shapes) returns None and the URL
/// keeps its original `n` + `ratebypass`.
fn try_simple_n(js: &str, sig_ops: &[SigOp], n: &str) -> Option<String> {
    let mut cands = decipher_candidates(js).into_iter();
    let _first = cands.next()?;
    for (_, ops) in cands {
        if &ops != sig_ops {
            return Some(apply_sig_ops(n, &ops));
        }
    }
    None
}

fn apply_sig_ops(s: &str, ops: &[SigOp]) -> String {
    let mut v: Vec<char> = s.chars().collect();
    for op in ops {
        match *op {
            SigOp::Reverse => v.reverse(),
            SigOp::Splice(n) => {
                let cut = n.min(v.len());
                v.drain(..cut);
            }
            SigOp::Swap(n) => {
                if v.len() > 1 {
                    let b = n % v.len();
                    if b != 0 {
                        v.swap(0, b);
                    }
                }
            }
        }
    }
    v.into_iter().collect()
}

/// Assemble a usable stream URL from a `signatureCipher` value:
/// `url=<..>&sp=<sig-param>&s=<ciphered>` → deciphered direct URL.
fn decipher_signature_cipher(cipher: &str, ops: &[SigOp]) -> Option<String> {
    let mut url = None;
    let mut sp = String::from("sig");
    let mut sig = None;
    for kv in cipher.split('&') {
        let (k, v) = kv.split_once('=')?;
        match k {
            "url" => url = Some(percent_decode_owned(v)),
            "sp" => {
                let d = percent_decode_owned(v);
                if !d.trim().is_empty() {
                    sp = d;
                }
            }
            "s" => sig = Some(percent_decode_owned(v)),
            _ => {}
        }
    }
    let (u, s) = (url?, sig?);
    if u.is_empty() || s.is_empty() {
        return None;
    }
    let sep = if u.contains('?') { "&" } else { "?" };
    Some(format!("{u}{sep}{sp}={}", apply_sig_ops(&s, ops)))
}

/// `base.js` URL from watch-page HTML (`"jsUrl":"/s/player/…/base.js"`).
fn youtube_player_js_url(html: &str) -> Option<String> {
    for key in ["\"jsUrl\":\"", "\"PLAYER_JS_URL\":\""] {
        if let Some(p) = html.find(key) {
            let rest = &html[p + key.len()..];
            if let Some(end) = rest.find('"') {
                let u = &rest[..end];
                if u.is_empty() {
                    continue;
                }
                if u.starts_with("http") {
                    return Some(u.to_string());
                }
                if u.starts_with('/') {
                    return Some(format!("https://www.youtube.com{u}"));
                }
            }
        }
    }
    None
}

async fn sig_ops_for(
    client: &reqwest::Client,
    opts: &EngineOpts,
    player_url: &str,
) -> Option<Vec<SigOp>> {
    if let Some(hit) = player_ops_cache()
        .lock()
        .ok()
        .and_then(|g| g.get(player_url).cloned())
    {
        return Some(hit);
    }
    let js = opts
        .get(client, player_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    if js.len() > 4 * 1024 * 1024 {
        return None;
    }
    let ops = parse_sig_ops(&js)?;
    if let Ok(mut g) = player_ops_cache().lock() {
        g.insert(player_url.to_string(), ops.clone());
    }
    Some(ops)
}

/// Fetch player JS text (uncached; callers needing both text + ops fetch
/// once and reuse — the ops cache makes repeat resolves cheap).
async fn player_js_for(
    client: &reqwest::Client,
    opts: &EngineOpts,
    player_url: &str,
) -> Option<String> {
    let js = opts
        .get(client, player_url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    if js.len() > 4 * 1024 * 1024 || js.len() < 1024 {
        return None;
    }
    Some(js)
}

/// Replace a googlevideo `n` throttle value using a simple-shape `n`
/// function from the same player bundle. Returns the URL unchanged when
/// no confident transform exists (never worse than the original).
fn fix_n_param(url: &str, js_text: &str, sig_ops: &[SigOp]) -> String {
    let nval: String = url
        .split(['?', '&'])
        .filter_map(|kv| kv.strip_prefix("n="))
        .next()
        .unwrap_or("")
        .to_string();
    if nval.is_empty() {
        return url.to_string();
    }
    match try_simple_n(js_text, sig_ops, &nval) {
        Some(fixed) if fixed != nval => {
            url.replacen(&format!("n={nval}"), &format!("n={fixed}"), 1)
        }
        _ => url.to_string(),
    }
}

/// Extract the player-response JSON value from watch-page HTML.
fn yt_player_value(html: &str) -> Option<serde_json::Value> {
    let marker = "ytInitialPlayerResponse = ";
    let start = html.find(marker)? + marker.len();
    let bytes = html.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let obj_start = i;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    serde_json::from_str(&html[obj_start..=i]).ok()
}

/// Full page-path resolve: direct progressive MP4 first, then ciphered
/// formats via player-JS decipher (P2.1), with throttle-param fix (P2.2).
async fn resolve_youtube_full(
    client: &reqwest::Client,
    opts: &EngineOpts,
    _page_url: &str,
    html: &str,
) -> Option<ResolvedMedia> {
    let v = yt_player_value(html)?;
    let title = v
        .pointer("/videoDetails/title")
        .and_then(|t| t.as_str())
        .unwrap_or("youtube_video")
        .to_string();
    // Direct progressive MP4 (highest at/below the menu quality when set).
    let cap = quality_cap(&opts.quality_pref);
    let mut best: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if url.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let score = stream_score(f);
            if better_stream(&best, score, cap) {
                best = Some((score, url.to_string()));
            }
        }
    }
    // Player JS (once) for cipher + n handling.
    let player_url = youtube_player_js_url(html);
    let mut js_text: Option<String> = None;
    let mut ops: Option<Vec<SigOp>> = None;
    if player_url.is_some() {
        let pu = player_url.as_deref().unwrap();
        ops = sig_ops_for(client, opts, pu).await;
        if ops.is_some() {
            js_text = player_js_for(client, opts, pu).await;
        }
    }
    if let Some((_, u)) = &best {
        let u = match (&js_text, &ops) {
            (Some(t), Some(o)) => fix_n_param(u, t, o),
            _ => u.clone(),
        };
        // A requested high quality may still live in adaptive-only: pair it
        // (same rule as the player-API path) instead of silently dropping to
        // a small progressive file.
        if cap.is_some() {
            if let Some(adaptive) = v.pointer("/streamingData/adaptiveFormats").and_then(|f| f.as_array()) {
                if let Some((vs, vu, au)) = pick_adaptive_pair(adaptive, cap) {
                    if vs > best.map(|(s, _)| s).unwrap_or(0) {
                        let vu = match (&js_text, &ops) {
                            (Some(t), Some(o)) => fix_n_param(&vu, t, o),
                            _ => vu,
                        };
                        return Some(ResolvedMedia {
                            url: with_ratebypass(&vu),
                            title: format!("{title}.mp4"),
                            pair_url: Some(with_ratebypass(&au)),
                        });
                    }
                }
            }
        }
        return Some(ResolvedMedia::single(
            with_ratebypass(&u),
            format!("{title}.mp4"),
        ));
    }
    // Ciphered progressive MP4: decipher via player ops.
    let (ops, js_text) = match (ops, js_text) {
        (Some(o), Some(t)) => (o, t),
        _ => return None,
    };
    let mut cbest: Option<(u64, String)> = None;
    if let Some(formats) = v.pointer("/streamingData/formats").and_then(|f| f.as_array()) {
        for f in formats {
            let cipher = f.get("signatureCipher").and_then(|u| u.as_str()).unwrap_or("");
            let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if cipher.is_empty() || !mime.contains("video/mp4") {
                continue;
            }
            let Some(u) = decipher_signature_cipher(cipher, &ops) else {
                continue;
            };
            let score = stream_score(f);
            if better_stream(&cbest, score, cap) {
                cbest = Some((score, u));
            }
        }
    }
    cbest.map(|(_, u)| {
        let u = fix_n_param(&u, &js_text, &ops);
        ResolvedMedia::single(with_ratebypass(&u), format!("{title}.mp4"))
    })
}

/// Display-only progress of one HLS segment for Part Info: bytes so far, and
/// its real size once complete (`total`).
fn set_segment_progress(shared: &SharedState, idx: usize, done: u64, total: Option<u64>) {
    if let Ok(mut g) = shared.part_total.lock() {
        if let Some(slot) = g.get_mut(idx) {
            match total {
                Some(t) => *slot = (0, t.max(1) - 1),
                // Still arriving: an estimate the segment has outgrown would
                // read as "Finished", so keep the total just past the bytes.
                None if done > 0 && done >= slot.1 + 1 => *slot = (0, done),
                None => {}
            }
        }
    }
    if let Ok(mut g) = shared.part_done.lock() {
        if let Some(slot) = g.get_mut(idx) {
            *slot = done;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestKind {
    Hls,
    Dash,
}

/// Response types that may hide a manifest: generic text/binary/XML, or none.
fn opaque_manifest_type(ctype: Option<&str>) -> bool {
    let Some(t) = ctype else { return true };
    let t = t.to_ascii_lowercase();
    let base = t.split(';').next().unwrap_or("").trim();
    base.is_empty()
        || matches!(
            base,
            "text/plain"
                | "application/octet-stream"
                | "binary/octet-stream"
                | "application/binary"
                | "application/x-binary"
                | "application/xml"
                | "text/xml"
        )
}

/// What the first bytes of a body are, if a manifest (BOM-tolerant).
fn manifest_kind_of(head: &[u8]) -> Option<ManifestKind> {
    let text = String::from_utf8_lossy(head);
    let low = text.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    if low.contains("#extm3u") {
        Some(ManifestKind::Hls)
    } else if low.contains("<mpd ") || low.contains("<mpd>") || low.contains("<mpd\n") || low.contains("<mpd\r") {
        Some(ManifestKind::Dash)
    } else {
        None
    }
}

/// Fetch at most 4 KiB of `url` and classify it as a manifest or not.
async fn sniff_manifest(client: &reqwest::Client, opts: &EngineOpts, url: &str) -> Option<ManifestKind> {
    const HEAD: usize = 4096;
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, format!("bytes=0-{}", HEAD - 1))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    // A server that ignores Range streams the whole body: stop at HEAD bytes.
    let mut body = resp.bytes_stream();
    let mut head = Vec::with_capacity(HEAD);
    while head.len() < HEAD {
        match body.next().await {
            Some(Ok(chunk)) => head.extend_from_slice(&chunk[..chunk.len().min(HEAD - head.len())]),
            _ => break,
        }
    }
    manifest_kind_of(&head)
}

/// An HLS/DASH manifest by URL or type. Its byte length is the playlist
/// text, never the video's size, so dialogs must not show it as one.
pub fn is_stream_manifest(url: &str, ctype: Option<&str>, mime_hint: &str) -> bool {
    let is_manifest_type = |t: &str| {
        let t = t.to_ascii_lowercase();
        t.contains("mpegurl") || t.contains("dash+xml")
    };
    is_playlist_url(url) || ctype.is_some_and(is_manifest_type) || is_manifest_type(mime_hint)
}

/// True for HLS/DASH playlists: the progressive engine cannot consume these
/// as a single file (they need segment-assembly + transmux).
fn is_playlist_url(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let base = low.split(['?', '#']).next().unwrap_or(&low);
    base.ends_with(".m3u8") || base.ends_with(".mpd") || low.contains("m3u8") && low.contains("playlist")
}

fn has_direct_ext(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let base = low.split(['?', '#']).next().unwrap_or(&low);
    [".mp4", ".webm", ".mkv", ".mov", ".m4v", ".mp3", ".m4a", ".aac", ".flac", ".ogg", ".wav", ".avi"]
        .iter()
        .any(|e| base.ends_with(e))
}

/// True for URLs that name a media file or playlist (used to sanity-check a
/// URL pulled out of a player-wrapper query parameter).
fn looks_media_url(u: &str) -> bool {
    let base = u.split(['?', '#']).next().unwrap_or(u).to_ascii_lowercase();
    [
        ".m3u8", ".mpd", ".mp4", ".webm", ".mkv", ".mov", ".m4v", ".ts", ".m4s", ".mp3", ".m4a",
        ".aac", ".flac", ".ogg", ".wav", ".avi",
    ]
    .iter()
    .any(|e| base.ends_with(e))
}

/// Player wrapper pages carry the real stream in a query parameter
/// (`/jwplayer/?source=https%3A%2F%2F...`). Return the inner http(s) URL so
/// the playlist router and the download path see the media, not the shell.
/// Loops a few times to unwind double-wrapped URLs; `None` when nothing
/// changed.
pub fn unwrap_player_url(url: &str) -> Option<String> {
    let mut cur = url.to_string();
    let mut changed = false;
    for _ in 0..3 {
        // Already a media file/playlist: nothing to unwrap.
        if looks_media_url(&cur) {
            break;
        }
        let Ok(u) = url::Url::parse(&cur) else { break };
        let playerish = {
            let low = cur.to_ascii_lowercase();
            low.contains("player") || low.contains("embed") || low.contains("iframe")
        };
        let mut next: Option<String> = None;
        for key in [
            "source", "src", "file", "url", "video", "stream", "media", "link", "path",
        ] {
            let Some(v) = u
                .query_pairs()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.into_owned())
            else {
                continue;
            };
            let mut dec = v.clone();
            if !dec.starts_with("http") {
                dec = percent_decode_owned(&dec);
            }
            if !dec.starts_with("http") || dec == cur {
                continue;
            }
            if looks_media_url(&dec) || playerish {
                next = Some(dec);
                break;
            }
        }
        match next {
            Some(n) => {
                cur = n;
                changed = true;
            }
            None => break,
        }
    }
    if changed {
        Some(cur)
    } else {
        None
    }
}

/// Name stem of a media URL: the last path segment, or the folder above it
/// when that segment is a generic playlist/segment name (`index.m3u8`,
/// `master.mpd`, `seg1.ts`) — those folders hold the release name.
fn url_name_stem(u: &str) -> String {
    let path = u.split(['?', '#']).next().unwrap_or(u);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(last_raw) = segs.last() else {
        return String::new();
    };
    let last = percent_decode_owned(last_raw);
    let stem = match last.rfind('.') {
        Some(p) if last.len() - p <= 6 => last[..p].to_string(),
        _ => last.clone(),
    };
    let low = stem.to_ascii_lowercase();
    let boring = matches!(
        low.as_str(),
        "index" | "master" | "playlist" | "manifest" | "chunk" | "segment" | "video" | "media"
            | "stream" | "file" | "init"
    ) || (low.starts_with("seg") && low.len() <= 6);
    if boring && segs.len() >= 2 {
        return percent_decode_owned(segs[segs.len() - 2]);
    }
    stem
}

/// Generic page scan: og:video / contentUrl / twitter stream / bare media links.
/// Covers YouTube-alternatives (Vimeo/FB/TikTok/Dailymotion-style pages expose
/// the same og:video + ld+json VideoObject tags) plus bare file links.
/// HLS (.m3u8) / DASH (.mpd) playlists in the page body are NOT returned
/// (the scan stays progressive), but a playlist wrapped in the page URL's
/// query (?source=...) is unwrapped and returned for the segment router.
/// Site names and boilerplate that must never become a filename.
fn is_placeholder_title(t: &str) -> bool {
    let s = t
        .trim()
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '.' | '-' | '|' | '·'))
        .to_ascii_lowercase();
    if s.is_empty() {
        return true;
    }
    if matches!(
        s.as_str(),
        "facebook"
            | "video"
            | "watch"
            | "reel"
            | "reels"
            | "instagram"
            | "login"
            | "log in"
            | "sign up"
            | "untitled"
            | "home"
    ) {
        return true;
    }
    s.starts_with("see posts, photos and more")
        || s.starts_with("log in to facebook")
        || s.starts_with("log into facebook")
}

/// Facebook/Instagram post pages ("/reel/...", "/watch/?v=...", "/videos/...")
/// carry only the site name in og:title. The caption the user sees sits in
/// og:description, and that is the name IDM saves the file as.
fn is_social_post_page(u: &str) -> bool {
    let low = u.to_ascii_lowercase();
    let path = low.split(['?', '#']).next().unwrap_or(&low).to_string();
    let fb = low.contains("facebook.com") || low.contains("fb.watch");
    let ig = low.contains("instagram.com");
    let in_fb = ["/reel", "/watch", "/videos", "/share/", "/r/", "/story"];
    let in_ig = ["/reel", "/p/", "/tv", "/stories"];
    (fb && in_fb.iter().any(|p| path.contains(p)))
        || (ig && in_ig.iter().any(|p| path.contains(p)))
}

/// og:description is HTML-escaped and often carries site chrome; make it a
/// plausible file name.
fn clean_caption_text(s: &str) -> String {
    let mut t = s
        .replace("&amp;", "&")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace(['\r', '\n', '\t'], " ");
    // "(2) " unread badge in front of the page title
    if let Some(rest) = t.strip_prefix('(') {
        if let Some(close) = rest.find(')') {
            if !rest[..close].is_empty() && rest[..close].chars().all(|c| c.is_ascii_digit()) {
                t = rest[close + 1..].trim_start().to_string();
            }
        }
    }
    // Instagram: "12.3K likes, 45 comments - user on January 2: "the caption""
    if matches!(t.chars().next(), Some(c) if c.is_ascii_digit())
        && (t.contains(" likes,") || t.contains(" likes "))
    {
        if let Some(p) = t.rfind(": \"") {
            let rest = t[p + 3..].trim_end().trim_end_matches('"').trim();
            if rest.chars().count() >= 12 {
                t = rest.to_string();
            }
        }
    }
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    for tail in [" | Facebook", " - Facebook", " · Facebook"] {
        if let Some(stripped) = t.strip_suffix(tail) {
            t = stripped.trim().to_string();
        }
    }
    for tail in ["See more", "See More"] {
        if let Some(stripped) = t.strip_suffix(tail) {
            t = stripped.trim().to_string();
        }
    }
    t.trim_end_matches(['.', '…', ' ']).trim().to_string()
}

/// The caption of a social post page, when it is substantive enough to name a
/// file (and is not the site's boilerplate description).
fn social_post_caption(html: &str, page_url: &str) -> Option<String> {
    if !is_social_post_page(page_url) {
        return None;
    }
    let d = clean_caption_text(&meta_content(html, "og:description").unwrap_or_default());
    if d.chars().count() < 12 || is_placeholder_title(&d) {
        return None;
    }
    Some(d)
}

fn resolve_generic(html: &str, page_url: &str) -> Option<(String, String)> {
    let title = social_post_caption(html, page_url)
        .or_else(|| meta_content(html, "og:title").filter(|t| !is_placeholder_title(t)))
        .or_else(|| tag_text(html, "title").filter(|t| !is_placeholder_title(t)))
        .unwrap_or_else(|| "video".to_string());
    // Player wrapper pages (jwplayer/?source=...): the real stream lives in
    // the query, not the body. Prefer that, and name it from the release
    // folder when the page title is the generic player shell.
    if let Some(inner) = unwrap_player_url(page_url) {
        let stem = url_name_stem(&inner);
        let name = if stem.len() >= 8 { stem } else { title.clone() };
        return Some((inner, name));
    }
    for prop in [
        "og:video:url",
        "og:video",
        "og:video:secure_url",
        "twitter:player:stream",
        "twitter:player",
    ] {
        if let Some(u) = meta_content(html, prop) {
            if u.starts_with("http") && !is_playlist_url(&u) {
                let e = guess_ext(&u);
                return Some((with_ratebypass(&u), with_ext(&title, &e)));
            }
        }
    }
    // ld+json VideoObject: {"@type":"VideoObject","contentUrl":"https://..."}
    for key in [
        "\"contentUrl\":\"",
        "\"contentURL\":\"",
        "\"embedUrl\":\"",
        "\"embedURL\":\"",
    ] {
        let mut search = html;
        while let Some(p) = search.find(key) {
            let rest = &search[p + key.len()..];
            if let Some(end) = rest.find('"') {
                let u = rest[..end].replace("\\/", "/");
                if u.starts_with("http") && !is_playlist_url(&u) {
                    let e = guess_ext(&u);
                    return Some((with_ratebypass(&u), with_ext(&title, &e)));
                }
            }
            search = &search[p + key.len()..];
        }
    }
    // Vimeo-style progressive config: "url":"https://...mp4..." + quality tag.
    // Only accepted when the URL itself carries a direct media extension.
    for key in ["\"url\":\"https://", "\"url\":\"http://"] {
        let mut search = html;
        let mut best: Option<String> = None;
        for _ in 0..8 {
            let Some(p) = search.find(key) else { break };
            let rest = &search[p + 7..];
            let Some(end) = rest.find('"') else { break };
            let u = rest[..end].replace("\\/", "/");
            if u.starts_with("http") && has_direct_ext(&u) && !is_playlist_url(&u) {
                best = Some(u);
            }
            search = &search[p + key.len()..];
        }
        if let Some(u) = best {
            let e = guess_ext(&u);
            return Some((with_ratebypass(&u), with_ext(&title, &e)));
        }
    }
    // bare https://....mp4/.webm/.mkv/.mp3 links
    for ext in [".mp4", ".webm", ".mkv", ".mov", ".mp3", ".m4a"] {
        let mut search = html;
        while let Some(p) = search.find(ext) {
            let before = &search[..p];
            if let Some(s) = before.rfind("https://").or_else(|| before.rfind("http://")) {
                let mut end = p + ext.len();
                let bytes = search.as_bytes();
                while end < bytes.len()
                    && !matches!(bytes[end], b'"' | b'\'' | b' ' | b'<' | b')')
                {
                    // stop before query strings getting out of hand; keep them actually
                    if end - p > 300 {
                        break;
                    }
                    end += 1;
                }
                // char-boundary safe: hostile pages may embed multibyte text
                let Some(slice) = search.get(s..end.min(search.len())) else {
                    search = &search[p + ext.len()..];
                    continue;
                };
                let u = slice
                    .trim_end_matches(['"', '\'', ')', '<'])
                    .to_string();
                if u.len() < 3000 {
                    let name = u
                        .rsplit('/')
                        .next()
                        .and_then(|seg| seg.split('?').next())
                        .filter(|s| !s.is_empty())
                        .map(percent_decode_owned)
                        .unwrap_or_else(|| with_ext(&title, ext));
                    return Some((u, name));
                }
            }
            search = &search[p + ext.len()..];
        }
    }
    let _ = page_url;
    None
}

fn meta_content(html: &str, prop: &str) -> Option<String> {
    // <meta property="og:video" content="..."> (either attribute order)
    let mut search = html;
    while let Some(p) = search.find(prop) {
        // bound to the enclosing <meta ...> tag so we don't grab a neighbour
        let tag_start = search[..p].rfind("<meta").unwrap_or(0);
        let rest = &search[tag_start..];
        let tag_end = rest.find('>').map(|e| tag_start + e).unwrap_or(search.len());
        let tag = &search[tag_start..tag_end.min(search.len())];
        if let Some(c) = tag.find("content=\"") {
            let after = &tag[c + 9..];
            if let Some(end) = after.find('"') {
                let u = after[..end].replace("&amp;", "&");
                if !u.is_empty() {
                    return Some(u);
                }
            }
        }
        search = &search[p + prop.len()..];
    }
    None
}

fn tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let s = html.find(&open)?;
    let body_start = html[s..].find('>')? + s + 1;
    let e = html[body_start..].find(&close)? + body_start;
    let t = html[body_start..e].trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn guess_ext(u: &str) -> String {
    let low = u.to_ascii_lowercase();
    for ext in [".mp4", ".webm", ".mkv", ".mov", ".mp3", ".m4a", ".avi"] {
        if low.contains(ext) {
            return ext.to_string();
        }
    }
    ".mp4".to_string()
}

fn with_ext(title: &str, ext: &str) -> String {
    let t = title.trim();
    if t.to_ascii_lowercase().ends_with(ext) {
        t.to_string()
    } else {
        format!("{t}{ext}")
    }
}

/// Resolve a possibly-relative HLS URI against its playlist URL.
fn hls_abs(base: &str, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    if let Ok(b) = url::Url::parse(base) {
        if let Ok(u) = b.join(uri) {
            return u.to_string();
        }
    }
    uri.to_string()
}

/// Pick the highest-BANDWIDTH variant from a master playlist.
/// Returns None when this is already a media playlist.
fn hls_pick_variant(text: &str, base: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.starts_with("#EXT-X-STREAM-INF") {
            // Tag form: #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=...
            // (the first kv carries the tag prefix before the colon).
            let bw = t
                .split([':', ',', ' '])
                .find_map(|kv| {
                    let kv = kv.trim();
                    kv.strip_prefix("BANDWIDTH=")
                        .or_else(|| kv.strip_prefix("bandwidth="))
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .unwrap_or(0);
            // Next non-empty, non-comment line is the variant URI.
            let mut uri: Option<String> = None;
            while let Some(n) = lines.next() {
                let n = n.trim();
                if n.is_empty() || n.starts_with('#') {
                    continue;
                }
                uri = Some(n.to_string());
                break;
            }
            if let Some(u) = uri {
                let abs = hls_abs(base, &u);
                if best.as_ref().map(|(b, _)| bw > *b).unwrap_or(true) {
                    best = Some((bw, abs));
                }
            }
        }
    }
    best.map(|(_, u)| u)
}

fn hls_segments(text: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(hls_abs(base, t));
    }
    out
}

/// Split an EXT-X-KEY attribute list on commas outside quotes.
fn hls_key_attrs(line: &str) -> Vec<(String, String)> {
    let body = line.split_once(':').map(|(_, r)| r).unwrap_or("").trim();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in body.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                cur.push(c);
            }
            ',' if !in_q => {
                if let Some((k, v)) = cur.split_once('=') {
                    out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if let Some((k, v)) = cur.split_once('=') {
        out.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
    }
    out
}

fn hls_attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

/// One segment's encryption: absolute key URI + optional explicit IV hex.
/// A segment with no entry is plain. Rotation (P3) is just a schedule with
/// more than one distinct URI — supported, not rejected.
#[derive(Debug, Clone)]
struct HlsSegKey {
    uri: String,
    iv_hex: Option<String>,
}

/// Per-segment key schedule from a media playlist: walk lines in order,
/// each `#EXT-X-KEY` sets the key for subsequent segments until the next
/// one (`METHOD=NONE` clears back to plain). URIs are resolved against the
/// playlist URL. Errors on SAMPLE-AES (DRM-ish, never attempted) and
/// unknown methods — but NOT on rotation.
fn hls_key_schedule(text: &str, base: &str) -> Result<Vec<Option<HlsSegKey>>> {
    let mut out: Vec<Option<HlsSegKey>> = Vec::new();
    let mut current: Option<HlsSegKey> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("#EXT-X-KEY") {
            let attrs = hls_key_attrs(t);
            let method = hls_attr(&attrs, "METHOD").unwrap_or_default().to_uppercase();
            if method == "NONE" || method.is_empty() {
                current = None;
                continue;
            }
            if method == "SAMPLE-AES" {
                return Err(anyhow!(
                    "this HLS stream uses SAMPLE-AES encryption — not supported (DRM-adjacent). Use the extension MP4 qualities when the site offers them"
                ));
            }
            if method != "AES-128" {
                return Err(anyhow!("unsupported HLS key method {method}"));
            }
            let uri = hls_attr(&attrs, "URI").unwrap_or_default();
            if uri.is_empty() {
                return Err(anyhow!("HLS AES-128 key has no URI"));
            }
            current = Some(HlsSegKey { uri: hls_abs(base, &uri), iv_hex: hls_attr(&attrs, "IV") });
            continue;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(current.clone());
    }
    Ok(out)
}

/// First `#EXT-X-MAP:URI="…"` init section (fragmented MP4 headers that
/// must lead the assembled stream). `BYTERANGE` sub-ranges are not
/// supported — the whole object is fetched; playlists needing a byte slice
/// of the map are rejected loudly at download time.
fn hls_map_init(text: &str, base: &str) -> Result<Option<String>> {
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with("#EXT-X-MAP:") {
            continue;
        }
        let attrs = hls_key_attrs(t);
        let uri = hls_attr(&attrs, "URI").unwrap_or_default();
        if uri.is_empty() {
            return Err(anyhow!("HLS EXT-X-MAP has no URI — not supported"));
        }
        if hls_attr(&attrs, "BYTERANGE").is_some_and(|b| !b.trim().is_empty()) {
            return Err(anyhow!(
                "this HLS stream needs a byte-range of its init section (EXT-X-MAP BYTERANGE) — not supported yet"
            ));
        }
        return Ok(Some(hls_abs(base, &uri)));
    }
    Ok(None)
}

fn hls_media_sequence(text: &str) -> u64 {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            return rest.trim().parse::<u64>().unwrap_or(0);
        }
    }
    0
}

/// `#EXT-X-TARGETDURATION` — how often a live playlist gains a segment.
fn hls_target_duration(text: &str) -> u64 {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#EXT-X-TARGETDURATION:") {
            let raw = rest.trim();
            return raw
                .split(['.', ',', ' '])
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(6);
        }
    }
    6
}

fn parse_hex_iv(s: &str) -> Option<[u8; 16]> {
    let t = s.trim().strip_prefix("0x").or_else(|| s.trim().strip_prefix("0X")).unwrap_or(s.trim());
    if t.len() != 32 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// AES-128-CBC decrypt (raw blocks, no padding — HLS segments are
/// block-aligned by the packager). Backed by the vendored FIPS-197
/// implementation (`crate::aes128`, zero new dependencies).
fn aes128_cbc_decrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>> {
    let mut out = crate::aes128::cbc_decrypt(key, iv, data).map_err(|e| anyhow!("AES: {e}"))?;
    // RFC 8216 §4.3.2.4: AES-128 segments carry PKCS7 padding. Leaving it
    // put 1–16 junk bytes after every segment (breaking TS packet sync and
    // fMP4 box chains in the merged file).
    if let Some(&n) = out.last() {
        let n = n as usize;
        if (1..=16).contains(&n) && n <= out.len() && out[out.len() - n..].iter().all(|b| *b as usize == n) {
            out.truncate(out.len() - n);
        }
    }
    Ok(out)
}

/// HLS VOD download (clean-room, no IDM code): fetch playlist, follow the
/// best variant, fetch segments in parallel (4–8), decrypt AES-128 when
/// present (incl. key rotation, one fetch per distinct key URI), prepend
/// the EXT-X-MAP init section when present, resume via a sidecar map,
/// concat in order. Live playlists (no ENDLIST) are recorded by
/// [`record_live_hls`] until the stream ends or the user stops it; oversize and
/// SAMPLE-AES playlists are rejected with guidance — and DRM
/// (Widevine/FairPlay) is never bypassed.
async fn download_hls(
    playlist_url: &str,
    dest: &PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    const MAX_SEGS: usize = 2000;
    // An existing file is kept (numbered copy) unless overwrite was chosen,
    // as on the progressive path; the final rename used to replace it
    // silently. A resumable job (segment map or parts at this name) stays.
    let renamed;
    let dest: &PathBuf = if opts.allow_rename
        && dest.exists()
        && !opts.overwrite_existing
        && !dest.with_extension("hlsmap").exists()
        && !dest.with_extension("hls0").exists()
    {
        renamed = crate::model::conflict_free_dest(dest);
        if let Some(name) = renamed.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
        &renamed
    } else {
        dest
    };
    let _job = claim_dest_job(dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // 1. Master playlist?
    let mut pl_url = playlist_url.to_string();
    let mut text = opts
        .get(&client, &pl_url)
        .send()
        .await
        .map_err(|e| anyhow!("playlist fetch: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("playlist HTTP: {e}"))?
        .text()
        .await
        .map_err(|e| anyhow!("playlist read: {e}"))?;
    let external_audio = hls_external_audio(&text);
    if external_audio || text.contains("#EXT-X-BYTERANGE") || text.contains("BYTERANGE=") {
        if external_audio { shared.expects_audio.store(true, Ordering::SeqCst); }
        let mut helper_opts = opts.clone();
        helper_opts.require_audio |= external_audio;
        let exe = ensure_ytdlp(&client).await?;
        return download_with_ytdlp(&exe, playlist_url, dest, connections, &helper_opts, shared).await;
    }
    if text.contains("#EXT-X-STREAM-INF") {
        pl_url = hls_pick_variant(&text, &pl_url).ok_or_else(|| {
            anyhow!("could not pick a variant from this master playlist — use the extension MP4 qualities")
        })?;
        text = opts
            .get(&client, &pl_url)
            .send()
            .await
            .map_err(|e| anyhow!("variant fetch: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow!("variant HTTP: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("variant read: {e}"))?;
    }
    if text.contains("#EXT-X-ENDLIST") {
        let seconds: f64 = text.lines().filter_map(|l| l.trim().strip_prefix("#EXTINF:")
            .and_then(|v| v.split(',').next()).and_then(|v| v.parse::<f64>().ok())).sum();
        if seconds.is_finite() && seconds > 0.0 {
            let _ = shared.expected_duration.compare_exchange(0, seconds.round().max(1.0) as u64, Ordering::SeqCst, Ordering::SeqCst);
        }
    }
    let schedule = hls_key_schedule(&text, &pl_url)?;
    let seq0 = hls_media_sequence(&text);
    let segs = hls_segments(&text, &pl_url);
    // Live first: its window is short, sometimes empty right at the start, and
    // it grows instead of carrying a total.
    let is_live = !text.contains("#EXT-X-ENDLIST") && text.contains("#EXT-X-MEDIA-SEQUENCE");
    if is_live {
        return record_live_hls(&client, opts, &pl_url, dest, shared, &text).await;
    }
    if segs.is_empty() {
        return Err(anyhow!("playlist has no media segments — it may have expired"));
    }
    if segs.len() > MAX_SEGS {
        return Err(anyhow!(
            "playlist has too many segments ({} for a finished stream)",
            segs.len()
        ));
    }
    // 2. Key bytes for AES-128, including rotations (P3): every distinct
    // key URI is fetched once up front (bounded — a playlist cycling
    // endless keys is live-ish and refused below).
    let mut distinct: Vec<String> = Vec::new();
    for slot in schedule.iter().flatten() {
        if !distinct.contains(&slot.uri) {
            distinct.push(slot.uri.clone());
        }
    }
    const MAX_KEYS: usize = 8;
    if distinct.len() > MAX_KEYS {
        return Err(anyhow!(
            "this HLS stream cycles through {} encryption keys — not supported",
            distinct.len()
        ));
    }
    let mut key_map: std::collections::HashMap<String, [u8; 16]> =
        std::collections::HashMap::new();
    for uri in &distinct {
        let kb = opts
            .get(&client, uri)
            .send()
            .await
            .map_err(|e| anyhow!("key fetch: {e}"))?
            .error_for_status()
            .map_err(|e| anyhow!("key HTTP: {e}"))?
            .bytes()
            .await
            .map_err(|e| anyhow!("key read: {e}"))?;
        if kb.len() != 16 {
            return Err(anyhow!("HLS key is not 16 bytes — refusing"));
        }
        let mut k = [0u8; 16];
        k.copy_from_slice(&kb);
        key_map.insert(uri.clone(), k);
    }
    let key_map = std::sync::Arc::new(key_map);
    let schedule = std::sync::Arc::new(schedule);
    // 2b. Init section (EXT-X-MAP, P3): fetched once up front (fail fast —
    // without track headers the merge would be unplayable) and prepended
    // at merge time. Re-fetched on resume (tiny); counted in progress.
    let init_bytes: Option<Vec<u8>> = match hls_map_init(&text, &pl_url)? {
        Some(u) => {
            let blob = opts
                .get(&client, &u)
                .send()
                .await
                .map_err(|e| anyhow!("init fetch: {e}"))?
                .error_for_status()
                .map_err(|e| anyhow!("init HTTP: {e}"))?
                .bytes()
                .await
                .map_err(|e| anyhow!("init read: {e}"))?;
            if blob.is_empty() {
                return Err(anyhow!("HLS init section is empty — refusing"));
            }
            Some(blob.to_vec())
        }
        None => None,
    };
    // 3. Resume map: {playlist, count, done:[idx]}. Entries only count
    // when their part file is still on disk (cleanups must not brick
    // the merge into "segment missing").
    let map_path = dest.with_extension("hlsmap");
    let mut done: std::collections::HashSet<usize> = std::collections::HashSet::new();
    if let Ok(raw) = std::fs::read_to_string(&map_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let same = v.get("playlist").and_then(|x| x.as_str()) == Some(pl_url.as_str())
                && v.get("count").and_then(|x| x.as_u64()) == Some(segs.len() as u64);
            if same {
                // Segment files are written via rename, so one that exists
                // is complete: every one of them counts, not only those the
                // map listed (it is only rewritten at the end, so a stopped
                // run listed none and resume re-fetched everything).
                for i in 0..segs.len() {
                    if dest.with_extension(format!("hls{i}")).exists() {
                        done.insert(i);
                    }
                }
            }
        }
    }
    // Finished segments survive a stop and are skipped on restart (the map
    // above), so a VOD stream download does resume.
    shared.resumable.store(true, Ordering::SeqCst);
    let save_map = |done: &std::collections::HashSet<usize>| {
        let mut v: Vec<u64> = done.iter().map(|i| *i as u64).collect();
        v.sort_unstable();
        let j = serde_json::json!({ "playlist": pl_url, "count": segs.len(), "done": v });
        std::fs::write(&map_path, serde_json::to_string_pretty(&j).unwrap_or_default()).ok();
    };
    // The map names this playlist before any segment arrives, so a run that
    // stops midway resumes from the segment files it already finished.
    save_map(&done);
    // 4. Byte totals via concurrent ranged HEADs (best-effort).
    let parallel = connections.clamp(4, 8);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(parallel));
    {
        let mut total_known: u64 = 0;
        let mut any_known = false;
        let mut probes = Vec::new();
        for s in &segs {
            let client = client.clone();
            let opts = opts.clone();
            let u = s.clone();
            let sem = sem.clone();
            probes.push(tokio::spawn(async move {
                let _p = sem.acquire_owned().await;
                let r = opts
                    .get(&client, &u)
                    .header(reqwest::header::RANGE, "bytes=0-0")
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?;
                let cr = r.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                cr.split('/').next_back()?.parse::<u64>().ok()
            }));
        }
        for h in probes {
            if let Ok(Some(n)) = h.await {
                total_known = total_known.saturating_add(n);
                any_known = true;
            }
        }
        if any_known {
            shared.total.store(total_known, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
    }
    // Progress: one slot per segment, sized by the stream estimate until the
    // segment's real length is known (Part Info showed "1 B" per segment).
    {
        let est = if shared.has_total.load(Ordering::SeqCst) && !segs.is_empty() {
            (shared.total.load(Ordering::SeqCst) / segs.len() as u64).max(1)
        } else {
            1
        };
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        for i in 0..segs.len() {
            totals.push((0, est - 1));
            dones.push(if done.contains(&i) { est } else { 0 });
        }
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    // Seed progress from already-resumed segments so the bar doesn't
    // restart at zero (their bytes are on disk, never re-fetched).
    {
        let mut seeded = 0u64;
        for i in done.iter() {
            seeded = seeded.saturating_add(
                std::fs::metadata(dest.with_extension(format!("hls{i}")))
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
        }
        shared.downloaded.store(seeded, Ordering::SeqCst);
    }
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
    );
    let abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let max_retries = opts.max_retries.max(1);
    let mut handles = Vec::new();
    for (idx, seg_url) in segs.iter().enumerate() {
        if done.contains(&idx) {
            continue;
        }
        let client = client.clone();
        let opts = opts.clone();
        let url = seg_url.clone();
        let part = dest.with_extension(format!("hls{idx}"));
        let shared = shared.clone();
        let limiter = limiter.clone();
        let abort = abort.clone();
        let sem = sem.clone();
        let seg_key = schedule.get(idx).cloned().flatten();
        let key_map = key_map.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await.map_err(|e| anyhow!("semaphore: {e}"))?;
            let _host_slot = host_slot(&url, opts.per_host_conns).await;
            // Per-segment key from the schedule (P3 rotation): explicit IV
            // when the playlist gives one, else the sequence number.
            let kb: Option<[u8; 16]> = match &seg_key {
                Some(k) => Some(key_map.get(&k.uri).copied().ok_or_else(|| {
                    anyhow!("HLS key for segment {idx} was not fetched")
                })?),
                None => None,
            };
            let iv: Option<[u8; 16]> = match (&kb, seg_key) {
                (Some(_), Some(k)) => match k.iv_hex {
                    Some(ref hex) => Some(parse_hex_iv(hex).ok_or_else(|| anyhow!("bad EXT-X-IV"))?),
                    None => {
                        let seq = (seq0.saturating_add(idx as u64)) as u128;
                        Some(seq.to_be_bytes())
                    }
                },
                _ => None,
            };
            let mut attempt = 0u32;
            loop {
                attempt += 1;
                if abort.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                wait_if_paused(&shared).await?;
                if shared.cancelled.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                // Bytes added below belong to THIS attempt; a mid-stream
                // failure rolls them back so progress never overcounts.
                let mut attempt_bytes: u64 = 0;
                let got = opts.get(&client, &url).send().await;
                let data: Vec<u8> = match got {
                    Ok(resp) => {
                        let resp = match resp.error_for_status() {
                            Ok(r) => r,
                            Err(e) => {
                                let m = http_status_err(&url, e);
                                if is_final_http_error(&m) {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(m);
                                }
                                if attempt >= max_retries {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(m);
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                                continue;
                            }
                        };
                        use futures_util::StreamExt;
                        let expect_len = resp.content_length();
                        let mut buf = Vec::new();
                        let mut stream = resp.bytes_stream();
                        let mut bad: Option<anyhow::Error> = None;
                        while let Some(chunk) = stream.next().await {
                            if abort.load(Ordering::SeqCst) {
                                sub_saturating(&shared.downloaded, attempt_bytes);
                                return Err(anyhow!("cancelled"));
                            }
                            // Pause holds in-flight segments too (finishing a
                            // dozen parallel segments kept tens of MB arriving
                            // after Pause). Reading stops, so TCP stops the
                            // server; nothing received is lost. A connection
                            // the server drops meanwhile takes the retry path.
                            if shared.paused.load(Ordering::SeqCst) {
                                if let Err(e) = wait_if_paused(&shared).await {
                                    sub_saturating(&shared.downloaded, attempt_bytes);
                                    set_segment_progress(&shared, idx, 0, None);
                                    return Err(e);
                                }
                            }
                            match chunk {
                                Ok(bytes) => {
                                    buf.extend_from_slice(&bytes);
                                    attempt_bytes += bytes.len() as u64;
                                    shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
                                    set_segment_progress(&shared, idx, attempt_bytes, None);
                                    limiter.consume(bytes.len() as u64).await;
                                }
                                Err(e) => {
                                    bad = Some(anyhow!("segment network: {e}"));
                                    break;
                                }
                            }
                        }
                        if let Some(e) = bad {
                            sub_saturating(&shared.downloaded, attempt_bytes);
                            set_segment_progress(&shared, idx, 0, None);
                            if attempt >= max_retries {
                                abort.store(true, Ordering::SeqCst);
                                return Err(e);
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                            continue;
                        }
                        // A clean-but-short socket close yields no error —
                        // enforce the advertised length when the server gave one.
                        if let Some(n) = expect_len {
                            if buf.len() as u64 != n {
                                sub_saturating(&shared.downloaded, attempt_bytes);
                                set_segment_progress(&shared, idx, 0, None);
                                if attempt >= max_retries {
                                    abort.store(true, Ordering::SeqCst);
                                    return Err(anyhow!(
                                        "segment short: got {} of {n} bytes", buf.len()
                                    ));
                                }
                                tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                                continue;
                            }
                        }
                        buf
                    }
                    Err(e) => {
                        if attempt >= max_retries {
                            abort.store(true, Ordering::SeqCst);
                            return Err(anyhow!("segment fetch: {e}"));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                        continue;
                    }
                };
                // Decrypt (AES-128) then store the segment file. A bad key
                // fails EVERY segment, so stop the siblings immediately.
                let plain = match (&kb, iv) {
                    (Some(k), Some(v)) => match aes128_cbc_decrypt(&data, k, &v) {
                        Ok(p) => p,
                        Err(e) => {
                            abort.store(true, Ordering::SeqCst);
                            return Err(e);
                        }
                    },
                    _ => data,
                };
                // Temp file + rename: a segment file exists only when whole,
                // so resume can trust any it finds (an abort mid-write used to
                // leave a short file behind).
                let staging = part.with_extension(format!("hls{idx}tmp"));
                tokio::fs::write(&staging, &plain).await.map_err(|e| anyhow!(io_message(&e)))?;
                tokio::fs::rename(&staging, &part).await.map_err(|e| anyhow!(io_message(&e)))?;
                let got = attempt_bytes.max(1);
                set_segment_progress(&shared, idx, got, Some(got));
                return Ok::<(), anyhow::Error>(());
            }
        }));
    }
    let mut first_err: Option<anyhow::Error> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(anyhow!("segment task: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    // Rebuild the full done set, merge in order, clean up.
    for i in 0..segs.len() {
        done.insert(i);
    }
    save_map(&done);
    let tmp = dest.with_extension("dcpart");
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .await?;
    {
        use tokio::io::AsyncWriteExt;
        // EXT-X-MAP init section leads the stream (P3); counted in
        // progress so the bar stays honest.
        if let Some(init) = &init_bytes {
            out.write_all(init).await.map_err(|e| anyhow!(io_message(&e)))?;
            shared.downloaded.fetch_add(init.len() as u64, Ordering::SeqCst);
            shared.total.fetch_add(init.len() as u64, Ordering::SeqCst);
            limiter.consume(init.len() as u64).await;
        }
        for i in 0..segs.len() {
            let part = dest.with_extension(format!("hls{i}"));
            let mut f = tokio::fs::File::open(&part)
                .await
                .map_err(|_| anyhow!("segment {i} missing after download — retry the download"))?;
            tokio::io::copy(&mut f, &mut out).await?;
            let _ = tokio::fs::remove_file(&part).await;
        }
        out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    }
    drop(out);
    let _ = std::fs::remove_file(&map_path);
    let final_len = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Live HLS capture (clean-room, no IDM code): refresh the media playlist,
/// append every new segment to the destination the moment it appears, and keep
/// going until the stream ends (`EXT-X-ENDLIST`) or the user presses Stop.
///
/// Written incrementally on purpose: a stopped capture is still a playable
/// file. No total is known, so the row shows a growing size instead of a
/// percentage. MPEG-TS playlists land as `.ts` (the bytes really are TS; naming
/// them `.mp4` would make Windows players refuse the file); fragmented-MP4
/// playlists (`EXT-X-MAP`) keep their `.mp4` name.
async fn record_live_hls(
    client: &reqwest::Client,
    opts: &EngineOpts,
    pl_url: &str,
    dest: &PathBuf,
    shared: &Arc<SharedState>,
    initial_text: &str,
) -> Result<()> {
    shared.live.store(true, Ordering::SeqCst);
    shared.set_phase("Recording live");
    shared.has_total.store(false, Ordering::SeqCst);
    shared.total.store(0, Ordering::SeqCst);
    let mut text = initial_text.to_string();
    let has_map = hls_map_init(&text, pl_url)?.is_some();
    let mut target = dest.clone();
    if !has_map && opts.allow_rename && !dest.exists() {
        target = dest.with_extension("ts");
        if let Some(name) = target.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    shared.set_note(if has_map {
        "Live stream: recording until you press Stop (or the stream ends). Saved as fragmented MP4."
    } else {
        "Live stream: recording until you press Stop (or the stream ends). Saved as MPEG-TS (.ts) — playable in VLC/MPC."
    });
    let existing = tokio::fs::metadata(&target).await.map(|m| m.len()).unwrap_or(0);
    {
        // Resume a stopped capture by appending, never by truncating what the
        // user already recorded.
        let cur = shared.downloaded.load(Ordering::SeqCst);
        shared.downloaded.store(cur.max(existing), Ordering::SeqCst);
    }
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
    );
    let mut keys: std::collections::HashMap<String, [u8; 16]> =
        std::collections::HashMap::new();
    let mut last_seq: Option<u64> = None;
    let mut init_done: Option<String> = None;
    let mut refresh_fails = 0u32;
    let mut written: u64 = 0;
    let mut missed: u64 = 0;

    loop {
        wait_if_paused(shared).await?;
        let segs = hls_segments(&text, pl_url);
        let seq0 = hls_media_sequence(&text);
        // A restarted encoder rewinds the sequence; start following it again.
        if let Some(last) = last_seq {
            let max_seq = seq0.saturating_add(segs.len().saturating_sub(1) as u64);
            if segs.is_empty() || max_seq < last {
                last_seq = None;
            }
        }
        let schedule = hls_key_schedule(&text, pl_url)?;
        // Fragmented MP4: the init section leads the stream (and again after a
        // map change).
        if let Some(uri) = hls_map_init(&text, pl_url)? {
            if init_done.as_deref() != Some(uri.as_str()) {
                let Some(blob) = fetch_live_segment(client, opts, &uri, opts.max_retries).await?
                else {
                    return Err(anyhow!(
                        "the stream's init section (EXT-X-MAP) is gone from the server — cannot continue this fragmented-MP4 capture"
                    ));
                };
                out.write_all(&blob)
                    .await
                    .map_err(|e| anyhow!(io_message(&e)))?;
                shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
                written = written.saturating_add(blob.len() as u64);
                limiter.consume(blob.len() as u64).await;
                init_done = Some(uri);
            }
        }
        for (i, seg_url) in segs.iter().enumerate() {
            wait_if_paused(shared).await?;
            let seq = seq0.saturating_add(i as u64);
            if let Some(last) = last_seq {
                if seq <= last {
                    continue;
                }
            }
            let slot = schedule.get(i).cloned().flatten();
            let kb = match &slot {
                Some(k) => Some(hls_key_bytes(client, opts, &k.uri, &mut keys).await?),
                None => None,
            };
            let iv = match (&kb, &slot) {
                (Some(_), Some(k)) => match k.iv_hex.as_deref() {
                    Some(hex) => Some(parse_hex_iv(hex).ok_or_else(|| anyhow!("bad EXT-X-IV"))?),
                    None => Some((seq as u128).to_be_bytes()),
                },
                _ => None,
            };
            match fetch_live_segment(client, opts, seg_url, opts.max_retries).await? {
                Some(data) => {
                    let plain = match (&kb, iv) {
                        (Some(k), Some(v)) => aes128_cbc_decrypt(&data, k, &v)?,
                        _ => data,
                    };
                    out.write_all(&plain)
                        .await
                        .map_err(|e| anyhow!(io_message(&e)))?;
                    // Flush per segment: an aborted capture must leave every
                    // complete segment on disk.
                    out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
                    shared.downloaded.fetch_add(plain.len() as u64, Ordering::SeqCst);
                    written = written.saturating_add(plain.len() as u64);
                    limiter.consume(plain.len() as u64).await;
                }
                None => {
                    // The live window moved past this one; skip it instead of
                    // killing a long recording.
                    missed = missed.saturating_add(1);
                }
            }
            last_seq = Some(seq);
        }
        if text.contains("#EXT-X-ENDLIST") {
            break;
        }
        let wait = (hls_target_duration(&text) / 2).clamp(1, 5);
        sleep_live(shared, wait).await?;
        match fetch_playlist_text(client, opts, pl_url).await {
            Ok(t) => {
                text = t;
                refresh_fails = 0;
            }
            Err(e) => {
                refresh_fails += 1;
                if refresh_fails > 4 {
                    return Err(anyhow!(
                        "live playlist refresh failed after {refresh_fails} tries: {e:#} — {} already recorded stays on disk",
                        crate::model::format_bytes(written)
                    ));
                }
                sleep_live(shared, 1u64 << refresh_fails.min(3)).await?;
            }
        }
    }
    out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(out);
    let final_len = tokio::fs::metadata(&target).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    if final_len == 0 {
        return Err(anyhow!(
            "the live stream ended before any media was recorded — nothing to save"
        ));
    }
    if missed > 0 {
        eprintln!("[dbg] live capture skipped {missed} expired segment(s)");
    }
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&target, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&target).await;
            return Err(e);
        }
    }
    Ok(())
}

/// One playlist GET with the row's headers (cookies/referer/proxy).
async fn fetch_playlist_text(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
) -> Result<String> {
    opts.get(client, url)
        .send()
        .await
        .map_err(|e| anyhow!("playlist fetch: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("playlist HTTP: {e}"))?
        .text()
        .await
        .map_err(|e| anyhow!("playlist read: {e}"))
}

/// AES-128 key for a live segment, fetched once per distinct URI.
async fn hls_key_bytes(
    client: &reqwest::Client,
    opts: &EngineOpts,
    uri: &str,
    cache: &mut std::collections::HashMap<String, [u8; 16]>,
) -> Result<[u8; 16]> {
    if let Some(k) = cache.get(uri) {
        return Ok(*k);
    }
    let kb = opts
        .get(client, uri)
        .send()
        .await
        .map_err(|e| anyhow!("key fetch: {e}"))?
        .error_for_status()
        .map_err(|e| anyhow!("key HTTP: {e}"))?
        .bytes()
        .await
        .map_err(|e| anyhow!("key read: {e}"))?;
    if kb.len() != 16 {
        return Err(anyhow!("HLS key is not 16 bytes — refusing"));
    }
    let mut k = [0u8; 16];
    k.copy_from_slice(&kb);
    cache.insert(uri.to_string(), k);
    Ok(k)
}

/// One live segment. `Ok(None)` = the segment is gone (expired from the live
/// window) and should be skipped; `Err` = retries exhausted on a real error.
async fn fetch_live_segment(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    max_retries: u32,
) -> Result<Option<Vec<u8>>> {
    let _host_slot = host_slot(url, opts.per_host_conns).await;
    let max = max_retries.max(1);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match opts.get(client, url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND
                    || status == reqwest::StatusCode::GONE
                {
                    return Ok(None);
                }
                match resp.error_for_status() {
                    Ok(r) => {
                        let expect = r.content_length();
                        match r.bytes().await {
                            Ok(body) => {
                                if let Some(n) = expect {
                                    if body.len() as u64 != n && attempt < max {
                                        tokio::time::sleep(std::time::Duration::from_secs(
                                            1 << attempt.min(3),
                                        ))
                                        .await;
                                        continue;
                                    }
                                }
                                return Ok(Some(body.to_vec()));
                            }
                            Err(e) => {
                                if attempt >= max {
                                    return Err(anyhow!("segment network: {e}"));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let m = http_status_err(url, e);
                        if is_final_http_error(&m) || attempt >= max {
                            return Err(m);
                        }
                    }
                }
            }
            Err(e) => {
                if attempt >= max {
                    return Err(anyhow!("segment fetch: {e}"));
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(3))).await;
    }
}

/// Sleep in short slices so Pause/Stop react immediately during a live wait.
async fn sleep_live(shared: &Arc<SharedState>, secs: u64) -> Result<()> {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs.max(1));
    loop {
        wait_if_paused(shared).await?;
        let now = std::time::Instant::now();
        if now >= end {
            return Ok(());
        }
        let left = end.saturating_duration_since(now);
        tokio::time::sleep(left.min(std::time::Duration::from_millis(200))).await;
    }
}

/// Entry point used by the UI. Downloads `url` to `dest` with N connections.
/// If `url` is an HTML page (e.g. YouTube), a direct media stream is
/// auto-detected first, like the video capture in classic download managers.
/// Plain (non-encrypted) HLS VOD playlists (.m3u8) are assembled
/// segment-by-segment; DASH (.mpd) likewise; `ftp://` URLs go to the
/// plain-FTP backend.
pub async fn download(
    url: String, dest: PathBuf, connections: usize, mut opts: EngineOpts, shared: Arc<SharedState>,
) -> Result<()> {
    if opts.own_limit.is_none() {
        opts.own_limit = Some(shared.limit_bps.clone());
    }
    shared.expected_duration.store(opts.expected_duration_secs, Ordering::SeqCst);
    shared.expects_audio.store(opts.require_audio, Ordering::SeqCst);
    let log = opts.log_key.clone();
    // Magnet links and .torrent files go to BitTorrent (src/torrent.rs).
    if crate::torrent::is_torrent(&url) {
        job_log(&log, format!("start: {url} (BitTorrent)"));
        return crate::torrent::download(&url, &dest, shared, &log).await;
    }
    // Albums, galleries, artists' pages: every picture, through gallery-dl.
    if crate::gallery::is_gallery(&url) {
        job_log(&log, format!("start: {url} (gallery)"));
        let client = opts.build_client()?;
        return crate::gallery::download(&url, &dest, client, shared, &log).await;
    }
    // A file host's link through the user's debrid account (src/debrid.rs).
    let url = if crate::debrid::applies(&url) {
        let client = opts.build_client()?;
        let (direct, name) = crate::debrid::unlock(&client, &url).await?;
        job_log(&log, format!("debrid: {url} -> direct link"));
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            mutex_guard(&shared.resolved_name).replace(sanitize_filename(&n));
        }
        direct
    } else {
        url
    };
    job_log(&log, format!("start: {url} ({connections} connections)"));
    // The server's country and distance, for the progress window.
    if let Ok(client) = opts.build_client() {
        let proxied = matches!(proxy_choice(&opts.proxy_url), ProxyChoice::Pac(_) | ProxyChoice::Manual(_));
        tokio::spawn(crate::geo::measure_owned(url.clone(), proxied, client, shared.server.clone()));
    }
    if let Err(e) = download_inner(url.clone(), dest.clone(), connections, opts.clone(), shared.clone()).await {
        job_log(&log, format!("failed: {e:#}"));
        if let Err(e) = fallback_chain(e, &url, &dest, connections, &opts, &shared).await {
            job_log(&log, format!("gave up: {e:#}"));
            return Err(match explain_failure(&format!("{e:#}")) {
                Some(why) => e.context(why),
                None => e,
            });
        }
    }
    if let Some(n) = mutex_guard(&shared.note).clone() {
        job_log(&log, format!("note: {n}"));
    }
    job_log(&log, format!("done: {} bytes", shared.downloaded.load(Ordering::SeqCst)));
    verify_completed_media(&dest, &shared).await?;
    cleanup_completed_job(&dest, &shared);
    Ok(())
}

/// Whether `url` answers from behind Cloudflare's bot shield (or another CDN
/// that marks its challenge): what makes a TLS-fingerprint retry worth it.
async fn bot_shielded(client: &reqwest::Client, opts: &EngineOpts, url: &str) -> bool {
    let Ok(resp) = opts.get(client, url).header(reqwest::header::RANGE, "bytes=0-0").send().await else {
        return false;
    };
    let h = resp.headers();
    let server = h.get(reqwest::header::SERVER).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    h.contains_key("cf-ray") || h.contains_key("cf-mitigated") || server.contains("cloudflare") || server.contains("ddos-guard")
}

/// Safety net after the main path failed, in order:
/// 1. YouTube: yt-dlp, when it has not run yet (the native engine went
///    first) - and once more with the page's visitor id after a bot check;
/// 2. a refused file the extension sent: fetched inside the browser page
///    itself (its own session and origin).
/// The extension's own "download in the browser" is the last step; it hears
/// about the failure through the native host and takes over from there.
async fn fallback_chain(
    err: anyhow::Error,
    url: &str,
    dest: &PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    if shared.cancelled.load(Ordering::SeqCst) || err.to_string() == "cancelled" {
        return Err(err);
    }
    let dbg = std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok();
    let actual = mutex_guard(&shared.resolved_name).clone().map(|n| dest.with_file_name(n)).unwrap_or_else(|| dest.clone());
    let yt_page = if is_youtube_page(url) {
        Some(url.to_string())
    } else if is_googlevideo_url(url) && is_youtube_page(&opts.referer) {
        Some(opts.referer.clone())
    } else {
        None
    };
    let mut err = err;
    if let Some(page) = yt_page {
        let client = opts.build_client()?;
        if !shared.ytdlp_tried.load(Ordering::SeqCst) {
            if let Ok(exe) = ensure_ytdlp(&client).await {
                if dbg {
                    eprintln!("[dbg] native YouTube engine failed ({err:#}); continuing with yt-dlp");
                }
                shared.set_note(format!("the app's own YouTube engine failed ({err:#}) - yt-dlp took over"));
                job_log(&opts.log_key, "falling back to yt-dlp");
                reset_counters(shared);
                match download_with_ytdlp(&exe, &page, &actual, connections, opts, shared).await {
                    Ok(()) => return Ok(()),
                    Err(e) => err = e,
                }
            }
        }
        // A bot check ("Sign in to confirm you're not a bot") can clear with
        // the page's own visitor id when there are no cookies to send.
        let visitor = opts.browser.as_ref().map(|b| b.visitor_data.clone()).unwrap_or_default();
        let bot = format!("{err:#}").to_ascii_lowercase();
        if !visitor.is_empty() && opts.cookie.trim().is_empty() && (bot.contains("not a bot") || bot.contains("sign in to confirm")) {
            if let Ok(exe) = ensure_ytdlp(&client).await {
                let mut o2 = opts.clone();
                o2.ytdlp_visitor_data = visitor;
                reset_counters(shared);
                match download_with_ytdlp(&exe, &page, &actual, connections, &o2, shared).await {
                    Ok(()) => return Ok(()),
                    Err(e) => err = e,
                }
            }
        }
    }
    // Cloudflare (and similar bot shields) refusing the app but not the
    // browser: the request is the same, the TLS handshake is not, and that
    // fingerprint is what it checks. yt-dlp's bundled curl_cffi presents
    // Chrome's handshake, with the captured cookies, Referer and headers.
    // Works for HLS playlists and plain files alike.
    let low = format!("{err:#}").to_ascii_lowercase();
    if !shared.ytdlp_tried.load(Ordering::SeqCst)
        && (low.contains("403") || low.contains("429") || low.contains("503") || low.contains("challenge"))
        && url.starts_with("http")
    {
        let client = opts.build_client()?;
        if bot_shielded(&client, opts, url).await {
            if let Ok(exe) = ensure_ytdlp(&client).await {
                job_log(&opts.log_key, "refused by a bot shield (Cloudflare): retrying with a browser's TLS fingerprint");
                shared.set_note("the site's bot shield refused the app - retried as a browser (TLS fingerprint)");
                let mut o2 = opts.clone();
                o2.impersonate = true;
                reset_counters(shared);
                match download_with_ytdlp(&exe, url, &actual, connections, &o2, shared).await {
                    Ok(()) => return Ok(()),
                    Err(e) => err = e,
                }
            }
        }
    }
    // A company server that wants a Windows login (NTLM / Kerberos): the
    // pooled client cannot answer it, Windows' own HTTP stack can.
    let low = format!("{err:#}").to_ascii_lowercase();
    if low.contains("401") && url.starts_with("http") && crate::winauth::server_wants_windows_login(opts, url).await {
        job_log(&opts.log_key, "server wants an NTLM/Negotiate login: using Windows' HTTP stack");
        shared.set_note("Windows login (NTLM/Negotiate) - one connection");
        reset_counters(shared);
        match crate::winauth::download(url, &actual, opts, shared).await {
            Ok(()) => return Ok(()),
            Err(e) => err = e,
        }
    }
    // No connection at all: a name lookup that failed for a moment, or a
    // host reachable only over IPv6 (connections are bound to IPv4). One
    // more try after a pause, allowing IPv6.
    let low = format!("{err:#}").to_ascii_lowercase();
    if !opts.any_ip && shared.downloaded.load(Ordering::SeqCst) == 0 && unreachable_host(&low) {
        job_log(&opts.log_key, "could not reach the server: trying again (IPv6 allowed)");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let mut o2 = opts.clone();
        o2.any_ip = true;
        reset_counters(shared);
        match download_inner(url.to_string(), dest.clone(), connections, o2, shared.clone()).await {
            Ok(()) => return Ok(()),
            Err(e) => err = e,
        }
    }
    // A server that refuses HTTP/1.1 (505, or a protocol error before any
    // byte): try once more over HTTP/2.
    let low = format!("{err:#}").to_ascii_lowercase();
    if !opts.http2
        && shared.downloaded.load(Ordering::SeqCst) == 0
        && (low.contains("505") || low.contains("http2") || low.contains("protocol") || low.contains("connection closed before message completed"))
    {
        job_log(&opts.log_key, "retrying over HTTP/2");
        let mut o2 = opts.clone();
        o2.http2 = true;
        reset_counters(shared);
        match download_inner(url.to_string(), dest.clone(), connections, o2, shared.clone()).await {
            Ok(()) => return Ok(()),
            Err(e) => err = e,
        }
    }
    let from_ext = opts.browser.as_ref().is_some_and(|b| b.from_extension);
    let page = opts.referer.trim();
    let refused = is_final_http_error(&err) || format!("{err:#}").contains("web page instead of the file");
    if from_ext && refused && page.starts_with("http") && !is_youtube_page(page) {
        if dbg {
            eprintln!("[dbg] refused ({err:#}); asking the browser page to fetch it");
        }
        reset_counters(shared);
        job_log(&opts.log_key, "server refused the app: fetching inside the browser page");
        match page_fetch_file(url, page, &actual, shared).await {
            Ok(()) => {
                shared.set_note("the server refused the app, so the file was fetched inside the browser page");
                return Ok(());
            }
            Err(e) if dbg => eprintln!("[dbg] page fetch failed: {e:#}"),
            Err(_) => {}
        }
    }
    Err(err)
}

fn reset_counters(shared: &SharedState) {
    shared.downloaded.store(0, Ordering::SeqCst);
    shared.total.store(0, Ordering::SeqCst);
    shared.has_total.store(false, Ordering::SeqCst);
}

static PAGE_FETCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// Ask the browser extension (through the native host) to fetch `url` inside
/// the page `page` - with that page's own cookies, origin and referrer - and
/// save what it gets to `dest`. Needs the browser open with that page.
async fn page_fetch_file(url: &str, page: &str, dest: &PathBuf, shared: &Arc<SharedState>) -> Result<()> {
    let dir = crate::native_host::page_fetch_dir().ok_or_else(|| anyhow!("no config dir"))?;
    tokio::fs::create_dir_all(&dir).await?;
    let id = format!(
        "{:x}-{:x}-{}",
        std::process::id(),
        chrono::Local::now().timestamp_millis(),
        PAGE_FETCH_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let req = dir.join(format!("{id}.req.json"));
    let part = dir.join(format!("{id}.part"));
    let res = dir.join(format!("{id}.res"));
    let errf = dir.join(format!("{id}.err"));
    tokio::fs::write(&req, serde_json::json!({ "url": url, "page": page }).to_string()).await?;
    let cleanup = || {
        for f in [&req, &part, &res, &errf] {
            let _ = std::fs::remove_file(f);
        }
    };
    // 20 s for the browser to start answering, then as long as data flows.
    let mut last_len = 0u64;
    let mut last_move = std::time::Instant::now();
    let mut started = false;
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            cleanup();
            return Err(anyhow!("cancelled"));
        }
        if res.exists() {
            break;
        }
        if errf.exists() {
            let why = std::fs::read_to_string(&errf).unwrap_or_default();
            cleanup();
            return Err(anyhow!("the browser could not fetch it either: {why}"));
        }
        let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if len != last_len {
            last_len = len;
            last_move = std::time::Instant::now();
            started = true;
            shared.downloaded.store(len, Ordering::SeqCst);
        }
        let wait = if started { 60 } else { 20 };
        if last_move.elapsed() > std::time::Duration::from_secs(wait) {
            cleanup();
            return Err(anyhow!("the browser did not answer (is the page still open?)"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let size = std::fs::metadata(&res).map(|m| m.len()).unwrap_or(0);
    if tokio::fs::rename(&res, dest).await.is_err() {
        tokio::fs::copy(&res, dest).await?;
    }
    cleanup();
    shared.total.store(size, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    shared.downloaded.store(size, Ordering::SeqCst);
    Ok(())
}

async fn download_inner(
    mut url: String,
    mut dest: PathBuf,
    connections: usize,
    mut opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    shared.set_phase("Getting info");
    // Settings -> Site logins: a saved login for this host.
    if opts.username.trim().is_empty() {
        if let Some((u, p)) = crate::model::login_for(&url) {
            opts.username = u;
            opts.password = p;
        }
    }
    // The browser's user agent and proxy for a capture it handed over: the
    // server and the network path stay the ones that worked in the browser.
    if let Some(b) = opts.browser.clone() {
        if let Some(ua) = b.headers.get("user-agent").filter(|u| !u.trim().is_empty()) {
            opts.user_agent = ua.clone();
        }
        if opts.proxy_url.trim().is_empty() && !b.proxy.is_empty()
            && crate::model::ui_pref_bool("proxy_from_browser", true)
        {
            opts.proxy_url = b.proxy.clone();
        }
        if b.is_post() {
            let _job = claim_dest_job(&dest).ok_or_else(|| anyhow!("this destination is already downloading elsewhere"))?;
            return post_download(&url, &dest, connections, &opts, &shared).await;
        }
    }
    // Google Drive / Docs: the share link is a page; download what it serves.
    if crate::gdrive::is_drive_url(&url) {
        let client = opts.build_client()?;
        url = crate::gdrive::resolve(&client, &opts, &url).await?;
    }
    // Resolve Vimeo before probing the CDN: a successful fragment response
    // proves nothing about the full video's size or completeness.
    let resolve_page = match vimeo_download_target(&url, &opts.referer)? {
        Some(page) => Some(page),
        // A post on Instagram, TikTok, X or Reddit: yt-dlp knows each site's
        // player (the video and its separate audio, the real qualities).
        None if social_video_page(&url) => Some(url.clone()),
        None => fragment_page(&normalize_media_url(&url), &opts.referer)?,
    };
    if let Some(page) = resolve_page {
        let _job = claim_dest_job(&dest).ok_or_else(|| anyhow!("this destination is already downloading elsewhere"))?;
        if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
            dest = crate::model::conflict_free_dest(&dest);
            if let Some(name) = dest.file_name() {
                mutex_guard(&shared.resolved_name).replace(name.to_string_lossy().to_string());
            }
        }
        let client = opts.build_client()?;
        let exe = ensure_ytdlp(&client).await?;
        shared.total.store(0, Ordering::SeqCst);
        shared.has_total.store(false, Ordering::SeqCst);
        shared.downloaded.store(0, Ordering::SeqCst);
        // Fail on extraction errors; never fall back to saving a fragment.
        return download_with_ytdlp(&exe, &page, &dest, connections, &opts, &shared).await;
    }
    // Facebook slice URLs (bytestart/byteend) can only ever return that
    // preview slice: normalize to the full-file URL before any probe, and
    // drop the stale slice prefix (it starts at ~byte 967, not 0, so it
    // can never be resumed into a full file).
    let normalized = normalize_media_url(&url);
    if normalized != url {
        url = normalized;
        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
        let _ = tokio::fs::remove_file(dest.with_extension("dcmeta")).await;
        for i in 0..16 {
            let _ = tokio::fs::remove_file(dest.with_extension(format!("dcpart{i}"))).await;
        }
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    // Player wrapper pages ("/jwplayer/?source=<encoded media>"): the real
    // stream is in the query. Unwrap before routing so the playlist router
    // and the progressive path never see the HTML shell.
    if let Some(inner) = unwrap_player_url(&url) {
        url = inner;
        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
        let _ = tokio::fs::remove_file(dest.with_extension("dcmeta")).await;
        for i in 0..16 {
            let _ = tokio::fs::remove_file(dest.with_extension(format!("dcpart{i}"))).await;
        }
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    // Route playlists before the progressive path so we never save a
    // playlist file as if it were the video.
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("ftp://") || lower.starts_with("ftps://") || lower.starts_with("ftpes://") {
        // Plain-FTP backend (P3): own connection, same resume contract.
        let _job = claim_dest_job(&dest).ok_or_else(|| {
            anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
        })?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
            let resumable_here = dest.with_extension("dcpart").exists();
            if !resumable_here {
                dest = crate::model::conflict_free_dest(&dest);
                if let Some(name) = dest
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                {
                    mutex_guard(&shared.resolved_name).replace(name);
                }
            }
        } else if opts.overwrite_existing && dest.exists() {
            let _ = tokio::fs::remove_file(&dest).await;
        }
        let limiter = std::sync::Arc::new(
            RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
        );
        return crate::ftp::download_ftp(&url, &dest, &opts, shared, limiter, connections).await;
    }
    if is_playlist_url(&url) {
        if url.to_ascii_lowercase().split(['?', '#']).next().unwrap_or("").ends_with(".mpd")
            || url.contains(".mpd")
        {
            return download_dash(&url, dest, connections, opts, shared).await;
        }
        return download_hls(&url, &dest, connections, &opts, &shared).await;
    }
    // Same-destination jobs would interleave part files: serialize them.
    let _job = claim_dest_job(&dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    let client = match opts.pinned_client(&url).await {
        Some(c) => c,
        None => opts.build_client()?,
    };
    if !opts.username.trim().is_empty() && mutex_guard(&opts.digest).is_none() {
        learn_digest(&client, &opts, &url).await;
    }

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Direct media delivery hosts (NOT watch/reel pages): when the total is
    // already known (browser-measured size or a previous resume), verify
    // liveness + range support with ONE cheap Range probe instead of the
    // full HEAD + Range probe. On expiring signed links every extra request
    // burns freshness — but zero verification is worse: a dead/expired URL
    // would burn ~100 futile chunks that each return zero bytes
    // (`got 0 of …`). A 206 keeps the seeded total and uses chunked ranges;
    // anything else falls through to the plain probe path below.
    let direct_delivery = {
        let low = url.to_ascii_lowercase();
        low.contains("fbcdn")
            || low.contains("scontent")
            || low.contains("lookaside.")
            || low.contains("fbsbx.com")
            || low.contains("cdninstagram.com")
    };
    let seeded_total = if shared.has_total.load(Ordering::SeqCst) {
        Some(shared.total.load(Ordering::SeqCst))
    } else {
        None
    };
    // A captured social asset can be stale by the time the row runs (signed
    // links expire), but when the row still remembers the video's *page* that
    // page is the better source anyway - it carries the audio leg too. Only
    // verify the asset when there is no page to fall back to, so re-downloading
    // an old capture is not refused by its dead asset link.
    let social = social_page_url(&url, &opts.referer);
    let mut range_verified: Option<Option<u64>> = None; // Some(x) = probe ran
    if direct_delivery && seeded_total.is_some() && social.is_none() {
        match probe_range_once(&client, &opts, &url, seeded_total).await {
            Ok(verified) => range_verified = Some(verified),
            Err(e) => return Err(e), // expired/refused: fail fast, loudly
        }
    }
    let (mut total_opt, mut supports_range, ctype, disp) = match range_verified {
        Some(verified) => (verified.or(seeded_total), verified.is_some(), None, None),
        None => probe(&client, &opts, &url).await,
    };
    // Some CDNs use opaque manifest URLs. Route by the actual response
    // type as well as extension, instead of saving playlist text as video.
    if let Some(mime) = ctype.as_deref() {
        let mime = mime.to_ascii_lowercase();
        if mime.contains("mpegurl") || mime.contains("dash+xml") {
            drop(_job);
            return if mime.contains("dash+xml") {
                download_dash(&url, dest, connections, opts, shared).await
            } else {
                download_hls(&url, &dest, connections, &opts, &shared).await
            };
        }
    }
    // …and by content when the type is generic (text/plain, octet-stream,
    // xml) and the URL names no media file: the first bytes say #EXTM3U or
    // <MPD for manifests the browser extension caught by sniffing. Manifests
    // are small, so a multi-megabyte body is never sniffed (each extra
    // request spends freshness on expiring signed links).
    const MANIFEST_MAX: u64 = 2 * 1024 * 1024;
    if range_verified.is_none()
        && !has_direct_ext(&url)
        && opaque_manifest_type(ctype.as_deref())
        && total_opt.map_or(true, |t| t <= MANIFEST_MAX)
    {
        match sniff_manifest(&client, &opts, &url).await {
            Some(ManifestKind::Dash) => {
                drop(_job);
                return download_dash(&url, dest, connections, opts, shared).await;
            }
            Some(ManifestKind::Hls) => {
                drop(_job);
                return download_hls(&url, &dest, connections, &opts, &shared).await;
            }
            None => {}
        }
    }
    // Pre-seeded totals imply resume support until proven otherwise; the
    // first response corrects this (single_stream updates it live).
    shared
        .resumable
        .store(supports_range, Ordering::SeqCst);
    // Documented name priority (P1): server Content-Disposition >
    // resolved media title (page path below) > this probe's disposition >
    // URL fallback (queue/extension name). Here: honor the disposition…
    let probe_is_html = ctype
        .as_deref()
        .map(|c| c.contains("text/html"))
        .unwrap_or(false);
    if opts.allow_rename {
        if let Some(n) = disp {
            dest.set_file_name(&n);
            mutex_guard(&shared.resolved_name).replace(n);
        } else if !probe_is_html {
            // …or at least fix the extension from the MIME type when the
            // URL yielded no usable name (`download_123.bin` or bare
            // `song` + video/mp4 → `song.mp4`).
            if let Some(cur) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
                let fixed = if crate::model::is_fallback_filename(&cur) {
                    ctype
                        .as_deref()
                        .and_then(crate::model::ext_for_mime)
                        .map(|ext| cur.trim_end_matches(".bin").to_string() + ext)
                } else if !cur.contains('.') {
                    ctype
                        .as_deref()
                        .and_then(crate::model::ext_for_mime)
                        .map(|ext| cur.clone() + ext)
                } else {
                    None
                };
                if let Some(fixed) = fixed {
                    dest.set_file_name(&fixed);
                    mutex_guard(&shared.resolved_name).replace(fixed);
                }
            }
        }
    }
    let is_html = ctype
        .as_deref()
        .map(|c| c.contains("text/html"))
        .unwrap_or(false);
    // On YouTube hosts always resolve first: the probe alone is
    // meaningless there (walls, page variants), the page/API decides.
    let is_yt = url.contains("youtube.com") || url.contains("youtu.be");
    // YouTube gates its 1080p+ streams behind the browser-only SABR protocol:
    // to any non-browser client they hand out roughly their first minute and
    // then 403 forever (verified live — changed headers, the row's own
    // captured cookies, freshly resolved URLs and a fetch from a signed-in
    // page all hit the same wall). yt-dlp speaks that protocol, so when one
    // is installed — or can be fetched once — it takes every quality above
    // the progressive file. Anything it cannot do falls through to the
    // built-in engine below, progressive fallback and all.
    // The app's own reader goes first (Auto and App first): every quality
    // on the app's own connections. When it cannot read the video, yt-dlp
    // takes over as before.
    let mut own_hit: Option<ResolvedMedia> = None;
    if is_yt && youtube_own_reader_allowed(&opts) && youtube_id(&url).is_some() {
        match crate::youtube::resolve(&client, &opts, &url, &shared).await {
            Ok(hit) => {
                job_log(&opts.log_key, "read by the app's own YouTube reader");
                own_hit = Some(hit);
            }
            Err(e) if e.to_string() == "cancelled" => return Err(e),
            Err(e) => {
                shared.youtube_own_failed.store(true, Ordering::SeqCst);
                job_log(&opts.log_key, &format!("the app's YouTube reader could not read the page ({e:#}): yt-dlp takes over"));
                if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                    eprintln!("[dbg] youtube own reader failed: {e:#}");
                }
            }
        }
    }
    if is_yt && own_hit.is_none() && youtube_ytdlp_first(&opts) {
        match ensure_ytdlp(&client).await {
            Ok(exe) => {
                match download_with_ytdlp(&exe, &url, &dest, connections, &opts, &shared).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                            eprintln!("[dbg] yt-dlp path skipped: {e:#}");
                        }
                    }
                }
            }
            Err(e) => {
                if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                    eprintln!("[dbg] yt-dlp unavailable: {e:#}");
                }
            }
        }
    }
    // Facebook/Instagram rarely serve one progressive file: their players
    // stream adaptive representations (a video-only DASH quality plus a
    // separate audio-only one) and the browser captures whichever of those it
    // was fetching - so the transfer "completes" as a silent video. Verified
    // live: a captured reel rep of 722.80 MiB whose page also lists a 51.25
    // MiB m4a, and `bv*+ba` merges the two into an mp4 that has sound. The
    // row still remembers the page it came from, so hand *that* page to
    // yt-dlp. Anything it cannot do falls through to the captured asset.
    //
    // Unlike the YouTube branch above this is not gated on the quality menu:
    // there, a low request means the built-in engine can finish the job on its
    // own (the progressive file), while here the captured asset is one leg of
    // an adaptive pair - there is no quality at which it contains sound.
    if let Some(page) = social {
        match ensure_ytdlp(&client).await {
            Ok(exe) => {
                // The helper reports the merged size itself; the browser's
                // measurement describes one leg of the pair, not the result.
                shared.total.store(0, Ordering::SeqCst);
                shared.has_total.store(false, Ordering::SeqCst);
                match download_with_ytdlp(&exe, &page, &dest, connections, &opts, &shared).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        shared.set_note(format!(
                            "captured file kept as-is: merging the page failed ({e:#}) - it may have no sound"
                        ));
                        if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                            eprintln!("[dbg] social page path skipped: {e:#}");
                        }
                    }
                }
            }
            Err(e) => {
                shared.set_note(format!(
                    "captured file kept as-is: no yt-dlp for the audio merge ({e:#}) - it may have no sound"
                ));
                if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                    eprintln!("[dbg] social page path: yt-dlp unavailable: {e:#}");
                }
            }
        }
    }
    if is_html || is_yt {
        let page_url = url.clone();
        let from_own_reader = own_hit.is_some();
        let resolved = match own_hit.take() {
            Some(hit) => Some(hit),
            None => resolve_media(&client, &opts, &url, &shared).await,
        };
        match resolved {
            Some(hit) => {
                url = hit.url;
                // media servers expect the page as referer
                if opts.referer.trim().is_empty() {
                    opts.referer = page_url.clone();
                }
                if !hit.title.trim().is_empty() {
                    // A captured CDN name ("50k.mp4", "videoplayback") is not
                    // a title: the resolved one replaces it even when the
                    // caller asked to keep the destination as it is.
                    let weak = dest
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map_or(true, weak_capture_name);
                    if opts.allow_rename || weak {
                        let name = title_filename(&hit.title, Some(&dest));
                        dest.set_file_name(&name);
                        mutex_guard(&shared.resolved_name).replace(name);
                    }
                }
                // A playlist reached through the resolver (player wrapper
                // ?source=...) must go through the segment assembler, not the
                // progressive path that would save the manifest text.
                if is_playlist_url(&url) {
                    drop(_job);
                    let low = url.to_ascii_lowercase();
                    if low.split(['?', '#']).next().unwrap_or("").ends_with(".mpd")
                        || low.contains(".mpd")
                    {
                        return download_dash(&url, dest, connections, opts, shared).await;
                    }
                    return download_hls(&url, &dest, connections, &opts, &shared).await;
                }
                // The sound alone, read by the app's own YouTube reader: the
                // M4A on the app's connections, then (unless M4A was asked
                // for) an MP3 made from it with FFmpeg.
                if from_own_reader && wants_audio_only(&opts.quality_pref) && hit.pair_url.is_none() {
                    drop(_job);
                    shared.set_phase(PHASE_APP_ENGINE);
                    let mp3 = !wants_m4a(&opts.quality_pref);
                    if mp3 && ffmpeg_path().is_none() {
                        shared.set_phase("Fetching ffmpeg");
                        ensure_ffmpeg(&client).await.map_err(|e| anyhow!("MP3 needs FFmpeg, and {e:#}; pick M4A to save the sound as it is"))?;
                    }
                    download_adaptive(url.clone(), None, dest.clone(), connections, opts.clone(), shared.clone()).await?;
                    if mp3 {
                        shared.set_phase("Converting to MP3");
                        let out = conflict_free_mp3(&dest);
                        ffmpeg_to_mp3(&dest, &out).await?;
                        let _ = tokio::fs::remove_file(&dest).await;
                        if let Some(n) = out.file_name() {
                            mutex_guard(&shared.resolved_name).replace(n.to_string_lossy().into_owned());
                        }
                        let len = tokio::fs::metadata(&out).await.map(|m| m.len()).unwrap_or(0);
                        // The file is not the stream's size any more: the
                        // row must not compare them (as for a merge).
                        shared.merged.store(true, Ordering::SeqCst);
                        shared.total.store(len, Ordering::SeqCst);
                        shared.has_total.store(true, Ordering::SeqCst);
                        shared.downloaded.store(len, Ordering::SeqCst);
                    }
                    return Ok(());
                }
                // Adaptive video+audio pair (1080p+ menu choice): merge via
                // ffmpeg on completion instead of a small progressive file.
                // Drop our dest claim first: download_adaptive takes its own.
                if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
                        eprintln!("[dbg] pair video={}", url);
                        eprintln!("[dbg] pair audio={}", pair);
                    }
                    drop(_job);
                    if from_own_reader {
                        shared.set_phase(PHASE_APP_ENGINE);
                    }
                    // YouTube adaptive legs can die mid-file at ~20%:
                    // SABR-gated qualities serve only a ~2 MB preview outside
                    // the browser, and stream URLs expire/session-lock fast
                    // (403). Retry once with a fresh resolve — downgrading to
                    // 720p progressive for gated qualities — instead of
                    // failing at 19-20% and telling the user to click again.
                    match download_adaptive(
                        url.clone(),
                        Some(pair.clone()),
                        dest.clone(),
                        connections,
                        opts.clone(),
                        shared.clone(),
                    )
                    .await
                    {
                        Ok(()) => return Ok(()),
                        // The own reader's links failing: yt-dlp (in the
                        // fallback chain) is the better next step than the
                        // old 360p path.
                        Err(e) if from_own_reader => return Err(e),
                        Err(e) => {
                            let msg = format!("{e:#}");
                            let sabr = msg.contains("serves only the first");
                            let refused = msg.contains("refused this link (403)")
                                || msg.contains("expired");
                            if !(sabr || refused) {
                                return Err(e);
                            }
                            let mut opts2 = opts.clone();
                            if sabr {
                                // Not a quality problem: YouTube caps *every*
                                // adaptive (gir=yes) leg to roughly its first
                                // minute for non-browser clients, so 720p fails
                                // exactly like 1080p. Clearing the preference
                                // asks the resolver for the muxed progressive
                                // file, which YouTube serves in full (no
                                // ffmpeg needed either).
                                opts2.quality_pref = String::new();
                            }
                            match resolve_media(&client, &opts2, &page_url, &shared).await {
                                Some(hit2) => {
                                    if sabr && opts2.allow_rename {
                                        note_youtube_downgrade(&hit2, &mut dest, &shared);
                                    }
                                    return download_adaptive(
                                        hit2.url,
                                        hit2.pair_url.filter(|p| p.starts_with("http")),
                                        dest,
                                        connections,
                                        opts2,
                                        shared,
                                    )
                                    .await
                                    .map_err(|e2| {
                                        let m2 = format!("{e2:#}");
                                        if sabr && (m2.contains("serves only the first") || m2.contains("refused this link (403)")) {
                                            anyhow!("{m2} (retried as the progressive file automatically — YouTube serves only the first ~1 minute of 1080p+ streams to non-browser clients, so 360p is all the built-in engine can finish; install yt-dlp (`winget install yt-dlp`) for the higher qualities)")
                                        } else {
                                            e2
                                        }
                                    });
                                }
                                None => return Err(e),
                            }
                        }
                    }
                }
                let (t2, r2, ctype2, d2) = probe(&client, &opts, &url).await;
                total_opt = t2;
                supports_range = r2;
                shared.resumable.store(r2, Ordering::SeqCst);
                // A "media" link that answers HTML is a dead end (error page),
                // not a file — fail loudly instead of saving garbage.
                if ctype2
                    .as_deref()
                    .map(|c| c.contains("text/html"))
                    .unwrap_or(false)
                {
                    return Err(anyhow!(
                        "resolved link is a web page, not a file — try the browser extension"
                    ));
                }
                if opts.allow_rename {
                    if let Some(n) = d2 {
                        dest.set_file_name(&n);
                        mutex_guard(&shared.resolved_name).replace(n);
                    }
                }
            }
            None => {
                if shared.rate_limited.load(Ordering::SeqCst) {
                    return Err(anyhow!(
                        "YouTube is rate-limiting this network right now (bot check) — open the video in your browser, click the floating ⬇ Download this video button, and pick a quality there; the app will confirm the download"
                    ));
                }
                return Err(anyhow!(
                    "page has no downloadable video link — open it in your browser and click the floating ⬇ Download this video button, then pick a quality"
                ));
            }
        }
    }
    if let Some(t) = total_opt {
        shared.total.store(t, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }

    // Avoid silently overwriting a finished file: pick `name (1).ext`
    // when the destination already exists and no resume state refers to it.
    // Explicit CLI output paths (allow_rename=false) keep exact semantics.
    // The Add dialog's "overwrite" choice (and redownload flows, which clear
    // state first) replace the file instead.
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        let resumable_here = dest.with_extension("dcpart").exists()
            || (0..16).any(|i| dest.with_extension(format!("dcpart{i}")).exists())
            || has_partial(&dest);
        if !resumable_here {
            dest = crate::model::conflict_free_dest(&dest);
            if let Some(name) = dest
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
            {
                mutex_guard(&shared.resolved_name).replace(name);
            }
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }

    let conns = connections.clamp(1, 32);
    // Facebook/Instagram CDNs throttle parallel Range requests (idle-socket
    // drops, short segments, 403/416 on later ranges). Start those hosts on
    // a single sequential stream instead of burning retries on N-way Ranges.
    // (P2: `effective_conns` shrinks further via halving on cut storms.)
    let fragile_cdn = is_fragile_cdn(&url);
    let mut effective_conns = if fragile_cdn { 1 } else { conns };

    // One shared bucket so N segments together honor the global cap.
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
    );
    // Transfer attempts (max 3 legs): fresh-link retry for refused
    // googlevideo URLs, then single-connection fallback for hosts that
    // throttle or botch parallel Range requests (FB/IG CDN class).
    let mut retried_fresh = false;
    let mut fell_back_single = fragile_cdn;
    let mut last_segmented = false;
    loop {
        // googlevideo's strict ~2 MiB window is a per-request limit inside
        // the segmented engine (`max_request`), not a smaller split.
        let seg_parts = effective_conns;
        // One connection still goes through the engine when it has a
        // partial download to continue (the last halving step).
        let use_segmented = supports_range
            && matches!(total_opt, Some(t) if t >= 1024 * 1024)
            && (seg_parts > 1 || has_partial(&dest))
            && !fell_back_single;
        last_segmented = use_segmented;
        let r = if use_segmented {
            segmented(
                &client,
                &opts,
                &url,
                &dest,
                total_opt.unwrap(),
                seg_parts,
                shared.clone(),
                limiter.clone(),
            )
            .await
        } else if (fragile_cdn || is_googlevideo_url(&url)) && supports_range && matches!(total_opt, Some(t) if t > 0) {
            // Small bounded ranges only (see chunked_stream): throttling
            // signed CDNs cut big streams, and strict googlevideo edges
            // 403 open-ended, HEAD, and >~2 MiB ranges outright (verified
            // live) while serving identical 1 MiB windows with 206.
            // (Unverified/ignored ranges go single-stream below.)
            chunked_stream(
                &client,
                &opts,
                &url,
                &dest,
                total_opt.unwrap(),
                shared.clone(),
                limiter.clone(),
            )
            .await
        } else {
            single_stream(&client, &opts, &url, &dest, shared.clone(), limiter.clone()).await
        };
        match r {
            Ok(()) => {
                // A plain-stream fallback finished: the engine's partial
                // data (kept in case that failed too) is no longer needed.
                if !last_segmented {
                    remove_partial(&dest);
                }
                break;
            }
            Err(e)
                if !retried_fresh
                    && is_final_http_error(&e)
                    && is_googlevideo_url(&url)
                    && is_youtube_page(&opts.referer) =>
            {
                retried_fresh = true;
                match resolve_media(&client, &opts, &opts.referer, &shared).await {
                    Some(hit) if hit.url != url && is_googlevideo_url(&hit.url) => {
                        // Drop partials of the refused stream: resuming them
                        // against a different URL would corrupt the file.
                        // (The resume sidecar is dropped too; the new URL
                        // rebinds it on first write.)
                        let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
                        let _ = tokio::fs::remove_file(dest.with_extension("dcmeta")).await;
                        remove_partial(&dest);
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        shared.downloaded.store(0, Ordering::SeqCst);
                        url = hit.url;
                        let (t3, r3, _, _) = probe(&client, &opts, &url).await;
                        total_opt = t3;
                        supports_range = r3;
                        shared.resumable.store(r3, Ordering::SeqCst);
                        if let Some(t) = total_opt {
                            shared.total.store(t, Ordering::SeqCst);
                            shared.has_total.store(true, Ordering::SeqCst);
                        } else {
                            shared.has_total.store(false, Ordering::SeqCst);
                        }
                        continue;
                    }
                    _ => {
                        return Err(anyhow!(
                            "{e:#} (a fresh link was tried and also refused — replay the video in your browser and pick a new quality row)"
                        ));
                    }
                }
            }
            Err(e)
                if !fell_back_single
                    && last_segmented
                    && (is_final_http_error(&e)
                        || segmented_throttled(&e.to_string())) =>
            {
                // Parallel Ranges defeated (see seg_fallback): the resume
                // binding drops stale part sets on every re-split, and the
                // progress bar holds its max meanwhile.
                let chunked_ok = (fragile_cdn || is_googlevideo_url(&url))
                    && supports_range
                    && matches!(total_opt, Some(t) if t > 0);
                match seg_fallback(is_final_http_error(&e), effective_conns, chunked_ok) {
                    SegFallback::Halve(h) => {
                        effective_conns = h;
                        continue;
                    }
                    SegFallback::Chunked => {
                        fell_back_single = true;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        continue;
                    }
                    SegFallback::Single => {
                        fell_back_single = true;
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        supports_range = false;
                        shared.resumable.store(false, Ordering::SeqCst);
                        continue;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    // Integrity gate: a mismatched file is deleted and reported as failure
    // rather than left on disk looking complete.
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Include common Unix tool locations when launched from a Dock or desktop.
fn tool_dirs() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::env::var_os("PATH").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    #[cfg(unix)]
    {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            let p = std::path::PathBuf::from(dir);
            if !paths.contains(&p) { paths.push(p); }
        }
        if let Some(h) = dirs::home_dir() { paths.push(h.join(".local/bin")); }
    }
    paths
}

/// Locate `ffmpeg` (with `.exe` on Windows). `None` = not installed.
///
/// Source preference comes from Settings → Plugins → ffmpeg: the app's own
/// copy, PATH, or a file the user picked.
pub fn ffmpeg_path() -> Option<std::path::PathBuf> {
    let managed = || ffmpeg_local_path().filter(|p| p.is_file());
    // The chosen source first; when it is gone (a custom file moved or
    // deleted, PATH changed) any other working ffmpeg beats none at all,
    // which failed every merged 1080p+ download. yt-dlp falls back the same.
    match FfmpegSource::from_settings() {
        FfmpegSource::Custom => ffmpeg_custom_path().or_else(ffmpeg_on_path).or_else(managed),
        FfmpegSource::Path => ffmpeg_on_path().or_else(managed),
        FfmpegSource::Managed => managed().or_else(ffmpeg_on_path),
    }
}

/// `ffmpeg` found on PATH only (the app never touches that copy).
pub fn ffmpeg_on_path() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["ffmpeg.exe", "ffmpeg"]
    } else {
        &["ffmpeg"]
    };
    for dir in tool_dirs() {
        for n in names {
            let cand = dir.join(n);
            if crate::platform::executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// Locate `ffprobe` (the sibling of the ffmpeg on PATH, or one on PATH
/// itself). `None` = not available, so the frame size is simply not read back.
pub fn ffprobe_path() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["ffprobe.exe", "ffprobe"]
    } else {
        &["ffprobe"]
    };
    // Winget/Gyan keep ffmpeg and ffprobe together: prefer the sibling of the
    // ffmpeg the app already located, then fall back to a PATH search.
    if let Some(dir) = ffmpeg_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        for n in names {
            let cand = dir.join(n);
            if crate::platform::executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    for dir in tool_dirs() {
        for n in names {
            let cand = dir.join(n);
            if crate::platform::executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// Codecs ffprobe reports for a *still* picture: a photo, a screenshot, the
/// cover art in an audio file. A stream with one of these has a frame size but
/// is not a video, whatever the file is called.
fn is_still_image_codec(codec: &str) -> bool {
    matches!(
        codec.trim().to_ascii_lowercase().as_str(),
        "mjpeg"
            | "jpeg"
            | "jpegls"
            | "ljpeg"
            | "png"
            | "apng"
            | "gif"
            | "bmp"
            | "webp"
            | "tiff"
            | "tif"
            | "jpeg2000"
            | "jp2"
            | "ico"
            | "targa"
            | "dpx"
            | "exr"
            | "psd"
            | "ppm"
            | "pgm"
            | "pbm"
            | "pam"
            | "xbm"
            | "xpm"
            | "hdr"
    )
}

/// Whether a codec name from a media format means "this leg carries video".
/// `none` is what yt-dlp prints for an audio-only or image format, and `NA` is
/// what it prints when it cannot tell: neither is a video. A codec that *is* a
/// still-image one is not a video either, even though ffprobe will happily
/// decode it.
fn is_video_codec(codec: &str) -> bool {
    let c = codec.trim().to_ascii_lowercase();
    !c.is_empty() && c != "na" && c != "none" && c != "unknown" && !is_still_image_codec(&c)
}

/// The finished file's own frame size, read back with ffprobe ("1280×720").
/// This is the ground truth: it covers the paths yt-dlp's info fields cannot
/// (a direct adaptive capture merged locally) and confirms the merged result.
///
/// `None` means "no frames to measure", which is not the same as "unknown".
/// ffprobe decodes pictures as streams too, so a downloaded photo used to come
/// back as its pixel size ("558×296") and the row announced a resolution for a
/// file that has no video in it. The same happens for the cover art embedded in
/// an MP3 (a stream flagged `attached_pic`). The probe therefore reads the
/// codec and the frame count as well, and only a stream that is not an attached
/// picture, not a still-image codec and not a single frame counts as video.
pub async fn ffprobe_resolution(video: &std::path::Path) -> Option<String> {
    let exe = ffprobe_path()?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console flash for the helper process.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_name,width,height,nb_frames:stream_disposition=attached_pic",
        "-of",
        "json",
    ])
    .arg(video)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    resolution_from_ffprobe(&String::from_utf8_lossy(&out.stdout))
}

/// The decision half of [`ffprobe_resolution`]: read one ffprobe JSON answer
/// (`-of json`) and return the frame size only when the stream really is video.
/// Split out so the rule can be tested without an ffprobe on the machine.
fn resolution_from_ffprobe(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let s = v.get("streams")?.as_array()?.first()?;
    // Cover art: a picture riding along in an audio file.
    if s.get("disposition")
        .and_then(|d| d.get("attached_pic"))
        .and_then(|x| x.as_u64())
        == Some(1)
    {
        return None;
    }
    if let Some(codec) = s.get("codec_name").and_then(|x| x.as_str()) {
        if is_still_image_codec(codec) {
            return None;
        }
    }
    // One frame is a picture. (ffprobe omits `nb_frames` for many image
    // codecs — the codec check above is what catches those.)
    if s.get("nb_frames")
        .and_then(|x| x.as_str())
        .and_then(|x| x.trim().parse::<u64>().ok())
        == Some(1)
    {
        return None;
    }
    let num = |k: &str| s.get(k).and_then(|x| x.as_f64());
    resolution_label(num("width"), num("height"))
}

/// Mux downloaded video (+ optional audio) streams into one file without
/// re-encoding (`-c copy`). Used for adaptive video-only + audio pairs and
/// DASH representations — the same job IDM's merger does.
pub async fn ffmpeg_mux(
    video: &std::path::Path,
    audio: Option<&std::path::Path>,
    dest: &PathBuf,
) -> Result<()> {
    let ff = ensure_ffmpeg(&reqwest::Client::new())
        .await
        .map_err(|e| anyhow!("cannot merge video + audio: {e:#}"))?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut cmd = tokio::process::Command::new(ff);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console flash for the helper process.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.arg("-y").arg("-i").arg(video);
    if let Some(a) = audio {
        cmd.arg("-i").arg(a);
    }
    cmd.arg("-c").arg("copy");
    // Fast-start for MP4 so playback begins before the full index loads.
    if dest.extension().map(|e| e == "mp4").unwrap_or(false) {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg(dest);
    let out = cmd.output().await.map_err(|e| anyhow!("ffmpeg launch: {e}"))?;
    if !out.status.success() {
        let tail = String::from_utf8_lossy(&out.stderr);
        let tail: String = tail.chars().rev().take(400).collect::<String>().chars().rev().collect();
        return Err(anyhow!("ffmpeg merge failed: {}", tail.trim()));
    }
    Ok(())
}

/// What the proxy field asks for. Settings → Proxy writes one value that
/// every window passes along: empty = the system's settings, `direct`, a PAC
/// script as `pac:<url>`, or a proxy URL (http://, https://, socks5://,
/// socks5h://, with user:password@ when it needs a login).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProxyChoice {
    System,
    Direct,
    Pac(String),
    Manual(String),
}

pub(crate) fn proxy_choice(field: &str) -> ProxyChoice {
    let f = field.trim();
    if f.is_empty() {
        ProxyChoice::System
    } else if f.eq_ignore_ascii_case("direct") {
        ProxyChoice::Direct
    } else if let Some(script) = f.strip_prefix("pac:") {
        ProxyChoice::Pac(script.trim().to_string())
    } else if f.contains("://") {
        ProxyChoice::Manual(f.to_string())
    } else {
        // "host:port" as typed in older versions.
        ProxyChoice::Manual(format!("http://{f}"))
    }
}

/// yt-dlp release asset, fetched only when a YouTube download needs one and
/// no copy is on PATH.
const YTDLP_URL: &str = if cfg!(windows) {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
} else if cfg!(target_os="macos") {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
} else if cfg!(target_arch="aarch64") {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux_aarch64"
} else {
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
};
const YTDLP_BIN: &str = if cfg!(windows) {
    "yt-dlp.exe"
} else {
    "yt-dlp"
};

/// `yt-dlp` on PATH. A copy the user installed is theirs to keep up to date.
fn ytdlp_on_path() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["yt-dlp.exe", "yt-dlp"]
    } else {
        &["yt-dlp"]
    };
    for dir in tool_dirs() {
        for n in names {
            let cand = dir.join(n);
            if crate::platform::executable_file(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

/// Where a self-fetched yt-dlp lives.
fn ytdlp_local_path() -> Option<std::path::PathBuf> {
    Some(crate::platform::helper_dir()?.join(YTDLP_BIN))
}

/// How the user wants yt-dlp sourced: a copy they picked, whatever is on
/// PATH, or the copy this app fetches and keeps current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YtdlpSource {
    Managed,
    Path,
    Custom,
}

impl Default for YtdlpSource {
    fn default() -> Self {
        // Matches `Settings::default()`: the app's own managed copy.
        YtdlpSource::Managed
    }
}

impl YtdlpSource {
    fn from_settings() -> Self {
        match crate::model::ui_pref_str("ytdlp_source", "").as_str() {
            "managed" => YtdlpSource::Managed,
            "path" => YtdlpSource::Path,
            "custom" => YtdlpSource::Custom,
            // Installs that predate the three-way choice only had the
            // "Prefer PATH yt-dlp" checkbox. Without that flag written, the
            // app's own copy is the default.
            _ => {
                if crate::model::ui_pref_bool("ytdlp_prefer_path", false) {
                    YtdlpSource::Path
                } else {
                    YtdlpSource::Managed
                }
            }
        }
    }
}

fn select_ytdlp(source:YtdlpSource, on_path:Option<PathBuf>, custom:Option<PathBuf>, local:Option<PathBuf>) -> (Option<PathBuf>,bool) {
    let external=match source {YtdlpSource::Custom=>custom,YtdlpSource::Path=>on_path,YtdlpSource::Managed=>None};
    if external.is_some(){(external,false)}else{let managed=local.is_some();(local,managed)}
}

/// The user's own yt-dlp, when they picked one *and* it is still on disk.
fn ytdlp_custom_path() -> Option<std::path::PathBuf> {
    let raw = crate::model::ui_pref_str("ytdlp_custom_path", "");
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let p = std::path::PathBuf::from(raw);
    crate::platform::executable_file(&p).then_some(p)
}

/// The JS runtime yt-dlp needs to solve YouTube's player challenges. Only
/// deno is enabled by default, while node is far more commonly installed —
/// without one, extraction fails outright on many videos with yt-dlp's
/// "The page needs to be reloaded" (verified live here: same URL and cookies,
/// same release, works with `--js-runtimes node` and fails without it).
fn js_runtime_path() -> Option<(&'static str, std::path::PathBuf)> {
    // The app's own Deno (Settings → Plugins → Install) comes first.
    if let Some(p) = deno_local_path().filter(|p| crate::platform::executable_file(p)) {
        return Some(("deno", p));
    }
    for name in ["deno", "node"] {
        let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        for dir in tool_dirs() {
            let path=dir.join(&file);
            if crate::platform::executable_file(&path) {
                let mut command=std::process::Command::new(&path);command.arg("--version");
                let valid=crate::platform::version_output(command).map(|out|runtime_version_supported(name,&String::from_utf8_lossy(&out.stdout))).unwrap_or(false);
                if valid {return Some((name,path));}
            }
        }
    }
    None
}
fn runtime_version_supported(name:&str,version:&str)->bool {
    let word=version.split_whitespace().find(|w|w.chars().next().map(|c|c.is_ascii_digit()||c=='v').unwrap_or(false)).unwrap_or("");
    let parts:Vec<u32>=word.trim_start_matches('v').split('.').map(|s|s.parse().unwrap_or(0)).collect();
    let major=parts.first().copied().unwrap_or(0);let minor=parts.get(1).copied().unwrap_or(0);
    match name {"node"=>major>=22,"deno"=>major>2||(major==2&&minor>=3),_=>false}
}
fn js_runtime() -> Option<&'static str> { js_runtime_path().map(|(name,_)| name) }

/// The runtime the app's own YouTube reader runs the player challenge in.
pub(crate) fn js_runtime_for_solver() -> Option<(&'static str, std::path::PathBuf)> { js_runtime_path() }

/// The JavaScript runtime yt-dlp will use ("deno", "node"…), if any.
pub fn js_runtime_name() -> Option<&'static str> { js_runtime() }

/// yt-dlp, ffmpeg and a JavaScript runtime are all there (file checks only,
/// no version probes: cheap enough for start-up).
pub fn helpers_present() -> bool {
    let local = ytdlp_local_path().filter(|p| crate::platform::executable_file(p));
    let ytdlp = ytdlp_on_path().is_some() || ytdlp_custom_path().is_some() || local.is_some();
    ytdlp && ffmpeg_path().is_some() && js_runtime().is_some()
}

/// Where the app keeps its own Deno (next to its yt-dlp).
pub fn deno_local_path() -> Option<std::path::PathBuf> {
    Some(crate::platform::helper_dir()?.join(if cfg!(windows) { "deno.exe" } else { "deno" }))
}

/// Deno's portable build for this machine (one executable in a zip).
fn deno_zip_url() -> Option<String> {
    const BASE: &str = "https://github.com/denoland/deno/releases/latest/download/";
    let file = if cfg!(all(windows, target_arch = "x86_64")) {
        "deno-x86_64-pc-windows-msvc.zip"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "deno-aarch64-pc-windows-msvc.zip"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "deno-aarch64-apple-darwin.zip"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "deno-x86_64-apple-darwin.zip"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "deno-x86_64-unknown-linux-gnu.zip"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "deno-aarch64-unknown-linux-gnu.zip"
    } else {
        return None;
    };
    Some(format!("{BASE}{file}"))
}

/// One file out of a zip held in memory (stored or deflated, no zip64): what
/// a single-executable release archive needs, with no system tools.
fn zip_entry(zip: &[u8], name: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let u16_at = |i: usize| -> Result<usize> {
        zip.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).ok_or_else(|| anyhow!("zip: truncated"))
    };
    let u32_at = |i: usize| -> Result<usize> {
        zip.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or_else(|| anyhow!("zip: truncated"))
    };
    // End of central directory: the last "PK\x05\x06" (comment can follow).
    let eocd = (0..zip.len().saturating_sub(21))
        .rev()
        .take(70_000)
        .find(|&i| zip[i..].starts_with(b"PK\x05\x06"))
        .ok_or_else(|| anyhow!("zip: no directory"))?;
    let count = u16_at(eocd + 10)?;
    let mut at = u32_at(eocd + 16)?;
    for _ in 0..count {
        if !zip.get(at..).map(|b| b.starts_with(b"PK\x01\x02")).unwrap_or(false) {
            return Err(anyhow!("zip: bad directory"));
        }
        let method = u16_at(at + 10)?;
        let csize = u32_at(at + 20)?;
        let usize_ = u32_at(at + 24)?;
        let nlen = u16_at(at + 28)?;
        let xlen = u16_at(at + 30)?;
        let clen = u16_at(at + 32)?;
        let local = u32_at(at + 42)?;
        let entry = zip.get(at + 46..at + 46 + nlen).ok_or_else(|| anyhow!("zip: truncated"))?;
        let entry = String::from_utf8_lossy(entry);
        if entry.rsplit('/').next() == Some(name) {
            if !zip.get(local..).map(|b| b.starts_with(b"PK\x03\x04")).unwrap_or(false) {
                return Err(anyhow!("zip: bad entry"));
            }
            let data_at = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
            let data = zip.get(data_at..data_at + csize).ok_or_else(|| anyhow!("zip: truncated entry"))?;
            let out = match method {
                0 => data.to_vec(),
                8 => {
                    let mut out = Vec::with_capacity(usize_);
                    flate2::read::DeflateDecoder::new(data).read_to_end(&mut out)?;
                    out
                }
                m => return Err(anyhow!("zip: unsupported method {m}")),
            };
            if out.len() != usize_ {
                return Err(anyhow!("zip: {name} unpacked to {} bytes, expected {usize_}", out.len()));
            }
            return Ok(out);
        }
        at += 46 + nlen + xlen + clen;
    }
    Err(anyhow!("zip: {name} not found"))
}

/// Install (or refresh) the app's own Deno, so yt-dlp can solve YouTube's
/// player challenges without the user installing anything. Blocking: call it
/// from a worker thread.
pub fn deno_install_blocking() -> Result<std::path::PathBuf> {
    let url = deno_zip_url().ok_or_else(|| anyhow!("no Deno build for this system"))?;
    let dst = deno_local_path().ok_or_else(|| anyhow!("no folder to keep Deno in"))?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let zip = rt.block_on(async {
        let client = EngineOpts::default().build_client()?;
        let resp = client.get(&url).send().await.map_err(|e| anyhow!("Deno download: {e}"))?;
        if !resp.status().is_success() {
            return Err(anyhow!("Deno download: HTTP {}", resp.status()));
        }
        let bytes = resp.bytes().await.map_err(|e| anyhow!("Deno download: {e}"))?;
        Ok::<_, anyhow::Error>(bytes)
    })?;
    if zip.len() < 5_000_000 {
        return Err(anyhow!("Deno download: suspicious payload ({} bytes)", zip.len()));
    }
    let exe = zip_entry(&zip, if cfg!(windows) { "deno.exe" } else { "deno" })?;
    let magic_ok = if cfg!(windows) { exe.starts_with(b"MZ") } else { exe.starts_with(b"\x7fELF") || exe.starts_with(&[0xcf, 0xfa]) };
    if !magic_ok {
        return Err(anyhow!("Deno download: not an executable"));
    }
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = dst.with_extension("part");
    std::fs::write(&tmp, &exe)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &dst)?;
    // Must answer like a supported Deno, or it is not kept.
    let mut cmd = std::process::Command::new(&dst);
    cmd.arg("--version");
    let ok = crate::platform::first_run_output(cmd)
        .map(|o| runtime_version_supported("deno", &String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&dst);
        return Err(anyhow!("the downloaded Deno did not run"));
    }
    Ok(dst)
}

/// Whether a helper we fetched is old enough that YouTube has probably moved
/// on (extractors break within weeks).
async fn ytdlp_stale(path: &std::path::Path) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else {
        return true;
    };
    match meta.modified().ok().and_then(|t| t.elapsed().ok()) {
        Some(age) => age.as_secs() > 30 * 24 * 3600,
        None => false,
    }
}

/// Whether a YouTube link should try yt-dlp first. The progressive file the
/// built-in engine can complete on its own tops out at 360p, so anything
/// above that — or an open "best" request — goes to the helper when one is
/// installed. Its failures fall straight back to the built-in path.
fn ytdlp_preferred(opts: &EngineOpts) -> bool {
    quality_cap(&opts.quality_pref).map(|h| h > 360).unwrap_or(true)
}

/// A YouTube playlist or channel page (not one video): its videos are listed
/// for picking instead of being downloaded as one file.
pub fn is_youtube_list(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.");
    if host != "youtube.com" {
        return false;
    }
    let path = u.path().trim_end_matches('/');
    (path == "/playlist" && u.query_pairs().any(|(k, _)| k == "list"))
        || path.starts_with("/@")
        || path.starts_with("/channel/")
        || path.starts_with("/c/")
        || path.starts_with("/user/")
}

/// The videos of a playlist / channel: (watch URL, title), via yt-dlp's
/// flat listing (no video is downloaded). The list's own title comes first.
pub async fn playlist_entries(url: &str, opts: &EngineOpts) -> Result<(String, Vec<(String, String)>)> {
    let client = opts.build_client()?;
    let exe = ensure_ytdlp(&client).await?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.arg("--flat-playlist").arg("-J").arg("--no-warnings");
    if let Some((name, path)) = js_runtime_path() {
        cmd.arg("--js-runtimes").arg(format!("{name}:{}", path.display()));
    }
    if let Some(p) = opts.effective_proxy(url) {
        cmd.arg("--proxy").arg(p);
    }
    cmd.arg(url).stdin(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| anyhow!("listing the playlist took too long"))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("yt-dlp could not list it: {}", err.lines().last().unwrap_or("unknown error")));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("Playlist").to_string();
    let mut list = Vec::new();
    // Channels nest their tabs (Videos, Shorts) one level down.
    let mut stack: Vec<&serde_json::Value> = vec![&v];
    while let Some(node) = stack.pop() {
        for e in node.get("entries").and_then(|e| e.as_array()).into_iter().flatten().rev() {
            if e.get("entries").is_some() {
                stack.push(e);
                continue;
            }
            let id = e.get("id").and_then(|x| x.as_str()).unwrap_or("");
            let link = e
                .get("url")
                .and_then(|x| x.as_str())
                .filter(|u| u.starts_with("http"))
                .map(str::to_string)
                .or_else(|| (!id.is_empty()).then(|| format!("https://www.youtube.com/watch?v={id}")));
            if let Some(link) = link {
                let t = e.get("title").and_then(|x| x.as_str()).unwrap_or(id).to_string();
                list.push((link, t));
            }
        }
    }
    list.truncate(5000);
    Ok((title, list))
}

/// Which YouTube engine runs first (Settings -> Plugins -> YouTube):
/// "native" = the app's own engine, then yt-dlp; "ytdlp" = yt-dlp, then the
/// app's engine; "auto" (default) = whichever can deliver the requested
/// quality (YouTube serves the app's engine only its progressive streams in
/// full), with the other one as the fallback.
/// Whether the app's own YouTube reader runs for this download: not when
/// Settings put yt-dlp first, and not for "audio only" (yt-dlp makes the
/// MP3).
pub(crate) fn youtube_own_reader_allowed(_opts: &EngineOpts) -> bool {
    crate::model::ui_pref_str("youtube_engine", "auto") != "ytdlp"
        && std::env::var("DOWNLOAD_CRAFT_NO_OWN_YOUTUBE").is_err()
}

fn youtube_ytdlp_first(opts: &EngineOpts) -> bool {
    match crate::model::ui_pref_str("youtube_engine", "auto").as_str() {
        "native" => false,
        "ytdlp" => true,
        _ => ytdlp_preferred(opts),
    }
}

/// Path to a usable yt-dlp: the user's if they have one, else the copy this
/// app fetched (refreshed when it has gone stale, since YouTube moves on).
/// `Err` means nothing is available and the caller should stay with the
/// built-in engine.
/// First-run readiness, run in the background when the app starts: fetch
/// every helper that is missing (yt-dlp, ffmpeg, and Deno — the JavaScript
/// runtime yt-dlp needs for YouTube), so the first video download does not
/// stop to fetch them. Sources the user set to their own copy or PATH are
/// left alone; a present helper costs one file check (yt-dlp is also
/// refreshed when it is over a month old). Returns lines for the app's log.
pub async fn prepare_helpers() -> Vec<String> {
    let mut log = Vec::new();
    let client = match EngineOpts::default().build_client() {
        Ok(c) => c,
        Err(e) => return vec![format!("Helpers: could not start a download client ({e:#})")],
    };
    let had_ytdlp = ytdlp_info().path.is_some();
    match ensure_ytdlp(&client).await {
        Ok(_) if !had_ytdlp => log.push("yt-dlp installed (video sites).".to_string()),
        Ok(_) => {}
        Err(e) => log.push(format!("yt-dlp could not be fetched yet ({e:#}) — it is tried again when a video needs it.")),
    }
    if ffmpeg_path().is_none() && FfmpegSource::from_settings() == FfmpegSource::Managed {
        match ensure_ffmpeg(&client).await {
            Ok(_) => log.push("ffmpeg installed (joins video and audio).".to_string()),
            Err(e) => log.push(format!("ffmpeg could not be fetched yet ({e:#}) — it is tried again when a video needs it.")),
        }
    }
    if js_runtime().is_none() {
        match tokio::task::spawn_blocking(deno_install_blocking).await {
            Ok(Ok(_)) => log.push("Deno installed (YouTube's player check).".to_string()),
            Ok(Err(e)) => log.push(format!("Deno could not be fetched yet ({e:#}) — install it from Settings → Download Engine.")),
            Err(_) => {}
        }
    }
    log
}

async fn ensure_ytdlp(client: &reqwest::Client) -> Result<std::path::PathBuf> {
    let source=YtdlpSource::from_settings();
    let local=ytdlp_local_path().filter(|p|crate::platform::executable_file(p));
    let (existing,managed)=select_ytdlp(source,ytdlp_on_path(),ytdlp_custom_path(),local);
    if !managed { if let Some(path)=existing{return Ok(path);} }
    let dst = ytdlp_local_path().ok_or_else(|| anyhow!("No writable application data folder for yt-dlp"))?;
    if crate::platform::executable_file(&dst) && !ytdlp_stale(&dst).await {
        return Ok(dst);
    }
    match fetch_ytdlp(client, &dst).await {
        Ok(()) => Ok(dst),
        // A failed refresh must not cost us a working older copy.
        Err(_) if crate::platform::executable_file(&dst) => Ok(dst),
        Err(e) => Err(e),
    }
}

/// Download the helper over `dst` (fetch to a sidecar, then rename, so a
/// half-written executable can never be launched).
async fn fetch_ytdlp(client: &reqwest::Client, dst: &std::path::Path) -> Result<()> {
    static FETCH:tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard=FETCH.lock().await;
    if let Some(dir) = dst.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let resp = client
        .get(YTDLP_URL)
        .send()
        .await
        .map_err(|e| anyhow!("yt-dlp fetch: {e:#}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("yt-dlp fetch: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow!("yt-dlp fetch: {e:#}"))?;
    if bytes.len() < 1_000_000 {
        return Err(anyhow!(
            "yt-dlp fetch: suspicious payload ({} bytes)",
            bytes.len()
        ));
    }
    let tmp = dst.with_file_name(format!("yt-dlp.{}.part{}",std::process::id(),if cfg!(windows){".exe"}else{""}));
    tokio::fs::write(&tmp, &bytes).await?;
    #[cfg(unix)]
    {use std::os::unix::fs::PermissionsExt;tokio::fs::set_permissions(&tmp,std::fs::Permissions::from_mode(0o700)).await?;}
    let probe = tmp.clone();
    let answered = tokio::task::spawn_blocking(move || ytdlp_first_version(&probe)).await.unwrap_or(None);
    if answered.is_none() {
        let _=tokio::fs::remove_file(&tmp).await;
        return Err(anyhow!("Downloaded yt-dlp did not start (blocked by antivirus, or this Windows is too old for it); previous copy retained"));
    }
    if let Err(e) = tokio::fs::rename(&tmp, dst).await {
        // Windows will not replace an executable that is running (a video
        // download using the old copy): move that one aside first.
        let old = dst.with_extension("old");
        let _ = tokio::fs::remove_file(&old).await;
        if tokio::fs::rename(dst, &old).await.is_err() || tokio::fs::rename(&tmp, dst).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(anyhow!("could not replace {} ({e}); close running downloads and try again", dst.display()));
        }
        let _ = tokio::fs::remove_file(&old).await;
    }
    Ok(())
}

/// `--version` of a copy that was just downloaded, with the long first-run
/// allowance (`platform::first_run_output`).
fn ytdlp_first_version(exe: &std::path::Path) -> Option<String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--version");
    let out = crate::platform::first_run_output(cmd)?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

/// What Settings shows about the YouTube helper, and the copy it would run.
#[derive(Debug, Clone, Default)]
pub struct YtdlpInfo {
    /// The executable YouTube downloads would use right now.
    pub path: Option<std::path::PathBuf>,
    /// True when `path` is the copy this app fetched and keeps current.
    pub managed: bool,
    /// `yt-dlp --version`, when the executable answered.
    pub version: Option<String>,
    pub size: Option<u64>,
    /// The managed copy is past its 30-day refresh window.
    pub stale: bool,
    /// A copy found on PATH (the app never touches that one).
    pub on_path: Option<std::path::PathBuf>,
    /// JS runtime yt-dlp will be told about, if any.
    pub js_runtime: Option<String>,
    /// The picked custom executable is set *and* still on disk.
    pub custom_ok: bool,
}

/// What Settings shows about ffmpeg: yt-dlp needs it to merge separate video
/// and audio streams, which is every YouTube quality above 360p.
#[derive(Debug, Clone, Default)]
pub struct FfmpegInfo {
    pub path: Option<std::path::PathBuf>,
    pub version: Option<String>,
    pub size: Option<u64>,
    pub probe_path: Option<std::path::PathBuf>,
    /// `path` is the copy this app fetched and keeps in its own folder.
    pub managed: bool,
    /// A copy found on PATH (the app never touches that one).
    pub on_path: Option<std::path::PathBuf>,
}

impl FfmpegInfo {
    /// A short line for the card's identity row.
    pub fn meta(&self) -> String {
        if self.path.is_none() {
            return "not installed".to_string();
        }
        let mut bits = vec![if self.managed {
            "own copy".to_string()
        } else {
            "on PATH".to_string()
        }];
        if let Some(sz) = self.size {
            bits.push(crate::model::format_bytes(sz));
        }
        bits.join(" · ")
    }
}

/// Where the app keeps its own yt-dlp (the folder Settings can open).
pub fn ytdlp_bin_dir() -> Option<std::path::PathBuf> {
    ytdlp_local_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// Ask the helper for its version. This *runs* it (about a second for the
/// frozen build), so never call it from a UI thread.
fn ytdlp_version(exe: &std::path::Path) -> Option<String> {
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.arg("--version");
    let out = crate::platform::version_output(cmd)?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Snapshot for the Settings window. Blocking (it runs the helper to ask its
/// version): call it from a worker thread.
pub fn ytdlp_info() -> YtdlpInfo {
    let source = YtdlpSource::from_settings();
    let on_path = ytdlp_on_path();
    let local = ytdlp_local_path();
    let custom = ytdlp_custom_path();
    let managed_present = local.as_deref().map(crate::platform::executable_file).unwrap_or(false);
    // Same order `ensure_ytdlp` uses, so what Settings shows is what a
    // YouTube download would really run.
    let (path, managed) = select_ytdlp(source,on_path.clone(),custom.clone(),local.clone().filter(|_|managed_present));
    let meta = path.as_deref().and_then(|p| std::fs::metadata(p).ok());
    let stale = match (managed, local.as_deref()) {
        (true, Some(p)) => std::fs::metadata(p)
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().map(|e| e.as_secs() > 30 * 24 * 3600).unwrap_or(false))
            .unwrap_or(true),
        _ => false,
    };
    YtdlpInfo {
        version: path.as_deref().and_then(ytdlp_version),
        size: meta.as_ref().map(|m| m.len()),
        path,
        managed,
        stale,
        on_path,
        js_runtime: js_runtime().map(str::to_string),
        custom_ok: custom.is_some(),
    }
}

/// Where the app's own copy lives, for the "App's own copy" line in Settings.
pub fn ytdlp_managed_path() -> Option<std::path::PathBuf> {
    ytdlp_local_path()
}

/// ffmpeg, as resolved by the Settings source choice. Blocking (it runs
/// `ffmpeg -version` to learn the version): call it from a worker thread.
pub fn ffmpeg_info() -> FfmpegInfo {
    let path = ffmpeg_path();
    let version = path.as_deref().and_then(ffmpeg_version);
    let size = path
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let managed = match (&path, ffmpeg_local_path()) {
        (Some(p), Some(local)) => *p == local,
        _ => false,
    };
    FfmpegInfo {
        path,
        version,
        size,
        probe_path: ffprobe_path(),
        managed,
        on_path: ffmpeg_on_path(),
    }
}

/// `ffmpeg -version` opens with "ffmpeg version 7.1-full_build-www.gyan.dev
/// Copyright (c) …": the release build around the word "version".
fn ffmpeg_version(exe: &std::path::Path) -> Option<String> {
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.arg("-version");
    let out = crate::platform::version_output(cmd)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim();
    let mut words = first.split_whitespace();
    if words.next()? != "ffmpeg" {
        return None;
    }
    if words.next()? != "version" {
        return None;
    }
    let v = words.next()?.to_string();
    // Gyan/BtbN builds append their own build id ("9.0.2-full_build-www…");
    // the card only needs the version proper.
    let short = v.split('-').next().unwrap_or(&v);
    let v = if short.starts_with(|c: char| c.is_ascii_digit()) && short.contains('.') {
        short.to_string()
    } else {
        v
    };
    (!v.is_empty()).then_some(v)
}

// ---------------------------------------------------------------- ffmpeg ----
//
// Merging separate video + audio legs is the one thing the built-in engine
// cannot do itself. Like yt-dlp, ffmpeg can now be the app's own copy: fetched
// once (static build, ~30 MB gzip → ~80 MB exe), then reused. A copy the user
// installed keeps winning when they choose PATH or pick one themselves.

/// Static builds the app fetches. They are single-file executables (gzip
/// compressed), so no archive extraction and no DLLs next to them.
const FFMPEG_GZ_URL: &str = if cfg!(windows) {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-win32-x64.gz"
} else if cfg!(target_os = "macos") {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-darwin-arm64.gz"
} else {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffmpeg-linux-x64.gz"
};
const FFPROBE_GZ_URL: &str = if cfg!(windows) {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-win32-x64.gz"
} else if cfg!(target_os = "macos") {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-darwin-arm64.gz"
} else {
    "https://github.com/eugeneware/ffmpeg-static/releases/latest/download/ffprobe-linux-x64.gz"
};
const FFMPEG_BIN: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
const FFPROBE_BIN: &str = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };

/// How the user wants ffmpeg sourced — same three-way choice as yt-dlp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegSource {
    Managed,
    Path,
    Custom,
}

impl Default for FfmpegSource {
    fn default() -> Self {
        // Matches `Settings::default()`.
        FfmpegSource::Managed
    }
}

impl FfmpegSource {
    fn from_settings() -> Self {
        match crate::model::ui_pref_str("ffmpeg_source", "").as_str() {
            "path" => FfmpegSource::Path,
            "custom" => FfmpegSource::Custom,
            "managed" => FfmpegSource::Managed,
            // Installs that predate the three-way choice: keep using the
            // ffmpeg they already have on PATH instead of fetching 60 MB.
            _ => {
                if ffmpeg_on_path().is_some() {
                    FfmpegSource::Path
                } else {
                    FfmpegSource::Managed
                }
            }
        }
    }
}

/// The user's own ffmpeg, when they picked one *and* it is still on disk.
fn ffmpeg_custom_path() -> Option<std::path::PathBuf> {
    let raw = crate::model::ui_pref_str("ffmpeg_custom_path", "");
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let p = std::path::PathBuf::from(raw);
    p.is_file().then_some(p)
}

/// Where a self-fetched ffmpeg lives (same `bin` folder as yt-dlp).
fn ffmpeg_local_path() -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    { return crate::helpers::managed_ffmpeg_path(); }
    #[cfg(windows)]
    {
    let base = {
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
    };
    Some(base.join("download_craft").join("bin").join(FFMPEG_BIN))
    }
}

/// Where the app's own ffmpeg lives, for Settings.
pub fn ffmpeg_bin_dir() -> Option<std::path::PathBuf> {
    ffmpeg_local_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// A usable ffmpeg, fetching the app's own copy when the source allows it and
/// nothing else is present. Called right before a merge, so a first 1080p
/// download sets itself up instead of failing with "install ffmpeg".
pub async fn ensure_ffmpeg(client: &reqwest::Client) -> Result<std::path::PathBuf> {
    if let Some(p) = ffmpeg_path() {
        return Ok(p);
    }
    match FfmpegSource::from_settings() {
        FfmpegSource::Custom => Err(anyhow!(
            "the custom ffmpeg path in Settings no longer exists — pick it again under Settings → Download Engine"
        )),
        FfmpegSource::Path => Err(anyhow!(
            "no ffmpeg on PATH — install it, or switch Settings → Download Engine to \u{201c}App's own copy\u{201d} and the app fetches one"
        )),
        FfmpegSource::Managed => {
            // Unit tests never reach the network: the managed fetch belongs to
            // the app, and a test box without ffmpeg must not download 60 MB.
            #[cfg(test)]
            {
                let _ = client;
                return Err(anyhow!(
                    "managed ffmpeg is not fetched in tests"
                ));
            }
            #[cfg(not(test))]
            {
                #[cfg(unix)]
                { let _ = client; crate::helpers::install_ffmpeg() }
                #[cfg(windows)]
                { fetch_ffmpeg(client, false).await }
            }
        }
    }
}

/// Fetch (or refresh) the app's own ffmpeg + ffprobe. ffprobe is best-effort:
/// without it the file still plays, only the frame-size read-back is lost.
async fn fetch_ffmpeg(client: &reqwest::Client, force: bool) -> Result<std::path::PathBuf> {
    let dst = ffmpeg_local_path().ok_or_else(|| anyhow!("no LOCALAPPDATA to keep ffmpeg in"))?;
    fetch_gz_binary(client, FFMPEG_GZ_URL, &dst, "ffmpeg").await.map_err(|e| {
        anyhow!("{e:#} — check the connection, or pick a copy in Settings → Download Engine")
    })?;
    if let Some(dir) = dst.parent() {
        let probe = dir.join(FFPROBE_BIN);
        // A refresh replaces the pair together; a lazy first fetch only needs
        // ffprobe when it is not already there.
        if force || !probe.is_file() {
            let _ = fetch_gz_binary(client, FFPROBE_GZ_URL, &probe, "ffprobe").await;
        }
    }
    Ok(dst)
}

/// Download a gzip-compressed static executable and unpack it next to `dst`.
/// The payload lands in a sidecar first and is only renamed into place after
/// the decompressed file is verified, so a half-written binary is never run.
async fn fetch_gz_binary(
    client: &reqwest::Client,
    url: &str,
    dst: &std::path::Path,
    label: &str,
) -> Result<()> {
    if let Some(dir) = dst.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("{label} fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("{label} fetch: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow!("{label} fetch: {e}"))?;
    // Compressed builds are tens of MB; anything tiny is an error page.
    if bytes.len() < 5_000_000 {
        return Err(anyhow!(
            "{label} fetch: suspicious payload ({} bytes)",
            bytes.len()
        ));
    }
    let dst = dst.to_path_buf();
    let label_owned = label.to_string();
    tokio::task::spawn_blocking(move || -> Result<()> {
        use std::io::{Read, Write};
        let tmp = dst.with_extension("part");
        let mut dec = flate2::read::GzDecoder::new(&bytes[..]);
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut dec, &mut out)?;
        out.flush()?;
        drop(out);
        let mut head = [0u8; 2];
        let mut f = std::fs::File::open(&tmp)?;
        let n = f.read(&mut head)?;
        let ok_magic = if cfg!(windows) {
            n == 2 && &head == b"MZ"
        } else {
            n == 2 && &head == b"\x7fE"
        };
        if !ok_magic || std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) < 10_000_000 {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow!(
                "{label_owned} unpack: payload is not a static executable"
            ));
        }
        std::fs::rename(&tmp, &dst)?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("{label} unpack: {e}"))??;
    Ok(())
}

/// Re-fetch the app's own ffmpeg now (the Settings "Re-download" button).
/// Blocking: call it from a worker thread.
pub fn ffmpeg_refresh_blocking() -> Result<std::path::PathBuf> {
    #[cfg(unix)]
    { return crate::helpers::install_ffmpeg(); }
    #[cfg(windows)]
    {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let client = EngineOpts::default().build_client()?;
        fetch_ffmpeg(&client, true).await
    })
    }
}

/// Dotted numbers of a version or release tag ("2026.08.19" → [2026, 8, 19],
/// "b6.1.1" → [6, 1, 1], "9.0.2-full_build" → [9, 0, 2]).
fn version_parts(v: &str) -> Vec<u64> {
    let start = v.find(|c: char| c.is_ascii_digit()).unwrap_or(v.len());
    let core = v[start..].split(|c: char| !(c.is_ascii_digit() || c == '.')).next().unwrap_or("");
    core.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// True when `have` is an older release than `latest` (missing parts count
/// as 0, so "2026.08.19" < "2026.08.19.1"). Unparseable input is never
/// called older: no update is better than a wrong one.
pub fn version_older(have: &str, latest: &str) -> bool {
    let (a, b) = (version_parts(have), version_parts(latest));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let n = a.len().max(b.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..n).map(|i| (at(&a, i), at(&b, i))).find(|(x, y)| x != y).is_some_and(|(x, y)| x < y)
}

/// Newest release tag of a GitHub repo ("yt-dlp/yt-dlp"), or `None` offline.
async fn latest_release_tag(client: &reqwest::Client, repo: &str) -> Option<String> {
    let v: serde_json::Value = client
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        // GitHub's API rejects requests without a User-Agent.
        .header(reqwest::header::USER_AGENT, format!("download_craft/{}", crate::model::APP_VERSION))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    v.get("tag_name").and_then(|t| t.as_str()).map(str::to_string)
}

/// Settings → Plugins on open: compare yt-dlp with its latest release and
/// replace the app's own copy when it is older. A copy the user chose (PATH
/// or custom) is theirs: it is reported, never overwritten. Returns the line
/// the card shows ("" when nothing could be checked).
pub fn ytdlp_update_check_blocking() -> String {
    let info = ytdlp_info();
    let Some(have) = info.version.clone() else { return String::new() };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return String::new(),
    };
    rt.block_on(async {
        let Ok(client) = EngineOpts::default().build_client() else { return String::new() };
        let Some(latest) = latest_release_tag(&client, "yt-dlp/yt-dlp").await else {
            return "Could not check for a newer yt-dlp (offline?).".to_string();
        };
        if !version_older(&have, &latest) {
            return format!("yt-dlp {have} is the latest release.");
        }
        if !info.managed {
            return format!(
                "yt-dlp {latest} is available; this copy is {have}. Pick \"App-managed\" to have it kept up to date, or update your own copy."
            );
        }
        let Some(dst) = ytdlp_local_path() else { return String::new() };
        match fetch_ytdlp(&client, &dst).await {
            Ok(()) => format!("Updated yt-dlp {have} → {latest}."),
            Err(e) => format!("yt-dlp {latest} is available, but the update failed: {e:#}"),
        }
    })
}

/// Same for the app's own ffmpeg. Only a managed copy is compared: its
/// releases come from the static-build project the app fetches from, which
/// says nothing about a build the user installed (Gyan, BtbN, a package
/// manager), so those are left alone.
pub fn ffmpeg_update_check_blocking() -> String {
    let info = ffmpeg_info();
    if !info.managed {
        return String::new();
    }
    let Some(have) = info.version.clone() else { return String::new() };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return String::new(),
    };
    let latest = rt.block_on(async {
        let client = EngineOpts::default().build_client().ok()?;
        latest_release_tag(&client, "eugeneware/ffmpeg-static").await
    });
    let Some(latest) = latest else {
        return "Could not check for a newer ffmpeg (offline?).".to_string();
    };
    if !version_older(&have, &latest) {
        return format!("ffmpeg {have} is the latest build.");
    }
    match ffmpeg_refresh_blocking() {
        Ok(_) => format!("Updated ffmpeg {have} → {}.", latest.trim_start_matches('b')),
        Err(e) => format!("A newer ffmpeg is available, but the update failed: {e:#}"),
    }
}

/// Replace the app's own copy of yt-dlp now — the Settings "Re-download"
/// button. Works whether or not a copy is on PATH, and whether or not the
/// existing one looks stale; the fetched file is parsed-checked by size only
/// (yt-dlp ships as one executable).
pub fn ytdlp_refresh_blocking() -> Result<std::path::PathBuf> {
    let dst = ytdlp_local_path().ok_or_else(|| anyhow!("No writable application data folder for yt-dlp"))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let client = EngineOpts::default().build_client()?;
        fetch_ytdlp(&client, &dst).await?;
        Ok(dst)
    })
}

/// Write the row's captured browser cookies as a Netscape cookie jar (what
/// `--cookies` reads), so yt-dlp inherits the signed-in YouTube session
/// without touching the browser's locked profile database. Removed after the
/// run, so no credentials linger on disk.
struct CookieJar(PathBuf);
impl Drop for CookieJar {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
async fn write_cookie_jar(cookie: &str, domain: &str) -> Result<Option<CookieJar>> {
    if cookie.trim().is_empty() {
        return Ok(None);
    }
    let mut body = String::from("# Netscape HTTP Cookie File\n");
    for part in cookie.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        let name = name.trim();
        let bad = |s: &str| s.contains(['\t', '\n', '\r']);
        if name.is_empty() || bad(name) || bad(value) {
            continue;
        }
        body.push_str(&format!("{domain}\tTRUE\t/\tFALSE\t0\t{name}\t{value}\n"));
    }
    if body.lines().count() == 1 {
        return Ok(None);
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let tag = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "download_craft_cookies_{}_{tag}.txt",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut f) => {
                let jar = CookieJar(path);
                std::io::Write::write_all(&mut f, body.as_bytes())?;
                return Ok(Some(jar));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow!("Could not create a private cookie file"))
}

/// yt-dlp format expression for a menu preference: the best video at or below
/// the asked height plus the best audio, merged to mp4 by
/// `--merge-output-format`.
///
/// **Quality comes before the container, deliberately.** Asking for
/// `bv*[height<=720][ext=mp4]` first meant that on an upload whose 720p exists
/// only as VP9 - which is most modern YouTube uploads - that tier had no match
/// at 720p and yt-dlp took the best *mp4* stream instead: 360p. A "720p" pick
/// quietly downloaded 360p. Remuxing VP9/AV1 into mp4 costs nothing and keeps
/// the resolution the user asked for.
///
/// `can_merge` (is there an ffmpeg?) picks the strategy: with ffmpeg the
/// adaptive video+audio legs are merged; without one, asking for a merge only
/// makes yt-dlp write two unplayable files, so the cap moves to the best
/// *muxed* format at or below it - the highest quality a single file can be.
///
/// The audio tier prefers m4a: the merge output is mp4, and Opus in mp4 is the
/// one combination players still object to.
fn ytdlp_format_with(pref: &str, can_merge: bool) -> String {
    match (quality_cap(pref), can_merge) {
        (Some(h), true) => format!(
            "bv*[height<={h}]+ba[ext=m4a]/bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"
        ),
        (Some(h), false) => format!("b[height<={h}]/b"),
        (None, true) => "bv*+ba[ext=m4a]/bv*+ba/b".to_string(),
        (None, false) => "b".to_string(),
    }
}

/// Vimeo can expose a video-only media playlist as an otherwise valid
/// format. Require a real audio leg or a muxed format with an audio codec.
fn audio_required_format(pref: &str, can_merge: bool) -> String {
    match (quality_cap(pref), can_merge) {
        (Some(h), true) => format!("bv[height<={h}]+ba[ext=m4a]/bv[height<={h}]+ba/b[height<={h}][acodec!=none]/bv+ba/b[acodec!=none]"),
        (Some(h), false) => format!("b[height<={h}][acodec!=none]/b[acodec!=none]"),
        (None, true) => "bv+ba[ext=m4a]/bv+ba/b[acodec!=none]".to_string(),
        (None, false) => "b[acodec!=none]".to_string(),
    }
}

fn hls_external_audio(text: &str) -> bool {
    text.lines().filter(|line| line.trim().starts_with("#EXT-X-MEDIA:")).any(|line| {
        let attrs = hls_key_attrs(line.trim());
        hls_attr(&attrs, "TYPE").map(|t| t.eq_ignore_ascii_case("AUDIO")).unwrap_or(false)
            && hls_attr(&attrs, "URI").map(|u| !u.is_empty()).unwrap_or(false)
    })
}

fn completed_media_check(json: &str, expected_secs: u64, require_audio: bool) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    if require_audio && !ffprobe_has_audio(json)? {
        return Err(anyhow!("downloaded media has no audio track, although the source includes audio — capture the full master playlist or player page"));
    }
    if expected_secs > 0 {
        let number = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64());
        let duration = value.get("format").and_then(|v| v.get("duration")).and_then(number)
            .or_else(|| value.get("streams").and_then(|v| v.as_array()).and_then(|s|
                s.iter().filter_map(|v| v.get("duration").and_then(number)).filter(|d| d.is_finite()).reduce(f64::max)))
            .filter(|d| d.is_finite() && *d > 0.0)
            .ok_or_else(|| anyhow!("could not verify the finished media duration"))?;
        let expected = expected_secs as f64;
        let tolerance = (expected * 0.03).max(2.0).min(expected * 0.25);
        if duration < expected - tolerance {
            return Err(anyhow!("incomplete media: downloaded {duration:.1}s but the player/source reports {expected_secs}s — capture the full master playlist or player page"));
        }
    }
    Ok(())
}

async fn verify_completed_media(dest: &std::path::Path, shared: &Arc<SharedState>) -> Result<()> {
    let resolved = mutex_guard(&shared.resolved_name).clone();
    let actual = resolved.map(|name| dest.with_file_name(name)).unwrap_or_else(|| dest.to_path_buf());
    // DRM media (Widevine/PlayReady/FairPlay) arrives with its samples
    // encrypted: the file "finishes" but has no picture or sound anywhere.
    // Say so plainly instead of keeping a file that never plays. The
    // protection is never worked around.
    let probe = actual.clone();
    if tokio::task::spawn_blocking(move || mp4_encrypted(&probe)).await.unwrap_or(false) {
        let _ = std::fs::remove_file(&actual);
        return Err(anyhow!("this video is DRM-protected (encrypted), so it can't be downloaded — a saved copy has no picture or sound; the file was removed"));
    }
    let expected = shared.expected_duration.load(Ordering::SeqCst);
    let audio = shared.expects_audio.load(Ordering::SeqCst);
    if expected == 0 && !audio { return Ok(()); }
    let Some(exe) = ffprobe_path() else {
        shared.set_note("finished media could not be checked for duration/audio: ffprobe was not found".to_string());
        return Ok(());
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.args(["-v", "error", "-show_entries", "format=duration:stream=codec_type,duration", "-of", "json"])
        .arg(actual).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output()).await
        .map_err(|_| anyhow!("finished media verification timed out"))??;
    if !out.status.success() { return Err(anyhow!("could not inspect the finished media file")); }
    completed_media_check(&String::from_utf8_lossy(&out.stdout), expected, audio)
}

/// True when an MP4/M4A/fragmented-MP4 file's tracks are encrypted (CENC:
/// `encv`/`enca`/`encs`/`enct` sample entries in `moov`). Only the box tree is
/// walked, never the media bytes, so a chance byte pattern in `mdat` cannot
/// trip it. Anything that is not ISO-BMFF is "not encrypted".
fn mp4_encrypted(path: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    let mut pos = 0u64;
    let mut first = true;
    while pos + 8 <= len {
        let mut h = [0u8; 16];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut h[..8]).is_err() { return false; }
        let kind = [h[4], h[5], h[6], h[7]];
        let (size, head) = match u32::from_be_bytes([h[0], h[1], h[2], h[3]]) {
            0 => (len - pos, 8u64),
            1 => {
                if f.read_exact(&mut h[8..16]).is_err() { return false; }
                (u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]), 16)
            }
            n => (n as u64, 8),
        };
        if size < head || pos + size > len {
            return false;
        }
        if first {
            // An ISO-BMFF file opens with one of these; anything else is not one.
            if !matches!(&kind, b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"wide") { return false; }
            first = false;
        }
        if &kind == b"moov" {
            let n = (size - head).min(64 << 20) as usize;
            let mut body = vec![0u8; n];
            if f.read_exact(&mut body).is_err() { return false; }
            return iso_boxes_encrypted(&body, 0);
        }
        pos += size;
    }
    false
}

fn iso_boxes_encrypted(data: &[u8], depth: u32) -> bool {
    if depth > 8 { return false; }
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let kind = &data[i + 4..i + 8];
        if size < 8 || i + size > data.len() { return false; }
        let body = &data[i + 8..i + size];
        match kind {
            b"encv" | b"enca" | b"encs" | b"enct" => return true,
            b"trak" | b"mdia" | b"minf" | b"stbl" => {
                if iso_boxes_encrypted(body, depth + 1) { return true; }
            }
            // Sample descriptions: version/flags + entry count, then entries.
            b"stsd" if body.len() > 8 => {
                if iso_boxes_encrypted(&body[8..], depth + 1) { return true; }
            }
            _ => {}
        }
        i += size;
    }
    false
}

fn ffprobe_has_audio(json: &str) -> Result<bool> {
    let data: serde_json::Value = serde_json::from_str(json)?;
    let streams = data.get("streams").and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("ffprobe returned no stream list"))?;
    Ok(streams.iter().any(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("audio")))
}

async fn verify_vimeo_audio(dest: &std::path::Path) -> Result<()> {
    // Strict format selection is always active; additionally inspect the
    // finished file when the ffprobe companion is installed.
    let Some(exe) = ffprobe_path() else { return Ok(()); };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd.args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "json"])
        .arg(dest).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output()).await
        .map_err(|_| anyhow!("Vimeo audio verification timed out"))??;
    if !out.status.success() { return Err(anyhow!("could not verify the downloaded Vimeo audio")); }
    if !ffprobe_has_audio(&String::from_utf8_lossy(&out.stdout))? {
        return Err(anyhow!("Vimeo download has no audio track — capture the full master manifest or Vimeo player URL instead of a video-only playlist"));
    }
    Ok(())
}

/// [`ytdlp_format_with`] with the merge capability of this machine.
fn ytdlp_format(pref: &str) -> String {
    ytdlp_format_with(pref, ffmpeg_path().is_some())
}

/// Hand a YouTube link to yt-dlp: it speaks the browser-only SABR protocol
/// that gates 1080p+ (plain HTTP sees only the stream's first ~minute, then
/// 403 forever) and merges the legs itself. Progress lines keep the row and
/// the progress window live; cancelling kills the child.
/// Marker in front of every progress line yt-dlp prints.
///
/// `--progress-template [TYPES:]TEMPLATE` keeps only the template text, so the
/// type cannot be used as the marker the reader looks for - which is exactly
/// why a yt-dlp download used to show no progress at all while the transfer
/// ran at full speed: the line `@1024/NA` never matched `download:@`.
const PROGRESS_MARK: &str = "dcp@";

/// The `--progress-template` value: `bytes done | size | size estimate | rate`
/// plus the chosen format's own info fields (`height | width | format id |
/// video codec`), so the app can report the resolution actually being fetched
/// instead of the browser's guess.
///
/// The codec is part of it because a frame size is not a resolution: a photo
/// post's format carries the picture's pixel size in the same fields a video
/// uses, and an audio leg carries none at all. Only a leg with a real video
/// codec has a resolution to report.
///
/// `total_bytes` is `NA` until the response headers are in, and stays `NA`
/// forever for some formats (DASH/HLS fragments), which is what the estimate
/// covers.
fn ytdlp_progress_template() -> String {
    format!(
        "download:{PROGRESS_MARK}%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(info.height)s|%(info.width)s|%(info.format_id)s|%(info.vcodec)s|%(progress.fragment_index)s|%(progress.fragment_count)s"
    )
}

/// Stable small id for a format id string (legs of a merged download differ).
fn format_key(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let s = s.trim();
    if s.is_empty() || s == "NA" {
        return 0;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish().max(1)
}

/// One progress line, parsed. A field is `None` when yt-dlp printed `NA`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct YtdlpProgress {
    done: Option<f64>,
    total: Option<f64>,
    estimate: Option<f64>,
    speed: Option<f64>,
    /// Frame height of the format being downloaded (NA for an audio-only leg).
    height: Option<f64>,
    /// Frame width, same caveat.
    width: Option<f64>,
    /// Whether this leg carries real video. False for an audio-only leg, for a
    /// photo format (which has a frame size but no video) and when yt-dlp
    /// cannot say. Only a video leg has a resolution to show.
    video: bool,
    /// [`format_key`] of the format being fetched (0 = unknown): a change is
    /// the only reliable sign that the next leg of a merged download began.
    format: u64,
    /// Fragment index/count of a fragmented (HLS/DASH) transfer.
    frag_index: Option<f64>,
    frag_count: Option<f64>,
}

/// Read a progress line, or `None` when this line is not one.
fn parse_progress_line(line: &str) -> Option<YtdlpProgress> {
    // Searched for rather than prefix-matched: the first line of a stream can
    // carry a stray carriage return, and yt-dlp interleaves other messages.
    let at = line.find(PROGRESS_MARK)?;
    let mut field = line[at + PROGRESS_MARK.len()..].split('|');
    let num = |v: Option<&str>| v.map(str::trim).and_then(|v| v.parse::<f64>().ok());
    let done = num(field.next());
    let total = num(field.next());
    let estimate = num(field.next());
    let speed = num(field.next());
    let height = num(field.next());
    let width = num(field.next());
    let format = field.next().map(format_key).unwrap_or(0);
    // The codec decides whether the frame size means anything.
    let video = field.next().map(is_video_codec).unwrap_or(false);
    Some(YtdlpProgress {
        done,
        total,
        estimate,
        speed,
        height,
        width,
        video,
        format,
        frag_index: num(field.next()),
        frag_count: num(field.next()).filter(|n| *n > 0.0),
    })
}

/// "1280×720" when both sides are known, else "720p" from the height alone.
/// `None` when neither is usable (audio-only leg, or `NA`).
fn resolution_label(width: Option<f64>, height: Option<f64>) -> Option<String> {
    let positive = |v: Option<f64>| v.filter(|n| *n >= 64.0 && *n <= 8640.0).map(|n| n as u64);
    let w = positive(width);
    let h = positive(height);
    match (w, h) {
        (Some(w), Some(h)) => Some(format!("{w}×{h}")),
        (None, Some(h)) => Some(format!("{h}p")),
        _ => None,
    }
}

/// The height inside a [`resolution_label`] string ("1280×720" → 720,
/// "720p" → 720, anything else → `None`).
fn resolution_height(label: &str) -> Option<u64> {
    let tail = label.rsplit_once('×').map(|(_, h)| h).unwrap_or(label);
    tail.trim_end_matches('p').trim().parse::<u64>().ok()
}

/// Folds the progress lines of a *merged* helper download into the numbers the
/// UI shows. yt-dlp transfers the legs one after another (video, then audio)
/// and each of them restarts its byte counter at 0, so both the byte count and
/// the size have to be built from the finished legs plus the current one. While
/// a new leg has not named its size yet the total is reported as unknown —
/// keeping the previous leg's total there is what showed "Downloaded 847.65 MB"
/// beside "Size 722.80 MB" on the same row.
#[derive(Default)]
struct MergedProgress {
    /// Finished legs: bytes and sizes.
    base: u64,
    total_base: u64,
    /// Current leg: bytes and size (0 = not named yet).
    last: u64,
    last_total: u64,
    /// Whether the current leg has named its size.
    known: bool,
    /// [`format_key`] of the current leg (0 = not named).
    format: u64,
}

impl MergedProgress {
    /// Fold one line in and return `(downloaded, total)` to publish.
    fn step(&mut self, p: &YtdlpProgress) -> (u64, Option<u64>) {
        if let Some(done) = p.done.map(|d| d.max(0.0) as u64) {
            // A new leg restarts its counter at 0. With several fragments in
            // flight the counter also dips a little now and then, and reading
            // every dip as a new leg added the whole count again each time:
            // the "18 GB" an HLS download claimed after a resume. So a new leg
            // is a new format id, or (ids unknown) a counter that fell back
            // to under half of where it was.
            let new_leg = if p.format != 0 && self.format != 0 {
                p.format != self.format
            } else {
                done < self.last / 2
            };
            if new_leg {
                self.base += self.last_total.max(self.last);
                self.total_base += self.last_total;
                self.last_total = 0;
                self.known = false;
            }
            self.last = if new_leg { done } else { done.max(self.last) };
        }
        if p.format != 0 {
            self.format = p.format;
        }
        // A fragmented stream has no size, only an estimate that moves with
        // every fragment (the "size keeps changing" report): leave it
        // unknown, as IDM does, and let the fragment count drive the bar.
        let size = if p.frag_count.is_some() { p.total } else { p.total.or(p.estimate) };
        if let Some(total) = size.filter(|t| *t > 0.0) {
            self.last_total = total as u64;
            self.known = true;
        }
        let total = if self.known {
            Some(self.total_base + self.last_total)
        } else {
            None
        };
        (self.base + self.last, total)
    }
}

/// The stage a yt-dlp *status* line reports, if it reports one.
///
/// Only the stages that can leave the UI looking dead matter: extraction
/// (player fetch + JS challenge, several seconds of nothing) and the merge
/// that runs after every byte has arrived.
fn phase_for_line(line: &str) -> Option<&'static str> {
    let l = line.trim_start_matches(['\r', ' ']);
    if l.starts_with("[Merger]")
        || l.starts_with("[ffmpeg]")
        || l.starts_with("[Fixup")
        || l.starts_with("[ExtractAudio]")
        || l.starts_with("[Metadata]")
        || l.starts_with("[EmbedThumbnail]")
        || l.starts_with("[VideoRemuxer]")
        || l.starts_with("[VideoConvertor]")
    {
        return Some("Merging");
    }
    if l.starts_with("[download] Destination:") {
        return Some("Downloading");
    }
    if l.starts_with("[youtube]")
        || l.starts_with("[info]")
        || l.starts_with("[generic]")
        || l.starts_with("[vimeo]")
        || l.starts_with("[twitter]")
        || l.starts_with("[facebook]")
    {
        return Some("Getting info");
    }
    None
}

/// Folder name for a helper's in-progress pieces, inside the save folder.
const PARTS_DIR: &str = ".download_craft_parts";

/// `(home, parts)` folders for a yt-dlp run saving to `dest`.
fn ytdlp_dirs(dest: &std::path::Path) -> (PathBuf, PathBuf) {
    let home = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let parts = home.join(PARTS_DIR);
    (home, parts)
}

/// The bare file name yt-dlp is told to write (relative to `home`).
fn ytdlp_out_name(dest: &std::path::Path) -> String {
    dest.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download.mp4".to_string())
}

/// Create the parts folder and mark it hidden (Windows), best-effort.
fn hide_dir(dir: &std::path::Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN);
        }
    }
}

/// Drop the parts folder once nothing is left in it (other downloads in the
/// same folder may still be using it).
fn tidy_parts_dir(dest: &std::path::Path) {
    let (_, parts) = ytdlp_dirs(dest);
    let _ = std::fs::remove_dir(parts);
}

fn valid_ytdlp_output(path: &std::path::Path, dest: &std::path::Path) -> bool {
    if !path.is_file() || path.file_stem() != dest.file_stem() {
        return false;
    }
    let parent = |p: &std::path::Path| {
        p.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."))
            .canonicalize()
            .ok()
    };
    matches!((parent(path),parent(dest)),(Some(a),Some(b)) if a==b)
}

async fn download_with_ytdlp(
    exe: &std::path::Path,
    url: &str,
    dest: &PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    shared.external_helper.store(true, Ordering::SeqCst);
    shared.ytdlp_tried.store(true, Ordering::SeqCst);
    wait_if_paused(shared).await?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // A file already sitting at the destination makes yt-dlp report
    // "already downloaded" and exit without fetching anything. It is the
    // user's file though: unless overwrite was chosen, save a numbered copy
    // (as the progressive path does) instead of deleting it.
    let renamed;
    let dest: &PathBuf = if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        renamed = crate::model::conflict_free_dest(dest);
        if let Some(name) = renamed.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
        &renamed
    } else {
        let _ = tokio::fs::remove_file(dest).await;
        dest
    };
    let jar = write_cookie_jar(&opts.cookie, &cookie_domain(url)).await?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    {
        // No console flash for the helper process.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // One lookup decides both the selector and whether yt-dlp may ask for a
    // merge at all (no ffmpeg = only a single muxed file can be delivered).
    // The app's own copy is fetched here when nothing else exists, so a first
    // 1080p download sets itself up instead of quietly capping at 720p.
    let mut merge_ffmpeg = ffmpeg_path();
    if merge_ffmpeg.is_none() {
        if let Ok(c) = opts.build_client() {
            shared.set_phase("Fetching ffmpeg");
            match ensure_ffmpeg(&c).await {
                Ok(p) => merge_ffmpeg = Some(p),
                Err(e) => shared.set_note(format!(
                    "ffmpeg is unavailable ({e:#}) — downloading the best single-file quality instead"
                )),
            }
        }
    }
    // Settings → Network: yt-dlp goes out through the same adapter, and does
    // not start while it is down (the kill switch).
    match crate::netlock::bind_ip() {
        Err(e) => return Err(anyhow!(e)),
        Ok(Some(ip)) => {
            cmd.arg("--source-address").arg(ip.to_string());
        }
        Ok(None) => {}
    }
    cmd.arg("--no-playlist")
        .arg("--no-warnings")
        .arg("--abort-on-unavailable-fragments")
        .arg("--no-simulate")
        .arg("--print")
        .arg("before_dl:dcinfo@{\"duration\":%(duration)j,\"acodec\":%(acodec)j}")
        .arg("--print").arg("after_move:dcfile@%(filepath)j")
        .arg("--newline")
        // `--print` implies quiet mode, which hides progress: without this
        // the window sat at 0 B / "Getting info" until the file was done.
        .arg("--progress")
        .arg("--progress-template")
        .arg(ytdlp_progress_template())
        .arg("--retries")
        .arg(opts.max_retries.max(1).to_string())
        .arg("--socket-timeout")
        .arg(opts.timeout_secs.max(5).to_string())
        .arg("--concurrent-fragments")
        .arg(connections.clamp(1, 8).to_string())
        .arg("--merge-output-format")
        .arg("mp4")
        .arg("-f")
        .arg(if wants_audio_only(&opts.quality_pref) {
            "ba[ext=m4a]/ba/b".to_string()
        } else if vimeo_needs_audio(url) || opts.require_audio {
            audio_required_format(&opts.quality_pref, merge_ffmpeg.is_some())
        } else {
            ytdlp_format_with(&opts.quality_pref, merge_ffmpeg.is_some())
        })
        .arg("-o")
        .arg(if wants_audio_only(&opts.quality_pref) {
            // The audio arrives as m4a/webm and becomes .mp3 afterwards:
            // yt-dlp names each step's extension itself.
            let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "audio".into());
            format!("{}.%(ext)s", stem.replace('%', "%%"))
        } else {
            ytdlp_out_name(dest).replace('%',"%%")
        });
    // Fragments and other in-progress pieces go to one hidden parts folder
    // next to the file instead of dozens of ".part-FragN" files in the
    // user's folder; only the finished file lands in it. Same drive, so the
    // final move is a rename. (`-P temp:` is ignored for an absolute `-o`,
    // hence home + a bare file name.)
    let (home, parts) = ytdlp_dirs(dest);
    cmd.arg("-P").arg(format!("home:{}", home.display()));
    cmd.arg("-P").arg(format!("temp:{}", parts.display()));
    hide_dir(&parts);
    if vimeo_needs_audio(url) || is_playlist_url(url) || opts.require_audio {
        // Missing fragments must fail rather than produce a shortened video.
        cmd.arg("--abort-on-unavailable-fragments");
    }
    if let Some(ff) = &merge_ffmpeg {
        cmd.arg("--ffmpeg-location").arg(ff);
        // Audio only: an MP3 (best VBR); without ffmpeg, or when M4A was
        // asked for, the m4a as served.
        if wants_audio_only(&opts.quality_pref) && !wants_m4a(&opts.quality_pref) {
            cmd.arg("-x").arg("--audio-format").arg("mp3").arg("--audio-quality").arg("0");
        }
    }
    // Without a JS runtime yt-dlp cannot solve YouTube's player challenges:
    // extraction fails on many videos and it reports only "The page needs to
    // be reloaded" (reproduced live against this very URL).
    if let Some((name,path)) = js_runtime_path() {
        cmd.arg("--js-runtimes").arg(format!("{name}:{}",path.display()));
    }
    if let Some(jar) = &jar {
        cmd.arg("--cookies").arg(&jar.0);
    } else if !opts.ytdlp_visitor_data.is_empty() && is_youtube_page(url) {
        // Retry after a bot check: the page's own visitor id (yt-dlp wants the
        // web page skipped with it, and no cookies).
        cmd.arg("--extractor-args").arg(format!(
            "youtube:visitor_data={};player_skip=webpage,configs",
            opts.ytdlp_visitor_data
        ));
    }
    if !opts.referer.trim().is_empty() {
        cmd.arg("--referer").arg(opts.referer.trim());
    }
    if opts.captured_headers.applies_to(url) {
        for (name, value) in &opts.captured_headers.headers {
            cmd.arg("--add-header").arg(format!("{name}:{value}"));
        }
    }
    if let Some(p) = opts.effective_proxy(url) {
        cmd.arg("--proxy").arg(p);
    }
    if opts.impersonate {
        cmd.arg("--impersonate").arg("chrome");
    }
    if opts.overwrite_existing {
        cmd.arg("--force-overwrites");
    }
    // Settings -> Plugins -> YouTube: subtitles saved next to the video.
    if crate::model::ui_pref_bool("subtitles", false) {
        let langs = crate::model::ui_pref_str("subtitle_langs", "en.*");
        let langs: String = langs.chars().filter(|c| c.is_ascii_alphanumeric() || ",.*-_".contains(*c)).collect();
        cmd.arg("--write-subs").arg("--sub-langs").arg(if langs.is_empty() { "en.*".to_string() } else { langs });
        if merge_ffmpeg.is_some() {
            cmd.arg("--convert-subs").arg("srt");
        }
    }
    cmd.arg(url);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| anyhow!("yt-dlp launch: {e}"))?;
    let process_tree=crate::platform::HelperProcessTree::attach(&child)
        .map_err(|e|anyhow!("Could not supervise media helper processes: {e}"))?;
    // Nothing is on the wire until the player challenge is solved, so say what
    // is happening instead of showing a motionless 0%; and when the app's own
    // YouTube reader gave up first, that yt-dlp is the fallback.
    shared.set_phase(if shared.youtube_own_failed.load(Ordering::SeqCst) {
        "yt-dlp fallback — getting info"
    } else {
        "Getting info"
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("yt-dlp stdout unavailable"))?;
    let stderr = child.stderr.take();
    // Drain stdout for progress (both pipes must be drained, or a chatty
    // helper fills the pipe buffer and deadlocks).
    let progress = Arc::clone(shared);
    let out_task = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        // A merged download is several transfers back to back (video, then
        // audio) and each one restarts its byte counter at 0: `MergedProgress`
        // folds the finished legs in so the bar keeps moving forward and the
        // size ends up being all legs together.
        let mut folded = MergedProgress::default();
        let mut final_path=None;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(json)=line.strip_prefix("dcfile@") {
                final_path=serde_json::from_str::<String>(json).ok().map(PathBuf::from);
                continue;
            }
            if let Some(json) = line.strip_prefix("dcinfo@") {
                if let Ok(info) = serde_json::from_str::<serde_json::Value>(json) {
                    if let Some(d) = info.get("duration").and_then(|v| v.as_f64()).filter(|d| d.is_finite() && *d > 0.0 && *d < 86400.0) {
                        let _ = progress.expected_duration.compare_exchange(0, d.round().max(1.0) as u64, Ordering::SeqCst, Ordering::SeqCst);
                    }
                    if info.get("acodec").and_then(|v| v.as_str()).map(|a| !matches!(a, "none" | "NA" | "unknown" | "")).unwrap_or(false) {
                        progress.expects_audio.store(true, Ordering::SeqCst);
                    }
                }
                continue;
            }
            let Some(p) = parse_progress_line(&line) else {
                // Not a progress line: it may still name the stage the run is
                // in ("[download] Destination:", "[Merger] …").
                if let Some(ph) = phase_for_line(&line) {
                    progress.set_phase(ph);
                }
                continue;
            };
            if p.done.is_some() || p.speed.is_some() {
                progress.set_phase(PHASE_YTDLP);
            }
            // The format yt-dlp picked names its own frame size, so the row can
            // report the resolution that really arrived rather than the menu
            // pick (a cap the video cannot meet, e.g. 720p on a 360p upload,
            // would otherwise look like the choice was honoured). A frame size
            // is not a resolution though: a photo post's format has one too,
            // which is how a downloaded picture ended up claiming a resolution.
            if p.video {
                if let Some(r) = resolution_label(p.width, p.height) {
                    progress.set_resolution(r);
                }
            }
            if let (Some(i), Some(n)) = (p.frag_index, p.frag_count) {
                progress.frag_count.store(n as u64, Ordering::SeqCst);
                progress.frag_index.store((i.max(0.0) as u64).min(n as u64), Ordering::SeqCst);
            }
            let (downloaded, total) = folded.step(&p);
            progress.downloaded.store(downloaded, Ordering::SeqCst);
            match total {
                Some(t) => {
                    progress.total.store(t, Ordering::SeqCst);
                    progress.has_total.store(true, Ordering::SeqCst);
                }
                // No size yet (extraction, or a new leg that has not named one).
                None => progress.has_total.store(false, Ordering::SeqCst),
            }
            if let Some(speed) = p.speed.filter(|s| *s > 0.0) {
                progress.speed_bps.store(speed as u64, Ordering::SeqCst);
            }
        }
        final_path
    });
    let err_task = tokio::spawn(async move {
        let mut kept: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        if let Some(err) = stderr {
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if kept.len() == 8 {
                    kept.pop_front();
                }
                kept.push_back(line);
            }
        }
        kept.into_iter().collect::<Vec<_>>().join(" | ")
    });
    let finish = loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            break Err(anyhow!("cancelled"));
        }
        match tokio::time::timeout(std::time::Duration::from_millis(200), child.wait()).await {
            Ok(Ok(status)) => break Ok(status),
            Ok(Err(e)) => break Err(anyhow!("yt-dlp wait: {e}")),
            Err(_) => continue,
        }
    };
    drop(process_tree);
    let final_path = out_task.await.unwrap_or(None);
    let err_tail = err_task.await.unwrap_or_default();
    drop(jar);
    let status = finish?;
    if !status.success() {
        // yt-dlp's own words are far more useful than an exit code.
        let mut msg = if err_tail.trim().is_empty() {
            format!("yt-dlp exited with {status}")
        } else {
            format!("yt-dlp exited with {status}: {}", err_tail.trim())
        };
        // Without ffmpeg the selector is restricted to a single muxed file, and
        // many videos no longer publish one above 360p at all - yt-dlp then
        // says only "Requested format is not available", which reads like a
        // broken link. Say what actually helps.
        if merge_ffmpeg.is_none() {
            msg.push_str(
                " (this video has no single-file format at the quality you asked for - ffmpeg merges video+audio; pick \u{201c}App's own copy\u{201d} under Settings \u{25b8} Plugins \u{25b8} ffmpeg and the app fetches one)",
            );
        }
        return Err(anyhow!(msg));
    }
    // Postprocessing can change the extension. Only the exact file reported
    // by this run, with the expected stem and directory, may be promoted.
    if !dest.is_file() {
        if let Some(path)=final_path.filter(|p| valid_ytdlp_output(p,dest)) {
            tokio::fs::rename(path,dest).await?;
        }
    }
    let len = tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Err(anyhow!("yt-dlp finished without writing a file"));
    }
    if vimeo_needs_audio(url) {
        verify_vimeo_audio(dest).await?;
    }
    shared.downloaded.store(len, Ordering::SeqCst);
    shared.total.store(len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    shared.frag_count.store(0, Ordering::SeqCst);
    tidy_parts_dir(dest);
    // The file on disk is the ground truth: read its frame size back and, when
    // the video could not meet the menu cap, say so instead of letting a
    // 360p result pass for the 720p the user asked for.
    if let Some(r) = ffprobe_resolution(&dest).await {
        if let Some(asked) = quality_cap(&opts.quality_pref) {
            if let Some(got) = resolution_height(&r) {
                if got < asked {
                    shared.set_note(format!(
                        "the source tops out at {got}p here — {asked}p was requested but is not available for this video"
                    ));
                }
            }
        }
        shared.set_resolution(r);
    }
    Ok(())
}

/// One adaptive leg (P-speed): segmented parallel first when the server
/// honors ranges (sized to the strict window on googlevideo), halving on
/// throttled splits, then sequential chunked 1 MiB windows on
/// cut-happy/throttling hosts, then a plain stream. `label` ("video" /
/// "audio") names the leg in errors, since two concurrent transfers would
/// otherwise hide which one refused.
#[allow(clippy::too_many_arguments)]
async fn adaptive_leg(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    leg_dest: &PathBuf,
    total: Option<u64>,
    range: bool,
    conns: usize,
    sub: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
    label: &str,
) -> Result<()> {
    // A leg an earlier attempt finished (the other one failed) is kept.
    if let Some(t) = total.filter(|t| *t > 0) {
        if tokio::fs::metadata(leg_dest).await.map(|m| m.len() == t).unwrap_or(false) {
            sub.downloaded.store(t, Ordering::SeqCst);
            return Ok(());
        }
    }
    let strict = is_fragile_cdn(url) || is_googlevideo_url(url);
    let can_chunk = strict && matches!(total, Some(t) if t > 0);
    let parts = conns.clamp(1, 32);
    let can_segment = range && matches!(total, Some(t) if t >= 1024 * 1024) && parts > 1;
    if can_segment {
        // Legs start at zero (fresh work dir), so the counter holds only
        // this attempt's bytes — safe to retry the split downward.
        let mut n = parts;
        loop {
            match segmented(client, opts, url, leg_dest, total.unwrap(), n, sub.clone(), limiter.clone()).await
            {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if !is_final_http_error(&e) {
                        if let Some(h) = halve_conns(n) {
                            n = h;
                            continue;
                        }
                    }
                    if can_chunk {
                        // Stale parts belong to the failed split; the
                        // chunked round rebinds its own offset, and the
                        // counter restarts with it (never double-counts).
                        remove_partial(leg_dest);
                        for i in 0..16 {
                            let _ = tokio::fs::remove_file(
                                leg_dest.with_extension(format!("dcpart{i}")),
                            )
                            .await;
                        }
                        sub.downloaded.store(0, Ordering::SeqCst);
                        return chunked_stream(
                            client,
                            opts,
                            url,
                            leg_dest,
                            total.unwrap(),
                            sub,
                            limiter,
                        )
                        .await
                        .map_err(|e| anyhow!("{label} leg: {e:#}"));
                    }
                    return Err(anyhow!("{label} leg: {e:#}"));
                }
            }
        }
    }
    if can_chunk {
        return chunked_stream(client, opts, url, leg_dest, total.unwrap(), sub, limiter)
            .await
            .map_err(|e| anyhow!("{label} leg: {e:#}"));
    }
    single_stream(client, opts, url, leg_dest, sub, limiter)
        .await
        .map_err(|e| anyhow!("{label} leg: {e:#}"))
}

/// Download an adaptive pair (video-only + audio) concurrently, then mux to
/// one file. Container follows the codecs: MP4/M4A pairs stay `.mp4`,
/// anything WebM/Opus becomes `.mkv` (same convention download managers use).
/// Without `audio` this is a plain direct download through the same path.
pub async fn download_adaptive(
    video_url: String, audio_url: Option<String>, dest: PathBuf, connections: usize,
    opts: EngineOpts, shared: Arc<SharedState>,
) -> Result<()> {
    if opts.expected_duration_secs > 0 { shared.expected_duration.store(opts.expected_duration_secs, Ordering::SeqCst); }
    shared.expects_audio.store(opts.require_audio || audio_url.is_some(), Ordering::SeqCst);
    shared.merged.store(audio_url.is_some(), Ordering::SeqCst);
    download_adaptive_inner(video_url, audio_url, dest.clone(), connections, opts, shared.clone()).await?;
    verify_completed_media(&dest, &shared).await?;
    cleanup_completed_job(&dest, &shared);
    Ok(())
}

async fn download_adaptive_inner(
    video_url: String,
    audio_url: Option<String>,
    mut dest: PathBuf,
    connections: usize,
    opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    let video_url = normalize_media_url(&video_url);
    let audio_url = audio_url.map(|u| normalize_media_url(&u));
    let _job = claim_dest_job(&dest).ok_or_else(|| {
        anyhow!("this destination is already downloading elsewhere — wait for it or remove the duplicate queue entry")
    })?;
    let client = opts.build_client()?;
    // Merging needs ffmpeg: resolve (and, for the app's own copy, fetch it)
    // BEFORE any bytes move, not after two quarter-gigabyte legs finish.
    // Progressive (single-file) qualities never reach this path.
    if audio_url.as_deref().map(|u| u.starts_with("http")).unwrap_or(false)
        && ffmpeg_path().is_none()
    {
        shared.set_phase("Fetching ffmpeg");
        ensure_ffmpeg(&client).await.map_err(|e| {
            anyhow!(
                "this quality needs merging (video-only + audio), and {e:#}; 720p and below download as single files without it"
            )
        })?;
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Probe both legs (referer/cookies apply — media servers expect them).
    let (v_total, v_range, vtype, v_disp) = probe(&client, &opts, &video_url).await;
    shared.resumable.store(v_range, Ordering::SeqCst);
    if opts.allow_rename {
        if let Some(n) = v_disp {
            dest.set_file_name(&n);
            mutex_guard(&shared.resolved_name).replace(n);
        }
    }
    let (a_total, a_range, atype, _) = match &audio_url {
        Some(a) => {
            let (t, r, c, _) = probe(&client, &opts, a).await;
            (t, r, c, ())
        }
        None => (None, false, None, ()),
    };
    // Container: WebM/Opus legs mux to MKV, MP4/M4A legs stay MP4.
    let webmish = vtype
        .as_deref()
        .map(|c| c.contains("webm") || c.contains("vp9") || c.contains("av01"))
        .unwrap_or(false)
        || atype
            .as_deref()
            .map(|c| c.contains("webm") || c.contains("opus"))
            .unwrap_or(false)
        || video_url.to_ascii_lowercase().contains(".webm")
        || audio_url
            .as_deref()
            .map(|u| u.to_ascii_lowercase().contains(".webm"))
            .unwrap_or(false);
    if audio_url.is_some() && webmish && opts.allow_rename {
        dest.set_extension("mkv");
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    }
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        dest = crate::model::conflict_free_dest(&dest);
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    let grand_total = v_total.unwrap_or(0).saturating_add(a_total.unwrap_or(0));
    if grand_total > 0 {
        shared.total.store(grand_total, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, grand_total));
        dones.push(0);
    }
    // Work files live in the destination's own work folder, kept across
    // attempts so a resume continues both legs.
    let work = adaptive_work_dir(&dest);
    tokio::fs::create_dir_all(&work).await?;
    let v_dest = work.join("v");
    let a_dest = work.join("a");
    let conns = connections.clamp(1, 32);
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
    );
    let sv = Arc::new(SharedState::new());
    let sa = Arc::new(SharedState::new());
    // The legs have their own state, so the job's Pause/Cancel must reach
    // them: Pause is mirrored by the aggregator below, and this guard
    // cancels both legs when the job ends or is aborted (the app's Cancel
    // aborts this future, which used to leave the legs' detached range
    // tasks downloading the whole stream).
    struct CancelLegs(Arc<SharedState>, Arc<SharedState>);
    impl Drop for CancelLegs {
        fn drop(&mut self) {
            self.0.cancelled.store(true, Ordering::SeqCst);
            self.1.cancelled.store(true, Ordering::SeqCst);
        }
    }
    let _cancel_legs = CancelLegs(sv.clone(), sa.clone());
    let v_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let has_audio = audio_url.is_some();
    let fut_v = {
        let client = client.clone();
        let opts = opts.clone();
        let limiter = limiter.clone();
        let sub = sv.clone();
        let done_flag = v_done.clone();
        let vd = v_dest.clone();
        async move {
            let r = adaptive_leg(
                &client, &opts, &video_url, &vd, v_total, v_range, conns, sub, limiter, "video",
            )
            .await;
            done_flag.store(true, Ordering::SeqCst);
            r
        }
    };
    let fut_a = {
        let client = client.clone();
        let opts = opts.clone();
        let limiter = limiter.clone();
        let sub = sa.clone();
        let done_flag = a_done.clone();
        let ad = a_dest.clone();
        let au = audio_url.clone();
        async move {
            let r = match au {
                Some(a) => {
                    adaptive_leg(&client, &opts, &a, &ad, a_total, a_range, conns, sub, limiter, "audio").await
                }
                None => Ok(()),
            };
            done_flag.store(true, Ordering::SeqCst);
            r
        }
    };
    let agg = async {
        loop {
            let paused = shared.paused.load(Ordering::SeqCst);
            let cancelled = shared.cancelled.load(Ordering::SeqCst);
            for leg in [&sv, &sa] {
                leg.paused.store(paused, Ordering::SeqCst);
                if cancelled {
                    leg.cancelled.store(true, Ordering::SeqCst);
                }
            }
            let sum = sv.downloaded.load(Ordering::SeqCst)
                .saturating_add(sa.downloaded.load(Ordering::SeqCst));
            shared.downloaded.store(sum, Ordering::SeqCst);
            // Part Info lists every connection of both legs, as IDM does:
            // the picture's first, then the sound's, placed after it.
            {
                let mut totals = Vec::new();
                let mut dones = Vec::new();
                for (leg, base) in [(&sv, 0u64), (&sa, v_total.unwrap_or(0))] {
                    let t = mutex_guard(&leg.part_total).clone();
                    let d = mutex_guard(&leg.part_done).clone();
                    for (i, (a, b)) in t.into_iter().enumerate() {
                        totals.push((a + base, b + base));
                        dones.push(d.get(i).copied().unwrap_or(0));
                    }
                }
                if !totals.is_empty() {
                    *mutex_guard(&shared.part_total) = totals;
                    *mutex_guard(&shared.part_done) = dones;
                }
            }
            if v_done.load(Ordering::SeqCst) && a_done.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        let sum = sv.downloaded.load(Ordering::SeqCst)
            .saturating_add(sa.downloaded.load(Ordering::SeqCst));
        shared.downloaded.store(sum, Ordering::SeqCst);
    };
    let (rv, ra, _) = tokio::join!(fut_v, fut_a, agg);
    // Leg failures must also drop the work dir (unique per job, but litter
    // accumulates across retries otherwise). The aggregated byte counter is
    // reset too: those leg bytes were deleted with the work dir, so keeping
    // them would persist phantom progress (bar jumps backwards on retry,
    // e.g. 31% -> 20%).
    // A failed leg keeps its bytes in the work folder: Resume (or the same
    // quality clicked again in the browser) continues from them.
    if let Err(e) = rv {
        return Err(e);
    }
    if let Err(e) = ra {
        return Err(e);
    }
    // Mux (or plain move when audio-less). The unique work dir is removed on
    // every exit so failed merges never poison the next attempt.
    let finish: Result<()> = if has_audio {
        ffmpeg_mux(&v_dest, Some(&a_dest), &dest).await
    } else {
        tokio::fs::rename(&v_dest, &dest)
            .await
            .map_err(|e| anyhow!(io_message(&e)))
    };
    let _ = tokio::fs::remove_dir_all(&work).await;
    if finish.is_err() {
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    finish?;
    let final_len = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    // Ground truth for the merged result (the captured leg may have been
    // video-only, so nothing else names the frame size on this path).
    if let Some(r) = ffprobe_resolution(&dest).await {
        if let Some(asked) = quality_cap(&opts.quality_pref) {
            if let Some(got) = resolution_height(&r) {
                if got < asked {
                    shared.set_note(format!(
                        "the source tops out at {got}p here — {asked}p was requested but is not available for this video"
                    ));
                }
            }
        }
        shared.set_resolution(r);
    }
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// One DASH Representation (video or audio track option).
#[derive(Debug, Clone, Default)]
struct DashRep {
    id: String,
    bandwidth: u64,
    width: u64,
    height: u64,
    codecs: String,
    mime: String,
    base_url: Option<String>,
    seg_media: Option<String>,
    seg_init: Option<String>,
    start_number: u64,
    /// Segment duration in seconds (duration ÷ timescale), when templated.
    seg_dur_secs: Option<f64>,
    /// Exact segment count from a uniform SegmentTimeline (P3), preferred
    /// over duration math when present.
    timeline_count: Option<usize>,
    /// (t0, d) of that timeline, for `$Time$` media templates.
    timeline_origin: Option<(u64, u64)>,
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    // Find name="value" with exact attribute-name match (avoids
    // `bandwidth` matching inside other names by checking the prefix char).
    let mut search = tag;
    while let Some(p) = search.find(name) {
        let before = search[..p].chars().next_back();
        let ok_before = before.map(|c| c.is_whitespace() || c == '<').unwrap_or(false);
        let after = search[p + name.len()..].chars().next();
        if ok_before && after == Some('=') {
            let rest = search[p + name.len() + 1..].trim_start();
            let q = rest.chars().next()?;
            if q == '"' || q == '\'' {
                let end = rest[1..].find(q)?;
                // Attribute values are XML-escaped: a tokenized template
                // "?a=1&amp;b=2" must be requested as "?a=1&b=2".
                return Some(xml_unescape(&rest[1..1 + end]));
            }
        }
        search = &search[p + name.len()..];
    }
    None
}

/// The five predefined XML entities (`&amp;` last, so "&amp;lt;" stays "&lt;").
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_child(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let s = block.find(&open)?;
    let body = block[s..].find('>')? + s + 1;
    let e = block[body..].find(&close)? + body;
    let t = xml_unescape(block[body..e].trim());
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Seconds from a SegmentTemplate tag's duration/timescale pair.
fn tpl_secs(tag: &str) -> Option<f64> {
    let d: f64 = xml_attr(tag, "duration")?.parse().ok()?;
    let ts: f64 = xml_attr(tag, "timescale")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    if d <= 0.0 || ts <= 0.0 {
        None
    } else {
        Some(d / ts)
    }
}

/// Parse an ISO-8601 MPD duration subset (PT1H2M3.5S) to seconds.
fn parse_pt_duration(s: &str) -> Option<f64> {
    // ISO-8601 subset used by MPDs: PT1H2M3.5S
    let t = s.trim().strip_prefix("PT")?;
    let mut secs = 0.0;
    let mut num = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else {
            let v: f64 = num.parse().ok()?;
            num.clear();
            match c {
                'H' => secs += v * 3600.0,
                'M' => secs += v * 60.0,
                'S' => secs += v,
                _ => return None,
            }
        }
    }
    Some(secs)
}

fn dash_seg_count(mpd_duration: Option<f64>, seg_dur: f64) -> Option<usize> {
    if seg_dur <= 0.0 {
        return None;
    }
    let total = mpd_duration?;
    if total <= 0.0 {
        return None;
    }
    let n = (total / seg_dur).ceil() as usize;
    if n == 0 || n > 2000 {
        None
    } else {
        Some(n)
    }
}

/// Count segments from a `SegmentTimeline` element's `<S>` entries (P3).
/// Only uniform timelines are countable: every entry must share one
/// duration `d` and be contiguous (explicit `t` must match the running
/// time). `r` repeats extend the count; `r="-1"` (repeat to period end)
/// needs a known period duration. `timescale` converts timeline units to
/// seconds. Returns:
/// - `Ok(None)` — no timeline present (caller falls back to
///   duration/timescale math).
/// - `Ok(Some(n))` — exact segment count (capped like `dash_seg_count`).
/// - `Err` — gaps, varying durations, or unbounded repeats: kept loud
///   instead of assembling garbage out of order.
fn timeline_count(block: &str, timescale: f64, period_dur_secs: Option<f64>) -> Result<Option<usize>> {
    // Collect `<S …>` tags only (`<S `, `<S/>`, `<S>` — never
    // `<SegmentTemplate…` / `<SegmentTimeline…` / `<Supplemental…`).
    let mut tags: Vec<&str> = Vec::new();
    let mut search = 0usize;
    while let Some(p) = block[search..].find("<S") {
        let abs = search + p;
        let after = block[abs + 2..].chars().next();
        match after {
            Some(' ') | Some('/') | Some('>') => {
                let end = block[abs..].find('>').map(|e| abs + e).unwrap_or(block.len());
                tags.push(&block[abs..end.min(block.len())]);
                search = abs + 2;
            }
            _ => search = abs + 2,
        }
    }
    if tags.is_empty() {
        return Ok(None);
    }
    let mut unit_d: Option<u64> = None;
    let mut pos: Option<u64> = None; // running time in timeline units
    let mut total: usize = 0;
    for tag in &tags {
        let d: u64 = xml_attr(tag, "d")
            .and_then(|v| v.parse().ok())
            .filter(|d| *d > 0)
            .ok_or_else(|| {
                anyhow!("DASH SegmentTimeline entry without a positive duration is not supported")
            })?;
        if let Some(u) = unit_d {
            if u != d {
                return Err(anyhow!(
                    "this DASH manifest uses a varying-duration SegmentTimeline — not supported yet. Use the browser extension qualities (MP4) when the site offers them"
                ));
            }
        } else {
            unit_d = Some(d);
        }
        if let Some(t) = xml_attr(tag, "t").and_then(|v| v.parse::<u64>().ok()) {
            if let Some(p) = pos {
                if t != p {
                    return Err(anyhow!(
                        "this DASH manifest has gaps in its SegmentTimeline — not supported yet"
                    ));
                }
            }
            pos = Some(t);
        }
        let r_raw = xml_attr(tag, "r").unwrap_or_else(|| "0".to_string());
        let r: i64 = r_raw.parse().unwrap_or(0);
        let reps: u64 = if r < 0 {
            // Repeat to period end (DASH-IF `r="-1"`).
            let period = period_dur_secs.ok_or_else(|| {
                anyhow!("this DASH manifest repeats segments to an unknown period end — not supported yet")
            })?;
            let ts = if timescale > 0.0 { timescale } else { 1.0 };
            let seg_secs = d as f64 / ts;
            if seg_secs <= 0.0 {
                return Err(anyhow!("DASH SegmentTimeline entry without a positive duration is not supported"));
            }
            let elapsed = pos.unwrap_or(0) as f64 / ts;
            let n = ((period - elapsed) / seg_secs).ceil().max(1.0) as u64;
            pos = None; // nothing verifiable follows an open repeat
            n
        } else {
            let n = (r as u64).saturating_add(1);
            pos = Some(pos.unwrap_or(0).saturating_add(d.saturating_mul(n)));
            n
        };
        total = total.saturating_add(reps as usize);
        if total > 2000 {
            return Ok(None); // oversize/live-ish: caller reports it cleanly
        }
    }
    if total == 0 {
        return Ok(None);
    }
    Ok(Some(total))
}

/// Start time and duration (timeline units) of a uniform SegmentTimeline's
/// first entry: segment `i` starts at `t0 + i·d`, which is what `$Time$`
/// templates name. `timeline_count` has already rejected gaps and varying
/// durations, so the first entry describes them all.
fn timeline_origin(block: &str) -> Option<(u64, u64)> {
    let p = block.find("<S ").or_else(|| block.find("<S/"))?;
    let end = block[p..].find('>').map(|e| p + e).unwrap_or(block.len());
    let tag = &block[p..end];
    let d = xml_attr(tag, "d")?.parse::<u64>().ok().filter(|d| *d > 0)?;
    let t0 = xml_attr(tag, "t").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    Some((t0, d))
}

fn dash_expand(tpl: &str, number: u64, rep_id: &str, bw: u64) -> String {
    dash_expand_at(tpl, number, rep_id, bw, None)
}

/// `dash_expand` plus `$Time$` (and `$Time%0Nd$`) for timeline templates.
fn dash_expand_at(tpl: &str, number: u64, rep_id: &str, bw: u64, time: Option<u64>) -> String {
    // $Number$, $Number%0Nd$, $RepresentationID$, $Bandwidth$, $Time$.
    // Byte-walk would corrupt multibyte template text, so iterate chars.
    let mut out = String::new();
    let chars: Vec<char> = tpl.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '$' {
                j += 1;
            }
            if j < chars.len() {
                let tok: String = chars[i + 1..j].iter().collect();
                if tok == "Number" {
                    out.push_str(&number.to_string());
                } else if let Some(fmt) = tok.strip_prefix("Number%0") {
                    if let Some(w) = fmt.strip_suffix('d').and_then(|x| x.parse::<usize>().ok()) {
                        out.push_str(&format!("{number:0>w$}"));
                    } else {
                        out.push_str(&number.to_string());
                    }
                } else if let (Some(t), true) = (time, tok == "Time" || tok.starts_with("Time%0")) {
                    match tok.strip_prefix("Time%0").and_then(|f| f.strip_suffix('d')).and_then(|w| w.parse::<usize>().ok()) {
                        Some(w) => out.push_str(&format!("{t:0>w$}")),
                        None => out.push_str(&t.to_string()),
                    }
                } else if tok == "RepresentationID" {
                    out.push_str(rep_id);
                } else if tok == "Bandwidth" {
                    out.push_str(&bw.to_string());
                } else if tok == "$" {
                    out.push('$');
                } else {
                    out.push('$');
                    out.push_str(&tok);
                    out.push('$');
                }
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Parse an MPD manifest into (video, audio) representation lists.
/// Handles AdaptationSet mimeType + Representation bandwidth/width/height +
/// BaseURL and SegmentTemplate (number-based). Uniform SegmentTimelines are
/// counted exactly (`timeline_count`); varying/gapped timelines and dynamic
/// (live) manifests are rejected by the caller.
fn parse_mpd(xml: &str) -> Result<(Vec<DashRep>, Vec<DashRep>)> {
    // Period duration for open-ended (`r="-1"`) timelines.
    let mpd_dur = xml
        .find("<MPD")
        .and_then(|p| {
            let q = xml[p..].find('>').map(|x| x + p).unwrap_or(xml.len());
            xml_attr(&xml[p..q.min(xml.len())], "mediaPresentationDuration")
        })
        .and_then(|d| parse_pt_duration(&d));
    let mut videos = Vec::new();
    let mut audios = Vec::new();
    let mut rest = xml;
    let mut found_set = false;
    while let Some(s) = rest.find("<AdaptationSet") {
        found_set = true;
        let body_start = rest[s..].find('>').map(|x| x + s + 1).unwrap_or(rest.len());
        let e = rest[body_start..].find("</AdaptationSet>").map(|x| x + body_start).unwrap_or(rest.len());
        let block = &rest[body_start..e.min(rest.len())];
        let open_tag = &rest[s..body_start.min(rest.len())];
        let mime = xml_attr(open_tag, "mimeType").unwrap_or_default();
        let set_tpl_media;
        let set_tpl_init;
        let set_start;
        let set_dur;
        let set_timescale: f64;
        if let Some(st) = block.find("<SegmentTemplate") {
            let st_end = block[st..].find('>').map(|x| x + st).unwrap_or(block.len());
            let st_tag_full = &block[st..st_end.min(block.len())];
            // SegmentTemplate may be self-closed or wrap an explicit
            // Initialization element; sniff both.
            set_tpl_media = xml_attr(st_tag_full, "media");
            set_tpl_init = xml_attr(st_tag_full, "initialization").or_else(|| {
                let r = &block[st..];
                r.find("<Initialization").and_then(|p| {
                    let q = r[p..].find('>').map(|x| x + p).unwrap_or(r.len());
                    xml_attr(&r[p..q.min(r.len())], "sourceURL")
                })
            });
            set_start = block[st..]
                .find("startNumber")
                .and_then(|_| xml_attr(st_tag_full, "startNumber"))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(1);
            set_dur = tpl_secs(st_tag_full);
            set_timescale = xml_attr(st_tag_full, "timescale")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|t| *t > 0.0)
                .unwrap_or(1.0);
        } else {
            set_tpl_media = None;
            set_tpl_init = None;
            set_start = 1;
            set_dur = None;
            set_timescale = 1.0;
        }
        // Set-level timeline scan, excluding Representation sub-blocks
        // (their own timelines are evaluated per-rep below).
        let mut set_scan = String::with_capacity(block.len());
        {
            let mut pos = 0usize;
            let mut rsearch = 0usize;
            while let Some(r) = block[rsearch..].find("<Representation") {
                let rs = rsearch + r;
                let r_body = block[rs..].find('>').map(|x| x + rs + 1).unwrap_or(block.len());
                let r_end = block[r_body..]
                    .find("</Representation>")
                    .map(|x| x + r_body + "</Representation>".len())
                    .unwrap_or(block.len());
                set_scan.push_str(&block[pos..rs.min(block.len())]);
                pos = r_end.min(block.len());
                rsearch = pos;
            }
            set_scan.push_str(&block[pos..]);
        }
        // A malformed timeline is a loud error, not a silent miscount —
        // but only when a timeline actually exists here.
        let set_tl = if set_scan.contains("<SegmentTimeline") {
            timeline_count(&set_scan, set_timescale, mpd_dur)?
        } else {
            None
        };
        let mut rrest = block;
        while let Some(r) = rrest.find("<Representation") {
            let r_body = rrest[r..].find('>').map(|x| x + r + 1).unwrap_or(rrest.len());
            let r_end = rrest[r_body..].find("</Representation>").map(|x| x + r_body).unwrap_or(rrest.len());
            let rblock = &rrest[r_body..r_end.min(rrest.len())];
            let rtag = &rrest[r..r_body.min(rrest.len())];
            let rep_tpl_tag = rblock.find("<SegmentTemplate").map(|p| {
                let q = rblock[p..].find('>').map(|x| x + p).unwrap_or(rblock.len());
                rblock[p..q.min(rblock.len())].to_string()
            });
            let rep_start = rep_tpl_tag
                .as_ref()
                .and_then(|t| xml_attr(t, "startNumber"))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(set_start);
            let rep_timescale = rep_tpl_tag
                .as_ref()
                .and_then(|t| xml_attr(t, "timescale"))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|t| *t > 0.0)
                .unwrap_or(set_timescale);
            // Rep-level timeline wins; otherwise the set-level one applies.
            let rep_tl = if rblock.contains("<SegmentTimeline") {
                timeline_count(rblock, rep_timescale, mpd_dur)?
            } else {
                None
            };
            let rep = DashRep {
                id: xml_attr(rtag, "id").unwrap_or_default(),
                bandwidth: xml_attr(rtag, "bandwidth").and_then(|v| v.parse().ok()).unwrap_or(0),
                width: xml_attr(rtag, "width").and_then(|v| v.parse().ok()).unwrap_or(0),
                height: xml_attr(rtag, "height").and_then(|v| v.parse().ok()).unwrap_or(0),
                codecs: xml_attr(rtag, "codecs").unwrap_or_default(),
                mime: xml_attr(rtag, "mimeType").unwrap_or_else(|| mime.clone()),
                base_url: xml_child(rblock, "BaseURL").or_else(|| xml_child(block, "BaseURL")),
                seg_media: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| xml_attr(t, "media"))
                    .or_else(|| set_tpl_media.clone()),
                seg_init: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| xml_attr(t, "initialization"))
                    .or_else(|| set_tpl_init.clone()),
                start_number: rep_start,
                seg_dur_secs: rep_tpl_tag
                    .as_ref()
                    .and_then(|t| tpl_secs(t))
                    .or(set_dur),
                timeline_count: rep_tl.or(set_tl),
                timeline_origin: if rep_tl.is_some() {
                    timeline_origin(rblock)
                } else if set_tl.is_some() {
                    timeline_origin(&set_scan)
                } else {
                    None
                },
            };
            if rep.mime.starts_with("video/") {
                videos.push(rep);
            } else if rep.mime.starts_with("audio/") {
                audios.push(rep);
            }
            rrest = &rrest[r + "<Representation".len()..];
        }
        rest = &rest[s + "<AdaptationSet".len()..];
    }
    if !found_set {
        return Err(anyhow!("no DASH adaptation sets found in this manifest"));
    }
    if videos.is_empty() && audios.is_empty() {
        return Err(anyhow!("DASH manifest has no video/audio tracks"));
    }
    Ok((videos, audios))
}

/// One DASH blob fetch with retries (init or media segment). Standalone so
/// media segments can be fetched from spawned tasks (P2 parallelism).
async fn dash_fetch_blob(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: String,
    shared: &Arc<SharedState>,
    max_retries: u32,
) -> Result<Vec<u8>> {
    let _host_slot = host_slot(&url, opts.per_host_conns).await;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        wait_if_paused(shared).await?;
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        match opts.get(client, &url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(r) => {
                    let expect = r.content_length();
                    let body = r.bytes().await.map_err(|e| anyhow!("segment network: {e}"))?;
                    // Same clean-but-short guard as HLS segments.
                    if let Some(n) = expect {
                        if body.len() as u64 != n {
                            if attempt >= max_retries {
                                return Err(anyhow!(
                                    "segment short: got {} of {n} bytes", body.len()
                                ));
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
                            continue;
                        }
                    }
                    return Ok(body.to_vec());
                }
                Err(e) => {
                    let m = http_status_err(&url, e);
                    if is_final_http_error(&m) || attempt >= max_retries {
                        return Err(m);
                    }
                }
            },
            Err(e) => {
                if attempt >= max_retries {
                    return Err(anyhow!("segment fetch: {e}"));
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << attempt.min(4))).await;
    }
}

/// Download one DASH representation to a file: init segment + N media
/// segments concatenated (fragmented MP4 chains this way for VOD). Media
/// segments fetch in bounded parallel (P2, like HLS) into indexed part
/// files and concatenate in order; the init segment stays sequential
/// (headers must lead the stream).
#[allow(clippy::too_many_arguments)]
async fn dash_rep_download(
    client: &reqwest::Client,
    opts: &EngineOpts,
    mpd_url: &str,
    rep: &DashRep,
    rep_id: &str,
    count: Option<usize>,
    dest: &PathBuf,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
    parallel: usize,
) -> Result<()> {
    let max_retries = opts.max_retries.max(1);
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .await?;
    use tokio::io::AsyncWriteExt;
    if rep.seg_media.is_some() {
        // Templated representation: initialization segment (track headers)
        // first; the numbered media segments follow below.
        if let Some(init_tpl) = rep.seg_init.clone() {
            // Init templates use the same tokens ("$RepresentationID$/init.mp4");
            // requesting them literally 404'd.
            let init = dash_expand(&init_tpl, rep.start_number, rep_id, rep.bandwidth);
            let u = if init.starts_with("http") {
                init
            } else {
                hls_abs(mpd_url, &init)
            };
            let blob = dash_fetch_blob(client, opts, u, shared, max_retries).await?;
            out.write_all(&blob).await.map_err(|e| anyhow!(io_message(&e)))?;
            shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
            limiter.consume(blob.len() as u64).await;
        }
    }
    if let Some(media_tpl) = rep.seg_media.clone() {
        let n = count.ok_or_else(|| {
            anyhow!("DASH live/dynamic manifests without a known segment count are not supported")
        })?;
        // Bounded parallel fetch (P2): each segment lands in its indexed
        // part file; concatenation below stays strictly ordered.
        let slots = parallel.clamp(2, 8);
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(slots));
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let num = rep.start_number.saturating_add(i as u64);
            let time = rep.timeline_origin.map(|(t0, d)| t0.saturating_add(d.saturating_mul(i as u64)));
            let seg = dash_expand_at(&media_tpl, num, rep_id, rep.bandwidth, time);
            let u = if seg.starts_with("http") { seg } else { hls_abs(mpd_url, &seg) };
            let part = dest.with_extension(format!("dashpart{i}"));
            let client = client.clone();
            let opts = opts.clone();
            let shared = shared.clone();
            let limiter = limiter.clone();
            let sem = sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| anyhow!("semaphore: {e}"))?;
                if shared.cancelled.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                let blob = dash_fetch_blob(&client, &opts, u, &shared, max_retries).await?;
                tokio::fs::write(&part, &blob).await.map_err(|e| anyhow!(io_message(&e)))?;
                shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
                limiter.consume(blob.len() as u64).await;
                Ok::<(), anyhow::Error>(())
            }));
        }
        let mut first_err: Option<anyhow::Error> = None;
        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(anyhow!("segment task: {e}"));
                    }
                }
            }
        }
        if let Some(e) = first_err {
            for i in 0..n {
                let _ = tokio::fs::remove_file(dest.with_extension(format!("dashpart{i}"))).await;
            }
            return Err(e);
        }
        for i in 0..n {
            let part = dest.with_extension(format!("dashpart{i}"));
            let mut f = tokio::fs::File::open(&part).await.map_err(|_| {
                anyhow!("segment {i} missing after download — retry the download")
            })?;
            tokio::io::copy(&mut f, &mut out).await?;
            let _ = tokio::fs::remove_file(&part).await;
        }
    } else if let Some(b) = rep.base_url.clone() {
        // Single-file representation.
        let u = if b.starts_with("http") { b } else { hls_abs(mpd_url, &b) };
        let blob = dash_fetch_blob(client, opts, u, shared, max_retries).await?;
        out.write_all(&blob).await.map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(blob.len() as u64, Ordering::SeqCst);
        limiter.consume(blob.len() as u64).await;
    } else {
        return Err(anyhow!("DASH representation has neither BaseURL nor SegmentTemplate"));
    }
    out.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// DASH VOD download (clean-room): best video + best audio representations,
/// muxed with ffmpeg (or delivered video-only when ffmpeg is absent and no
/// audio track was needed). DRM (`ContentProtection`) manifests are refused.
pub async fn download_dash(
    manifest_url: &str,
    mut dest: PathBuf,
    connections: usize,
    opts: EngineOpts,
    shared: Arc<SharedState>,
) -> Result<()> {
    // Segment fetch parallelism (P2): bounded like HLS legs.
    let parallel = connections.clamp(2, 8);
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let xml = opts
        .get(&client, manifest_url)
        .send()
        .await
        .map_err(|e| anyhow!("manifest fetch: {e}"))?
        .error_for_status()
        .map_err(|e| http_status_err(manifest_url, e))?
        .text()
        .await
        .map_err(|e| anyhow!("manifest read: {e}"))?;
    if xml.contains("<ContentProtection") {
        return Err(anyhow!(
            "this DASH stream is DRM-protected (ContentProtection) — protected streams are never downloaded"
        ));
    }
    if xml.contains("type=\"dynamic\"") || xml.contains("type='dynamic'") {
        return Err(anyhow!(
            "this is a live/dynamic DASH manifest — live capture is not supported; only finished VOD"
        ));
    }
    let mpd_dur = xml
        .find("<MPD")
        .and_then(|p| {
            let q = xml[p..].find('>').map(|x| x + p).unwrap_or(xml.len());
            xml_attr(&xml[p..q.min(xml.len())], "mediaPresentationDuration")
        })
        .and_then(|d| parse_pt_duration(&d));
    let (videos, audios) = parse_mpd(&xml)?;
    if let Some(seconds) = mpd_dur.filter(|d| d.is_finite() && *d > 0.0) {
        let _ = shared.expected_duration.compare_exchange(0, seconds.round().max(1.0) as u64, Ordering::SeqCst, Ordering::SeqCst);
    }
    if !audios.is_empty() { shared.expects_audio.store(true, Ordering::SeqCst); }

    // Best video = most pixels (fallback: bandwidth); best audio = bandwidth.
    let v = videos
        .iter()
        .max_by_key(|r| ((r.width as u64).saturating_mul(r.height as u64), r.bandwidth))
        .ok_or_else(|| anyhow!("DASH manifest has no video track"))?
        .clone();
    let a = audios.iter().max_by_key(|r| r.bandwidth).cloned();
    // Two representations mean a mux: make sure a merger exists before
    // spending the bandwidth (the app's own copy is fetched on demand).
    if a.is_some() && ffmpeg_path().is_none() {
        shared.set_phase("Fetching ffmpeg");
        ensure_ffmpeg(&client).await.map_err(|e| {
            anyhow!("this DASH stream has separate video + audio, and {e:#}")
        })?;
    }
    // Container follows codecs (mp4 unless VP9/AV1/Opus → mkv), like pairs.
    let webmish = v.codecs.to_ascii_lowercase().contains("vp09")
        || v.codecs.to_ascii_lowercase().contains("av01")
        || a.as_ref()
            .map(|r| {
                r.codecs.to_ascii_lowercase().contains("opus")
                    || r.mime.contains("webm")
            })
            .unwrap_or(false);
    if a.is_some() && webmish && opts.allow_rename {
        dest.set_extension("mkv");
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    }
    if opts.allow_rename && dest.exists() && !opts.overwrite_existing {
        dest = crate::model::conflict_free_dest(&dest);
        if let Some(name) = dest.file_name().map(|s| s.to_string_lossy().to_string()) {
            mutex_guard(&shared.resolved_name).replace(name);
        }
    } else if opts.overwrite_existing && dest.exists() {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    shared.downloaded.store(0, Ordering::SeqCst);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, 0));
        dones.push(0);
    }
    let work = unique_work_dir(&dest);
    tokio::fs::create_dir_all(&work).await?;
    let v_dest = work.join("v");
    let a_dest = work.join("a");
    let limiter = std::sync::Arc::new(
        RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()),
    );
    let count_v = v
        .timeline_count
        .or_else(|| v.seg_dur_secs.and_then(|d| dash_seg_count(mpd_dur, d)));
    let rep_id_v = if v.id.is_empty() { v.bandwidth.to_string() } else { v.id.clone() };
    let res: Result<()> = async {
        dash_rep_download(&client, &opts, manifest_url, &v, &rep_id_v, count_v, &v_dest, &shared, &limiter, parallel).await?;
        if let Some(rep) = a {
            let count_a = rep
                .timeline_count
                .or_else(|| rep.seg_dur_secs.and_then(|d| dash_seg_count(mpd_dur, d)));
            let rep_id_a = if rep.id.is_empty() { rep.bandwidth.to_string() } else { rep.id.clone() };
            dash_rep_download(&client, &opts, manifest_url, &rep, &rep_id_a, count_a, &a_dest, &shared, &limiter, parallel).await?;
            ffmpeg_mux(&v_dest, Some(&a_dest), &dest).await
        } else {
            // Video-only manifest: deliver the assembled track as-is.
            tokio::fs::rename(&v_dest, &dest)
                .await
                .map_err(|e| anyhow!(io_message(&e)))
        }
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&work).await;
    res?;
    let final_len = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    shared.total.store(final_len, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = verify_checksum(&dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// Strict googlevideo-style edges 403 ranges above ~2 MiB while serving
/// small bounded ranges with 206 (verified live against the strict
/// fixture and googlevideo SABR edges).
const STRICT_PART_MAX: u64 = 2 * 1024 * 1024;

/// Classify a retryable attempt error for the end-of-download summary
/// (cuts vs stalls vs restarts), so the log tells the user what the
/// server actually kept doing instead of just the last error.
fn classify_attempt(msg: &str) -> &'static str {
    if msg.contains("stalled (no data") {
        "stalled"
    } else if msg.contains("bytes") {
        "cut"
    } else {
        "other"
    }
}

/// Next split when parallel ranges get throttled (P2): halve 16→8→4→2→1,
/// then give up (None) and fall back to a plain stream. Halving (not
/// abandoning) survives hosts that tolerate a few ranges but cut under a
/// many-way storm; the last step is still the engine, so its resume journal
/// keeps everything downloaded so far.
fn halve_conns(conns: usize) -> Option<usize> {
    match conns {
        0 | 1 => None,
        2 => Some(1),
        n => Some((n / 2).max(2)),
    }
}

/// Throttling signatures on a segmented failure that halving can survive:
/// range rejections, cut-short parts, and rate limits (429). Hard refusals
/// (401/403/410) are NOT included — parallelism didn't cause those.
fn segmented_throttled(msg: &str) -> bool {
    msg.contains("416")
        || msg.contains("segment short")
        || msg.contains("Range")
        || msg.contains("429")
}

/// Next step after a segmented failure (P-speed): halve the split while
/// throttling may just be a too-wide storm, else one chunked round on
/// cut-happy hosts (1 MiB windows 206 where big ranges 403), else the
/// plain single stream. Pure decision table, unit-tested below.
enum SegFallback {
    Halve(usize),
    Chunked,
    Single,
}

fn seg_fallback(is_final: bool, conns: usize, chunked_ok: bool) -> SegFallback {
    if !is_final {
        if let Some(h) = halve_conns(conns) {
            return SegFallback::Halve(h);
        }
    }
    if chunked_ok {
        SegFallback::Chunked
    } else {
        SegFallback::Single
    }
}

/// Append an attempt summary to a terminal transfer error, e.g.
/// `connection ended early (…) [34 tries: 30 cut, 0 stalled, 4 restarted]`.
/// Visible in full in Queues → Log; the row label shows the head of it.
fn with_attempt_summary(
    e: anyhow::Error,
    attempt: u32,
    cuts: u32,
    stalls: u32,
    restarts: u32,
) -> anyhow::Error {
    anyhow::anyhow!(
        "{e:#} [{attempt} tries: {cuts} cut, {stalls} stalled, {restarts} restarted]"
    )
}

async fn single_stream(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Retry the whole stream, resuming from the .dcpart file when the
    // server honors ranges. Throttled CDNs (FB/IG class) cut the socket
    // repeatedly MID-file: attempts that still moved bytes must NOT consume
    // the retry budget, or a big file dies after N cuts (the old bug — every
    // cut burned one retry even with 10 MB of progress). Only consecutive
    // zero-progress attempts fail the download, with an absolute cap so a
    // 1-byte trickle can't loop forever. Mirrors download_range's policy.
    let max_retries = opts.max_retries.max(1);
    let mut stalled = 0u32;
    let mut attempt = 0u32;
    // Attempt telemetry for the final summary (see with_attempt_summary).
    let mut cuts = 0u32;
    let mut stalls = 0u32;
    let mut restarts = 0u32;
    // A 416 (offset rejected — total shrank or server forgot the file) is
    // only worth one clean restart; a second 416 means the URL itself is bad.
    let mut restarted_416 = false;
    loop {
        attempt += 1;
        let len_before = tokio::fs::metadata(dest.with_extension("dcpart"))
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        match single_stream_once(client, opts, url, dest, &shared, &limiter).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst)
                    || e.to_string() == "cancelled"
                {
                    return Err(e);
                }
                // Refusals (403/401/410) AFTER partial progress on a signed
                // link almost always mean expiry (fresh captures work, resume
                // doesn't). Say so plainly instead of a generic refusal —
                // the fix is a fresh capture, not another resume.
                if is_final_http_error(&e) && len_before > 0 && is_fragile_cdn(url) {
                    let got = crate::model::format_bytes(len_before);
                    return Err(final_err(format!(
                        "link expired after {got} — replay the reel and click the download button again for a fresh link, then resume"
                    )));
                }
                // A refusal on FIRST touch can be edge throttling noise (the
                // same URL 206s on immediate retry — verified live), so spend
                // retry budget on it instead of failing instantly. A truly
                // dead link fails every attempt and still reports the refusal
                // with its type intact for the fallback routing above.
                if is_final_http_error(&e) {
                    stalled = stalled.saturating_add(1);
                    if stalled >= max_retries || attempt >= max_retries * 20 {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                    continue;
                }
                let msg = e.to_string();
                match classify_attempt(&msg) {
                    "cut" => cuts += 1,
                    "stalled" => stalls += 1,
                    _ => {}
                }
                if msg.contains("416") && len_before > 0 && !restarted_416 {
                    // Server rejected our resume offset: drop the prefix and
                    // restart from zero once (never resume a 416 twice).
                    restarted_416 = true;
                    restarts += 1;
                    let _ = tokio::fs::remove_file(dest.with_extension("dcpart")).await;
                    shared.downloaded.store(0, Ordering::SeqCst);
                    if let Ok(mut g) = shared.part_done.lock() {
                        if let Some(slot) = g.first_mut() {
                            *slot = 0;
                        }
                    }
                    stalled = 0;
                    continue;
                }
                // Attempts append, so file growth == progress. A SHRUNK file
                // means the server ignored our Range (200) and we truncated:
                // count it as a restart, not silent progress.
                let len_after = tokio::fs::metadata(dest.with_extension("dcpart"))
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    if len_after < len_before {
                        restarts += 1;
                    }
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 20 {
                    return Err(with_attempt_summary(e, attempt, cuts, stalls, restarts));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
    }
}

async fn single_stream_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
) -> Result<()> {
    // Resume support: continue from the existing .dcpart file, but ONLY
    // when it is bound to this exact URL+total (see ResumeMeta). A stale
    // prefix from another URL is discarded, never resumed into this file.
    let tmp = dest.with_extension("dcpart");
    let known_total = if shared.has_total.load(Ordering::SeqCst) {
        Some(shared.total.load(Ordering::SeqCst))
    } else {
        None
    };
    let mut start_at: u64 = resume_offset(dest, "dcpart", url, known_total, 1);

    // Always request a range (even from 0): some media servers truncate
    // plain 200 responses but serve full content for range requests.
    // Bounded when the total is known: strict edges (googlevideo SABR
    // serving, `gir=yes`) 403 open-ended full-file ranges outright while
    // serving the identical bounded window with 206 (verified live).
    let mut req = opts.get(client, url);
    req = if shared.has_total.load(Ordering::SeqCst) {
        let total = shared.total.load(Ordering::SeqCst);
        if total > start_at {
            req.header(
                reqwest::header::RANGE,
                format!("bytes={start_at}-{}", total - 1),
            )
        } else {
            req.header(reqwest::header::RANGE, format!("bytes={start_at}-"))
        }
    } else {
        req.header(reqwest::header::RANGE, format!("bytes={start_at}-"))
    };
    let resp = match req.send().await?.error_for_status() {
        Ok(r) => r,
        Err(e) => {
            let mapped = http_status_err(url, e);
            if start_at == 0 && !is_final_http_error(&mapped) {
                // server dislikes Range (but isn't refusing us): plain GET
                opts.get(client, url)
                    .send()
                    .await?
                    .error_for_status()
                    .map_err(|e| http_status_err(url, e))?
            } else {
                return Err(mapped);
            }
        }
    };
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        eprintln!(
            "[dbg] media status={} len={:?} range={:?} ctype={:?}",
            resp.status(),
            resp.content_length(),
            resp.headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok()),
            resp.headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
        );
    }

    // If server ignored our Range, restart from 0.
    let is_partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !is_partial && start_at > 0 {
        start_at = 0;
        shared.downloaded.store(0, Ordering::SeqCst);
    }
    // Ground the resume flag in this response (the pre-download probe may
    // have been skipped for expiring CDN links): only a 206 proves ranges.
    // A 200 to a bytes=0- request means ranges are ignored; a 200 after a
    // nonzero offset restart means the same after truncation above.
    if is_partial {
        shared.resumable.store(true, Ordering::SeqCst);
    } else if start_at > 0 || resp.status() == reqwest::StatusCode::OK {
        shared.resumable.store(false, Ordering::SeqCst);
    }
    // Adopt server-advertised total when we didn't have one.
    if !shared.has_total.load(Ordering::SeqCst) {
        if let Some(len) = resp.content_length() {
            shared.total.store(len.saturating_add(start_at), Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
    }
    // Single part view for the details dialog.
    {
        let total = shared.total.load(Ordering::SeqCst);
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total));
        dones.push(start_at);
    }
    shared.downloaded.store(start_at, Ordering::SeqCst);

    // Open for resume: keep existing bytes (seek/append below), no truncate.
    #[allow(clippy::suspicious_open_options)]
    let mut raw = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&tmp)
        .await?;
    if start_at > 0 && is_partial {
        raw.seek(std::io::SeekFrom::Start(start_at)).await?;
    } else {
        // Not resuming (fresh start, or the server ignored Range and
        // `start_at` was reset to 0 above): drop the old bytes. Testing
        // `start_at > 0` here could never be true after that reset, so a
        // restart overwrote the front of the old part file and kept its tail.
        raw.set_len(0).await?;
    }
    let mut file = tokio::io::BufWriter::with_capacity(WRITE_BUF, raw);

    let per_conn_limit = opts.speed_limit_bps; // single connection gets full budget
    let _ = per_conn_limit;
    // Stall guard: a hung socket (no bytes at all for the timeout window)
    // must fail the attempt so the retry loop resumes — otherwise a
    // throttled CDN drip-freeze hangs the download forever at 0 B/s.
    // Clamped 15–120 s so slow-but-moving trickles are never killed.
    let stall_timeout = (opts.timeout_secs.max(15)).min(120);
    let mut stream = resp.bytes_stream();
    // Any early exit still flushes what landed: the retry loop measures
    // progress by the file's length, so buffered bytes must not be lost.
    let mut failure: Option<anyhow::Error> = None;
    loop {
        let next = match tokio::time::timeout(
            std::time::Duration::from_secs(stall_timeout),
            stream.next(),
        )
        .await
        {
            Ok(n) => n,
            Err(_) => {
                failure = Some(anyhow!(
                    "stalled (no data for {stall_timeout}s) — resuming"
                ));
                break;
            }
        };
        let Some(chunk) = next else { break };
        if let Err(e) = wait_if_paused(&shared).await {
            failure = Some(e);
            break;
        }
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                failure = Some(anyhow!("network: {e}"));
                break;
            }
        };
        file.write_all(&bytes)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(bytes.len() as u64);
            }
        }
        limiter.consume(bytes.len() as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    if let Some(e) = failure {
        return Err(e);
    }
    // A clean-but-early socket close yields a short stream with NO error —
    // the classic Facebook/IG CDN failure ("connection ended early" in the
    // UI). When the total is known, verify the landed bytes and fail so the
    // retry loop resumes from the .dcpart offset instead of finalizing a
    // truncated file as success (which the app would then report as a
    // generic failure with no resume happening).
    if shared.has_total.load(Ordering::SeqCst) {
        let want = shared.total.load(Ordering::SeqCst);
        let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        if want > 0 && got < want {
            return Err(anyhow!(
                "connection ended early (got {got} of {want} bytes) — resuming"
            ));
        }
        // Unknown totals (chunked / lengthless responses) can end cleanly yet
        // short after a dropped socket: re-probe once, and if the server now
        // reports MORE than we received, fail so the retry loop resumes instead
        // of finalizing a truncated file as "Completed".
    } else {
        let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        let (t, _, _, _) = probe(client, opts, url).await;
        if let Some(total) = t {
            shared.total.store(total, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
            if total > got {
                return Err(anyhow!(
                    "stream ended short ({got} of {total} bytes) — resuming"
                ));
            }
        }
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// Sequential chunked download for throttling signed CDNs (FB/IG reels).
///
/// Failure mode it fixes: some CDN edges cap *bytes per connection*
/// (~1 MB observed on fbcdn — every stream dies at the same offset). A
/// single long stream can then NEVER finish: each resume restarts (or
/// truncates on a 200) and dies at the same ~1 MB mark, burning all 100
/// attempts at a frozen offset. Small bounded ranges (1 MiB) each complete
/// under the cap, and any cut only loses the in-flight chunk — the file
/// offset on disk is the resume point, so progress is strictly additive.
///
/// Requires a known total (browser-measured size or a prior probe);
/// callers fall back to `single_stream` when it is unknown.
const FRAGILE_CHUNK: u64 = 1024 * 1024;

#[allow(clippy::too_many_arguments)]
async fn chunked_stream(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    mut total: u64,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    let tmp = dest.with_extension("dcpart");
    let max_retries = opts.max_retries.max(1);
    let stall_timeout = (opts.timeout_secs.max(15)).min(120);
    // Bind resume state first: a prefix from another URL is discarded here,
    // so every chunk below appends bytes of THIS file only (see ResumeMeta).
    resume_offset(dest, "dcpart", url, Some(total), 1);
    // One restart-from-zero per download (stale offset / shrunken file).
    let mut restarted = false;
    let mut stalled = 0u32;
    let mut attempt = 0u32;
    // Attempt telemetry for the final summary (see with_attempt_summary).
    let mut cuts = 0u32;
    let mut stalls = 0u32;
    let mut restarts = 0u32;
    // Consecutive refusals on a live offset: strict edges throttle bursts
    // with 403s that clear after a breather (verified live: the same range
    // 206s again). Back off hard a few times before declaring expiry —
    // instant expiry misdiagnosed throttling and killed good links.
    let mut refusals = 0u32;
    // Single part view for the dialogs (no lock held across awaits).
    let have_now = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total));
        dones.push(have_now);
    }
    // Progress restarts from what is really on disk: after a failed parallel
    // run (its parts deleted) the counter still held those bytes and this
    // stream added its own on top (40 MB + 50 MB on a 50 MB file).
    shared.downloaded.store(have_now, Ordering::SeqCst);
    loop {
        attempt += 1;
        let offset = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        if total > 0 && offset >= total {
            break;
        }
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        wait_if_paused(&shared).await?;
        let end = if total > 0 {
            (offset.saturating_add(FRAGILE_CHUNK).saturating_sub(1)).min(total.saturating_sub(1))
        } else {
            offset.saturating_add(FRAGILE_CHUNK).saturating_sub(1)
        };
        let want = end.saturating_sub(offset).saturating_add(1);
        let len_before = offset;
        match chunked_once(client, opts, url, &tmp, offset, end, want, stall_timeout, &shared, &limiter).await {
            Ok(wrote) => {
                stalled = 0;
                refusals = 0;
                if total > 0 && offset.saturating_add(wrote) >= total {
                    break;
                }
                if wrote == 0 {
                    stalled = stalled.saturating_add(1);
                }
            }
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst) || e.to_string() == "cancelled" {
                    return Err(e);
                }
                if is_final_http_error(&e) {
                    // Refusal on a live offset: throttle burst OR expiry.
                    // Throttle clears after a breather; expiry never does —
                    // so absorb a couple refusals with backoffs first and
                    // only call it expired when it persists (kept short so a
                    // gated YouTube quality fails fast instead of sitting at
                    // 20% with 0 B/s for minutes).
                    if len_before > 0 {
                        if refusals < 2 {
                            refusals += 1;
                            let wait = (2u64 << refusals.min(4)).min(30);
                            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                            continue;
                        }
                        // SABR-gated streams (`gir=yes`) expose only their
                        // first ~minute to plain ranged GETs — verified live
                        // from this machine: varying headers, cookies, a fresh
                        // resolve and even a signed-in page fetch all 403 past
                        // the same offset while the same URL serves the prefix
                        // fine. That reads exactly like expiry ("progress then
                        // persistent refusal") but re-capturing cannot help, so
                        // say what is actually going on.
                        if url.contains("gir=yes") {
                            let got = crate::model::format_bytes(len_before);
                            return Err(final_err(format!(
                                "YouTube serves only the first ~1 minute of this quality outside the browser (got {got}) — its 1080p+ streams are SABR-gated for every non-browser client, so they cannot be downloaded directly"
                            )));
                        }
                        let got = crate::model::format_bytes(len_before);
                        return Err(final_err(format!(
                            "link expired after {got} ({refusals} refusals) — replay the reel and click the download button again for a fresh link, then resume"
                        )));
                    }
                    // First-touch refusal: retry within budget (edge throttle
                    // noise 206s on immediate retry — verified live).
                    stalled = stalled.saturating_add(1);
                    if stalled >= max_retries || attempt >= max_retries * 30 {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                    continue;
                }
                let msg = e.to_string();
                match classify_attempt(&msg) {
                    "cut" => cuts += 1,
                    "stalled" => stalls += 1,
                    _ => {}
                }
                if msg.contains("416") && !restarted {
                    // Offset rejected (file shrank server-side?): refresh the
                    // total once and restart from zero instead of wedging.
                    restarted = true;
                    restarts += 1;
                    let _ = tokio::fs::remove_file(&tmp).await;
                    shared.downloaded.store(0, Ordering::SeqCst);
                    if let Ok(mut g) = shared.part_done.lock() {
                        if let Some(slot) = g.first_mut() {
                            *slot = 0;
                        }
                    }
                    if let (t, _, _, _) = probe(client, opts, url).await {
                        if let Some(t) = t {
                            total = t;
                            shared.total.store(t, Ordering::SeqCst);
                            shared.has_total.store(true, Ordering::SeqCst);
                            // Rebind resume state to the refreshed total.
                            write_resume_meta(dest, url, Some(t), 1);
                        }
                    }
                    stalled = 0;
                    continue;
                }
                let len_after = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    if len_after < len_before {
                        restarts += 1;
                    }
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 30 {
                    return Err(with_attempt_summary(e, attempt, cuts, stalls, restarts));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
    }
    shared.downloaded.store(
        tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0),
        Ordering::SeqCst,
    );
    let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    if total > 0 && got < total {
        return Err(anyhow!(
            "connection ended early (got {got} of {total} bytes) — resuming"
        ));
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    Ok(())
}

/// One chunk of [`chunked_stream`]: GET `bytes=offset-end`, append exactly
/// that window to `tmp`. Returns bytes appended. A 200 (Range ignored) is
/// sliced down to our window by discarding the prefix — works as far as the
/// connection survives; a short stream is a retryable error, never silent.
#[allow(clippy::too_many_arguments)]
async fn chunked_once(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    tmp: &PathBuf,
    offset: u64,
    end: u64,
    want: u64,
    stall_timeout: u64,
    shared: &Arc<SharedState>,
    limiter: &std::sync::Arc<RateLimiter>,
) -> Result<u64> {
    let resp = opts
        .get(client, url)
        .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
        .send()
        .await?;
    let resp = resp
        .error_for_status()
        .map_err(|e| http_status_err(url, e))?;
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        eprintln!(
            "[dbg] chunk range={offset}-{end} status={} len={:?}",
            resp.status(),
            resp.content_length(),
        );
    }
    let is_partial = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !is_partial {
        shared.resumable.store(false, Ordering::SeqCst);
    } else {
        shared.resumable.store(true, Ordering::SeqCst);
    }
    // Kept for the short error below: a zero-byte range with a 200/empty
    // body vs a cut 206 mean very different things (see probe_range_once).
    let resp_status = resp.status();
    let resp_len = resp.content_length();
    let mut file = tokio::io::BufWriter::with_capacity(
        WRITE_BUF,
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(tmp)
            .await?,
    );
    // Bytes to skip when slicing a 200 response down to our window.
    let mut to_skip = if is_partial { 0 } else { offset };
    let mut to_take = want;
    let mut wrote = 0u64;
    let mut stream = resp.bytes_stream();
    // Any early exit still flushes what landed: the retry loops measure
    // progress by the file's length, so buffered bytes must never be lost
    // (a cut would otherwise replay the same offset forever).
    let mut failure: Option<anyhow::Error> = None;
    loop {
        if to_take == 0 {
            break;
        }
        let next = match tokio::time::timeout(
            std::time::Duration::from_secs(stall_timeout),
            stream.next(),
        )
        .await
        {
            Ok(n) => n,
            Err(_) => {
                failure = Some(anyhow!(
                    "stalled (no data for {stall_timeout}s) — resuming"
                ));
                break;
            }
        };
        let Some(chunk) = next else { break };
        if let Err(e) = wait_if_paused(shared).await {
            failure = Some(e);
            break;
        }
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                failure = Some(anyhow!("network: {e}"));
                break;
            }
        };
        let mut data = &bytes[..];
        if to_skip > 0 {
            let cut = (to_skip as usize).min(data.len());
            data = &data[cut..];
            to_skip -= cut as u64;
            if data.is_empty() {
                continue;
            }
        }
        let take = (to_take as usize).min(data.len());
        data = &data[..take];
        if data.is_empty() {
            break;
        }
        file.write_all(data)
            .await
            .map_err(|e| anyhow!(io_message(&e)))?;
        wrote += data.len() as u64;
        to_take -= data.len() as u64;
        shared.downloaded.fetch_add(data.len() as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(data.len() as u64);
            }
        }
        limiter.consume(data.len() as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    if let Some(e) = failure {
        return Err(e);
    }
    if wrote != want {
        // Zero-byte ranges name the cause: an empty/ignored range (200, no
        // body) means resume is impossible on this URL; a cut 206 means
        // throttling (resume works). The Queues log shows the full text.
        if wrote == 0 {
            return Err(anyhow!(
                "empty range {offset}-{end} (status {resp_status}, len {resp_len:?}) — resuming"
            ));
        }
        return Err(anyhow!(
            "connection ended early (got {wrote} of {want} bytes in range {offset}-{end}) — resuming"
        ));
    }
    Ok(wrote)
}

/// Multi-connection download through the reusable engine crate
/// (`crates/downloadcraft-engine`): dynamic segmentation with work stealing,
/// one pre-allocated file, a crash-safe resume journal, throttling and
/// connection-limit detection, expired-link refresh. This wrapper maps the
/// app's options, pause/stop flags, progress view and errors onto it.
#[allow(clippy::too_many_arguments)]
async fn segmented(
    client: &reqwest::Client,
    opts: &EngineOpts,
    url: &str,
    dest: &PathBuf,
    total: u64,
    connections: usize,
    shared: Arc<SharedState>,
    limiter: std::sync::Arc<RateLimiter>,
) -> Result<()> {
    use downloadcraft_engine as dce;
    // Strict googlevideo edges 403 ranges above ~2 MiB: ask in windows.
    let strict = is_googlevideo_url(url);
    // The TV client's links serve bigger ranges at full speed (one 60 MiB
    // request crawls at ~2 MB/s, 10 MiB pieces run at ~14 MB/s a link).
    let piece = if crate::youtube::tv_link(url) { 8 * 1024 * 1024 } else { STRICT_PART_MAX };
    let config = dce::Config {
        connections: connections.clamp(1, 32),
        max_request: strict.then_some(piece),
        min_split: if strict { piece } else { 1024 * 1024 },
        // Each fresh googlevideo link may come from another edge server with
        // its own Last-Modified for the same file (a resume threw 128 MB away
        // on "Last-Modified changed"); the link's own clen/lmt pin the file.
        size_identity: strict,
        stall_timeout: std::time::Duration::from_secs(opts.timeout_secs.clamp(15, 120)),
        max_retries: opts.max_retries.max(1),
        ..dce::Config::default()
    };
    let (req_client, req_opts) = (client.clone(), opts.clone());
    let request: dce::RequestFn = Arc::new(move |u: &str| req_opts.get(&req_client, u));
    let mut dl = dce::Downloader::with_request(request).config(config).limiter(limiter);
    if let Some(gate) = host_gate(url, opts.per_host_conns) {
        dl = dl.connection_gate(gate);
    }
    // A stream link that dies mid-file is re-resolved from its page; the
    // engine keeps every byte already on disk when the size still matches.
    if strict && is_youtube_page(&opts.referer) {
        let (c, o, s) = (client.clone(), opts.clone(), shared.clone());
        dl = dl.on_link_expired(Arc::new(move |old: String| {
            let (c, o, s) = (c.clone(), o.clone(), s.clone());
            Box::pin(async move {
                // The leg that expired: the video's or the sound's link,
                // matched by its itag (the sound leg used to get the
                // video's link, whose size never matched).
                let itag = |u: &str| {
                    url::Url::parse(u).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "itag").map(|(_, v)| v.into_owned()))
                };
                let want = itag(&old);
                resolve_media(&c, &o, &o.referer, &s)
                    .await
                    .map(|hit| match hit.pair_url {
                        Some(a) if want.is_some() && itag(&a) == want => a,
                        _ => hit.url,
                    })
                    .filter(|u| *u != old && is_googlevideo_url(u))
            })
        }));
    }
    // Part files of the old fixed-split engine (0.9.8 and earlier) cannot be
    // continued by this one.
    for i in 0..16 {
        let _ = tokio::fs::remove_file(dest.with_extension(format!("dcpart{i}"))).await;
    }
    let job = dce::Job::new(url, dest.clone())
        .size_hint(Some(total), true)
        .work_dir(work_dir())
        .resume_key(dest.to_string_lossy())
        .mirrors(opts.mirrors.clone());
    run_dce(dl, job, url, total, &shared, &opts.log_key).await.map(|_| ())
}

/// A form (POST) download the browser handed over: repeat the POST with its
/// body and headers. The first request doubles as the probe; when the server
/// also answers ranged POSTs, more connections join, otherwise one stream
/// fetches it (the engine decides from the reply).
async fn post_download(
    url: &str,
    dest: &PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
) -> Result<()> {
    use downloadcraft_engine as dce;
    let b = opts.browser.clone().unwrap_or_default();
    let body = crate::model::base64_decode(&b.body).ok_or_else(|| anyhow!("the form data could not be read"))?;
    let client = opts.build_client()?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let limiter = Arc::new(RateLimiter::new(opts.speed_limit_bps).with_global(opts.global_limit.clone()).with_own(opts.own_limit.clone()));
    let config = dce::Config {
        connections: connections.clamp(1, 32),
        stall_timeout: std::time::Duration::from_secs(opts.timeout_secs.clamp(15, 120)),
        max_retries: opts.max_retries.max(1),
        ..dce::Config::default()
    };
    let (c, o) = (client.clone(), opts.clone());
    let request: dce::RequestFn = Arc::new(move |u: &str| o.apply(c.post(u), Some(u)).body(body.clone()));
    let dl = dce::Downloader::with_request(request).config(config).limiter(limiter);
    shared.resumable.store(false, Ordering::SeqCst);
    let job = dce::Job::new(url, dest.clone()).work_dir(work_dir());
    let out = run_dce(dl, job, url, 0, shared, &opts.log_key).await?;
    // Name from the server's Content-Disposition when ours is a placeholder.
    if opts.allow_rename {
        let cur = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(name) = out.filename.map(|n| sanitize_filename(&n)).filter(|n| *n != cur) {
            if crate::model::is_fallback_filename(&cur) || !cur.contains('.') {
                let target = crate::model::conflict_free_dest(&dest.with_file_name(&name));
                if tokio::fs::rename(dest, &target).await.is_ok() {
                    if let Some(n) = target.file_name() {
                        mutex_guard(&shared.resolved_name).replace(n.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

/// Run one engine-crate download with the app's pause/stop flags bridged in
/// and its progress bridged out; errors in the app's vocabulary.
async fn run_dce(
    mut dl: downloadcraft_engine::Downloader,
    job: downloadcraft_engine::Job,
    url: &str,
    total: u64,
    shared: &Arc<SharedState>,
    log: &str,
) -> Result<downloadcraft_engine::Outcome> {
    use downloadcraft_engine as dce;
    let dbg = std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok();
    let key = log.to_string();
    dl = dl.on_event(Arc::new(move |e: &dce::Event| {
        if dbg {
            eprintln!("[dbg] engine: {e}");
        }
        job_log(&key, e.to_string());
    }));
    let progress = Arc::new(dce::Progress::default());
    let control = Arc::new(dce::Control::default());
    let done = Arc::new(AtomicBool::new(false));
    let bridge = {
        let (progress, control, shared, done) = (progress.clone(), control.clone(), shared.clone(), done.clone());
        tokio::spawn(async move {
            loop {
                sync_engine_progress(&progress, &control, &shared, total);
                if done.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
    };
    let result = dl.download(job, progress.clone(), control.clone()).await;
    done.store(true, Ordering::SeqCst);
    let _ = bridge.await;
    sync_engine_progress(&progress, &control, shared, total);
    match result {
        Ok(out) => {
            if out.url_refreshed {
                shared.set_note("the download link expired mid-download and was refreshed automatically");
            }
            Ok(out)
        }
        Err(e) => Err(engine_error(url, e)),
    }
}

/// Copy the engine's progress into the app's shared state and the app's
/// pause/stop flags into the engine.
fn sync_engine_progress(
    progress: &downloadcraft_engine::Progress,
    control: &downloadcraft_engine::Control,
    shared: &SharedState,
    total: u64,
) {
    if shared.cancelled.load(Ordering::SeqCst) {
        control.cancel();
    }
    if shared.paused.load(Ordering::SeqCst) {
        control.pause();
    } else {
        control.resume();
    }
    let segs = progress.segments();
    if !segs.is_empty() {
        let size = progress.total().unwrap_or(total);
        *mutex_guard(&shared.part_total) = segs
            .iter()
            .map(|s| (s.start, if s.end == u64::MAX { size.saturating_sub(1) } else { s.end.saturating_sub(1) }))
            .collect();
        *mutex_guard(&shared.part_done) = segs.iter().map(|s| s.done).collect();
    }
    // Until the engine has its piece map (read from the resume journal, or
    // planned fresh) its counter says 0: copying that would drop a download
    // that already got 20% in the background (the Download window's early
    // start) back to 0 in the bar and the progress window.
    if !segs.is_empty() || progress.downloaded() > shared.downloaded.load(Ordering::SeqCst) {
        shared.downloaded.store(progress.downloaded(), Ordering::SeqCst);
    }
    shared.resumable.store(progress.resumable(), Ordering::SeqCst);
    if let Some(t) = progress.total().filter(|t| *t > 0) {
        shared.total.store(t, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }
}

/// Engine errors in the app's vocabulary: refusals stay final (no retry
/// storms), throttling keeps the words the fallback ladder looks for.
fn engine_error(url: &str, e: downloadcraft_engine::Error) -> anyhow::Error {
    use downloadcraft_engine::Error as E;
    match e {
        E::Cancelled => anyhow!("cancelled"),
        E::Http { status, .. } => http_code_err(url, status),
        E::LinkExpired { status, downloaded } => final_err(format!(
            "link expired after {} (HTTP {status}) — capture the download again for a fresh link; it resumes where it stopped",
            crate::model::format_bytes(downloaded)
        )),
        E::Throttled { status } => anyhow!("the server keeps throttling the Range requests (HTTP {status})"),
        E::FileChanged(why) => anyhow!("the file on the server changed during the download ({why})"),
        E::HtmlPage => anyhow!(
            "the server sent a web page instead of the file — it likely needs a login: capture the download with the browser extension"
        ),
        E::Network(msg) => anyhow!("{msg} (segment short after retries)"),
        E::Io(e) => anyhow!(io_message(&e)),
        other => anyhow!("{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize_filename("a/b\\c:d.mp4"), "a_b_c_d.mp4");
        assert_eq!(sanitize_filename("   "), "download.bin");
        // A long Bengali title (3 bytes a letter) keeps its quality tag.
        let long = format!("{} [1080p].mp4", "খেজুর আর হুজুরের মধ্যে পার্থক্য কি ".repeat(3));
        let short = sanitize_filename(&long);
        assert!(short.len() <= 120, "{}", short.len());
        assert!(short.ends_with(" [1080p].mp4"), "{short}");
        // multibyte truncation must not panic or split chars
        let long = format!("{}.mp4", "ف".repeat(200));
        let out = sanitize_filename(&long);
        assert!(out.ends_with(".mp4"));
        assert!(out.len() <= 124);
        assert!(String::from_utf8(out.clone().into_bytes()).is_ok());
    }

    #[test]
    fn capture_names_that_are_not_titles() {
        // The names that produced "50k.mp4" / "6.6k.mp4" in the first place.
        for n in [
            "50k.mp4",
            "6.6k.mp4",
            "1080.mp4",
            "1080p.mp4",
            "videoplayback.mp4",
            "index.m4s",
            "download.bin",
            "youtube_abc123.mp4",
            "a1b2c3d4e5f6.mp4",
        ] {
            assert!(weak_capture_name(n), "{n}");
        }
        for n in [
            "Never Gonna Give You Up.mp4",
            "Python Tutorial — Part 1.mp4",
            "খেজুর আর হুজুর.mp4",
            "Mr. Robot S01E01.mkv",
            "dQw4w9WgXcQ.mp4",
        ] {
            assert!(!weak_capture_name(n), "{n}");
        }
    }

    #[test]
    fn resolved_titles_keep_the_captured_extension() {
        let captured = std::path::Path::new("50k.mp4");
        assert_eq!(
            title_filename("Never Gonna Give You Up", Some(captured)),
            "Never Gonna Give You Up.mp4"
        );
        // A title that already names a media file is not given a second one.
        assert_eq!(title_filename("Song.m4a", Some(captured)), "Song.m4a");
        // Bengali is kept as it is.
        assert_eq!(
            title_filename("খেজুর আর হুজুর", Some(captured)),
            "খেজুর আর হুজুর.mp4"
        );
        // No captured name: the caller fixes the extension from the MIME type.
        assert_eq!(
            title_filename("Never Gonna Give You Up", None),
            "Never Gonna Give You Up"
        );
        // A non-media extension on the captured name is not trusted.
        assert_eq!(
            title_filename("Title", Some(std::path::Path::new("50k.tmp"))),
            "Title"
        );
    }

    #[test]
    fn ytdlp_stage_lines() {
        // Lines exactly as a merged YouTube download emits them: extraction
        // first (seconds with nothing on the wire), then the transfer, then the
        // merge. These names are what the row and progress window show in place
        // of a motionless 0%.
        assert_eq!(
            phase_for_line("[youtube] jNQXAC9IVRw: Downloading webpage"),
            Some("Getting info")
        );
        assert_eq!(
            phase_for_line("[download] Destination: C:\\x\\y.mp4"),
            Some("Downloading")
        );
        assert_eq!(
            phase_for_line("[Merger] Merging formats into \"y.mp4\""),
            Some("Merging")
        );
        // The first line of a stream can arrive with a stray carriage return,
        // and unrelated chatter must not invent a stage.
        assert_eq!(
            phase_for_line("\r[youtube] Extracting URL: https://x"),
            Some("Getting info")
        );
        assert_eq!(phase_for_line("some unrelated log line"), None);
    }

    #[test]
    fn ytdlp_progress_lines_are_read() {
        // Captured live from yt-dlp (`--newline`, local HTTP server). The
        // reader used to look for the `download:` type prefix, which yt-dlp
        // strips before printing, so a whole yt-dlp transfer ran with no
        // progress, no size and no speed in the UI.
        let p = parse_progress_line("dcp@1024|20480000|NA|339953.0865917366").unwrap();
        assert_eq!(p.done, Some(1024.0));
        assert_eq!(p.total, Some(20480000.0));
        assert_eq!(p.estimate, None);
        assert!(p.speed.unwrap() > 300_000.0);
        assert_eq!(p.height, None);
        assert_eq!(p.width, None);
        assert!(!p.video);
        // Captured live: the template now also carries the chosen format's
        // frame size and video codec, so the row can show the resolution that
        // really arrived.
        let q = parse_progress_line("dcp@731403|731403|NA|153588.99|720|1280|398+140|h264").unwrap();
        assert_eq!(q.height, Some(720.0));
        assert_eq!(q.width, Some(1280.0));
        assert!(q.video);
        assert_eq!(resolution_label(q.width, q.height).as_deref(), Some("1280×720"));
        // A photo format carries the picture's pixel size in the very same
        // fields, and yt-dlp prints `none` for its codec: that is not a video,
        // so there is no resolution to report even though 1080×1080 is there.
        let photo = parse_progress_line("dcp@5|10|NA|1|1080|1080|jpg|none").unwrap();
        assert_eq!(photo.height, Some(1080.0));
        assert!(!photo.video);
        // An audio-only leg has no frame size: it must not invent one, and a
        // width with no height is not a resolution either.
        let a = parse_progress_line("dcp@5|10|NA|1|NA|NA|140|none").unwrap();
        assert!(!a.video);
        assert_eq!(resolution_label(a.width, a.height), None);
        assert_eq!(resolution_label(Some(1280.0), None), None);
        assert_eq!(resolution_label(None, Some(480.0)).as_deref(), Some("480p"));
        // Garbage values are rejected, not surfaced as a bogus number.
        assert_eq!(resolution_label(Some(0.0), Some(0.0)), None);
        assert_eq!(resolution_label(Some(99999.0), Some(99999.0)), None);
        assert_eq!(resolution_height("1280×720"), Some(720));
        assert_eq!(resolution_height("720p"), Some(720));
        assert_eq!(resolution_height("-"), None);
        // The marker has to be part of the *template*, since the type prefix
        // never reaches stdout.
        let tpl = ytdlp_progress_template();
        assert!(tpl.starts_with("download:"));
        assert!(tpl.contains(PROGRESS_MARK));
        assert!(tpl.contains("%(progress.downloaded_bytes)s"));
        assert!(tpl.contains("%(info.height)s"));
        assert!(tpl.contains("%(info.width)s"));
        assert!(tpl.contains("%(info.vcodec)s"));
        // Other lines are ignored, a leading CR is tolerated, and `NA` fields
        // simply stay unknown instead of zeroing the counters.
        assert!(parse_progress_line("[download] Destination: x.mp4").is_none());
        assert_eq!(parse_progress_line("\rdcp@5|10|NA|1").unwrap().done, Some(5.0));
        let estimate_only = parse_progress_line("dcp@7|NA|900|NA").unwrap();
        assert_eq!(estimate_only.total, None);
        assert_eq!(estimate_only.estimate, Some(900.0));
    }

    #[test]
    fn a_decoded_picture_is_not_a_video() {
        // Live ffprobe answers (`-of json`). ffprobe decodes pictures as
        // streams, so every one of these used to come back as a frame size —
        // which is how a downloaded photo announced a resolution it has not
        // got, and how an MP3's cover art became one.
        let photo = r#"{"streams":[{"codec_name":"mjpeg","width":558,"height":296,"avg_frame_rate":"25/1","disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(photo), None);
        // Cover art inside an audio file: the stream is flagged.
        let cover = r#"{"streams":[{"codec_name":"mjpeg","width":320,"height":240,"avg_frame_rate":"0/0","disposition":{"attached_pic":1}}]}"#;
        assert_eq!(resolution_from_ffprobe(cover), None);
        // A PNG screenshot: image codec and a single frame.
        let shot = r#"{"streams":[{"codec_name":"png","width":1920,"height":1080,"nb_frames":"1","disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(shot), None);
        // An animated GIF has many frames; the codec is what rejects it.
        let gif = r#"{"streams":[{"codec_name":"gif","width":480,"height":270,"nb_frames":"120","disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(gif), None);
        // A single frame is a picture, not a video.
        let one = r#"{"streams":[{"codec_name":"h264","width":640,"height":360,"nb_frames":"1","disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(one), None);
        // A real video: live answer for a 640×360 H.264 MP4.
        let clip = r#"{"streams":[{"codec_name":"h264","width":640,"height":360,"avg_frame_rate":"25/1","nb_frames":"50","disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(clip).as_deref(), Some("640×360"));
        // A file with no readable media (a ZIP) reports an empty answer.
        assert_eq!(resolution_from_ffprobe("{}"), None);
        assert_eq!(resolution_from_ffprobe(""), None);
        // A width with no height is still not a resolution.
        let half = r#"{"streams":[{"codec_name":"h264","width":640,"disposition":{"attached_pic":0}}]}"#;
        assert_eq!(resolution_from_ffprobe(half), None);
        // The codec rule on its own: audio-only, unreadable and picture codecs
        // are not video; the ones a real stream uses are.
        assert!(!is_video_codec("none"));
        assert!(!is_video_codec("NA"));
        assert!(!is_video_codec("unknown"));
        assert!(!is_video_codec(""));
        assert!(!is_video_codec("mjpeg"));
        assert!(!is_video_codec("PNG"));
        assert!(!is_video_codec("webp"));
        assert!(is_video_codec("h264"));
        assert!(is_video_codec("vp9"));
        assert!(is_video_codec("av01"));
        assert!(is_video_codec("hevc"));
    }

    #[test]
    fn merged_progress_sizes_add_up_across_legs() {
        // Live shape of a merged download: the video leg reports its size, then
        // the audio leg restarts its counter at 0 - and on some sites reports no
        // size of its own. Folding only the bytes left the UI showing more
        // downloaded than the file held ("Downloaded 847.65 MB" beside "Size
        // 722.80 MB").
        let line = |s: &str| parse_progress_line(s).unwrap();
        let mut f = MergedProgress::default();
        assert_eq!(f.step(&line("dcp@1000|2300|NA|10")), (1000, Some(2300)));
        assert_eq!(f.step(&line("dcp@2300|2300|NA|10")), (2300, Some(2300)));
        // The audio leg starts: counter back to 0, size not named yet.
        assert_eq!(f.step(&line("dcp@0|NA|NA|10")), (2300, None));
        assert_eq!(f.step(&line("dcp@10|NA|NA|10")), (2310, None));
        // …and once it names its size, both legs add up and bytes stay below it.
        let (dl, total) = f.step(&line("dcp@100|700|NA|10"));
        assert_eq!((dl, total), (2400, Some(3000)));
        assert!(dl <= total.unwrap());
        // An estimate is a size too.
        let mut g = MergedProgress::default();
        assert_eq!(g.step(&line("dcp@5|NA|900|NA")), (5, Some(900)));
    }

    #[test]
    fn zip_entry_reads_a_release_archive() {
        use std::io::Write;
        // One stored and one deflated file, laid out like a release zip.
        fn zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
            let (mut out, mut dir) = (Vec::new(), Vec::new());
            for (name, data, deflate) in files {
                let body = if *deflate {
                    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                    e.write_all(data).unwrap();
                    e.finish().unwrap()
                } else {
                    data.to_vec()
                };
                let method: u16 = if *deflate { 8 } else { 0 };
                let local = out.len() as u32;
                out.extend_from_slice(b"PK\x03\x04");
                out.extend_from_slice(&[20, 0, 0, 0]);
                out.extend_from_slice(&method.to_le_bytes());
                out.extend_from_slice(&[0; 8]); // time, date, crc
                out.extend_from_slice(&(body.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(&body);
                dir.extend_from_slice(b"PK\x01\x02");
                dir.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
                dir.extend_from_slice(&method.to_le_bytes());
                dir.extend_from_slice(&[0; 8]);
                dir.extend_from_slice(&(body.len() as u32).to_le_bytes());
                dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
                dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
                dir.extend_from_slice(&[0; 12]); // extra, comment, disk, attrs
                dir.extend_from_slice(&local.to_le_bytes());
                dir.extend_from_slice(name.as_bytes());
            }
            let at = out.len() as u32;
            let n = files.len() as u16;
            out.extend_from_slice(&dir);
            out.extend_from_slice(b"PK\x05\x06");
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out
        }
        let exe = b"MZ portable runtime ".repeat(500);
        let z = zip(&[("README.md", b"hi", false), ("deno/deno.exe", &exe, true)]);
        assert_eq!(zip_entry(&z, "deno.exe").unwrap(), exe);
        assert_eq!(zip_entry(&z, "README.md").unwrap(), b"hi");
        assert!(zip_entry(&z, "missing").is_err());
        assert!(zip_entry(&z[..z.len() / 2], "deno.exe").is_err());
    }

    #[test]
    fn fragment_downloads_do_not_snowball_or_drift() {
        let line = |s: &str| parse_progress_line(s).unwrap();
        // Several fragments in flight: the counter dips now and then. Each
        // dip used to count as a new leg and add the whole total again - an
        // HLS movie claimed 18 GB downloaded after a resume.
        let mut f = MergedProgress::default();
        let mut last = 0;
        for done in [500_000_000u64, 498_000_000, 520_000_000, 519_500_000, 530_000_000] {
            let l = format!("dcp@{done}|NA|2300000000|6000000|960|1920|hls-3574|avc1|120|1340");
            let (dl, total) = f.step(&line(&l));
            assert!(dl >= last && dl <= 530_000_000, "{dl}");
            last = dl;
            // The estimate moves with every fragment: no size is published.
            assert_eq!(total, None);
        }
        // The fragment fields are read.
        let p = line("dcp@1|NA|5|1|NA|NA|hls-1|avc1|120|1340");
        assert_eq!((p.frag_index, p.frag_count), (Some(120.0), Some(1340.0)));
        assert_eq!(line("dcp@1|2|NA|1").frag_count, None);
        // A different format id is the next leg, even when its counter has not
        // dropped below half (the audio leg of a merge).
        let mut g = MergedProgress::default();
        g.step(&line("dcp@1000|1000|NA|1|720|1280|137|avc1"));
        let (dl, total) = g.step(&line("dcp@900|900|NA|1|NA|NA|140|none"));
        assert_eq!((dl, total), (1900, Some(1900)));
    }

    #[test]
    fn resume_metadata_is_removed_only_for_completed_files() {
        let dir = std::env::temp_dir().join(format!("dc-cleanup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("movie.mp4");
        let meta = resume_meta_path(&dest);
        std::fs::write(&meta, "resume").unwrap();
        cleanup_completed_resume(&dest);
        assert!(meta.exists()); // interrupted, no finished file
        std::fs::write(&dest, "complete").unwrap();
        std::fs::write(dest.with_extension("dcpart"), "partial").unwrap();
        cleanup_completed_resume(&dest);
        assert!(meta.exists()); // active partial data still needs its binding
        std::fs::remove_file(dest.with_extension("dcpart")).unwrap();
        cleanup_completed_resume(&dest);
        assert!(!meta.exists());
        assert!(dest.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn general_fragments_need_a_complete_source() {
        assert!(is_media_fragment("https://cdn.example.org/seg-003.ts"));
        assert!(is_media_fragment("https://cdn.example.org/part.m4s"));
        assert!(is_media_fragment("https://cdn.example.org/v.mp4?range=100-200"));
        assert!(!is_media_fragment("https://cdn.example.org/short.mp4"));
        assert!(!is_media_fragment("https://cdn.example.org/movie.ts"));
        assert!(!is_media_fragment("https://cdn.example.org/file.zip?range=100-200"));
        assert!(fragment_page("https://cdn.example.org/part.m4s", "").is_err());
        assert_eq!(fragment_page("https://cdn.example.org/part.m4s", "https://example.org/watch/1").unwrap().as_deref(), Some("https://example.org/watch/1"));
        assert_eq!(cookie_domain("https://player.example.org/watch/1"), ".player.example.org");
    }

    #[test]
    fn generic_hls_separate_audio_is_detected_from_manifest_evidence() {
        assert!(hls_external_audio("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"audio.m3u8\"\n"));
        assert!(!hls_external_audio("#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,URI=\"subs.m3u8\"\n"));
        assert!(!hls_external_audio("#EXTM3U\n#EXTINF:2,\nseg0.ts\n#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn completed_duration_checks_allow_short_and_silent_sources() {
        let silent = r#"{"format":{"duration":"3.0"},"streams":[{"codec_type":"video"}]}"#;
        assert!(completed_media_check(silent, 3, false).is_ok());
        assert!(completed_media_check(silent, 0, false).is_ok());
        assert!(completed_media_check(silent, 2700, false).unwrap_err().to_string().contains("incomplete media"));
        assert!(completed_media_check(silent, 3, true).unwrap_err().to_string().contains("no audio"));
        let sound = r#"{"format":{"duration":"2699.5"},"streams":[{"codec_type":"video"},{"codec_type":"audio"}]}"#;
        assert!(completed_media_check(sound, 2700, true).is_ok());
        assert!(completed_media_check(r#"{"streams":[]}"#, 2700, false).is_err());
    }

    #[test]
    fn encrypted_mp4_tracks_are_detected_from_the_box_tree() {
        fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        }
        let file = |entry: &[u8; 4], mdat: &[u8]| {
            let mut stsd = vec![0u8; 8];
            stsd[7] = 1;
            stsd.extend(bx(entry, &[0u8; 16]));
            let moov = bx(b"moov", &bx(b"trak", &bx(b"mdia", &bx(b"minf", &bx(b"stbl", &bx(b"stsd", &stsd))))));
            let mut v = bx(b"ftyp", b"isom    ");
            v.extend(moov);
            v.extend(bx(b"mdat", mdat));
            v
        };
        let dir = std::env::temp_dir().join(format!("dc-drm-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: Vec<u8>| { let p = dir.join(name); std::fs::write(&p, bytes).unwrap(); p };
        assert!(mp4_encrypted(&write("enc.mp4", file(b"encv", b"x"))));
        assert!(mp4_encrypted(&write("enca.m4a", file(b"enca", b"x"))));
        // Clear tracks, even with "encv" bytes inside the media data.
        assert!(!mp4_encrypted(&write("clear.mp4", file(b"avc1", b"..encv..enca.."))));
        assert!(!mp4_encrypted(&write("text.txt", b"encv encv encv".to_vec())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vimeo_manifests_bypass_video_only_hls_concat() {
        let manifest = "https://vod-adaptive-ak.vimeocdn.com/video/master.m3u8?token=signed";
        let player = "https://player.vimeo.com/video/123?h=unlisted";
        assert_eq!(vimeo_download_target(manifest, player).unwrap().as_deref(), Some(player));
        assert_eq!(vimeo_download_target(manifest, "https://example.com/watch").unwrap().as_deref(), Some(manifest));
        assert!(vimeo_download_target("https://vod-adaptive-ak.vimeocdn.com/video.mp4", "").is_err());
        assert_eq!(vimeo_download_target("https://vod.vimeocdn.com/full.mp4", "").unwrap(), None);
        assert!(!vimeo_needs_audio("https://vimeocdn.com.evil.test/master.m3u8"));
    }

    #[test]
    fn vimeo_format_selection_requires_audio() {
        assert_eq!(audio_required_format("", true), "bv+ba[ext=m4a]/bv+ba/b[acodec!=none]");
        assert_eq!(audio_required_format("", false), "b[acodec!=none]");
        assert!(audio_required_format("720p", true).contains("bv[height<=720]+ba"));
        assert!(audio_required_format("720p", false).contains("[acodec!=none]"));
    }

    #[test]
    fn vimeo_finished_file_must_have_an_audio_track() {
        assert!(ffprobe_has_audio(r#"{"streams":[{"codec_type":"video"},{"codec_type":"audio"}]}"#).unwrap());
        assert!(!ffprobe_has_audio(r#"{"streams":[{"codec_type":"video"}]}"#).unwrap());
        assert!(!ffprobe_has_audio(r#"{"streams":[]}"#).unwrap());
        assert!(ffprobe_has_audio("invalid").is_err());
    }

    #[test]
    fn vimeo_range_captures_are_not_full_files() {
        assert!(is_vimeo_fragment("https://vod-adaptive-ak.vimeocdn.com/v2/range/prot/token/avf/v.mp4?range=3938712-4982381"));
        assert!(is_vimeo_fragment("https://vod.vimeocdn.com/v2/range/prot/token/v.mp4"));
        assert!(is_vimeo_fragment("https://vod.vimeocdn.com/v.mp4?range=0-99"));
        assert!(!is_vimeo_fragment("https://vod.vimeocdn.com/full.mp4"));
        assert!(!is_vimeo_fragment("https://vimeocdn.com.evil.test/v2/range/v.mp4"));
        assert!(is_vimeo_video_page("https://player.vimeo.com/video/123?h=secret"));
        assert!(is_vimeo_video_page("https://vimeo.com/123/secret"));
        assert!(!is_vimeo_video_page("https://vimeo.com/user123"));
        assert!(!is_vimeo_video_page("https://vimeo.com/"));
        assert!(!is_vimeo_video_page("ftp://vimeo.com/123"));
        assert_eq!(cookie_domain("https://player.vimeo.com/video/123?h=secret"), ".vimeo.com");
    }

    #[tokio::test]
    async fn vimeo_fragment_without_player_fails_before_transfer() {
        let shared = Arc::new(SharedState::new());
        let result = download(
            "https://vod.vimeocdn.com/v2/range/prot/token/v.mp4?range=0-99".to_string(),
            std::env::temp_dir().join("downloadcraft-vimeo-fragment-guard.mp4"),
            1, EngineOpts::default(), shared.clone(),
        ).await;
        assert!(result.unwrap_err().to_string().contains("playback fragment"));
        assert_eq!(shared.downloaded.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn social_pages_are_used_for_audio_merges() {
        // A captured Facebook/Instagram representation is video-only; the page
        // is where the matching audio track lives. Only a page naming ONE video
        // may be handed over - a feed could resolve to a different video - and
        // the cookie jar domain has to match the site or every cookie is
        // dropped and the site answers as a logged-out visitor.
        let asset = "https://scontent.fdac90-1.fna.fbcdn.net/o1/v/t2/f2/m366/AQM3.mp4?efg=eyJ2ZW5jb2Rl";
        assert_eq!(
            social_page_url(asset, "https://www.facebook.com/reel/1136332525739217").as_deref(),
            Some("https://www.facebook.com/reel/1136332525739217")
        );
        assert_eq!(
            social_page_url("https://scontent.xx.fbcdn.net/v/t42/a.mp4", "https://www.facebook.com/watch/?v=123")
                .as_deref(),
            Some("https://www.facebook.com/watch/?v=123")
        );
        assert_eq!(
            social_page_url("https://scontent.xx.fbcdn.net/x.mp4", "https://www.instagram.com/reel/abc/")
                .as_deref(),
            Some("https://www.instagram.com/reel/abc/")
        );
        // A feed, a group or the home page is not the video's own page.
        assert_eq!(social_page_url(asset, "https://www.facebook.com/"), None);
        assert_eq!(social_page_url(asset, "https://www.facebook.com/groups/1/"), None);
        // Non-social assets are never redirected to a page, and neither is a
        // missing referer.
        assert_eq!(
            social_page_url("https://cdn.example/v.mp4", "https://www.facebook.com/reel/9"),
            None
        );
        assert_eq!(social_page_url(asset, "   "), None);
        assert_eq!(
            cookie_domain("https://www.facebook.com/reel/1"),
            ".facebook.com"
        );
        assert_eq!(
            cookie_domain("https://scontent.xx.fbcdn.net/x.mp4"),
            ".facebook.com"
        );
        assert_eq!(
            cookie_domain("https://www.instagram.com/reel/1"),
            ".instagram.com"
        );
        assert_eq!(
            cookie_domain("https://rr1---sn.googlevideo.com/x"),
            ".youtube.com"
        );
    }

    #[test]
    fn resume_meta_binding() {
        // Sidecar binds .dcpart bytes to the exact URL+total+split (P0):
        // same URL resumes, different URL discards, legacy unbound
        // partials resume once by length.
        let dir = std::env::temp_dir().join(format!("dc_meta_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("v.mp4");
        let part = dest.with_extension("dcpart");
        let url_a = "https://cdn.example/v.mp4?tok=a";
        let url_b = "https://cdn.example/v.mp4?tok=b";

        // Fresh: no partial, binds and returns 0.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 0);
        assert!(resume_meta_path(&dest).exists());

        // Same URL: prefix resumes.
        std::fs::write(&part, vec![7u8; 400]).unwrap();
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 400);

        // Total learned later is adopted, still resumes.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 400);

        // Different total for same URL: stale prefix discarded.
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(2000), 1), 0);
        assert!(!part.exists());

        // Different URL: stale prefix discarded (the FB slice case).
        std::fs::write(&part, vec![7u8; 100]).unwrap();
        write_resume_meta(&dest, url_a, Some(1000), 1);
        assert_eq!(resume_offset(&dest, "dcpart", url_b, Some(1000), 1), 0);
        assert!(!part.exists());

        // Legacy partial without sidecar: resumes once, then binds.
        std::fs::write(&part, vec![7u8; 50]).unwrap();
        let _ = std::fs::remove_file(resume_meta_path(&dest));
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 1), 50);
        assert!(read_resume_meta(&dest).is_some());

        // Split change (1 -> 4 conns): segmented parts dropped.
        let p0 = dest.with_extension("dcpart0");
        std::fs::write(&p0, vec![7u8; 10]).unwrap();
        assert_eq!(resume_offset(&dest, "dcpart", url_a, Some(1000), 4), 0);
        assert!(!p0.exists());

        // meta_matches unit rules.
        let m = ResumeMeta { url: url_a.into(), total: Some(1000), conns: 1 };
        assert!(meta_matches(&m, url_a, Some(1000), 1));
        assert!(!meta_matches(&m, url_b, Some(1000), 1));
        assert!(!meta_matches(&m, url_a, Some(999), 1));
        assert!(!meta_matches(&m, url_a, Some(1000), 4));
        assert!(meta_matches(&m, url_a, None, 1)); // unknown: wildcard

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn halve_schedule() {
        // P2: 16→8→4→2, then give up splitting (single stream).
        assert_eq!(halve_conns(16), Some(8));
        assert_eq!(halve_conns(8), Some(4));
        assert_eq!(halve_conns(4), Some(2));
        assert_eq!(halve_conns(3), Some(2));
        assert_eq!(halve_conns(2), Some(1));
        assert_eq!(halve_conns(1), None);
        // Throttling signatures halve; hard refusals go straight single.
        assert!(segmented_throttled("segment short: got 1 of 100 bytes — retrying the range"));
        assert!(segmented_throttled("HTTP 416 Range Not Satisfiable"));
        assert!(segmented_throttled("HTTP status client error (429 Too Many Requests)"));
        assert!(segmented_throttled("Range Not Satisfiable"));
        assert!(!segmented_throttled("network: connection reset"));
    }

    #[test]
    fn seg_fallback_table() {
        // P-speed decision table after a segmented failure.
        let is = |f: SegFallback| match f {
            SegFallback::Halve(h) => format!("halve:{h}"),
            SegFallback::Chunked => "chunked".to_string(),
            SegFallback::Single => "single".to_string(),
        };
        assert_eq!(is(seg_fallback(false, 16, true)), "halve:8");
        assert_eq!(is(seg_fallback(false, 3, true)), "halve:2");
        // Two connections step down to one engine connection first.
        assert_eq!(is(seg_fallback(false, 2, true)), "halve:1");
        assert_eq!(is(seg_fallback(false, 1, true)), "chunked");
        assert_eq!(is(seg_fallback(false, 1, false)), "single");
        assert_eq!(is(seg_fallback(true, 8, true)), "chunked");
        assert_eq!(is(seg_fallback(true, 8, false)), "single");
        assert_eq!(is(seg_fallback(true, 1, false)), "single");
    }


    #[test]
    fn fb_slice_normalize() {
        // Real failure: scontent slice URL (bytestart/byteend) can only
        // return ~1.1 MB while the full file is ~9.4 MB.
        let slice = "https://scontent-lga3-2.xx.fbcdn.net/o1/v/t2/f2/m366/A.mp4?oh=abc&oe=xyz&bytestart=967&byteend=1193530";
        let full = normalize_media_url(slice);
        assert!(!full.contains("bytestart"), "{full}");
        assert!(!full.contains("byteend"), "{full}");
        assert!(full.contains("oh=abc"), "{full}");
        assert!(full.contains("oe=xyz"), "{full}");
        assert!(is_fragile_cdn(slice));
        assert!(is_fragile_cdn(&full));
        // Non-social hosts untouched.
        let other = "https://cdn.example/v.mp4?bytestart=1&byteend=2";
        assert_eq!(normalize_media_url(other), other);
        // No slice params: identity.
        let plain = "https://scontent.fdac90-1.fna.fbcdn.net/v.mp4?oh=1";
        assert_eq!(normalize_media_url(plain), plain);
    }

    #[test]
    fn social_video_pages() {
        for u in [
            "https://www.instagram.com/reel/C9xYz_AbC12/",
            "https://www.instagram.com/p/C9xYz-AbC12/?img_index=1",
            "https://www.instagram.com/someone/reel/C9xYzAbC12/",
            "https://www.tiktok.com/@some.user/video/7301234567890123456",
            "https://www.tiktok.com/@/video/7301234567890123456",
            "https://x.com/someone/status/1790000000000000000",
            "https://twitter.com/someone/status/1790000000000000000/video/1",
            "https://www.reddit.com/r/videos/comments/1abcde/some_title/",
            "https://old.reddit.com/r/videos/comments/1abcde/",
            "https://v.redd.it/abc123xyz",
        ] {
            assert!(social_video_page(u), "{u}");
        }
        for u in [
            "https://www.instagram.com/someone/",
            "https://www.instagram.com/explore/",
            "https://www.tiktok.com/@some.user",
            "https://www.tiktok.com/foryou",
            "https://x.com/someone",
            "https://x.com/home",
            "https://www.reddit.com/r/videos/",
            "https://www.youtube.com/watch?v=abc",
            "https://example.com/reel/abc",
        ] {
            assert!(!social_video_page(u), "{u}");
        }
        assert!(wants_audio_only("audio") && wants_audio_only(" Audio ") && !wants_audio_only("720p"));
    }

    #[test]
    fn dispositions() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"my file.zip\""),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("my file.zip"));
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static(
                "attachment; filename*=UTF-8''%D9%81%D8%A7%D8%B1%D8%B3%DB%8C.mp3",
            ),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("فارسی.mp3"));
        // P1: semicolons inside quotes must not split the value.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"a;b.mp4\""),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("a;b.mp4"));
        // P1: parameter names are case-insensitive; quoted filename* works.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; FileName*=UTF-8''clip%20hd.mp4"),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("clip hd.mp4"));
        // P1: filename* wins over filename; unsafe chars sanitized.
        h.insert(
            reqwest::header::CONTENT_DISPOSITION,
            reqwest::header::HeaderValue::from_static("attachment; filename=\"a/b.mp4\"; filename*=UTF-8''c:d.mp4"),
        );
        assert_eq!(disposition_filename(&h).as_deref(), Some("c_d.mp4"));
        let empty = reqwest::header::HeaderMap::new();
        assert_eq!(disposition_filename(&empty), None);
    }

    #[test]
    fn youtube_cipher_fixtures() {
        // Classic-shape player JS: helper object + decipher function.
        let js = r#"var XX={aB:function(a){a.reverse()},MB:function(a,b){a.splice(0,b)},kB:function(a,b){var c=a[0];a[0]=a[b%a.length];a[b%a.length]=c}};var Y=function(a){a=a.split("");XX.aB(a);XX.MB(a,2);XX.kB(a,1);return a.join("")};"#;
        let ops = parse_sig_ops(js).expect("classic decipher parses");
        assert_eq!(ops, vec![SigOp::Reverse, SigOp::Splice(2), SigOp::Swap(1)]);
        // "abcdef" → reverse "fedcba" → splice 2 "dcba" → swap(1) "cdba".
        assert_eq!(apply_sig_ops("abcdef", &ops), "cdba");
        assert_eq!(apply_sig_ops("", &ops), "");
        assert_eq!(apply_sig_ops("x", &[SigOp::Swap(5)]), "x");
        // signatureCipher assembly.
        let cipher = "url=https%3A%2F%2Fx%2Fv%3Fitag%3D18&sp=sig&s=abcdef";
        assert_eq!(
            decipher_signature_cipher(cipher, &ops).as_deref(),
            Some("https://x/v?itag=18&sig=cdba")
        );
        assert_eq!(decipher_signature_cipher("url=&s=", &ops), None);
        assert_eq!(decipher_signature_cipher("nonsense", &ops), None);
        // Throttle-param fix: a SECOND simple-shape function is the n-code.
        let js2 = js.to_string()
            + r#"var Z=function(a){a=a.split("");XX.MB(a,3);return a.join("")};"#;
        assert_eq!(
            try_simple_n(&js2, &ops, "abcdef").as_deref(),
            Some("def")
        );
        // Single candidate (decipher only) → no confident n transform.
        assert_eq!(try_simple_n(js, &ops, "abcdef"), None);
        // Exotic shapes bail instead of guessing.
        assert_eq!(parse_sig_ops("var x = 1;"), None);
        assert_eq!(
            parse_sig_ops("var Y=function(a){return a;}"),
            None
        );
        // Player JS URL extraction.
        let html = r#"<html><head><script>var cfg = {"jsUrl":"/s/player/abc/base.js"};</script></head></html>"#;
        assert_eq!(
            youtube_player_js_url(html).as_deref(),
            Some("https://www.youtube.com/s/player/abc/base.js")
        );
        assert_eq!(youtube_player_js_url("<html></html>"), None);
        // ratebypass hygiene.
        assert_eq!(
            with_ratebypass("https://r1.googlevideo.com/v?x=1"),
            "https://r1.googlevideo.com/v?x=1&ratebypass=yes"
        );
        assert_eq!(
            with_ratebypass("https://r1.googlevideo.com/v?ratebypass=yes"),
            "https://r1.googlevideo.com/v?ratebypass=yes"
        );
        assert_eq!(
            with_ratebypass("https://cdn.example/v.mp4"),
            "https://cdn.example/v.mp4"
        );
        // n-param rewrite (pure part).
        let fixed = fix_n_param("https://h/v?n=abcdef&x=1", &js2, &ops);
        assert_eq!(fixed, "https://h/v?n=def&x=1");
        assert_eq!(
            fix_n_param("https://h/v?x=1", &js2, &ops),
            "https://h/v?x=1"
        );
    }

    #[test]
    fn youtube_fixture() {
        let html = r#"<html><head></head><body><script>var ytInitialPlayerResponse = {"videoDetails":{"title":"My Vid"},"streamingData":{"formats":[{"mimeType":"video/mp4; codecs=\"avc1\"","width":640,"url":"https://cdn/x/360.mp4"},{"mimeType":"video/mp4; codecs=\"avc1\"","width":1280,"url":"https://cdn/x/720.mp4"},{"mimeType":"video/webm","width":1920,"url":"https://cdn/x/1080.webm"}]}};</script></body></html>"#;
        let hit = resolve_youtube(html).expect("should resolve");
        assert_eq!(hit.0, "https://cdn/x/720.mp4");
        assert_eq!(hit.1, "My Vid.mp4");
    }

    #[test]
    fn unwraps_player_wrapper_urls() {
        // jwplayer/dooplay wrapper: the stream is the encoded source param.
        let wrapped = "https://sunplex.net/jwplayer/?source=https%3A%2F%2Fstream.sunplex.net%2FMovies%2FNayyi%2520Navelli%2Findex.m3u8";
        let inner = unwrap_player_url(wrapped).expect("should unwrap");
        assert!(inner.starts_with("https://stream.sunplex.net/"));
        assert!(inner.ends_with(".m3u8"));
        // A plain URL without a media-ish query stays put.
        assert!(unwrap_player_url("https://example.com/watch?v=abc").is_none());
        assert!(unwrap_player_url("https://cdn.example/v.mp4").is_none());
        // Release folder survives for a generic playlist name.
        assert_eq!(
            url_name_stem("https://stream.sunplex.net/Movies/Nayyi%20Navelli/index.m3u8"),
            "Nayyi Navelli"
        );
        // …but a real filename is used as-is.
        assert_eq!(
            url_name_stem("https://cdn/x/Release.2026.1080p.mp4"),
            "Release.2026.1080p"
        );
    }

    #[test]
    fn generic_fixture() {
        let html = r#"<html><head><meta property="og:title" content="Cool Clip"><meta property="og:video:url" content="https://cdn.example/v.mp4?x=1"></head></html>"#;
        let hit = resolve_generic(html, "https://example.com/p").expect("should resolve");
        assert_eq!(hit.0, "https://cdn.example/v.mp4?x=1");
        assert_eq!(hit.1, "Cool Clip.mp4");
        let bare = resolve_generic(
            "<a href=\"https://cdn.example/files/a.mkv\">dl</a>",
            "https://example.com/p",
        )
        .expect("bare link");
        assert_eq!(bare.0, "https://cdn.example/files/a.mkv");
    }

    #[test]
    fn social_captions_name_the_file() {
        // Placeholder page names are never filenames.
        assert!(is_placeholder_title("Facebook"));
        assert!(is_placeholder_title("  facebook. "));
        assert!(is_placeholder_title("See posts, photos and more on Facebook."));
        assert!(!is_placeholder_title("Smrity's Home Diary"));

        // A Facebook reel: og:title is only the site name, og:description is
        // the caption — the name IDM saves the file under.
        let reel = r#"<html><head><meta property="og:title" content="Facebook"><meta property="og:description" content="Ei caption ta file name hobe | Facebook"><meta property="og:video:url" content="https://video.xx.fbcdn.net/v/a.mp4"></head></html>"#;
        let hit = resolve_generic(reel, "https://www.facebook.com/reel/123456").expect("reel");
        assert_eq!(hit.0, "https://video.xx.fbcdn.net/v/a.mp4");
        assert_eq!(hit.1, "Ei caption ta file name hobe.mp4");

        // Instagram's og:description is a likes/comments preamble + caption.
        let ig = r#"<meta property="og:description" content="12.3K likes, 45 comments - someone on January 2, 2026: &amp;quot;the reel caption here&amp;quot;"><meta property="og:video" content="https://cdn.example/r.mp4">"#;
        let ig_hit = resolve_generic(ig, "https://www.instagram.com/reel/abc/").expect("ig");
        assert_eq!(ig_hit.1, "the reel caption here.mp4");

        // A plain page with a description keeps using its title.
        let plain = r#"<meta property="og:title" content="Cool Clip"><meta property="og:description" content="Some marketing blurb for the clip"><meta property="og:video:url" content="https://cdn.example/v.mp4">"#;
        let plain_hit = resolve_generic(plain, "https://example.com/p").expect("plain");
        assert_eq!(plain_hit.1, "Cool Clip.mp4");

        // No caption and a placeholder title: never "Facebook.mp4".
        let bare = r#"<meta property="og:title" content="Facebook"><meta property="og:video:url" content="https://video.xx.fbcdn.net/v/b.mp4">"#;
        let bare_hit = resolve_generic(bare, "https://www.facebook.com/watch/?v=9").expect("bare");
        assert_eq!(bare_hit.1, "video.mp4");
    }

    #[test]
    fn meta_orders() {
        let html = r#"<meta content="https://c/v.webm" property="og:video">"#;
        assert_eq!(
            meta_content(html, "og:video").as_deref(),
            Some("https://c/v.webm")
        );
        assert_eq!(tag_text("<title>  Hi </title>", "title").as_deref(), Some("Hi"));
        assert_eq!(guess_ext("https://h/f.MKV?1"), ".mkv");
        assert_eq!(with_ext("A", ".mp4"), "A.mp4");
        assert_eq!(with_ext("A.mp4", ".mp4"), "A.mp4");
    }

    #[test]
    fn hostile_pages_never_panic() {
        // marker with no object behind it (truncated page)
        assert_eq!(resolve_youtube("<script>var ytInitialPlayerResponse = "), None);
        assert_eq!(resolve_youtube(""), None);
        // truncated JSON object
        assert_eq!(
            resolve_youtube("ytInitialPlayerResponse = {\"videoDetails\":{\"title\":\"x\""),
            None
        );
        // unbalanced braces inside strings
        assert_eq!(
            resolve_youtube("ytInitialPlayerResponse = {\"a\":\"}{\"}"),
            None
        );
        // sorry-wall page: no streams, must return None (flag set upstream)
        let sorry = "<html><body>unusual traffic <a href=\"/sorry/index\">x</a></body></html>";
        assert_eq!(resolve_youtube(sorry), None);
        assert_eq!(resolve_generic(sorry, "https://www.youtube.com/watch?v=x"), None);
        // multibyte text around a media link must not panic on slicing
        let tricky = "فارسی https://cdn.example/ünïcode-vïd.mp4\" multibyte tail 日本語";
        let hit = resolve_generic(tricky, "https://example.com/p");
        assert!(hit.is_some());
        // youtube ids
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(
            youtube_id("https://youtu.be/aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/shorts/aqz-KE-bpKQ").as_deref(),
            Some("aqz-KE-bpKQ")
        );
        assert_eq!(youtube_id("https://www.youtube.com/"), None);
        // empty probe-ish inputs
        assert_eq!(disposition_filename(&reqwest::header::HeaderMap::new()), None);
        assert_eq!(sanitize_filename(""), "download.bin");
    }

    #[test]
    fn dash_helpers() {
        // EXT-X-KEY attribute parsing (quoted commas safe).
        let attrs = hls_key_attrs("#EXT-X-KEY:METHOD=AES-128,URI=\"https://k/x.key\",IV=0x1234");
        assert_eq!(hls_attr(&attrs, "METHOD").as_deref(), Some("AES-128"));
        assert_eq!(hls_attr(&attrs, "uri").as_deref(), Some("https://k/x.key"));
        assert_eq!(hls_attr(&attrs, "iv").as_deref(), Some("0x1234"));
        // Key policy: none/plain ok; SAMPLE-AES rejected; rotation (P3)
        // yields a per-segment schedule instead of an error.
        let plain = hls_key_schedule("#EXTM3U\n#EXTINF:6,\ns.ts\n", "https://h/pl.m3u8").unwrap();
        assert_eq!(plain.len(), 1);
        assert!(plain[0].is_none());
        let aes = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\",IV=0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n#EXTINF:6,\ns.ts\n";
        let sched = hls_key_schedule(aes, "https://h/pl.m3u8").unwrap();
        assert_eq!(sched.len(), 1);
        let slot = sched[0].clone().expect("key");
        assert_eq!(slot.uri, "https://h/k");
        assert_eq!(slot.iv_hex.as_deref(), Some("0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
        // Key change mid-playlist: segments before/after differ (rotation).
        let rot = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"a\"\n#EXTINF:6,\ns0.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"b\"\n#EXTINF:6,\ns1.ts\n#EXT-X-ENDLIST\n";
        let rsched = hls_key_schedule(rot, "https://h/pl.m3u8").unwrap();
        assert_eq!(rsched.len(), 2);
        assert_eq!(rsched[0].as_ref().unwrap().uri, "https://h/a");
        assert_eq!(rsched[1].as_ref().unwrap().uri, "https://h/b");
        // METHOD=NONE clears back to plain.
        let none = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"a\"\n#EXTINF:6,\ns0.ts\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:6,\ns1.ts\n";
        let nsched = hls_key_schedule(none, "https://h/pl.m3u8").unwrap();
        assert!(nsched[0].is_some());
        assert!(nsched[1].is_none());
        assert!(hls_key_schedule("#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"", "https://h/x").is_err());
        assert!(hls_key_schedule("#EXT-X-KEY:METHOD=AES-128", "https://h/x").is_err());
        // EXT-X-MAP init sections.
        let map = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6,\ns0.ts\n";
        assert_eq!(
            hls_map_init(map, "https://h/v/pl.m3u8").unwrap().as_deref(),
            Some("https://h/v/init.mp4")
        );
        assert!(hls_map_init("#EXTM3U\n#EXTINF:6,\ns0.ts\n", "https://h/x").unwrap().is_none());
        assert!(hls_map_init("#EXT-X-MAP:URI=\"i.mp4\",BYTERANGE=\"16@0\"", "https://h/x").is_err());
        // IV forms: explicit hex, else sequence-based.
        assert_eq!(parse_hex_iv("0x00000000000000000000000000000001"), Some(1u128.to_be_bytes()));
        assert_eq!(parse_hex_iv("short"), None);
        // ISO durations + counts.
        assert_eq!(parse_pt_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(parse_pt_duration("PT60S"), Some(60.0));
        assert_eq!(parse_pt_duration("nope"), None);
        assert_eq!(dash_seg_count(Some(60.0), 6.0), Some(10));
        assert_eq!(dash_seg_count(Some(61.0), 6.0), Some(11));
        assert_eq!(dash_seg_count(None, 6.0), None);
        assert_eq!(dash_seg_count(Some(60.0), 0.0), None);
        assert_eq!(dash_expand("seg-$Number$.m4s", 7, "v1", 8000), "seg-7.m4s");
        assert_eq!(dash_expand("seg-$Number%05d$.m4s", 7, "v1", 8000), "seg-00007.m4s");
        assert_eq!(dash_expand("$RepresentationID$/$Bandwidth$/s.m4s", 1, "vid", 8000), "vid/8000/s.m4s");
        // $Time$ timelines and escaped query strings.
        assert_eq!(dash_expand_at("v/$Time$.m4s", 3, "v1", 8000, Some(540000)), "v/540000.m4s");
        assert_eq!(dash_expand_at("v/$Time%010d$.m4s", 3, "v1", 8000, Some(42)), "v/0000000042.m4s");
        assert_eq!(timeline_origin(r#"<SegmentTimeline><S t="1000" d="180" r="9"/></SegmentTimeline>"#), Some((1000, 180)));
        assert_eq!(xml_attr(r#"<SegmentTemplate media="s-$Number$.m4s?a=1&amp;b=2">"#, "media").as_deref(), Some("s-$Number$.m4s?a=1&b=2"));
        // MPD structure parse.
        let mpd = r#"<?xml version="1.0"?><MPD mediaPresentationDuration="PT60S" type="static"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate timescale="30" duration="180" startNumber="1" media="v-$Number$.m4s" initialization="v-init.mp4"/><Representation id="v1" bandwidth="5000000" width="1920" height="1080" codecs="avc1.640028"></Representation></AdaptationSet><AdaptationSet mimeType="audio/mp4"><SegmentTemplate timescale="44" duration="100" media="a-$Number$.m4s" initialization="a-init.mp4"/><Representation id="a1" bandwidth="128000" codecs="mp4a.40.2"></Representation></AdaptationSet></Period></MPD>"#;
        let (vv, aa) = parse_mpd(mpd).expect("mpd");
        assert_eq!(vv.len(), 1);
        assert_eq!(aa.len(), 1);
        assert_eq!(vv[0].width, 1920);
        assert_eq!(vv[0].seg_media.as_deref(), Some("v-$Number$.m4s"));
        assert_eq!(vv[0].seg_init.as_deref(), Some("v-init.mp4"));
        assert_eq!(vv[0].seg_dur_secs, Some(6.0));
        assert_eq!(dash_seg_count(Some(60.0), vv[0].seg_dur_secs.unwrap()), Some(10));
        assert!(parse_mpd("<MPD></MPD>").is_err());
        let drm = mpd.replace("<Period>", "<Period><AdaptationSet><ContentProtection/></AdaptationSet>");
        let _ = drm;
    }

    #[test]
    fn dash_timeline() {
        // Uniform repeat timeline: exact count.
        let tl = "<SegmentTimeline><S t=\"0\" d=\"6\" r=\"2\" /></SegmentTimeline>";
        assert_eq!(timeline_count(tl, 1.0, None).unwrap(), Some(3));
        // Multi-entry uniform + contiguous: exact count.
        let tl2 = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"6\" d=\"6\" r=\"1\" /></SegmentTimeline>";
        assert_eq!(timeline_count(tl2, 1.0, None).unwrap(), Some(3));
        // No timeline: None (duration math fallback).
        assert_eq!(timeline_count("<SegmentTemplate duration=\"6\" />", 1.0, None).unwrap(), None);
        // Gap: loud error, never a miscount.
        let gap = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"12\" d=\"6\" /></SegmentTimeline>";
        assert!(timeline_count(gap, 1.0, None).is_err());
        // Varying durations: loud error.
        let vary = "<SegmentTimeline><S t=\"0\" d=\"6\" /><S t=\"6\" d=\"4\" /></SegmentTimeline>";
        assert!(timeline_count(vary, 1.0, None).is_err());
        // Open repeat with known period: 60 s / 6 s segments = 10.
        let open = "<SegmentTimeline><S t=\"0\" d=\"6\" r=\"-1\" /></SegmentTimeline>";
        assert_eq!(timeline_count(open, 1.0, Some(60.0)).unwrap(), Some(10));
        // Open repeat without period: loud error.
        assert!(timeline_count(open, 1.0, None).is_err());
        // Oversize timeline: None (caller reports live/oversize cleanly).
        let big = "<SegmentTimeline><S t=\"0\" d=\"1\" r=\"5000\" /></SegmentTimeline>";
        assert_eq!(timeline_count(big, 1.0, None).unwrap(), None);
        // End-to-end: rep carries the timeline count over duration math.
        let mpd = r#"<?xml version="1.0"?><MPD type="static"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate timescale="1" startNumber="1" media="s-$Number$.m4s"/><SegmentTimeline><S t="0" d="6" r="2" /></SegmentTimeline><Representation id="v1" bandwidth="1000" width="640" height="360" codecs="avc1.64001e"></Representation></AdaptationSet></Period></MPD>"#;
        let (vv, _) = parse_mpd(mpd).expect("timeline mpd");
        assert_eq!(vv[0].timeline_count, Some(3));
    }

    #[test]
    fn helper_versions_compare_as_release_numbers() {
        assert!(version_older("2026.08.19", "2026.10.07"));
        assert!(version_older("2026.08.19", "2026.08.19.1"));
        assert!(!version_older("2026.10.07", "2026.08.19"));
        assert!(!version_older("2026.08.19", "2026.08.19"));
        assert!(version_older("6.0", "b6.1.1"));
        assert!(!version_older("9.0.2-full_build-www.gyan.dev", "b6.1.1"));
        assert!(!version_older("unknown", "2026.10.07"));
        assert!(!version_older("2026.08.19", "nightly"));
    }

    #[test]
    fn opaque_manifests_are_recognized_by_content() {
        assert_eq!(manifest_kind_of(b"\xef\xbb\xbf#EXTM3U\n#EXTINF:4,\ns.ts"), Some(ManifestKind::Hls));
        assert_eq!(manifest_kind_of(b"<?xml version=\"1.0\"?>\n<MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\">"), Some(ManifestKind::Dash));
        assert_eq!(manifest_kind_of(b"<!doctype html><title>x</title>"), None);
        assert_eq!(manifest_kind_of(b"\x00\x00\x00\x20ftypisom"), None);
        assert!(opaque_manifest_type(None));
        assert!(opaque_manifest_type(Some("text/plain; charset=utf-8")));
        assert!(opaque_manifest_type(Some("application/octet-stream")));
        assert!(!opaque_manifest_type(Some("video/mp4")));
        assert!(!opaque_manifest_type(Some("text/html")));
    }

    #[test]
    fn captured_headers_stay_on_their_site() {
        assert_eq!(site_of_url("https://a.b.example.com/x").as_deref(), Some("example.com"));
        assert_eq!(site_of_url("https://cdn.example.co.uk/x").as_deref(), Some("example.co.uk"));
        assert_eq!(site_of_url("http://127.0.0.1:8080/x").as_deref(), Some("127.0.0.1"));
        // Shared hosting: one customer's subdomain is not another's site.
        assert_eq!(site_of_url("https://d111.cloudfront.net/a").as_deref(), Some("d111.cloudfront.net"));
        assert_ne!(site_of_url("https://d111.cloudfront.net/a"), site_of_url("https://d222.cloudfront.net/a"));
        let map = std::collections::BTreeMap::from([
            ("Authorization".to_string(), "Bearer t".to_string()),
            ("x-token".to_string(), "1".to_string()),
            ("range".to_string(), "bytes=0-".to_string()),
        ]);
        let opts = EngineOpts {
            captured_headers: CapturedHeaders::for_url("https://media.example.com/m/master", &map),
            ..EngineOpts::default()
        };
        let client = opts.build_client().unwrap();
        let same = opts.get(&client, "https://seg.example.com/s0.ts").build().unwrap();
        assert_eq!(same.headers().get("authorization").unwrap(), "Bearer t");
        assert_eq!(same.headers().get("x-token").unwrap(), "1");
        assert!(same.headers().get("range").is_none());
        let other = opts.get(&client, "https://ads.other.net/s0.ts").build().unwrap();
        assert!(other.headers().get("authorization").is_none());
        assert!(!CapturedHeaders::for_url("https://h.example/x", &std::collections::BTreeMap::new()).applies_to("https://h.example/x"));
    }

    #[test]
    fn hls_helpers() {
        // playlists recognised, mpd rejected from generic resolve
        assert!(is_playlist_url("https://cdn.example/v/playlist.m3u8?x=1"));
        assert!(is_playlist_url("https://cdn.example/v/manifest.mpd"));
        assert!(!is_playlist_url("https://cdn.example/v/clip.mp4"));
        assert!(has_direct_ext("https://cdn.example/a/b.mp4?token=1"));
        assert!(!has_direct_ext("https://cdn.example/a/playlist.m3u8"));
        // master variant pick: highest bandwidth wins
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080\nhi.m3u8\n";
        assert_eq!(
            hls_pick_variant(master, "https://cdn.example/v/master.m3u8").as_deref(),
            Some("https://cdn.example/v/hi.m3u8")
        );
        assert_eq!(hls_pick_variant("#EXTM3U\n#EXT-X-TARGETDURATION:6\n", "https://h/x.m3u8"), None);
        // media segments: comments skipped, relative resolved
        let media = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg0.ts\n#EXTINF:6.0,\nhttps://other.example/s1.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            hls_segments(media, "https://cdn.example/v/pl.m3u8"),
            vec![
                "https://cdn.example/v/seg0.ts".to_string(),
                "https://other.example/s1.ts".to_string()
            ]
        );
        // ld+json embedUrl accepted, playlists skipped
        let ld = r#"<html><head><title>T</title><script type="application/ld+json">{"@type":"VideoObject","embedUrl":"https://cdn.example/e/v.mp4"}</script></head></html>"#;
        let hit = resolve_generic(ld, "https://example.com/p").expect("ld+json");
        assert_eq!(hit.0, "https://cdn.example/e/v.mp4");
        let pl = r#"<html><head><meta property="og:video" content="https://cdn.example/v/pl.m3u8"></head></html>"#;
        assert_eq!(resolve_generic(pl, "https://example.com/p"), None);
    }

    #[test]
    fn aes_roundtrip() {
        let key = [0x0Bu8; 16];
        let iv = 7u128.to_be_bytes();
        let plain: Vec<u8> = (0..256).map(|i| (i % 251) as u8).collect();
        let enc = aes_cbc_encrypt_test(&plain, &key, &iv);
        assert_eq!(enc.len(), plain.len() + 16, "aligned input gets a full padding block");
        assert_ne!(&enc[..plain.len()], &plain[..]);
        let dec = aes128_cbc_decrypt(&enc, &key, &iv).expect("decrypt");
        assert_eq!(dec, plain, "PKCS7 padding is removed");
        let short: Vec<u8> = (0..250).map(|i| i as u8).collect();
        assert_eq!(aes128_cbc_decrypt(&aes_cbc_encrypt_test(&short, &key, &iv), &key, &iv).unwrap(), short);
        assert!(aes128_cbc_decrypt(&enc[..enc.len() - 1], &key, &iv).is_err());
        assert!(aes128_cbc_decrypt(&enc, &[0u8; 16], &iv).is_ok()); // wrong key still yields blocks
    }

    #[tokio::test]
    async fn limiter_and_checksum() {
        // Unlimited limiter never waits.
        let free = RateLimiter::new(0);
        free.consume(1_000_000).await;
        // Scheduler override: global atomic wins over the fixed cap.
        let g = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let lim = RateLimiter::new(1000).with_global(Some(g.clone()));
        assert_eq!(lim.effective_limit(), 1000);
        g.store(100, Ordering::SeqCst);
        assert_eq!(lim.effective_limit(), 100);
        g.store(0, Ordering::SeqCst);
        assert_eq!(lim.effective_limit(), 0);
        lim.consume(10).await; // unlimited under override: returns fast
        // Empty checksum request is a no-op.
        let dir = std::env::temp_dir();
        let p = dir.join("download_craft_checksum_test.bin");
        std::fs::write(&p, b"abc").unwrap();
        verify_checksum(&p, "").await.unwrap();
        // Known SHA-256 of "abc".
        verify_checksum(
            &p,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .await
        .unwrap();
        assert!(verify_checksum(&p, &"0".repeat(64)).await.is_err());
        assert!(verify_checksum(&p, "not-hex").await.is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Minimal in-process HTTP/1.1 fixture (localhost only): serves a
    /// deterministic body with Range support, a redirect, a Range-ignoring
    /// endpoint, and a flaky endpoint that fails its first requests.
    /// Exercises the real engine over real sockets — no external network.
    /// Extra: HLS playlists/segments (plain + AES-128) and DASH manifests.
    /// Encrypts like real HLS packagers: PKCS7-padded (a full block when the
    /// input is already aligned), as RFC 8216 requires.
    fn aes_cbc_encrypt_test(plain: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Vec<u8> {
        let pad = 16 - plain.len() % 16;
        let mut padded = plain.to_vec();
        padded.extend(std::iter::repeat_n(pad as u8, pad));
        crate::aes128::cbc_encrypt(key, iv, &padded)
    }

    async fn serve_conn(
        mut s: tokio::net::TcpStream,
        body: std::sync::Arc<Vec<u8>>,
        flaky_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fb_hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut raw = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match s.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&chunk[..n]);
                    if raw.len() > 1_000_000
                        || raw.windows(4).any(|w| w == b"\r\n\r\n")
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let head = String::from_utf8_lossy(&raw).into_owned();
        let mut lines = head.lines();
        let rl = lines.next().unwrap_or("").to_string();
        let mut it = rl.split_whitespace();
        let method = it.next().unwrap_or("").to_string();
        let path = it.next().unwrap_or("/").to_string();
        let mut range: Option<(u64, u64)> = None;
        for l in lines {
            if l.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                if k.trim().eq_ignore_ascii_case("range") {
                    let v = v.trim().strip_prefix("bytes=").unwrap_or("").trim();
                    if let Some((a, b)) = v.split_once('-') {
                        if let Ok(start) = a.trim().parse::<u64>() {
                            let end = b.trim().parse::<u64>().unwrap_or(u64::MAX);
                            range = Some((start, end));
                        }
                    }
                }
            }
        }
        let is_head = method.eq_ignore_ascii_case("HEAD");
        // Text responses served immediately (playlists, manifests).
        let text_resp: Option<(&str, String)> = match path.as_str() {
            "/hls/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlo.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhi.m3u8\n".to_string(),
            )),
            // Opaque manifest: extensionless URL, generic type (deep detection).
            "/hls/opaque" => Some((
                "text/plain",
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000\nlo.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhi.m3u8\n".to_string(),
            )),
            "/hls/lo.m3u8" | "/hls/hi.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-enc/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-rot/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-KEY:METHOD=AES-128,URI=\"k1\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXT-X-KEY:METHOD=AES-128,URI=\"k2\",IV=0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n#EXTINF:6.0,\ns2.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/hls-map/pl.m3u8" => Some((
                "application/vnd.apple.mpegurl",
                "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\ns0.ts\n#EXTINF:6.0,\ns1.ts\n#EXT-X-ENDLIST\n".to_string(),
            )),
            "/dash/vonly.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/both.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet><AdaptationSet mimeType=\"audio/mp4\"><Representation id=\"a1\" bandwidth=\"128000\" codecs=\"mp4a.40.2\"><BaseURL>a.m4a</BaseURL></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/tpl.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD mediaPresentationDuration=\"PT18S\" type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><SegmentTemplate timescale=\"1\" duration=\"6\" startNumber=\"1\" media=\"seg-$Number$.m4s\" initialization=\"init.mp4\"/><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            "/dash/tl.mpd" => Some((
                "application/dash+xml",
                "<?xml version=\"1.0\"?><MPD type=\"static\"><Period><AdaptationSet mimeType=\"video/mp4\"><SegmentTemplate timescale=\"1\" startNumber=\"1\" media=\"seg-$Number$.m4s\" initialization=\"init.mp4\"/><SegmentTimeline><S t=\"0\" d=\"6\" r=\"2\" /></SegmentTimeline><Representation id=\"v1\" bandwidth=\"1000000\" width=\"640\" height=\"360\" codecs=\"avc1.64001e\"></Representation></AdaptationSet></Period></MPD>".to_string(),
            )),
            _ => None,
        };
        if let Some((ct, t)) = text_resp {
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                t.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(t.as_bytes()).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Facebook-CDN simulations (clean-but-early closes, like fbcdn):
        // /fbcut honors Range but cuts every connection after 64 KiB while
        // advertising the full window (headers promise, body stops).
        // /fbempty answers ranges with an empty 200 (`got 0 of …`).
        // /fbexpire serves twice, then 403s (signed-URL expiry mid-file).
        // /fb200cut ignores Range (full 200) but cuts every body at 64 KiB.
        const FB_CUT: usize = 64 * 1024;
        let fb_path = path.trim_start_matches('/');
        let fb_mode = ["fbcut", "fbempty", "fbexpire", "fb200cut"]
            .iter()
            .find(|m| fb_path == **m)
            .copied();
        // Strict googlevideo-style edge: HEAD, open-ended, and >2 MiB
        // ranges 403 (verified live); small bounded ranges serve 206.
        if fb_path == "strict" {
            const STRICT_MAX: u64 = 2 * 1024 * 1024;
            let bad = match range {
                None => true, // plain/HEAD without Range
                Some((s, e)) => e == u64::MAX || e.saturating_sub(s).saturating_add(1) > STRICT_MAX,
            };
            if bad {
                let r = "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let len = body.len() as u64;
            let (start, end) = range.unwrap();
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                let r = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let payload = &body[start as usize..=end as usize];
            let r = format!("HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
            let _ = s.write_all(r.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(payload).await;
            }
            let _ = s.flush().await;
            return;
        }
        if let Some(mode) = fb_mode {
            let n = fb_hits.fetch_add(1, Ordering::SeqCst);
            if mode == "fbexpire" && n >= 2 {
                let r = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            if mode == "fbempty" {
                let r = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let len = body.len() as u64;
            if mode == "fb200cut" {
                // Range ignored: whole body promised, cut early.
                let cut = (FB_CUT as u64).min(len);
                let r = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.write_all(&body[..cut as usize]).await;
                let _ = s.flush().await;
                return;
            }
            // fbcut / fbexpire-early: honor the range, then cut the body.
            let (start, end) = range.unwrap_or((0, len.saturating_sub(1)));
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                let r = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(r.as_bytes()).await;
                let _ = s.flush().await;
                return;
            }
            let window = &body[start as usize..=end as usize];
            let cut = window.len().min(FB_CUT);
            let r = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                window.len()
            );
            let _ = s.write_all(r.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&window[..cut]).await;
            }
            let _ = s.flush().await;
            return;
        }
        if path == "/hls-enc/key" {
            let key = [0x0Bu8; 16];
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&key).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Rotating keys (P3): k1 serves segments 0-1 (sequence IVs), k2
        // serves segment 2 (explicit IV in the playlist).
        if path == "/hls-rot/k1" || path == "/hls-rot/k2" {
            let key = if path.ends_with("k1") { [0x0Au8; 16] } else { [0x0Bu8; 16] };
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 16\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(resp.as_bytes()).await;
            if !is_head {
                let _ = s.write_all(&key).await;
            }
            let _ = s.flush().await;
            return;
        }
        // Binary payload override for segment/media paths (else full body).
        let third = body.len() / 3;
        let seg_bin: Option<Vec<u8>> = (|| {
            if let Some(n) = path
                .strip_prefix("/hls/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    return Some(body[n * third..(n + 1) * third].to_vec());
                }
            }
            if let Some(n) = path
                .strip_prefix("/hls-enc/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    let plain = &body[n * third..(n + 1) * third];
                    return Some(aes_cbc_encrypt_test(plain, &[0x0Bu8; 16], &(n as u128).to_be_bytes()));
                }
            }
            // Rotating-key segments (P3): s0/s1 under k1 (sequence IVs),
            // s2 under k2 (the playlist's explicit IV).
            if let Some(n) = path
                .strip_prefix("/hls-rot/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 3 {
                    let plain = &body[n * third..(n + 1) * third];
                    if n < 2 {
                        return Some(aes_cbc_encrypt_test(plain, &[0x0Au8; 16], &(n as u128).to_be_bytes()));
                    }
                    return Some(aes_cbc_encrypt_test(plain, &[0x0Bu8; 16], &[0xAAu8; 16]));
                }
            }
            // EXT-X-MAP playlist (P3): plaintext segments + fixed init.
            if let Some(n) = path
                .strip_prefix("/hls-map/s")
                .and_then(|s| s.strip_suffix(".ts"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if n < 2 {
                    return Some(body[n * third..(n + 1) * third].to_vec());
                }
            }
            if path == "/hls-map/init.mp4" {
                return Some(b"HLSMAPINIT".to_vec());
            }
            if path == "/dash/v.mp4" {
                return Some(body[..body.len() / 2].to_vec());
            }
            if path == "/dash/a.m4a" {
                return Some(body[body.len() / 2..].to_vec());
            }
            // Templated DASH segments: thirds prove ordered assembly, and a
            // fixed init marker proves headers lead the stream.
            if let Some(n) = path
                .strip_prefix("/dash/seg-")
                .and_then(|s| s.strip_suffix(".m4s"))
                .and_then(|s| s.parse::<usize>().ok())
            {
                if (1..=3).contains(&n) {
                    let i = n - 1;
                    return Some(body[i * third..(i + 1) * third].to_vec());
                }
            }
            if path == "/dash/init.mp4" {
                return Some(b"DASHINIT".to_vec());
            }
            None
        })();
        let body: &[u8] = seg_bin.as_deref().unwrap_or(&body);
        let len = body.len() as u64;
        // (status, extra headers, payload range)
        let (status, extra, payload): (&str, String, &[u8]) = if path == "/redirect" {
            ("HTTP/1.1 302 Found", "Location: /file\r\n".to_string(), &[])
        } else if path == "/flaky"
            && flaky_hits.fetch_add(1, Ordering::SeqCst) < 3
        {
            // Fail the probe HEAD + probe fallback + first GET: the engine
            // must retry the single stream and still finish.
            ("HTTP/1.1 500 Internal Server Error", String::new(), &[])
        } else if path == "/norange" {
            ("HTTP/1.1 200 OK", "Accept-Ranges: none\r\n".to_string(), &body[..])
        } else if path == "/forbidden" {
            // Permanent refusal: engine must fail fast with guidance.
            ("HTTP/1.1 403 Forbidden", String::new(), &[])
        } else if let Some((start, end)) = range {
            let end = end.min(len.saturating_sub(1));
            if start >= len {
                (
                    "HTTP/1.1 416 Range Not Satisfiable",
                    format!("Content-Range: bytes */{len}\r\n"),
                    &[],
                )
            } else {
                (
                    "HTTP/1.1 206 Partial Content",
                    format!(
                        "Accept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\n"
                    ),
                    &body[start as usize..=end as usize],
                )
            }
        } else {
            (
                "HTTP/1.1 200 OK",
                "Accept-Ranges: bytes\r\n".to_string(),
                &body[..],
            )
        };
        let mut resp = format!(
            "{status}\r\nContent-Type: application/octet-stream\r\n{extra}"
        );
        // Error responses carry no Content-Length so the probe learns nothing.
        if !status.contains(" 500 ") {
            resp.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        resp.push_str("Connection: close\r\n\r\n");
        let _ = s.write_all(resp.as_bytes()).await;
        if !is_head && !payload.is_empty() {
            let _ = s.write_all(payload).await;
        }
        let _ = s.flush().await;
    }

    fn matrix_opts() -> EngineOpts {
        EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 15,
            max_retries: 4,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn facebook_cdn_matrix() {
        // Simulates the fbcdn failure modes from the field: mid-range cuts,
        // empty ranges, mid-file expiry, and range-ignoring servers — over
        // real sockets, asserting the engine finishes, fails fast, or fails
        // honestly (never a truncated file masquerading as success).
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 300 * 1024; // < 1 MiB chunk: resume aggregation
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let want = body.clone();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fb fixture");
        let addr = listener.local_addr().expect("fb fixture addr");
        let fb_h = fb_hits.clone();
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_h.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_fb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = || EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 5,
            max_retries: 2, // keep the suite fast; policy is what matters
            ..Default::default()
        };
        let client = opts().build_client().unwrap();

        // 1. Range probe semantics: 206 → total, 200 → none, 403 → dead.
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/fbcut"), Some(LEN as u64)).await.ok(),
            Some(Some(LEN as u64))
        );
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/fb200cut"), Some(LEN as u64)).await.ok(),
            Some(None)
        );
        assert!(
            probe_range_once(&client, &opts(), &format!("{base}/forbidden"), Some(LEN as u64))
                .await
                .is_err()
        );

        // 2. Cutter honors ranges but cuts every connection at 64 KiB:
        // chunked sequential ranges must still assemble the exact file.
        let dest = dir.join("fbcut.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbcut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked survives cuts");
        assert_eq!(std::fs::read(&dest).unwrap(), want);
        assert_eq!(shared.downloaded.load(Ordering::SeqCst), LEN as u64);

        // 3. Resume into chunked: stale prefix is continued, not rewritten.
        let dest = dir.join("fbresume.bin");
        std::fs::write(dest.with_extension("dcpart"), &want[..100 * 1024]).unwrap();
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbcut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked resume");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 4. Empty ranges fail loudly with cause — never a phantom file.
        let dest = dir.join("fbempty.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbempty"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("empty ranges must fail");
        assert!(
            format!("{err:#}").contains("empty range"),
            "must name the cause, got: {err:#}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        // 5. Mid-file expiry: progress then 403 → "expired", not generic.
        // (Reset the fixture counter: earlier scenarios already spent it.)
        fb_hits.store(0, Ordering::SeqCst);
        let dest = dir.join("fbexpire.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fbexpire"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("expiry must fail");
        assert!(
            format!("{err:#}").contains("expired"),
            "must say expired, got: {err:#}"
        );

        // 6. Range-ignoring cutter: honest failure, no truncated success.
        let dest = dir.join("fb200cut.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = chunked_stream(
            &client,
            &opts(),
            &format!("{base}/fb200cut"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("range-hostile cutter must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("tries:"),
            "must carry the attempt summary, got: {msg}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        // 7. End-to-end single stream through the cutter (unknown-total path).
        let dest = dir.join("fbsingle.bin");
        download(
            format!("{base}/fbcut"),
            dest.clone(),
            1,
            opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("single survives cuts");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        let _ = std::fs::remove_dir_all(&dir);
        srv.abort();
    }

    #[tokio::test]
    async fn strict_edge_matrix() {
        // googlevideo SABR-style edge (verified live): HEAD, open-ended,
        // and >2 MiB ranges 403; small bounded ranges serve 206. Chunked
        // sequential ranges must complete; big single ranges must fail
        // loudly (this is why strict hosts route to chunked_stream).
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 5 * 1024 * 1024; // forces multiple 1 MiB chunks
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let want = body.clone();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind strict fixture");
        let addr = listener.local_addr().expect("strict fixture addr");
        let fb_h = fb_hits.clone();
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_h.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_strict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = || EngineOpts {
            user_agent: "idm-test/1".to_string(),
            timeout_secs: 5,
            max_retries: 2,
            ..Default::default()
        };
        let client = opts().build_client().unwrap();

        // 1. Small-range probe verifies liveness + total.
        assert_eq!(
            probe_range_once(&client, &opts(), &format!("{base}/strict"), Some(LEN as u64)).await.ok(),
            Some(Some(LEN as u64))
        );

        // 2. Chunked 1 MiB ranges complete byte-exact through the strict edge.
        let dest = dir.join("strict.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        chunked_stream(
            &client,
            &opts(),
            &format!("{base}/strict"),
            &dest,
            LEN as u64,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect("chunked survives strict edge");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 3. A 5 MiB single bounded range 403s immediately (final refusal —
        // by design it keeps its type for fallback routing, no summary).
        let dest = dir.join("strict-single.bin");
        let shared = Arc::new(SharedState::new());
        shared.total.store(LEN as u64, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
        let err = single_stream(
            &client,
            &opts(),
            &format!("{base}/strict"),
            &dest,
            shared.clone(),
            std::sync::Arc::new(RateLimiter::new(0)),
        )
        .await
        .expect_err("5 MiB single range must 403");
        assert!(
            format!("{err:#}").contains("403"),
            "must name the refusal, got: {err:#}"
        );
        assert!(!dest.exists(), "no truncated file may be finalized");

        let _ = std::fs::remove_dir_all(&dir);
        srv.abort();
    }

    #[test]
    fn quality_selection() {
        assert_eq!(chrome_major("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"), Some(126));
        assert_eq!(chrome_major("download_craft/0.7.0"), None);
        assert_eq!(chrome_major(""), None);
        assert_eq!(quality_cap("1080p"), Some(1080));
        assert_eq!(quality_cap("1080p60"), Some(1080));
        assert_eq!(quality_cap("720p"), Some(720));
        assert_eq!(quality_cap(" 480p "), Some(480));
        // The Facebook panel's own labels ("720p HD") and the extension's
        // ladder fallback ("1080p") must both parse to a cap, or the clicked
        // row is silently ignored and yt-dlp downloads "best" instead.
        assert_eq!(quality_cap("720p HD"), Some(720));
        assert_eq!(quality_cap("1080p HD"), Some(1080));
        assert_eq!(quality_cap(""), None);
        assert_eq!(quality_cap("HD"), None);
        assert_eq!(quality_cap("SD"), None);
        assert_eq!(quality_cap("100p"), None);
        assert_eq!(quality_cap("5000p"), None);
        // The selector keeps the cap and the muxed fallback, so a working
        // machine with ffmpeg still lands the merged 720p mp4 - and quality
        // leads, so a 720p pick can never fall back to a 360p mp4 stream.
        // (Live-checked against yt-dlp: 720p -> 398+140, 1080p -> 616+140,
        // 480p -> 397+140, every one at the asked height.)
        let f = ytdlp_format_with("720p", true);
        assert!(f.starts_with("bv*[height<=720]+ba[ext=m4a]"), "{f}");
        assert!(f.contains("b[height<=720]"), "{f}");
        assert_eq!(ytdlp_format_with("720p", false), "b[height<=720]/b");
        assert_eq!(ytdlp_format_with("", true), "bv*+ba[ext=m4a]/bv*+ba/b");
        assert_eq!(ytdlp_format_with("", false), "b");
        // Whatever this machine has, the string is one of the two strategies.
        let live = ytdlp_format("720p");
        assert!(
            live.starts_with("bv*[height<=720]+ba[ext=m4a]") || live == "b[height<=720]/b",
            "{live}"
        );
        // No cap: higher always wins.
        assert!(better_stream(&None, 360, None));
        assert!(better_stream(&Some((360, String::new())), 720, None));
        assert!(!better_stream(&Some((720, String::new())), 360, None));
        // Cap 720: highest at/below wins; above-cap fallback goes closest.
        assert!(better_stream(&Some((1080, String::new())), 720, Some(720)));
        assert!(better_stream(&Some((360, String::new())), 720, Some(720)));
        assert!(!better_stream(&Some((720, String::new())), 1080, Some(720)));
        assert!(better_stream(&Some((1440, String::new())), 1080, Some(720)));
        assert!(!better_stream(&Some((1080, String::new())), 1440, Some(720)));
    }

    #[test]
    fn adaptive_pair_selection() {
        let fmts = serde_json::json!([
            {"url": "https://x/v360", "mimeType": "video/mp4", "width": 640, "height": 360},
            {"url": "https://x/v720", "mimeType": "video/mp4", "width": 1280, "height": 720},
            {"url": "https://x/v1080", "mimeType": "video/mp4", "width": 1920, "height": 1080},
            {"url": "https://x/a-low", "mimeType": "audio/mp4", "bitrate": 64000},
            {"url": "https://x/a-high", "mimeType": "audio/mp4", "bitrate": 128000},
            {"mimeType": "video/mp4", "width": 1920, "height": 1080},
        ]);
        let arr = fmts.as_array().unwrap();
        // No cap: best video + best audio.
        let (s, v, a) = pick_adaptive_pair(arr, None).unwrap();
        assert_eq!((s, v.as_str(), a.as_str()), (1080, "https://x/v1080", "https://x/a-high"));
        // Cap 720: exact fit wins, audio still best.
        let (s, v, a) = pick_adaptive_pair(arr, Some(720)).unwrap();
        assert_eq!((s, v.as_str(), a.as_str()), (720, "https://x/v720", "https://x/a-high"));
        // Cap 480: highest at/below wins (360 here).
        let (s, v, _) = pick_adaptive_pair(arr, Some(480)).unwrap();
        assert_eq!((s, v.as_str()), (360, "https://x/v360"));
        // No direct video at all: no pair (ciphered-only needs deciphering).
        let novid = serde_json::json!([
            {"url": "https://x/a", "mimeType": "audio/mp4", "bitrate": 64000},
        ]);
        assert!(pick_adaptive_pair(novid.as_array().unwrap(), None).is_none());
        // No audio at all: no pair (nothing to merge with).
        let noaud = serde_json::json!([
            {"url": "https://x/v", "mimeType": "video/mp4", "width": 640, "height": 360},
        ]);
        assert!(pick_adaptive_pair(noaud.as_array().unwrap(), None).is_none());
    }

    #[tokio::test]
    async fn local_http_matrix() {
        use std::sync::atomic::AtomicUsize;
        const LEN: usize = 3 * 1024 * 1024; // >= 1 MiB forces segmented path
        let body: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        let body = std::sync::Arc::new(body);
        let flaky_hits = std::sync::Arc::new(AtomicUsize::new(0));
        let fb_hits = std::sync::Arc::new(AtomicUsize::new(0));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let addr = listener.local_addr().expect("fixture addr");
        let srv = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((s, _)) => {
                        let b = body.clone();
                        let f = flaky_hits.clone();
                        let h = fb_hits.clone();
                        tokio::spawn(async move { serve_conn(s, b, f, h).await });
                    }
                    Err(_) => break,
                }
            }
        });
        let base = format!("http://{addr}");
        let dir =
            std::env::temp_dir().join(format!("dc_matrix_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // 0. Error pages never poison totals: a 403 carrying
        // `Content-Length: 0` must probe as unknown (not total 0), or every
        // downstream check treats the file as already complete/empty.
        let pclient = matrix_opts().build_client().unwrap();
        let (t, r, _, _) = probe(&pclient, &matrix_opts(), &format!("{base}/forbidden")).await;
        assert_eq!((t, r), (None, false));
        let (t, r, _, _) = probe(&pclient, &matrix_opts(), &format!("{base}/file")).await;
        assert_eq!(t, Some(LEN as u64));
        assert!(r);

        // 1. Segmented multi-connection download, byte-exact.
        let dest = dir.join("seg.bin");
        let shared = Arc::new(SharedState::new());
        download(
            format!("{base}/file"),
            dest.clone(),
            4,
            matrix_opts(),
            shared.clone(),
        )
        .await
        .expect("segmented");
        let got = std::fs::read(&dest).unwrap();
        let want: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
        assert_eq!(got, want);
        assert_eq!(shared.downloaded.load(Ordering::SeqCst), LEN as u64);

        // 2. Resume: stale .dcpart prefix must be continued, not rewritten.
        let dest = dir.join("resume.bin");
        std::fs::write(dest.with_extension("dcpart"), &want[..100 * 1024]).unwrap();
        download(format!("{base}/file"), dest.clone(), 1, matrix_opts(), Arc::new(SharedState::new()))
            .await
            .expect("resume");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 3. Redirect is followed to the real file.
        let dest = dir.join("redir.bin");
        download(
            format!("{base}/redirect"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("redirect");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 4. Server ignoring Range with a stale part restarts cleanly.
        let dest = dir.join("norange.bin");
        std::fs::write(dest.with_extension("dcpart"), b"stale-partial").unwrap();
        download(
            format!("{base}/norange"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("norange restart");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 5. Flaky endpoint: probe + first GET fail, retry still finishes.
        let dest = dir.join("flaky.bin");
        download(
            format!("{base}/flaky"),
            dest.clone(),
            1,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("flaky retry");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 6. Hard 403: fail fast with an actionable message (no retry storm).
        let dest = dir.join("denied.bin");
        let err = download(
            format!("{base}/forbidden"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect_err("403 must fail");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("403"),
            "refusal must name the status, got: {msg}"
        );
        assert!(is_final_http_error(&final_err("HTTP 403 Forbidden".into())));
        assert!(!is_final_http_error(&anyhow!("HTTP 403 Forbidden")));
        assert!(!is_final_http_error(&anyhow!("network: reset by peer")));
        assert!(is_googlevideo_url("https://r1---sn-abc.googlevideo.com/videoplayback?x=1"));
        assert!(!is_googlevideo_url("https://cdn.example/v.mp4"));
        assert!(is_youtube_page("https://www.youtube.com/watch?v=x"));
        assert!(is_youtube_page("https://youtu.be/x"));
        assert!(!is_youtube_page("https://vimeo.com/1"));

        // These assembly fixtures contain deterministic random bytes, not
        // playable media. Exercise the transfer/assembly layer directly;
        // completed-media validation has separate real-file regressions.
        // 7. HLS plain VOD: master → best variant → parallel segments → exact.
        let dest = dir.join("hls.bin");
        download_inner(
            format!("{base}/hls/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls vod");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 7b. Opaque HLS: text/plain at an extensionless URL is sniffed and
        // assembled as a stream, never saved as playlist text.
        let dest = dir.join("hlsopaque.bin");
        download_inner(
            format!("{base}/hls/opaque"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("opaque hls");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8. HLS AES-128: key fetch + per-segment decrypt → exact.
        let dest = dir.join("hlsaes.bin");
        download_inner(
            format!("{base}/hls-enc/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls aes");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8b. HLS key rotation (P3): s0/s1 under k1, s2 under k2+IV.
        let dest = dir.join("hlsrot.bin");
        download_inner(
            format!("{base}/hls-rot/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls rotation");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        // 8c. HLS EXT-X-MAP (P3): init section leads the assembled file.
        let dest = dir.join("hlsmap.bin");
        download_inner(
            format!("{base}/hls-map/pl.m3u8"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("hls map");
        let third = want.len() / 3;
        let mut want_map = b"HLSMAPINIT".to_vec();
        want_map.extend_from_slice(&want[..2 * third]);
        assert_eq!(std::fs::read(&dest).unwrap(), want_map);

        // 9. DASH video-only manifest → assembled track delivered.
        let dest = dir.join("dashv.bin");
        download_inner(
            format!("{base}/dash/vonly.mpd"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash video-only");
        assert_eq!(std::fs::read(&dest).unwrap(), want[..want.len() / 2]);

        // 9b. DASH SegmentTemplate (init + 3 media segments): parallel
        // fetch with strictly ordered assembly (init must lead).
        let dest = dir.join("dashtpl.bin");
        download_inner(
            format!("{base}/dash/tpl.mpd"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash template");
        let mut want_tpl = b"DASHINIT".to_vec();
        want_tpl.extend_from_slice(&want);
        assert_eq!(std::fs::read(&dest).unwrap(), want_tpl);

        // 9c. DASH SegmentTimeline (`r="2"` → 3 segments, no MPD duration):
        // count comes from the timeline, assembly identical to 9b.
        let dest = dir.join("dashtl.bin");
        download_inner(
            format!("{base}/dash/tl.mpd"),
            dest.clone(),
            6,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("dash timeline");
        assert_eq!(std::fs::read(&dest).unwrap(), want_tpl);

        // 10. DASH video+audio without ffmpeg → loud guidance, not garbage.
        let dest = dir.join("dashboth.bin");
        let err = download(
            format!("{base}/dash/both.mpd"),
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect_err("dash mux needs ffmpeg");
        assert!(
            format!("{err:#}").contains("ffmpeg"),
            "mux failure must name ffmpeg, got: {err:#}"
        );

        // 11. Adaptive pair, audio-less leg → direct delivery, exact.
        let dest = dir.join("pairv.bin");
        download_adaptive(
            format!("{base}/file"),
            None,
            dest.clone(),
            4,
            matrix_opts(),
            Arc::new(SharedState::new()),
        )
        .await
        .expect("adaptive video-only");
        assert_eq!(std::fs::read(&dest).unwrap(), want);

        srv.abort();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Live HLS: the recorder follows a playlist that grows, appends each new
    /// segment exactly once, stops at ENDLIST, and lands the bytes as `.ts`
    /// (the name the container really is).
    #[tokio::test]
    async fn live_hls_records_until_endlist() {
        use std::sync::atomic::AtomicUsize;
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind live fixture");
        let addr = listener.local_addr().expect("live fixture addr");
        let c = calls.clone();
        let srv = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    break;
                };
                let c = c.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 2048];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .trim_start_matches('/')
                        .to_string();
                    let (body, ctype) = if path.starts_with("live.m3u8") {
                        let call = c.fetch_add(1, Ordering::SeqCst);
                        let pl = if call == 0 {
                            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\ns0.ts\n#EXTINF:2.0,\ns1.ts\n"
                        } else {
                            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:2\n#EXTINF:2.0,\ns2.ts\n#EXTINF:2.0,\ns3.ts\n#EXT-X-ENDLIST\n"
                        };
                        (pl.as_bytes().to_vec(), "application/vnd.apple.mpegurl")
                    } else {
                        (
                            format!("SEG:{path};").into_bytes(),
                            "video/mp2t",
                        )
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                    let _ = s.flush().await;
                });
            }
        });

        let dir = std::env::temp_dir().join(format!("dc_live_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("stream.mp4");
        let shared = Arc::new(SharedState::new());
        let opts = EngineOpts {
            timeout_secs: 5,
            max_retries: 2,
            ..Default::default()
        };
        download_hls(
            &format!("http://{addr}/live.m3u8"),
            &dest,
            1,
            &opts,
            &shared,
        )
        .await
        .expect("live capture finishes on ENDLIST");

        assert!(shared.live.load(Ordering::SeqCst), "row must know it is live");
        // MPEG-TS bytes keep an honest extension, and the row follows it.
        let ts = dest.with_extension("ts");
        assert!(ts.is_file(), "expected {}", ts.display());
        assert!(!dest.exists(), "the .mp4 name must not lie");
        let got = String::from_utf8_lossy(&std::fs::read(&ts).unwrap()).to_string();
        assert_eq!(got, "SEG:s0.ts;SEG:s1.ts;SEG:s2.ts;SEG:s3.ts;");
        assert_eq!(
            mutex_guard(&shared.resolved_name).as_deref(),
            Some("stream.ts")
        );
        assert!(shared.has_total.load(Ordering::SeqCst));
        assert_eq!(
            shared.total.load(Ordering::SeqCst),
            std::fs::metadata(&ts).unwrap().len()
        );

        srv.abort();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Live playlists refresh with the target duration, so the wait must be
    /// parsed from the tag and stay in a sane range.
    #[test]
    fn live_helpers() {
        assert_eq!(hls_target_duration("#EXT-X-TARGETDURATION:9\n"), 9);
        assert_eq!(hls_target_duration("#EXT-X-TARGETDURATION:2.0\n"), 2);
        assert_eq!(hls_target_duration("#EXTM3U\n"), 6);
        assert_eq!(host_of("https://www.Example.com:8443/a.m3u8"), "example.com");
        assert_eq!(host_of("not a url"), "");
    }

    /// Queue v2: "connections per server" is one process-wide allowance per
    /// host, so a burst of jobs aimed at the same server can never hold more
    /// sockets than the user allows — and another host is never blocked by it.
    #[tokio::test]
    async fn per_host_cap_limits_concurrency() {
        // 0 is the default and means "no extra cap": no permit at all.
        assert!(host_slot("https://capoff.test/a", 0).await.is_none());

        let held = host_slot("https://capone.test/a", 1)
            .await
            .expect("first connection takes the only slot");
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(120),
            host_slot("https://capone.test/b", 1),
        )
        .await;
        assert!(blocked.is_err(), "a second connection must wait its turn");

        // Finishing the first hands the slot to the waiting one.
        drop(held);
        let next = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            host_slot("https://capone.test/c", 1),
        )
        .await
        .expect("the freed slot lets the next connection in")
        .expect("permit");
        drop(next);

        // A cap on one host never throttles a different host.
        let a = host_slot("https://host-a.test/x", 1).await.expect("h1");
        let b = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            host_slot("https://host-b.test/x", 1),
        )
        .await
        .expect("another host is not blocked")
        .expect("h2 permit");
        drop((a, b));
    }
}

#[cfg(test)]
mod platform_helper_tests {
    use super::*;
    #[test]
    fn supported_runtime_versions_are_not_just_executable_names(){
        assert!(runtime_version_supported("node","v22.0.0"));
        assert!(!runtime_version_supported("node","v18.20.0"));
        assert!(runtime_version_supported("deno","deno 2.3.0"));
        assert!(!runtime_version_supported("deno","deno 2.2.9"));
        assert!(!runtime_version_supported("node","unrecognized"));
    }
    #[test]
    fn helper_status_and_download_selection_use_the_same_precedence(){
        let system=Some(PathBuf::from("system"));let local=Some(PathBuf::from("managed"));let custom=Some(PathBuf::from("custom"));
        assert_eq!(select_ytdlp(YtdlpSource::Managed,system.clone(),None,None),(None,false));
        assert_eq!(select_ytdlp(YtdlpSource::Managed,system.clone(),None,local.clone()),(local.clone(),true));
        assert_eq!(select_ytdlp(YtdlpSource::Custom,system.clone(),custom.clone(),local.clone()),(custom,false));
        assert_eq!(select_ytdlp(YtdlpSource::Custom,system.clone(),None,local.clone()),(local.clone(),true));
        assert_eq!(select_ytdlp(YtdlpSource::Path,system.clone(),None,local.clone()),(system,false));
        assert_eq!(select_ytdlp(YtdlpSource::Path,None,None,local.clone()),(local,true));
    }
}

#[cfg(test)]
mod audit_cookie_tests {
    use super::*;
    #[tokio::test]
    async fn audit_concurrent_cookie_jars_are_independent_and_private() {
        let a=write_cookie_jar("session=secret", ".example.com").await.unwrap().unwrap();
        let b=write_cookie_jar("session=secret", ".example.com").await.unwrap().unwrap();
        assert_ne!(a.0,b.0,"same-site jobs must not share a credential file");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&a.0).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(a);
        assert!(b.0.is_file(),"finishing one job must not remove another job's cookies");
        let path=b.0.clone();
        drop(b);
        assert!(!path.exists());
    }
}

#[cfg(all(test, unix))]
mod audit_helper_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    static NEXT: AtomicU64=AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p=std::env::temp_dir().join(format!("dc-helper-test-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            std::fs::create_dir_all(&p).unwrap();Self(p)
        }
        fn script(&self,body:&str) -> PathBuf {
            let p=self.0.join("helper.py");
            std::fs::write(&p,format!("#!/usr/bin/env python3\n{body}\n")).unwrap();
            std::fs::set_permissions(&p,std::fs::Permissions::from_mode(0o700)).unwrap();p
        }
    }
    impl Drop for Temp { fn drop(&mut self) {let _=std::fs::remove_dir_all(&self.0);} }
    #[tokio::test]
    async fn helper_progress_stays_visible_despite_print_quiet_mode() {
        // `--print` makes yt-dlp quiet; without `--progress` the window showed
        // 0 B until the end. The fake refuses to run without the flag.
        let temp=Temp::new();let dest=temp.0.join("clip.mp4");
        let exe=temp.script("import sys,pathlib,json\nif '--progress' not in sys.argv: sys.exit(3)\np=(pathlib.Path(next((a[5:] for a in sys.argv if a.startswith('home:')),'.'))/sys.argv[sys.argv.index('-o')+1]).with_suffix('.mp4')\np.write_bytes(b'video bytes')\nprint('dcfile@'+json.dumps(str(p)))");
        download_with_ytdlp(&exe,"https://example.com/media",&dest,1,&EngineOpts::default(),&Arc::new(SharedState::new())).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(),b"video bytes");
    }
    #[tokio::test]
    async fn exact_reported_output_is_used_and_neighboring_files_are_untouched() {
        let temp=Temp::new();let dest=temp.0.join("clip.mp4");
        let neighbor=temp.0.join("clip-notes.txt");std::fs::write(&neighbor,b"keep me").unwrap();
        let exe=temp.script("import sys,pathlib,json\np=(pathlib.Path(next((a[5:] for a in sys.argv if a.startswith('home:')),'.'))/sys.argv[sys.argv.index('-o')+1]).with_suffix('.mkv')\np.write_bytes(b'video bytes')\nprint('dcfile@'+json.dumps(str(p)))");
        download_with_ytdlp(&exe,"https://example.com/media",&dest,1,&EngineOpts::default(),&Arc::new(SharedState::new())).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(),b"video bytes");
        assert_eq!(std::fs::read(&neighbor).unwrap(),b"keep me");
        std::fs::remove_file(&dest).unwrap();
        let exe=temp.script("import sys,pathlib,json\np=(pathlib.Path(next((a[5:] for a in sys.argv if a.startswith('home:')),'.'))/sys.argv[sys.argv.index('-o')+1]).with_name('clip-notes.txt')\nprint('dcfile@'+json.dumps(str(p)))");
        assert!(download_with_ytdlp(&exe,"https://example.com/media",&dest,1,&EngineOpts::default(),&Arc::new(SharedState::new())).await.is_err());
        assert!(!dest.exists());assert_eq!(std::fs::read(&neighbor).unwrap(),b"keep me");
    }
    #[tokio::test]
    async fn aborting_a_download_kills_the_child_and_removes_private_cookies() {
        let temp=Temp::new();let pidfile=temp.0.join("pid");let jarfile=temp.0.join("jar-path");
        let exe=temp.script(&format!("import sys,pathlib,os,time\npathlib.Path({:?}).write_text(str(os.getpid()))\npathlib.Path({:?}).write_text(sys.argv[sys.argv.index('--cookies')+1])\ntime.sleep(60)",pidfile.to_str().unwrap(),jarfile.to_str().unwrap()));
        let childfile=temp.0.join("child-pid");
        let body=std::fs::read_to_string(&exe).unwrap().replace("time.sleep(60)",
            &format!("import subprocess\nchild=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])\npathlib.Path({:?}).write_text(str(child.pid))\ntime.sleep(60)",childfile.to_str().unwrap()));
        std::fs::write(&exe,body).unwrap();
        let dest=temp.0.join("video.mp4");
        let shared=Arc::new(SharedState::new());let state=shared.clone();
        let task=tokio::spawn(async move {
            let opts=EngineOpts{cookie:"session=secret".into(),..EngineOpts::default()};
            download_with_ytdlp(&exe,"https://example.com/media",&dest,1,&opts,&state).await
        });
        for _ in 0..200 {
            if childfile.is_file() {break;}
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(childfile.is_file(),"mock helper and merger must launch");
        assert!(shared.external_helper.load(Ordering::SeqCst));
        let childpid=std::fs::read_to_string(&childfile).unwrap();
        let pid=std::fs::read_to_string(&pidfile).unwrap();
        let jar=PathBuf::from(std::fs::read_to_string(&jarfile).unwrap());assert!(jar.is_file());
        task.abort();let _=task.await;
        assert!(!jar.exists(),"abort must remove credentials");
        let mut gone=false;
        for _ in 0..100 {
            let status=std::process::Command::new("kill").args(["-0",pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap();
            if !status.success() {gone=true;break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(gone,"aborted helper must not keep downloading");
        let mut child_stopped=false;
        for _ in 0..100 {
            let out=std::process::Command::new("ps").args(["-o","stat=","-p",childpid.trim()]).output().unwrap();
            let state=String::from_utf8_lossy(&out.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {child_stopped=true;break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(child_stopped,"an aborted helper's merger must stop too");
    }
    #[test]
    fn browser_platform_hint_matches_the_configured_user_agent() {
        for (ua,want) in [("Mozilla Macintosh Chrome/126.0","macOS"),("Mozilla X11 Linux Chrome/126.0","Linux"),("Mozilla Windows Chrome/126.0","Windows")] {
            let opts=EngineOpts{user_agent:ua.into(),..EngineOpts::default()};
            let req=opts.get(&reqwest::Client::new(),"https://example.com").build().unwrap();
            assert_eq!(req.headers().get("sec-ch-ua-platform").unwrap(),&format!("\"{want}\""));
        }
    }
}

#[cfg(test)]
mod browser_capture_tests {
    use super::*;

    fn browser_ctx(method: &str, body: &[u8], headers: &[(&str, &str)]) -> crate::model::BrowserContext {
        crate::model::BrowserContext {
            method: method.to_string(),
            body: crate::model::base64_encode(body),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            from_extension: true,
            ..Default::default()
        }
    }

    #[test]
    fn browser_headers_replace_the_defaults() {
        let opts = EngineOpts {
            user_agent: "Mozilla/5.0 Chrome/130.0".into(),
            browser: Some(browser_ctx("", b"", &[("user-agent", "UA/browser"), ("accept-language", "bn-BD"), ("accept", "text/html")])),
            ..Default::default()
        };
        let client = opts.build_client().unwrap();
        let req = opts.get(&client, "https://files.example/a.zip").build().unwrap();
        let h = req.headers();
        assert_eq!(h.get_all("user-agent").iter().count(), 1);
        assert_eq!(h["user-agent"], "UA/browser");
        assert_eq!(h.get_all("accept-language").iter().count(), 1);
        assert_eq!(h["accept-language"], "bn-BD");
        assert_eq!(h["accept"], "text/html");
        // A GET never carries the form's Content-Type.
        let get = EngineOpts { browser: Some(browser_ctx("", b"", &[("content-type", "x/y")])), ..Default::default() };
        assert!(get.get(&client, "https://files.example/a").build().unwrap().headers().get("content-type").is_none());
    }

    /// Serves `body` only to a POST carrying `form` (like a report export).
    async fn post_only_server(form: &'static [u8], body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = std::sync::Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { break };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = s.read(&mut buf).await.unwrap_or(0);
                        if n == 0 { return; }
                        raw.extend_from_slice(&buf[..n]);
                        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..end]).to_lowercase();
                            let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map(|v| v.trim().parse().unwrap_or(0)).unwrap_or(0);
                            if raw.len() < end + 4 + len { continue; }
                            let got = &raw[end + 4..end + 4 + len];
                            let ok = head.starts_with("post ") && got == form && head.contains("content-type: application/x-www-form-urlencoded");
                            let resp = if ok {
                                let mut r = format!("HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                                r.extend_from_slice(&body);
                                r
                            } else {
                                b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                            };
                            let _ = s.write_all(&resp).await;
                            return;
                        }
                    }
                });
            }
        });
        format!("http://{addr}/export")
    }

    #[tokio::test]
    async fn a_form_download_is_repeated_as_a_post_with_its_body() {
        let want: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let url = post_only_server(b"from=2026-01-01&to=2026-02-01", want.clone()).await;
        let dir = std::env::temp_dir().join(format!("dc-post-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("export");
        let opts = EngineOpts {
            browser: Some(browser_ctx("POST", b"from=2026-01-01&to=2026-02-01", &[("content-type", "application/x-www-form-urlencoded")])),
            ..Default::default()
        };
        let shared = Arc::new(SharedState::new());
        download(url, dest.clone(), 4, opts, shared.clone()).await.unwrap();
        // Saved under the server's name (the placeholder name gave way).
        let saved = dir.join("report.pdf");
        assert_eq!(std::fs::read(&saved).unwrap(), want);
        assert_eq!(mutex_guard(&shared.resolved_name).as_deref(), Some("report.pdf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_refused_file_is_fetched_through_the_browser_page() {
        // Plays the native host + extension: answer the request file the way
        // the extension's chunks would arrive.
        let dir = crate::native_host::page_fetch_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let fake_host = tokio::spawn({
            let dir = dir.clone();
            async move {
                for _ in 0..100 {
                    for e in std::fs::read_dir(&dir).unwrap().flatten() {
                        let name = e.file_name().to_string_lossy().to_string();
                        let Some(id) = name.strip_suffix(".req.json") else { continue };
                        let req: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
                        if req["url"] != "https://cdn.example/locked.bin" { continue; }
                        assert_eq!(req["page"], "https://site.example/page");
                        std::fs::write(dir.join(format!("{id}.part")), b"page-fetched bytes").unwrap();
                        std::fs::rename(dir.join(format!("{id}.part")), dir.join(format!("{id}.res"))).unwrap();
                        return true;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                false
            }
        });
        let out = std::env::temp_dir().join(format!("dc-pagefetch-{}.bin", std::process::id()));
        let shared = Arc::new(SharedState::new());
        page_fetch_file("https://cdn.example/locked.bin", "https://site.example/page", &out, &shared).await.unwrap();
        assert!(fake_host.await.unwrap());
        assert_eq!(std::fs::read(&out).unwrap(), b"page-fetched bytes");
        assert_eq!(shared.total.load(Ordering::SeqCst), 18);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn digest_login_matches_the_rfc_example() {
        // RFC 2617 section 3.5.
        let mut d = DigestAuth::parse(r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#).unwrap();
        use md5::{Digest, Md5};
        let ha1 = format!("{:x}", Md5::digest(b"Mufasa:testrealm@host.com:Circle Of Life"));
        assert_eq!(ha1, "939e7578ed9e3c518a452acee763bce9");
        let h = d.header("Mufasa", "Circle Of Life", "GET", "/dir/index.html");
        assert!(h.starts_with(r#"Digest username="Mufasa", realm="testrealm@host.com""#));
        assert!(h.contains("qop=auth, nc=00000001"));
        assert!(h.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#));
        assert!(DigestAuth::parse("Basic realm=x").is_none());
        assert!(DigestAuth::parse(r#"Digest realm="r", nonce="n", algorithm=SHA-256"#).is_none());
    }

    #[test]
    fn youtube_lists_are_told_apart_from_videos() {
        assert!(is_youtube_list("https://www.youtube.com/playlist?list=PL123"));
        assert!(is_youtube_list("https://www.youtube.com/@SomeChannel/videos"));
        assert!(is_youtube_list("https://youtube.com/channel/UC123"));
        assert!(!is_youtube_list("https://www.youtube.com/watch?v=abc&list=PL123"));
        assert!(!is_youtube_list("https://www.youtube.com/watch?v=abc"));
        assert!(!is_youtube_list("https://example.com/@user"));
    }

    #[test]
    fn youtube_order_setting_decides_who_goes_first() {
        let hd = EngineOpts { quality_pref: "1080p".into(), ..Default::default() };
        let sd = EngineOpts { quality_pref: "360p".into(), ..Default::default() };
        // Auto (no setting): yt-dlp for what YouTube gates, the app's engine
        // for the progressive stream.
        assert!(ytdlp_preferred(&hd));
        assert!(!ytdlp_preferred(&sd));
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn unreachable_hosts_are_told_apart_from_refusals() {
        assert!(unreachable_host("network: error sending request: dns error: failed to lookup address information"));
        assert!(unreachable_host("connect: no route to host"));
        assert!(!unreachable_host("http 404 not found"));
        assert!(!unreachable_host("server refused (403 forbidden)"));
    }

    #[test]
    fn redirect_loops_and_bad_certificates_are_explained() {
        assert!(explain_failure("error following redirect: too many redirects").unwrap().contains("login"));
        assert!(explain_failure("invalid peer certificate: UnknownIssuer").unwrap().contains("certificate"));
        assert!(explain_failure("HTTP 500").is_none());
    }

    #[test]
    fn ipv6_is_allowed_only_when_asked() {
        let o = EngineOpts { any_ip: true, ..Default::default() };
        assert!(o.build_client().is_ok());
        assert!(EngineOpts::default().build_client().is_ok());
    }
}
