//! # downloadcraft-engine
//!
//! A multi-connection HTTP(S) download engine â€” the one inside
//! [Download Craft](https://github.com/hungry-detective/downloadcraft) â€” built
//! on `tokio` and `reqwest`, free to use in your own downloader (MIT).
//!
//! What makes it fast and robust:
//!
//! - **Dynamic segmentation (work stealing).** Whenever a connection is free,
//!   it takes the part of the file that would otherwise finish last and splits
//!   it; the busy connection's end simply moves closer. No connection idles
//!   while another crawls, and splits follow the measured speeds.
//! - **First request = probe.** Without a known size the first connection
//!   starts downloading at once; more connections join as soon as its reply
//!   proves the server supports ranges.
//! - **One pre-allocated file.** Every connection writes at its own offset, so
//!   there is no merge pass at the end (sparse on Windows: no zero-fill).
//! - **Crash-safe resume.** A journal of the missing ranges is rewritten every
//!   few seconds after the data is flushed; any number of connections can
//!   continue it, and a changed server file is detected (size, ETag,
//!   Last-Modified) instead of being stitched onto old bytes.
//! - **Self-healing connections.** Throttling (429/503) lowers the connection
//!   count, a per-IP connection limit is detected, stalled or very slow
//!   connections are reopened, failed ranges go back to the pool.
//! - **Expired links.** A refused redirect target is retried through the
//!   original URL, then through your own refresh hook (for example "resolve
//!   the video page again"), keeping everything downloaded so far.
//! - **Checks every reply.** Wrong `Content-Range`, a different file size, an
//!   HTML error page instead of the file â€” all caught before they corrupt it.
//!
//! ```no_run
//! use downloadcraft_engine::{Control, Downloader, Job, Progress};
//! use std::sync::Arc;
//!
//! # async fn run() -> Result<(), downloadcraft_engine::Error> {
//! let client = reqwest::Client::new();
//! let progress = Arc::new(Progress::default());
//! let control = Arc::new(Control::default());
//! let job = Job::new("https://example.com/big.iso", "big.iso");
//! let done = Downloader::new(client).download(job, progress, control).await?;
//! println!("{} bytes in {}", done.size, done.path.display());
//! # Ok(())
//! # }
//! ```
//!
//! While it runs, read [`Progress`] from any thread (bytes, size, live
//! connections, every segment) and use [`Control`] to pause, resume or stop.
//! A stopped or failed download resumes from where it was when the same
//! [`Job`] runs again.

mod download;
mod file;
mod journal;
mod limiter;
mod segments;
pub mod sniff;
pub mod zip;

pub use limiter::RateLimiter;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Builds the GET request for a URL. Add headers, cookies or auth here; the
/// engine adds `Range` itself.
pub type RequestFn = Arc<dyn Fn(&str) -> reqwest::RequestBuilder + Send + Sync>;

/// Called when the link stops working mid-download (401/403/410 on every
/// connection). Return a fresh URL for the **same** file, or `None`. The
/// engine checks the new link serves the same size before continuing.
pub type RefreshFn =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> + Send + Sync>;

/// Receives engine events (connections opened, splits, retries...).
pub type EventFn = Arc<dyn Fn(&Event) + Send + Sync>;

/// Tuning knobs. `Config::default()` suits most servers.
#[derive(Debug, Clone)]
pub struct Config {
    /// Most connections one download may use (1â€“32). Default 8.
    pub connections: usize,
    /// Never split a segment into parts smaller than this. Default 1 MiB.
    pub min_split: u64,
    /// Largest byte range asked for in one request (`None` = a whole
    /// segment). Some CDNs refuse big ranges but serve small ones.
    pub max_request: Option<u64>,
    /// A connection that delivers nothing for this long is reopened.
    pub stall_timeout: Duration,
    /// Give up after this many failed attempts in a row with no progress
    /// anywhere. Default 8.
    pub max_retries: u32,
    /// How often the resume journal is saved. Default 3 s.
    pub journal_interval: Duration,
    /// Flush data to disk before each journal save (crash safety at the
    /// cost of some disk syncs). Default true.
    pub durable: bool,
    /// Reopen a connection running below `slow_ratio` Ã— the median speed for
    /// this long. `None` disables it. Default 1.5 s.
    pub slow_restart_after: Option<Duration>,
    pub slow_ratio: f64,
    /// Fail when the server sends an HTML page instead of the file (unless
    /// the destination itself is `.html`/`.htm`). Default true.
    pub reject_html: bool,
    /// Bytes buffered per connection before a disk write. Default 512 KiB.
    pub write_buffer: usize,
    /// The size alone is the file's identity: ETag and Last-Modified are not
    /// compared on resume. For links that pin the exact file themselves
    /// (YouTube's carry its length and version) and are served by many edge
    /// servers, each with its own date. Default false.
    pub size_identity: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connections: 8,
            min_split: 1024 * 1024,
            max_request: None,
            stall_timeout: Duration::from_secs(30),
            max_retries: 8,
            journal_interval: Duration::from_secs(3),
            durable: true,
            slow_restart_after: Some(Duration::from_millis(1500)),
            slow_ratio: 0.2,
            reject_html: true,
            write_buffer: 512 * 1024,
            size_identity: false,
        }
    }
}

/// One file to download.
#[derive(Debug, Clone)]
pub struct Job {
    pub url: String,
    /// Final path. While downloading, data lives in `<dest>.dcdata` and the
    /// resume journal in `<dest>.dcjournal`.
    pub dest: PathBuf,
    /// Size, when you already know it (from a HEAD or earlier probe).
    pub size_hint: Option<u64>,
    /// True when ranges are known to work: with `size_hint`, the file is
    /// split across all connections at once instead of ramping up.
    pub ranges_hint: bool,
    /// What a resume journal must belong to (default: the URL). Use a stable
    /// id when the URL changes between runs (signed links).
    pub resume_key: Option<String>,
    /// Keep the unfinished data and journal in this folder (a fast or roomy
    /// disk) instead of next to `dest`; the finished file is moved to `dest`.
    pub work_dir: Option<PathBuf>,
    /// Other URLs serving the same file (IDM "alternative addresses"):
    /// connections spread over them, and a mirror that refuses or serves a
    /// different file is dropped. The size must match; ETags may differ.
    pub mirrors: Vec<String>,
}

impl Job {
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        Self { url: url.into(), dest: dest.into(), size_hint: None, ranges_hint: false, resume_key: None, work_dir: None, mirrors: Vec::new() }
    }

    pub fn size_hint(mut self, size: Option<u64>, ranges: bool) -> Self {
        self.size_hint = size;
        self.ranges_hint = ranges && size.is_some();
        self
    }

    pub fn resume_key(mut self, key: impl Into<String>) -> Self {
        self.resume_key = Some(key.into());
        self
    }

    /// See [`Job::mirrors`].
    pub fn mirrors(mut self, urls: Vec<String>) -> Self {
        self.mirrors = urls.into_iter().filter(|u| u.starts_with("http://") || u.starts_with("https://")).collect();
        self
    }

    /// See [`Job::work_dir`]. `None` = next to the destination.
    pub fn work_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.work_dir = dir.filter(|d| !d.as_os_str().is_empty());
        self
    }

    pub(crate) fn data(&self) -> PathBuf {
        data_path_in(&self.dest, self.work_dir.as_deref())
    }

    pub(crate) fn journal(&self) -> PathBuf {
        journal_path_in(&self.dest, self.work_dir.as_deref())
    }
}

/// Pause, resume or stop a running download from any thread.
#[derive(Debug, Default)]
pub struct Control {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

impl Control {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    /// Stop: the download returns [`Error::Cancelled`] and keeps its resume
    /// state, so running the same job again continues it.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// One segment as shown in a progress view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentView {
    pub start: u64,
    /// One past the last byte (`u64::MAX` while the size is unknown).
    pub end: u64,
    /// Bytes received from `start`.
    pub done: u64,
    /// A connection is working on it right now.
    pub active: bool,
}

/// Live progress, readable from any thread.
#[derive(Debug, Default)]
pub struct Progress {
    downloaded: AtomicU64,
    total: AtomicU64,
    has_total: AtomicBool,
    resumable: AtomicBool,
    connections: AtomicUsize,
    segments: Mutex<Vec<SegmentView>>,
}

impl Progress {
    /// Bytes received so far (including what an earlier run left).
    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::SeqCst)
    }
    /// File size, once the server has told it.
    pub fn total(&self) -> Option<u64> {
        self.has_total.load(Ordering::SeqCst).then(|| self.total.load(Ordering::SeqCst))
    }
    /// The server supports ranges, so a stopped download can be resumed.
    pub fn resumable(&self) -> bool {
        self.resumable.load(Ordering::SeqCst)
    }
    /// Connections working right now.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    /// Every segment, in file order.
    pub fn segments(&self) -> Vec<SegmentView> {
        self.segments.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_total(&self, t: u64) {
        self.total.store(t, Ordering::SeqCst);
        self.has_total.store(true, Ordering::SeqCst);
    }
}

/// What finished.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub path: PathBuf,
    pub size: u64,
    /// The URL that finally served the file (after redirects / refresh).
    pub final_url: String,
    pub content_type: Option<String>,
    /// File name from `Content-Disposition`, if the server sent one.
    pub filename: Option<String>,
    /// Bytes that were already on disk from an earlier run.
    pub resumed: u64,
    /// Most connections that ran at the same time.
    pub peak_connections: usize,
    /// The link had to be refreshed on the way.
    pub url_refreshed: bool,
    /// Format recognised from the first bytes (see [`sniff::sniff`]).
    pub detected: Option<&'static str>,
}

/// Something the engine reports while it works (for logs and debugging).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// Picked up where an earlier run stopped.
    Resumed { bytes: u64 },
    /// A connection started on `[from, to)`.
    Connecting { worker: usize, from: u64, to: u64 },
    /// The server ignores ranges: one connection, no resume.
    RangesUnsupported,
    /// An attempt failed and will be retried.
    Retry { worker: usize, error: String },
    /// Fewer connections from now on.
    ConnectionsReduced { to: usize, reason: String },
    /// A very slow connection was reopened.
    SlowRestart { worker: usize },
    /// The link expired and was replaced.
    UrlRefreshed { url: String },
    /// The server's file changed; starting over.
    Restarted { reason: String },
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Event::Resumed { bytes } => write!(f, "resuming with {bytes} bytes already downloaded"),
            Event::Connecting { worker, from, to } => {
                if *to == u64::MAX {
                    write!(f, "connection {worker}: bytes {from}-")
                } else {
                    write!(f, "connection {worker}: bytes {from}-{}", to.saturating_sub(1))
                }
            }
            Event::RangesUnsupported => write!(f, "server ignores ranges: one connection, no resume"),
            Event::Retry { worker, error } => write!(f, "connection {worker}: {error} â€” retrying"),
            Event::ConnectionsReduced { to, reason } => write!(f, "using {to} connection(s): {reason}"),
            Event::SlowRestart { worker } => write!(f, "connection {worker} was very slow â€” reopened"),
            Event::UrlRefreshed { url } => write!(f, "link refreshed: {url}"),
            Event::Restarted { reason } => write!(f, "starting over: {reason}"),
        }
    }
}

/// Why a download stopped.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// [`Control::cancel`] was called. Resume state is kept.
    Cancelled,
    /// The server refused the download outright (no bytes ever arrived).
    Http { status: u16, url: String },
    /// The link stopped working mid-download and could not be refreshed.
    /// Resume state is kept: run again with a fresh URL and the same
    /// [`Job::resume_key`].
    LinkExpired { status: u16, downloaded: u64 },
    /// The server kept answering 429/503 even with one connection.
    Throttled { status: u16 },
    /// The server's file is not the one partly downloaded (size, ETag or
    /// Last-Modified changed) and starting over did not help.
    FileChanged(String),
    /// The server sent an HTML page instead of the file.
    HtmlPage,
    /// Network failures with no progress, `max_retries` times in a row.
    Network(String),
    /// Disk error (full disk, permissions...).
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => write!(f, "cancelled"),
            Error::Http { status, .. } => write!(f, "HTTP {status}"),
            Error::LinkExpired { status, downloaded } => {
                write!(f, "link expired (HTTP {status}) after {downloaded} bytes")
            }
            Error::Throttled { status } => write!(f, "server is throttling (HTTP {status})"),
            Error::FileChanged(why) => write!(f, "the file on the server changed: {why}"),
            Error::HtmlPage => write!(f, "the server sent a web page instead of the file"),
            Error::Network(e) => write!(f, "{e}"),
            Error::Io(e) => write!(f, "file error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Where a job's data lives while downloading.
pub fn data_path(dest: &Path) -> PathBuf {
    data_path_in(dest, None)
}

/// Where a job's resume journal lives.
/// Point a resume journal at a new key (the job's `resume_key`, or its URL),
/// keeping every byte it records: for a partial download moved to another
/// file name before it continues (the Download window's early start, which
/// downloads into a scratch folder). Returns whether a journal was rewritten.
pub fn rekey_journal(journal: &Path, new_key: &str) -> bool {
    match journal::Journal::load(journal) {
        Some(mut j) if j.key != new_key => {
            j.key = new_key.to_string();
            j.save(journal).is_ok()
        }
        _ => false,
    }
}

pub fn journal_path(dest: &Path) -> PathBuf {
    journal_path_in(dest, None)
}

/// The working copy of `dest` inside `work_dir` (or next to `dest`). The
/// name carries a hash of the full destination, so two files with the same
/// name in different folders never share one.
fn work_base(dest: &Path, work_dir: Option<&Path>) -> PathBuf {
    match work_dir {
        None => dest.to_path_buf(),
        Some(dir) => {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in dest.to_string_lossy().as_bytes() {
                h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
            }
            let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            dir.join(format!("{h:016x}-{name}"))
        }
    }
}

/// [`data_path`] with a work folder (see [`Job::work_dir`]).
pub fn data_path_in(dest: &Path, work_dir: Option<&Path>) -> PathBuf {
    journal::with_suffix(&work_base(dest, work_dir), ".dcdata")
}

/// [`journal_path`] with a work folder.
pub fn journal_path_in(dest: &Path, work_dir: Option<&Path>) -> PathBuf {
    journal::with_suffix(&work_base(dest, work_dir), ".dcjournal")
}

/// Whether an unfinished download for `dest` can be resumed.
pub fn has_partial(dest: &Path) -> bool {
    has_partial_in(dest, None)
}

/// [`has_partial`] with a work folder.
pub fn has_partial_in(dest: &Path, work_dir: Option<&Path>) -> bool {
    data_path_in(dest, work_dir).is_file() && journal_path_in(dest, work_dir).is_file()
}

/// Delete the resume state of `dest` (data and journal).
pub fn remove_partial(dest: &Path) {
    remove_partial_in(dest, None)
}

/// [`remove_partial`] with a work folder.
pub fn remove_partial_in(dest: &Path, work_dir: Option<&Path>) {
    let j = journal_path_in(dest, work_dir);
    let _ = std::fs::remove_file(data_path_in(dest, work_dir));
    let _ = std::fs::remove_file(&j);
    let _ = std::fs::remove_file(journal::with_suffix(&j, ".tmp"));
}

/// The engine. Cheap to clone; one instance can run many downloads.
#[derive(Clone)]
pub struct Downloader {
    request: RequestFn,
    config: Config,
    limiter: Option<Arc<RateLimiter>>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    refresh: Option<RefreshFn>,
    events: Option<EventFn>,
}

impl Downloader {
    /// Plain GET requests through `client`.
    pub fn new(client: reqwest::Client) -> Self {
        Self::with_request(Arc::new(move |url: &str| client.get(url)))
    }

    /// Requests built by your own function (headers, cookies, auth...).
    pub fn with_request(request: RequestFn) -> Self {
        Self { request, config: Config::default(), limiter: None, gate: None, refresh: None, events: None }
    }

    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Share a speed limit (with other downloads, too).
    pub fn limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// Every connection holds a permit of this semaphore while it runs â€” a
    /// cap on connections to one server across many downloads.
    pub fn connection_gate(mut self, gate: Arc<tokio::sync::Semaphore>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// See [`RefreshFn`].
    pub fn on_link_expired(mut self, refresh: RefreshFn) -> Self {
        self.refresh = Some(refresh);
        self
    }

    pub fn on_event(mut self, events: EventFn) -> Self {
        self.events = Some(events);
        self
    }

    /// The request builder in use (for [`zip::list_remote`] and friends).
    pub fn request_fn(&self) -> &RequestFn {
        &self.request
    }

    /// Download `job`. Resumes automatically when an earlier run of the same
    /// job left a journal.
    pub async fn download(
        &self,
        job: Job,
        progress: Arc<Progress>,
        control: Arc<Control>,
    ) -> Result<Outcome, Error> {
        download::run(self, job, progress, control).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Callers spawn downloads on multi-threaded runtimes.
    #[allow(dead_code)]
    fn download_future_is_send(d: Downloader, j: Job, p: Arc<Progress>, c: Arc<Control>) {
        fn send<T: Send>(_: T) {}
        send(d.download(j, p, c));
        send(zip::list_remote(d.request_fn(), ""));
    }
}
