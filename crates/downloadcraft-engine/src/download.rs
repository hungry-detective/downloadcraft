//! The download loop: a monitor that opens connections while there is work,
//! and workers that each fetch one segment at a time.

use crate::journal::Journal;
use crate::segments::{Claim, SegmentMap, OPEN_END};
use crate::{
    file, remove_partial_in, sniff, Control, Downloader, Error, Event, Job, Outcome, Progress,
    SegmentView,
};
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(200);
/// Another connection that received data this recently counts as healthy.
const HEALTHY_WINDOW: Duration = Duration::from_secs(5);
/// Link refreshes allowed per run.
const MAX_REFRESHES: u32 = 3;
/// After this long without a reason to cut connections, try one more.
const CAP_PROBE: Duration = Duration::from_secs(30);
/// Wait at most this long for a reply while other connections are working
/// (a server at its connection limit often just never answers).
const BUSY_ANSWER: Duration = Duration::from_secs(20);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `bytes 0-99/1000` → (0, 99, Some(1000)); `bytes 0-99/*` → (0, 99, None).
pub(crate) fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (a, b) = range.trim().split_once('-')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
}

/// File name from a `Content-Disposition` header (`filename*=` preferred).
fn disposition_name(v: &str) -> Option<String> {
    let mut plain = None;
    for part in v.split(';') {
        let (k, val) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let k = k.trim().to_ascii_lowercase();
        let val = val.trim().trim_matches('"');
        if k == "filename*" {
            let enc = val.splitn(3, '\'').nth(2).unwrap_or(val);
            let name = percent_decode(enc);
            if !name.is_empty() {
                return Some(name);
            }
        } else if k == "filename" && !val.is_empty() {
            plain = Some(val.to_string());
        }
    }
    plain
}

fn percent_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Per-connection counters shared with the monitor.
struct WorkerStat {
    bytes: AtomicU64,
    active: AtomicBool,
    /// Got a 2xx reply and is reading its body.
    streaming: AtomicBool,
    restart: AtomicBool,
    started: Mutex<Instant>,
    last_byte: Mutex<Option<Instant>>,
    last_restart: Mutex<Option<Instant>>,
}

impl WorkerStat {
    fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            active: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            started: Mutex::new(Instant::now()),
            last_byte: Mutex::new(None),
            last_restart: Mutex::new(None),
        }
    }
}

#[derive(Default)]
struct Meta {
    etag: Option<String>,
    last_modified: Option<String>,
    content_type: Option<String>,
    filename: Option<String>,
}

/// How one attempt on a segment ended badly.
enum Fail {
    Cancelled,
    Fatal(Error),
    Retry { msg: String, progressed: bool },
    Throttled { status: u16, retry_after: Option<Duration> },
    Refused { status: u16, url: String },
    /// The server ignores ranges and another connection carries on alone.
    RangeIgnored,
}

struct Run {
    dl: Downloader,
    /// Mirrors still in use (see Job::mirrors).
    mirrors: Mutex<Vec<String>>,
    has_mirrors: bool,
    job: Job,
    data: PathBuf,
    journal: PathBuf,
    key: String,
    map: Mutex<SegmentMap>,
    progress: Arc<Progress>,
    control: Arc<Control>,
    url: RwLock<String>,
    cap: AtomicUsize,
    last_cut: Mutex<Instant>,
    live: AtomicUsize,
    /// Most connections seen receiving data at the same time.
    proven: AtomicUsize,
    /// Fastest one connection has gone in this run (bytes/s).
    best_speed: Mutex<f64>,
    peak: AtomicUsize,
    ranges_ok: AtomicBool,
    single: AtomicBool,
    stats: Mutex<Vec<Arc<WorkerStat>>>,
    speeds: Mutex<Vec<f64>>,
    meta: Mutex<Meta>,
    fail_streak: AtomicU32,
    /// Last time any connection wrote data.
    last_progress: Mutex<Instant>,
    fatal: Mutex<Option<Error>>,
    stop: AtomicBool,
    refreshes: AtomicU32,
    refreshed: AtomicBool,
    refresh_lock: tokio::sync::Mutex<()>,
    dirty: AtomicBool,
    resumed: u64,
    dest_is_html: bool,
}

impl Run {
    fn emit(&self, e: Event) {
        if let Some(f) = &self.dl.events {
            f(&e);
        }
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.control.is_cancelled()
    }

    fn set_fatal(&self, e: Error) {
        let mut g = lock(&self.fatal);
        if g.is_none() {
            *g = Some(e);
        }
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Leave when more connections run than allowed (the live count drops
    /// here, exactly once).
    fn over_cap(&self) -> bool {
        loop {
            let live = self.live.load(Ordering::SeqCst);
            if live <= self.cap.load(Ordering::SeqCst).max(1) {
                return false;
            }
            if self.live.compare_exchange(live, live - 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                return true;
            }
        }
    }

    fn reduce_cap(&self, to: usize, reason: String) {
        let to = to.max(1);
        *lock(&self.last_cut) = Instant::now();
        let old = self.cap.fetch_min(to, Ordering::SeqCst);
        if to < old {
            self.emit(Event::ConnectionsReduced { to, reason });
        }
    }

    /// Additive increase: after a quiet spell, allow one more connection
    /// again (the limit may have been temporary).
    fn probe_cap(&self, configured: usize) {
        if self.single.load(Ordering::SeqCst) {
            return;
        }
        let mut cut = lock(&self.last_cut);
        if cut.elapsed() >= CAP_PROBE && self.cap.load(Ordering::SeqCst) < configured {
            self.cap.fetch_add(1, Ordering::SeqCst);
            *cut = Instant::now();
        }
    }

    /// Sleep, waking early when the download stops.
    async fn nap(&self, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until && !self.stopping() {
            tokio::time::sleep(Duration::from_millis(150).min(until - Instant::now())).await;
        }
    }

    fn others_healthy(&self, me: usize) -> bool {
        lock(&self.stats).iter().enumerate().any(|(i, s)| {
            i != me
                && s.active.load(Ordering::SeqCst)
                && (s.streaming.load(Ordering::SeqCst)
                    || lock(&s.last_byte).is_some_and(|t| t.elapsed() < HEALTHY_WINDOW))
        })
    }

    /// Other connections receiving data right now.
    fn streaming_count(&self, me: usize) -> usize {
        lock(&self.stats)
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != me && s.active.load(Ordering::SeqCst) && s.streaming.load(Ordering::SeqCst))
            .count()
    }

    fn others_connecting(&self, me: usize) -> bool {
        lock(&self.stats).iter().enumerate().any(|(i, s)| i != me && s.active.load(Ordering::SeqCst))
    }

    /// Adopt what a reply says about the file; refuse a different file.
    fn learn(&self, total: Option<u64>, range_reply: bool) -> Result<(), Fail> {
        let mut m = lock(&self.map);
        match (m.total(), total) {
            (Some(a), Some(b)) if a != b => {
                return Err(Fail::Fatal(Error::FileChanged(format!("its size is now {b} bytes, it was {a}"))));
            }
            (None, Some(t)) => {
                file::set_len(&self.data, t).map_err(|e| Fail::Fatal(Error::Io(e)))?;
                m.set_total(t);
                self.progress.set_total(t);
            }
            _ => {}
        }
        drop(m);
        if range_reply {
            self.ranges_ok.store(true, Ordering::SeqCst);
            self.progress.resumable.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Which mirror connection `w` uses (None = the main URL).
    fn mirror_for(&self, w: usize) -> Option<String> {
        let m = lock(&self.mirrors);
        let i = w % (m.len() + 1);
        (i > 0).then(|| m[i - 1].clone())
    }

    fn drop_mirror(&self, url: &str, why: &str) {
        let mut m = lock(&self.mirrors);
        if let Some(i) = m.iter().position(|u| u == url) {
            m.remove(i);
            drop(m);
            self.emit(Event::Retry { worker: 0, error: format!("mirror {url} dropped: {why}") });
        }
    }

    /// The first ETag / Last-Modified seen is the file's identity.
    fn check_validators(&self, etag: Option<String>, lm: Option<String>) -> Result<(), Fail> {
        // Mirrors each have their own ETag/date: the size is the identity.
        if self.has_mirrors {
            return Ok(());
        }
        let mut meta = lock(&self.meta);
        let strong = |e: &Option<String>| e.clone().filter(|v| !v.starts_with("W/"));
        if let (Some(a), Some(b)) = (strong(&meta.etag), strong(&etag)) {
            if a != b {
                return Err(Fail::Fatal(Error::FileChanged(format!("its ETag changed ({a} → {b})"))));
            }
        }
        if let (Some(a), Some(b)) = (&meta.last_modified, &lm) {
            if a != b {
                return Err(Fail::Fatal(Error::FileChanged(format!("Last-Modified changed ({a} → {b})"))));
            }
        }
        if meta.etag.is_none() {
            meta.etag = etag;
        }
        if meta.last_modified.is_none() {
            meta.last_modified = lm;
        }
        Ok(())
    }

    /// Only one connection from now on: the server ignores ranges.
    fn go_single(&self) {
        if !self.single.swap(true, Ordering::SeqCst) {
            self.cap.store(1, Ordering::SeqCst);
            self.ranges_ok.store(false, Ordering::SeqCst);
            self.progress.resumable.store(false, Ordering::SeqCst);
            self.emit(Event::RangesUnsupported);
        }
    }

    fn note_failure(&self, progressed: bool, msg: String, worker: usize) -> Option<u32> {
        if progressed {
            self.fail_streak.store(0, Ordering::SeqCst);
            self.emit(Event::Retry { worker, error: msg });
            return Some(0);
        }
        let streak = self.fail_streak.fetch_add(1, Ordering::SeqCst) + 1;
        let budget = self.dl.config.max_retries.max(1) * (self.cap.load(Ordering::SeqCst).clamp(1, 4) as u32);
        if streak >= budget {
            self.set_fatal(Error::Network(msg));
            return None;
        }
        self.emit(Event::Retry { worker, error: msg });
        Some(streak)
    }

    /// A refusal (401/403/404/410) on `failed_url`: a per-IP connection
    /// limit while others still work, otherwise an expired link.
    async fn on_refused(&self, worker: usize, status: u16, failed_url: &str) -> Result<(), ()> {
        // Extra connections refused at the very start, before the first ones
        // have answered: give those a moment to show the link works.
        if !self.others_healthy(worker) && self.others_connecting(worker) {
            self.nap(Duration::from_secs(2)).await;
        }
        if self.others_healthy(worker) {
            let live = self.live.load(Ordering::SeqCst);
            self.reduce_cap(live.saturating_sub(1), format!("the server refused an extra connection (HTTP {status})"));
            return Err(());
        }
        let anything = self.resumed > 0 || lock(&self.map).written() > 0 || self.refreshes.load(Ordering::SeqCst) > 0;
        if !anything {
            self.set_fatal(Error::Http { status, url: failed_url.to_string() });
            return Err(());
        }
        let _g = self.refresh_lock.lock().await;
        if *self.url.read().unwrap_or_else(|e| e.into_inner()) != failed_url {
            return Ok(()); // someone refreshed meanwhile
        }
        if self.refreshes.fetch_add(1, Ordering::SeqCst) >= MAX_REFRESHES {
            self.expired(status);
            return Err(());
        }
        // "Try original url": a refused redirect target (signed CDN link)
        // gets a fresh signature from the address it came from.
        if failed_url != self.job.url {
            *self.url.write().unwrap_or_else(|e| e.into_inner()) = self.job.url.clone();
            self.emit(Event::UrlRefreshed { url: self.job.url.clone() });
            return Ok(());
        }
        if let Some(f) = self.dl.refresh.clone() {
            if let Some(new) = f(failed_url.to_string()).await.filter(|u| u != failed_url) {
                *self.url.write().unwrap_or_else(|e| e.into_inner()) = new.clone();
                self.refreshed.store(true, Ordering::SeqCst);
                self.emit(Event::UrlRefreshed { url: new });
                return Ok(());
            }
        }
        self.expired(status);
        Err(())
    }

    fn expired(&self, status: u16) {
        let downloaded = lock(&self.map).written();
        self.set_fatal(Error::LinkExpired { status, downloaded });
    }

    fn snapshot(&self) {
        let m = lock(&self.map);
        let views: Vec<SegmentView> = m
            .segments()
            .iter()
            .map(|s| SegmentView { start: s.start, end: s.end, done: s.pos - s.start, active: s.owner.is_some() })
            .collect();
        let done: u64 = views.iter().map(|v| v.done).sum();
        drop(m);
        let mut v = views;
        v.sort_by_key(|s| s.start);
        *lock(&self.progress.segments) = v;
        self.progress.downloaded.store(done, Ordering::SeqCst);
        self.progress.connections.store(self.live.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    /// Flush data, then write the journal (never the other way round).
    async fn save_journal(self: &Arc<Self>) {
        if !self.ranges_ok.load(Ordering::SeqCst) || self.single.load(Ordering::SeqCst) {
            return;
        }
        let (total, remaining) = {
            let m = lock(&self.map);
            match m.total() {
                Some(t) => (t, m.journal_ranges()),
                None => return,
            }
        };
        let j = {
            let meta = lock(&self.meta);
            Journal::new(&self.key, total, meta.etag.clone(), meta.last_modified.clone(), remaining)
        };
        let (data, path, durable) = (self.data.clone(), self.journal.clone(), self.dl.config.durable);
        self.dirty.store(false, Ordering::SeqCst);
        let _ = tokio::task::spawn_blocking(move || {
            if durable {
                file::sync(&data)?;
            }
            j.save(&path)
        })
        .await;
    }
}

pub(crate) async fn run(
    dl: &Downloader,
    job: Job,
    progress: Arc<Progress>,
    control: Arc<Control>,
) -> Result<Outcome, Error> {
    match run_once(dl, job.clone(), progress.clone(), control.clone()).await {
        Err(Error::FileChanged(why)) if !control.is_cancelled() => {
            if let Some(f) = &dl.events {
                f(&Event::Restarted { reason: why });
            }
            remove_partial_in(&job.dest, job.work_dir.as_deref());
            let fresh = Job { size_hint: None, ranges_hint: false, ..job };
            run_once(dl, fresh, progress, control).await
        }
        r => r,
    }
}

fn load_resume(job: &Job, key: &str) -> Option<Journal> {
    let j = Journal::load(&job.journal())?;
    let len = std::fs::metadata(job.data()).ok()?.len();
    (j.key == key && len == j.total && job.size_hint.is_none_or(|t| t == j.total)).then_some(j)
}

async fn run_once(
    dl: &Downloader,
    job: Job,
    progress: Arc<Progress>,
    control: Arc<Control>,
) -> Result<Outcome, Error> {
    let cfg = dl.config.clone();
    let conns = cfg.connections.clamp(1, 32);
    let min_split = cfg.min_split.max(64 * 1024);
    let key = job.resume_key.clone().unwrap_or_else(|| job.url.clone());
    let data = job.data();
    for dir in [job.dest.parent(), data.parent()].into_iter().flatten() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut meta = Meta::default();
    let (map, ranges_ok) = match load_resume(&job, &key) {
        Some(j) => {
            meta.etag = j.etag.clone();
            meta.last_modified = j.last_modified.clone();
            (SegmentMap::from_remaining(j.total, &j.remaining, min_split), true)
        }
        None => {
            remove_partial_in(&job.dest, job.work_dir.as_deref());
            match job.size_hint {
                Some(t) if job.ranges_hint && t > 0 => {
                    file::create(&data, Some(t))?;
                    (SegmentMap::even(t, conns, min_split), true)
                }
                _ => {
                    file::create(&data, None)?;
                    (SegmentMap::whole(None, min_split), false)
                }
            }
        }
    };
    let resumed = map.written();
    if let Some(t) = map.total() {
        progress.set_total(t);
    }
    progress.resumable.store(ranges_ok, Ordering::SeqCst);
    let dest_is_html = job
        .dest
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "xhtml" | "shtml"));
    let run = Arc::new(Run {
        dl: dl.clone(),
        mirrors: Mutex::new(job.mirrors.clone()),
        has_mirrors: !job.mirrors.is_empty() || cfg.size_identity,
        url: RwLock::new(job.url.clone()),
        journal: job.journal(),
        data,
        key,
        map: Mutex::new(map),
        progress,
        control,
        cap: AtomicUsize::new(conns),
        last_cut: Mutex::new(Instant::now()),
        live: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        proven: AtomicUsize::new(0),
        best_speed: Mutex::new(0.0),
        ranges_ok: AtomicBool::new(ranges_ok),
        single: AtomicBool::new(false),
        stats: Mutex::new(Vec::new()),
        speeds: Mutex::new(Vec::new()),
        meta: Mutex::new(meta),
        fail_streak: AtomicU32::new(0),
        last_progress: Mutex::new(Instant::now()),
        fatal: Mutex::new(None),
        stop: AtomicBool::new(false),
        refreshes: AtomicU32::new(0),
        refreshed: AtomicBool::new(false),
        refresh_lock: tokio::sync::Mutex::new(()),
        dirty: AtomicBool::new(false),
        resumed,
        dest_is_html,
        job,
    });
    if resumed > 0 {
        run.emit(Event::Resumed { bytes: resumed });
    }
    monitor(run).await
}

fn spawn_worker(run: &Arc<Run>, set: &mut tokio::task::JoinSet<()>) {
    let id = {
        let mut stats = lock(&run.stats);
        stats.push(Arc::new(WorkerStat::new()));
        lock(&run.speeds).push(0.0);
        stats.len() - 1
    };
    let live = run.live.fetch_add(1, Ordering::SeqCst) + 1;
    run.peak.fetch_max(live, Ordering::SeqCst);
    set.spawn(worker(run.clone(), id));
}

async fn monitor(run: Arc<Run>) -> Result<Outcome, Error> {
    let cfg = run.dl.config.clone();
    let mut set = tokio::task::JoinSet::new();
    let mut last_journal = Instant::now();
    let mut last_speed = Instant::now();
    let mut last_bytes: Vec<u64> = Vec::new();
    loop {
        while set.try_join_next().is_some() {}
        if run.stopping() {
            break;
        }
        let (complete, claimable) = {
            let m = lock(&run.map);
            (m.is_complete(), m.claimable())
        };
        if complete && set.is_empty() {
            break;
        }
        // Open connections while there is work: one until the first reply
        // proves ranges work and names the size, then up to the cap.
        if !complete && claimable && !run.control.is_paused() {
            let parallel = run.ranges_ok.load(Ordering::SeqCst)
                && !run.single.load(Ordering::SeqCst)
                && lock(&run.map).total().is_some();
            let cap = run.cap.load(Ordering::SeqCst).max(1);
            while run.live.load(Ordering::SeqCst) < cap
                && (parallel || run.live.load(Ordering::SeqCst) == 0)
                && lock(&run.map).claimable()
            {
                spawn_worker(&run, &mut set);
                if !parallel {
                    break;
                }
            }
        }
        if set.is_empty() && !complete && !claimable {
            run.set_fatal(Error::Network("download stopped with data missing".into()));
            break;
        }
        run.snapshot();
        run.probe_cap(cfg.connections.clamp(1, 32));
        if last_speed.elapsed() >= Duration::from_secs(1) {
            measure(&run, &cfg, &mut last_bytes, last_speed.elapsed());
            last_speed = Instant::now();
        }
        if run.dirty.load(Ordering::SeqCst) && last_journal.elapsed() >= cfg.journal_interval {
            run.save_journal().await;
            last_journal = Instant::now();
        }
        if set.is_empty() {
            tokio::time::sleep(TICK).await;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(TICK) => {}
                _ = set.join_next() => {}
            }
        }
    }

    // Stop or finish: let workers write what they have, then cut the rest.
    run.stop.store(true, Ordering::SeqCst);
    let grace = Instant::now() + Duration::from_secs(2);
    while !set.is_empty() && Instant::now() < grace {
        let _ = tokio::time::timeout(Duration::from_millis(100), set.join_next()).await;
    }
    set.abort_all();
    while set.join_next().await.is_some() {}
    run.snapshot();

    let fatal = lock(&run.fatal).take();
    let complete = lock(&run.map).is_complete();
    if let Some(e) = fatal {
        run.save_journal().await;
        return Err(e);
    }
    if !complete {
        run.save_journal().await;
        return Err(Error::Cancelled);
    }
    finish(&run).await
}

/// Speeds per connection, and reopening the very slow ones.
fn measure(run: &Run, cfg: &crate::Config, last: &mut Vec<u64>, dt: Duration) {
    let stats = lock(&run.stats).clone();
    let mut speeds = lock(&run.speeds);
    last.resize(stats.len(), 0);
    speeds.resize(stats.len(), 0.0);
    let secs = dt.as_secs_f64().max(0.001);
    let streaming = stats.iter().filter(|s| s.active.load(Ordering::SeqCst) && s.streaming.load(Ordering::SeqCst)).count();
    run.proven.fetch_max(streaming, Ordering::SeqCst);
    for (i, s) in stats.iter().enumerate() {
        let b = s.bytes.load(Ordering::SeqCst);
        let inst = (b - last[i]) as f64 / secs;
        last[i] = b;
        speeds[i] = if speeds[i] == 0.0 { inst } else { 0.4 * speeds[i] + 0.6 * inst };
    }
    let best = speeds.iter().copied().fold(0.0_f64, f64::max);
    lock(&run.map).set_speeds(speeds.clone(), best);
    let best = {
        let mut b = lock(&run.best_speed);
        *b = b.max(best);
        *b
    };
    let Some(after) = cfg.slow_restart_after else { return };
    let active: Vec<usize> = (0..stats.len()).filter(|&i| stats[i].active.load(Ordering::SeqCst)).collect();
    if active.is_empty() {
        return;
    }
    // Compared with the others while there are enough of them, else with
    // the fastest this download has gone (the last two crawling alone).
    // Never below half the best: when every connection still running is a
    // crawling one, their own median would call none of them slow.
    let median = if active.len() >= 3 {
        let mut v: Vec<f64> = active.iter().map(|&i| speeds[i]).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    } else {
        0.0
    }
    .max(best * 0.5);
    if median <= 0.0 {
        return;
    }
    // Anything a crawling connection holds past half a megabyte is worth a
    // fresh connection (the file waits for its slowest part).
    let big = 512 * 1024;
    let m = lock(&run.map);
    for &i in &active {
        let s = &stats[i];
        let owns_big = m.segments().iter().any(|seg| seg.owner == Some(i) && seg.remaining() >= big);
        let recent = lock(&s.last_restart).is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if owns_big && !recent && speeds[i] < cfg.slow_ratio * median && lock(&s.started).elapsed() >= after {
            s.restart.store(true, Ordering::SeqCst);
            *lock(&s.last_restart) = Some(Instant::now());
            run.emit(Event::SlowRestart { worker: i });
        }
    }
}

async fn finish(run: &Arc<Run>) -> Result<Outcome, Error> {
    let (size, total_known) = {
        let m = lock(&run.map);
        (m.total().unwrap_or_else(|| m.written()), m.total().is_some())
    };
    let data = run.data.clone();
    let durable = run.dl.config.durable;
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if !total_known {
            file::set_len(&data, size)?;
        }
        if durable {
            file::sync(&data)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| Error::Io(std::io::Error::other(e)))??;
    let head = file::read_head(&run.data, 512);
    let detected = sniff::sniff(&head);
    if detected == Some("html") && run.dl.config.reject_html && !run.dest_is_html {
        return Err(Error::HtmlPage);
    }
    if tokio::fs::rename(&run.data, &run.job.dest).await.is_err() {
        let _ = tokio::fs::remove_file(&run.job.dest).await;
        if tokio::fs::rename(&run.data, &run.job.dest).await.is_err() {
            // Work folder on another drive: copy, then drop the working copy.
            tokio::fs::copy(&run.data, &run.job.dest).await?;
            let _ = tokio::fs::remove_file(&run.data).await;
        }
    }
    let _ = tokio::fs::remove_file(&run.journal).await;
    let meta = lock(&run.meta);
    Ok(Outcome {
        path: run.job.dest.clone(),
        size,
        final_url: run.url.read().unwrap_or_else(|e| e.into_inner()).clone(),
        content_type: meta.content_type.clone(),
        filename: meta.filename.clone(),
        resumed: run.resumed,
        peak_connections: run.peak.load(Ordering::SeqCst),
        url_refreshed: run.refreshed.load(Ordering::SeqCst),
        detected,
    })
}

/// Drops the live count when a worker ends (unless `over_cap` already did).
struct LiveGuard<'a>(&'a Run, bool);

impl Drop for LiveGuard<'_> {
    fn drop(&mut self) {
        if !self.1 {
            self.0.live.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

async fn worker(run: Arc<Run>, w: usize) {
    let mut guard = LiveGuard(&run, false);
    let stat = lock(&run.stats)[w].clone();
    let _permit = match &run.dl.gate {
        Some(g) => g.clone().acquire_owned().await.ok(),
        None => None,
    };
    let mut out: Option<Arc<std::fs::File>> = None;
    loop {
        if run.stopping() {
            break;
        }
        if run.over_cap() {
            guard.1 = true;
            break;
        }
        while run.control.is_paused() && !run.stopping() {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let claim = {
            let speeds = lock(&run.speeds).clone();
            lock(&run.map).claim(w, &|i| speeds.get(i).copied().unwrap_or(0.0))
        };
        let Some(claim) = claim else { break };
        // A new request can run at a very different speed (another server
        // slot, another path): its speed is measured afresh, or a connection
        // fast on its last piece hides that its new one crawls.
        if let Some(v) = lock(&run.speeds).get_mut(w) {
            *v = 0.0;
        }
        run.emit(Event::Connecting { worker: w, from: claim.pos, to: claim.end });
        *lock(&stat.started) = Instant::now();
        stat.restart.store(false, Ordering::SeqCst);
        stat.active.store(true, Ordering::SeqCst);
        let mirror = run.mirror_for(w);
        let res = fetch(&run, w, &stat, claim, &mut out, mirror.as_deref()).await;
        // A mirror that refuses or serves something else is dropped; the
        // segment goes back to the pool for the main URL or another mirror.
        let res = match (res, mirror) {
            (Err(f), Some(m)) if !matches!(f, Fail::Cancelled | Fail::Retry { progressed: true, .. }) => {
                let why = match &f {
                    Fail::Fatal(e) => e.to_string(),
                    Fail::Refused { status, .. } => format!("HTTP {status}"),
                    Fail::Retry { msg, .. } => msg.clone(),
                    Fail::Throttled { status, .. } => format!("HTTP {status}"),
                    _ => "ignores ranges".into(),
                };
                run.drop_mirror(&m, &why);
                Ok(())
            }
            (r, _) => r,
        };
        stat.active.store(false, Ordering::SeqCst);
        stat.streaming.store(false, Ordering::SeqCst);
        lock(&run.map).release(claim.id, w);
        match res {
            Ok(()) => {}
            Err(Fail::Cancelled) => break,
            Err(Fail::Fatal(e)) => {
                run.set_fatal(e);
                break;
            }
            Err(Fail::RangeIgnored) => break,
            Err(Fail::Retry { msg, progressed }) => {
                // Nothing came through while other connections are working
                // (or just were): that is the server's connection limit, not
                // a broken link. The last connection keeps trying instead.
                let live = run.live.load(Ordering::SeqCst);
                let recent = lock(&run.last_progress).elapsed() < cfg_stall(&run) * 2;
                if !progressed && live > 1 && (run.others_healthy(w) || recent) {
                    run.reduce_cap(live.saturating_sub(1), format!("the server did not take another connection ({msg})"));
                    break;
                }
                match run.note_failure(progressed, msg, w) {
                    Some(streak) => run.nap(backoff(streak)).await,
                    None => break,
                }
            }
            Err(Fail::Throttled { status, retry_after }) => {
                let cap = run.cap.load(Ordering::SeqCst);
                // "Busy" while other connections are receiving data is the
                // server's per-client limit: settle on the ones that work
                // (halving on every refusal fell to one connection).
                let working = run.streaming_count(w);
                if working > 0 {
                    // Never below what the server has already served at
                    // once: a connection asking for its next piece is often
                    // refused only because its last one is not closed yet.
                    let allowed = working.max(run.proven.load(Ordering::SeqCst));
                    run.reduce_cap(allowed, format!("the server allows {allowed} connection(s) (HTTP {status})"));
                    break;
                }
                if cap > 1 {
                    run.reduce_cap(cap / 2, format!("the server is throttling (HTTP {status})"));
                } else if run.note_failure(false, format!("server is throttling (HTTP {status})"), w).is_none() {
                    *lock(&run.fatal) = Some(Error::Throttled { status });
                    break;
                }
                let wait = retry_after.unwrap_or_else(|| backoff(run.fail_streak.load(Ordering::SeqCst).max(2)));
                run.nap(wait.min(Duration::from_secs(60))).await;
            }
            Err(Fail::Refused { status, url }) => {
                if run.on_refused(w, status, &url).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn cfg_stall(run: &Run) -> Duration {
    run.dl.config.stall_timeout
}

fn backoff(streak: u32) -> Duration {
    Duration::from_millis(500u64 << streak.min(6)).min(Duration::from_secs(30))
}

enum End {
    Body,
    Reached,
    Restart,
}

async fn fetch(
    run: &Arc<Run>,
    w: usize,
    stat: &WorkerStat,
    claim: Claim,
    out: &mut Option<Arc<std::fs::File>>,
    mirror: Option<&str>,
) -> Result<(), Fail> {
    let cfg = &run.dl.config;
    let url = match mirror {
        Some(m) => m.to_string(),
        None => run.url.read().unwrap_or_else(|e| e.into_inner()).clone(),
    };
    let Claim { id, pos, end } = claim;
    let req_end = match cfg.max_request {
        Some(mr) => end.min(pos.saturating_add(mr.max(64 * 1024))),
        None => end,
    };
    let range = if req_end == OPEN_END {
        format!("bytes={pos}-")
    } else {
        format!("bytes={pos}-{}", req_end - 1)
    };
    let send = (run.dl.request)(&url).header(reqwest::header::RANGE, range).send();
    let answer = if run.others_healthy(w) { cfg.stall_timeout.min(BUSY_ANSWER) } else { cfg.stall_timeout };
    let resp = match tokio::time::timeout(answer, send).await {
        Err(_) => {
            return Err(Fail::Retry { msg: format!("no answer in {}s", answer.as_secs()), progressed: false })
        }
        Ok(Err(e)) => return Err(Fail::Retry { msg: format!("network: {e}"), progressed: false }),
        Ok(Ok(r)) => r,
    };
    let status = resp.status().as_u16();
    match status {
        200 | 206 => {}
        416 => {
            let total = lock(&run.map).total();
            if total.is_some_and(|t| pos >= t) {
                return Ok(());
            }
            return Err(Fail::Fatal(Error::FileChanged("the server rejected the byte range (HTTP 416)".into())));
        }
        429 | 503 => {
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(Fail::Throttled { status, retry_after });
        }
        401 | 403 | 404 | 410 | 451 => return Err(Fail::Refused { status, url }),
        408 | 425 | 500..=599 => {
            return Err(Fail::Retry { msg: format!("HTTP {status}"), progressed: false })
        }
        _ => return Err(Fail::Fatal(Error::Http { status, url })),
    }

    let h = resp.headers();
    let text = |name: reqwest::header::HeaderName| h.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let etag = text(reqwest::header::ETAG);
    let lm = text(reqwest::header::LAST_MODIFIED);
    let ctype = text(reqwest::header::CONTENT_TYPE);
    let disposition = text(reqwest::header::CONTENT_DISPOSITION);
    let partial = status == 206;
    let mut skip = 0u64;
    let expected: Option<u64>;
    if partial {
        match text(reqwest::header::CONTENT_RANGE).as_deref().and_then(parse_content_range) {
            Some((a, b, total)) => {
                if a != pos {
                    return Err(Fail::Retry {
                        msg: format!("the server sent bytes from {a} instead of {pos}"),
                        progressed: false,
                    });
                }
                expected = Some(b.saturating_sub(a) + 1);
                run.learn(total, true)?;
            }
            None if pos == 0 => {
                expected = resp.content_length();
                run.learn(None, false)?;
            }
            None => {
                return Err(Fail::Retry { msg: "206 reply without Content-Range".into(), progressed: false });
            }
        }
    } else {
        // 200: the server ignored our Range.
        expected = resp.content_length();
        let multi = lock(&run.map).segments().len() > 1;
        if pos > 0 || multi {
            run.go_single();
        }
        // Nothing is on disk yet: let this one stream carry the whole file
        // instead of re-reading the start for every segment.
        if pos == 0 && multi {
            lock(&run.map).absorb(id);
        }
        run.learn(expected, false)?;
        if pos > 0 {
            if run.live.load(Ordering::SeqCst) > 1 {
                return Err(Fail::RangeIgnored);
            }
            skip = pos;
        }
    }
    run.check_validators(etag, lm)?;
    stat.streaming.store(true, Ordering::SeqCst);
    {
        let mut meta = lock(&run.meta);
        if meta.content_type.is_none() {
            meta.content_type = ctype.clone();
        }
        if meta.filename.is_none() {
            meta.filename = disposition.as_deref().and_then(disposition_name);
        }
    }
    if cfg.reject_html
        && !run.dest_is_html
        && ctype.as_deref().is_some_and(|c| c.to_ascii_lowercase().contains("text/html"))
    {
        return Err(Fail::Fatal(Error::HtmlPage));
    }
    // Follow-up requests go straight to the redirect target (one round trip
    // less); a refusal there falls back to the original URL.
    let landed = resp.url().as_str().to_string();
    if landed != url && mirror.is_none() {
        let mut cur = run.url.write().unwrap_or_else(|e| e.into_inner());
        if *cur == url {
            *cur = landed;
        }
    }

    let mut buf: Vec<u8> = Vec::with_capacity(cfg.write_buffer);
    let mut buf_off = pos;
    let mut got = 0u64;
    let mut body_bytes = 0u64;
    let mut stream = resp.bytes_stream();
    let ended: Result<End, Fail> = loop {
        if run.stopping() {
            break Err(Fail::Cancelled);
        }
        while run.control.is_paused() && !run.stopping() {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        if stat.restart.swap(false, Ordering::SeqCst) {
            break Ok(End::Restart);
        }
        let chunk = match tokio::time::timeout(cfg.stall_timeout, stream.next()).await {
            Err(_) => {
                break Err(Fail::Retry {
                    msg: format!("stalled: no data for {}s", cfg.stall_timeout.as_secs()),
                    progressed: got > 0,
                })
            }
            Ok(None) => break Ok(End::Body),
            Ok(Some(Err(e))) => break Err(Fail::Retry { msg: format!("network: {e}"), progressed: got > 0 }),
            Ok(Some(Ok(b))) => b,
        };
        body_bytes += chunk.len() as u64;
        let mut data = &chunk[..];
        if skip > 0 {
            let cut = (skip as usize).min(data.len());
            data = &data[cut..];
            skip -= cut as u64;
            if data.is_empty() {
                continue;
            }
        }
        let acc = lock(&run.map).take(id, w, data.len() as u64) as usize;
        if acc > 0 {
            buf.extend_from_slice(&data[..acc]);
            got += acc as u64;
            stat.bytes.fetch_add(acc as u64, Ordering::SeqCst);
            *lock(&stat.last_byte) = Some(Instant::now());
            if let Some(l) = &run.dl.limiter {
                l.consume(acc as u64).await;
            }
        }
        if acc < data.len() {
            break Ok(End::Reached);
        }
        if buf.len() >= cfg.write_buffer {
            if let Err(e) = flush(run, out, id, &mut buf, &mut buf_off).await {
                break Err(Fail::Fatal(Error::Io(e)));
            }
        }
    };
    if let Err(e) = flush(run, out, id, &mut buf, &mut buf_off).await {
        return Err(Fail::Fatal(Error::Io(e)));
    }
    match ended? {
        End::Reached | End::Restart => Ok(()),
        End::Body => {
            let (p, e) = lock(&run.map).bounds(id).unwrap_or((0, 0));
            if p >= e {
                return Ok(());
            }
            if e == OPEN_END {
                if expected.is_some_and(|x| body_bytes < x) {
                    return Err(Fail::Retry {
                        msg: format!("connection ended early (got {body_bytes} of {} bytes)", expected.unwrap_or(0)),
                        progressed: got > 0,
                    });
                }
                lock(&run.map).close_open(id);
                return Ok(());
            }
            if expected.is_some_and(|x| body_bytes >= x) {
                // The server sent everything it promised, just less than we
                // asked for (or a max_request window ended): carry on.
                return Ok(());
            }
            Err(Fail::Retry {
                msg: format!("connection ended early (got {got} of {} bytes)", e - pos),
                progressed: got > 0,
            })
        }
    }
}

/// Write the buffer at its offset (on a blocking thread), then record it.
async fn flush(
    run: &Arc<Run>,
    out: &mut Option<Arc<std::fs::File>>,
    id: u64,
    buf: &mut Vec<u8>,
    off: &mut u64,
) -> std::io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let f = match out {
        Some(f) => f.clone(),
        None => {
            let f = Arc::new(file::open_writer(&run.data)?);
            *out = Some(f.clone());
            f
        }
    };
    let data = std::mem::take(buf);
    let at = *off;
    let len = data.len() as u64;
    let mut data = tokio::task::spawn_blocking(move || file::write_all_at(&f, &data, at).map(|_| data))
        .await
        .map_err(std::io::Error::other)??;
    data.clear();
    *buf = data;
    *off += len;
    lock(&run.map).mark_written(id, *off);
    run.dirty.store(true, Ordering::SeqCst);
    run.fail_streak.store(0, Ordering::SeqCst);
    *lock(&run.last_progress) = Instant::now();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 99, Some(1000))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[test]
    fn disposition() {
        assert_eq!(disposition_name("attachment; filename=\"a b.zip\""), Some("a b.zip".into()));
        assert_eq!(
            disposition_name("attachment; filename=\"x.bin\"; filename*=UTF-8''%E0%A6%95.mp4"),
            Some("ক.mp4".into())
        );
        assert_eq!(disposition_name("inline"), None);
    }
}
