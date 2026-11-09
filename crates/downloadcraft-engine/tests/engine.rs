//! End-to-end tests against a local HTTP server that misbehaves on purpose:
//! slow connections, cut responses, throttling, expiring links, servers that
//! ignore ranges, HTML error pages and files that change between runs.

use downloadcraft_engine::{Config, Control, Downloader, Error, Event, Job, Progress};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MIB: usize = 1024 * 1024;

fn byte_at(i: usize, version: u8) -> u8 {
    (i.wrapping_mul(7) ^ (i >> 11)).wrapping_add(version as usize) as u8
}

fn body(size: usize, version: u8) -> Vec<u8> {
    (0..size).map(|i| byte_at(i, version)).collect()
}

#[derive(Default)]
struct Behaviour {
    size: usize,
    no_ranges: bool,
    /// Bytes/s for the first connection that asks for byte 0 (0 = full speed).
    slow_first: u64,
    /// Bytes/s for every connection (0 = full speed).
    rate: u64,
    /// More concurrent connections than this get 429.
    max_conns: usize,
    /// More concurrent connections than this get 403 (per-IP limit).
    refuse_over: usize,
    /// More concurrent connections than this never get an answer.
    hang_over: usize,
    /// Cut this many responses after `cut_after` bytes.
    cut: AtomicUsize,
    cut_after: usize,
    html: bool,
    /// Ranges bigger than this get 403 (googlevideo style). 0 = no limit.
    max_range: usize,
    /// `/start` redirects to `/signed/<n>`; a signature stops working after
    /// it has served this many bytes. 0 = no expiry.
    expire_after: usize,
    /// `/old` refuses with 403 once this many bytes were served. 0 = never.
    old_dies_after: usize,
}

struct Server {
    base: String,
    b: Arc<Behaviour>,
    version: AtomicU64,
    active: AtomicUsize,
    peak: AtomicUsize,
    slow_taken: AtomicBool,
    served: AtomicU64,
    signature: AtomicU64,
    sig_served: AtomicU64,
    requests: Mutex<Vec<String>>,
}

impl Server {
    async fn start(b: Behaviour) -> Arc<Server> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let s = Arc::new(Server {
            base,
            b: Arc::new(b),
            version: AtomicU64::new(1),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            slow_taken: AtomicBool::new(false),
            served: AtomicU64::new(0),
            signature: AtomicU64::new(1),
            sig_served: AtomicU64::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let srv = s.clone();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let srv = srv.clone();
                tokio::spawn(async move {
                    let _ = srv.serve(sock).await;
                });
            }
        });
        s
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn serve(&self, mut sock: TcpStream) -> std::io::Result<()> {
        let mut slow_conn = false;
        loop {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if sock.read(&mut byte).await? == 0 {
                    return Ok(());
                }
                head.push(byte[0]);
            }
            // Concurrent requests count, not idle kept-alive sockets.
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            let r = self.respond(&mut sock, &head, &mut slow_conn).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            if !r? {
                return Ok(());
            }
        }
    }

    /// Answer one request. Ok(false) = drop the connection.
    async fn respond(&self, sock: &mut TcpStream, head: &[u8], slow_conn: &mut bool) -> std::io::Result<bool> {
        {
            let text = String::from_utf8_lossy(head).to_string();
            let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
            let range = text
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string));
            self.requests.lock().unwrap().push(format!("{path} {range:?}"));
            let b = &self.b;
            let size = b.size;
            let version = self.version.load(Ordering::SeqCst) as u8;
            if b.max_conns > 0 && self.active.load(Ordering::SeqCst) > b.max_conns {
                sock.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n").await?;
                return Ok(true);
            }
            if b.hang_over > 0 && self.active.load(Ordering::SeqCst) > b.hang_over {
                // Hold the socket without answering until the client gives up.
                let mut sink = [0u8; 64];
                let _ = tokio::time::timeout(Duration::from_secs(60), sock.read(&mut sink)).await;
                return Ok(false);
            }
            if b.refuse_over > 0 && self.active.load(Ordering::SeqCst) > b.refuse_over {
                sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
                return Ok(true);
            }
            if path == "/start" {
                let sig = self.signature.load(Ordering::SeqCst);
                let r = format!("HTTP/1.1 302 Found\r\nLocation: /signed/{sig}\r\nContent-Length: 0\r\n\r\n");
                sock.write_all(r.as_bytes()).await?;
                return Ok(true);
            }
            if let Some(sig) = path.strip_prefix("/signed/") {
                let current = self.signature.load(Ordering::SeqCst).to_string();
                if sig != current {
                    sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
                    return Ok(true);
                }
                if b.expire_after > 0 && self.sig_served.load(Ordering::SeqCst) as usize >= b.expire_after {
                    // This signature is used up: the next redirect issues a new one.
                    self.sig_served.store(0, Ordering::SeqCst);
                    self.signature.fetch_add(1, Ordering::SeqCst);
                    sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
                    return Ok(true);
                }
            }
            if path == "/old" && b.old_dies_after > 0 && self.served.load(Ordering::SeqCst) as usize >= b.old_dies_after {
                sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
                return Ok(true);
            }
            if b.html {
                let page = b"<!DOCTYPE html><html><body>Please log in</body></html>";
                let r = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n", page.len());
                sock.write_all(r.as_bytes()).await?;
                sock.write_all(page).await?;
                return Ok(true);
            }
            let (mut start, mut end) = (0usize, size - 1);
            let mut partial = false;
            if let (Some(r), false) = (range.as_deref(), b.no_ranges) {
                let (a, z) = r.trim().split_once('-').unwrap();
                start = a.parse().unwrap();
                end = if z.is_empty() { size - 1 } else { z.parse::<usize>().unwrap().min(size - 1) };
                partial = true;
                if start >= size {
                    let r = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n");
                    sock.write_all(r.as_bytes()).await?;
                    return Ok(true);
                }
                if b.max_range > 0 && end - start + 1 > b.max_range {
                    sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
                    return Ok(true);
                }
            }
            if start == 0 && b.slow_first > 0 && !self.slow_taken.swap(true, Ordering::SeqCst) {
                *slow_conn = true;
            }
            let len = end - start + 1;
            let mut r = String::new();
            if partial {
                r.push_str(&format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{size}\r\n"));
            } else {
                r.push_str("HTTP/1.1 200 OK\r\n");
            }
            if !b.no_ranges {
                r.push_str("Accept-Ranges: bytes\r\n");
            }
            r.push_str(&format!("ETag: \"v{version}\"\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\n\r\n"));
            sock.write_all(r.as_bytes()).await?;
            let cut_here = b.cut_after > 0 && take_one(&b.cut);
            let rate = if *slow_conn { b.slow_first } else { b.rate };
            let data = body(size, version);
            let t0 = Instant::now();
            let mut sent = 0usize;
            while sent < len {
                let n = (len - sent).min(16 * 1024);
                if cut_here && sent + n > b.cut_after {
                    return Ok(false); // drop the connection mid-body
                }
                sock.write_all(&data[start + sent..start + sent + n]).await?;
                sent += n;
                self.served.fetch_add(n as u64, Ordering::SeqCst);
                if path.starts_with("/signed/") {
                    self.sig_served.fetch_add(n as u64, Ordering::SeqCst);
                }
                if rate > 0 {
                    let ahead = sent as f64 / rate as f64 - t0.elapsed().as_secs_f64();
                    if ahead > 0.0 {
                        tokio::time::sleep(Duration::from_secs_f64(ahead)).await;
                    }
                }
            }
        }
        Ok(true)
    }
}

/// Decrement a counter if it is above zero.
fn take_one(c: &AtomicUsize) -> bool {
    let mut v = c.load(Ordering::SeqCst);
    while v > 0 {
        match c.compare_exchange(v, v - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => v = now,
        }
    }
    false
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dce-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("file.bin")
}

fn quick() -> Config {
    Config {
        connections: 4,
        min_split: 256 * 1024,
        stall_timeout: Duration::from_secs(5),
        journal_interval: Duration::from_millis(200),
        ..Config::default()
    }
}

fn recorder() -> (Arc<Mutex<Vec<String>>>, downloadcraft_engine::EventFn) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l = log.clone();
    (log, Arc::new(move |e: &Event| l.lock().unwrap().push(format!("{e:?}"))))
}

async fn get(dl: &Downloader, job: Job) -> Result<downloadcraft_engine::Outcome, Error> {
    dl.download(job, Arc::new(Progress::default()), Arc::new(Control::default())).await
}

#[tokio::test]
async fn parallel_download_is_byte_exact() {
    let srv = Server::start(Behaviour { size: 8 * MIB + 123, ..Default::default() }).await;
    let dest = tmp("parallel");
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(8 * MIB as u64 + 123), true);
    let out = get(&Downloader::new(reqwest::Client::new()).config(quick()), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(8 * MIB + 123, 1));
    assert!(out.peak_connections >= 2, "peak {}", out.peak_connections);
    assert!(!downloadcraft_engine::data_path(&dest).exists());
    assert!(!downloadcraft_engine::journal_path(&dest).exists());
}

#[tokio::test]
async fn first_request_is_the_probe() {
    // No size hint: one connection starts, the others join after its reply.
    let srv = Server::start(Behaviour { size: 6 * MIB, rate: 4 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("ramp");
    let out = get(&Downloader::new(reqwest::Client::new()).config(quick()), Job::new(srv.url("/f"), &dest))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
    assert!(out.peak_connections >= 2, "peak {}", out.peak_connections);
    let first = srv.requests.lock().unwrap()[0].clone();
    assert_eq!(first, "/f Some(\"0-\")", "no HEAD, no separate probe");
}

#[tokio::test]
async fn a_slow_connection_does_not_hold_the_download_back() {
    // The connection serving the start crawls at 64 KiB/s. With a fixed
    // 4-way split it would need 2 MiB / 64 KiB/s = 32 s.
    let srv = Server::start(Behaviour { size: 8 * MIB, slow_first: 64 * 1024, rate: 8 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("slow");
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(8 * MIB as u64), true);
    let t0 = Instant::now();
    get(&Downloader::new(reqwest::Client::new()).config(quick()), job).await.unwrap();
    assert!(t0.elapsed() < Duration::from_secs(12), "took {:?}", t0.elapsed());
    assert_eq!(std::fs::read(&dest).unwrap(), body(8 * MIB, 1));
}

#[tokio::test]
async fn a_server_without_ranges_still_works() {
    // A paced server: unpaced, a fast OS lets every extra connection's full
    // reply land in socket buffers before the engine drops it, and `served`
    // counts what the server wrote, not what the engine read.
    let srv = Server::start(Behaviour { size: 3 * MIB, no_ranges: true, rate: 4 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("noranges");
    let progress = Arc::new(Progress::default());
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(3 * MIB as u64), true);
    Downloader::new(reqwest::Client::new())
        .config(quick())
        .download(job, progress.clone(), Arc::new(Control::default()))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(3 * MIB, 1));
    assert!(!progress.resumable());
    // One stream filled the file: the start was not re-read per segment.
    assert!(srv.served.load(Ordering::SeqCst) <= 2 * 3 * MIB as u64);
}

#[tokio::test]
async fn cut_connections_are_resumed() {
    let srv = Server::start(Behaviour { size: 4 * MIB, cut: AtomicUsize::new(5), cut_after: 300_000, ..Default::default() }).await;
    let dest = tmp("cuts");
    let (log, ev) = recorder();
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(4 * MIB as u64), true);
    get(&Downloader::new(reqwest::Client::new()).config(quick()).on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(4 * MIB, 1));
    assert!(log.lock().unwrap().iter().any(|e| e.contains("Retry")));
}

#[tokio::test]
async fn throttling_lowers_the_connection_count() {
    let srv = Server::start(Behaviour { size: 4 * MIB, max_conns: 2, rate: 4 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("throttle");
    let (log, ev) = recorder();
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(4 * MIB as u64), true);
    let cfg = Config { connections: 8, ..quick() };
    get(&Downloader::new(reqwest::Client::new()).config(cfg).on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(4 * MIB, 1));
    assert!(log.lock().unwrap().iter().any(|e| e.contains("ConnectionsReduced")));
}

#[tokio::test]
async fn stopped_downloads_resume_where_they_were() {
    let srv = Server::start(Behaviour { size: 6 * MIB, rate: 1024 * 1024, ..Default::default() }).await;
    let dest = tmp("resume");
    let dl = Downloader::new(reqwest::Client::new()).config(quick());
    let control = Arc::new(Control::default());
    let progress = Arc::new(Progress::default());
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(6 * MIB as u64), true);
    let (c, pr) = (control.clone(), progress.clone());
    // Stop once some bytes have landed: a fixed delay raced an optimized
    // build that finished the file first.
    tokio::spawn(async move {
        while pr.downloaded() < 512 * 1024 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        c.cancel();
    });
    let err = dl.download(job.clone(), progress.clone(), control).await.unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err:?}");
    assert!(downloadcraft_engine::has_partial(&dest));
    let first_run = progress.downloaded();
    assert!(first_run > 0);
    let out = get(&dl, job).await.unwrap();
    assert!(out.resumed > 0, "resumed {}", out.resumed);
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
}

#[tokio::test]
async fn a_partial_moved_to_a_new_name_resumes_there() {
    // The capture dialog starts a download under a temporary name and hands
    // its data over to the name the user picks: the journal is keyed by the
    // URL, so the moved data resumes.
    let srv = Server::start(Behaviour { size: 6 * MIB, rate: 1024 * 1024, ..Default::default() }).await;
    let first = tmp("pre-start");
    let dest = tmp("pre-final");
    let dl = Downloader::new(reqwest::Client::new()).config(quick());
    let control = Arc::new(Control::default());
    let progress = Arc::new(Progress::default());
    let (c, pr) = (control.clone(), progress.clone());
    tokio::spawn(async move {
        while pr.downloaded() < 512 * 1024 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        c.cancel();
    });
    let job = Job::new(srv.url("/f"), &first).size_hint(Some(6 * MIB as u64), true);
    assert!(matches!(dl.download(job, progress.clone(), control).await.unwrap_err(), Error::Cancelled));
    for (a, b) in [
        (downloadcraft_engine::data_path(&first), downloadcraft_engine::data_path(&dest)),
        (downloadcraft_engine::journal_path(&first), downloadcraft_engine::journal_path(&dest)),
    ] {
        std::fs::rename(a, b).unwrap();
    }
    let out = get(&dl, Job::new(srv.url("/f"), &dest).size_hint(Some(6 * MIB as u64), true)).await.unwrap();
    assert!(out.resumed > 0, "resumed {}", out.resumed);
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
}

#[tokio::test]
async fn a_partial_keyed_by_its_path_resumes_after_a_rekeyed_move() {
    // The app keys a journal by the file's path. The Download window's early
    // start writes under a scratch path; moved to the real name, the journal
    // must be re-keyed, or the bytes are thrown away and it starts over.
    let srv = Server::start(Behaviour { size: 6 * MIB, rate: 1024 * 1024, ..Default::default() }).await;
    let first = tmp("pre-path-start");
    let dest = tmp("pre-path-final");
    let dl = Downloader::new(reqwest::Client::new()).config(quick());
    let control = Arc::new(Control::default());
    let progress = Arc::new(Progress::default());
    let (c, pr) = (control.clone(), progress.clone());
    tokio::spawn(async move {
        while pr.downloaded() < 512 * 1024 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        c.cancel();
    });
    let job = Job::new(srv.url("/f"), &first).size_hint(Some(6 * MIB as u64), true).resume_key(first.to_string_lossy());
    assert!(matches!(dl.download(job, progress.clone(), control).await.unwrap_err(), Error::Cancelled));
    let had = progress.downloaded();
    for (a, b) in [
        (downloadcraft_engine::data_path(&first), downloadcraft_engine::data_path(&dest)),
        (downloadcraft_engine::journal_path(&first), downloadcraft_engine::journal_path(&dest)),
    ] {
        std::fs::rename(a, b).unwrap();
    }
    assert!(downloadcraft_engine::rekey_journal(&downloadcraft_engine::journal_path(&dest), &dest.to_string_lossy()));
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(6 * MIB as u64), true).resume_key(dest.to_string_lossy());
    let out = get(&dl, job).await.unwrap();
    assert!(out.resumed >= had.min(512 * 1024), "resumed {} of {had}", out.resumed);
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
}

#[tokio::test]
async fn a_changed_file_is_not_stitched_onto_old_bytes() {
    let srv = Server::start(Behaviour { size: 4 * MIB, rate: 1024 * 1024, ..Default::default() }).await;
    let dest = tmp("changed");
    let dl = Downloader::new(reqwest::Client::new()).config(quick());
    let control = Arc::new(Control::default());
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(4 * MIB as u64), true);
    let progress = Arc::new(Progress::default());
    let (c, pr) = (control.clone(), progress.clone());
    tokio::spawn(async move {
        while pr.downloaded() < 512 * 1024 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        c.cancel();
    });
    let _ = dl.download(job.clone(), progress, control).await;
    srv.version.store(2, Ordering::SeqCst); // new ETag, new bytes
    let (log, ev) = recorder();
    get(&dl.clone().on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(4 * MIB, 2));
    assert!(log.lock().unwrap().iter().any(|e| e.contains("Restarted")));
}

#[tokio::test]
async fn html_instead_of_the_file_is_refused() {
    let srv = Server::start(Behaviour { size: 1, html: true, ..Default::default() }).await;
    let dest = tmp("html");
    let err = get(&Downloader::new(reqwest::Client::new()).config(quick()), Job::new(srv.url("/f"), &dest))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::HtmlPage), "{err:?}");
}

#[tokio::test]
async fn an_expired_redirect_target_is_renewed_through_the_original_url() {
    let srv = Server::start(Behaviour { size: 6 * MIB, expire_after: 2 * MIB, rate: 4 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("expiring");
    // Small requests: one long request per connection could finish the file
    // without ever asking again, so the expiry would never be met.
    let cfg = Config { max_request: Some(512 * 1024), ..quick() };
    let out = get(&Downloader::new(reqwest::Client::new()).config(cfg), Job::new(srv.url("/start"), &dest))
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
    assert!(srv.signature.load(Ordering::SeqCst) > 1, "the signature was renewed");
    assert!(out.final_url.contains("/signed/"));
}

#[tokio::test]
async fn the_refresh_hook_replaces_a_dead_link() {
    let srv = Server::start(Behaviour { size: 4 * MIB, old_dies_after: MIB, rate: 2 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("refresh");
    let fresh = srv.url("/new");
    let cfg = Config { max_request: Some(512 * 1024), ..quick() };
    let dl = Downloader::new(reqwest::Client::new()).config(cfg).on_link_expired(Arc::new(move |_old| {
        let fresh = fresh.clone();
        Box::pin(async move { Some(fresh) })
    }));
    let job = Job::new(srv.url("/old"), &dest).size_hint(Some(4 * MIB as u64), true);
    let out = get(&dl, job).await.unwrap();
    assert!(out.url_refreshed);
    assert_eq!(std::fs::read(&dest).unwrap(), body(4 * MIB, 1));
}

#[tokio::test]
async fn a_dead_link_without_progress_fails_fast() {
    let srv = Server::start(Behaviour { size: MIB, old_dies_after: 1, ..Default::default() }).await;
    srv.served.store(10, Ordering::SeqCst);
    let dest = tmp("dead");
    let t0 = Instant::now();
    let err = get(&Downloader::new(reqwest::Client::new()).config(quick()), Job::new(srv.url("/old"), &dest))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Http { status: 403, .. }), "{err:?}");
    assert!(t0.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn small_windows_for_strict_cdns() {
    let srv = Server::start(Behaviour { size: 5 * MIB, max_range: MIB, ..Default::default() }).await;
    let dest = tmp("windows");
    let cfg = Config { max_request: Some(MIB as u64), ..quick() };
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(5 * MIB as u64), true);
    get(&Downloader::new(reqwest::Client::new()).config(cfg), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(5 * MIB, 1));
}

#[tokio::test]
async fn connections_spread_over_mirrors_and_bad_ones_are_dropped() {
    // /old dies at once (403); /f and /g serve the file.
    let srv = Server::start(Behaviour { size: 6 * MIB, old_dies_after: 1, rate: 2 * MIB as u64, ..Default::default() }).await;
    srv.served.store(10, Ordering::SeqCst);
    let dest = tmp("mirrors");
    let (log, ev) = recorder();
    let job = Job::new(srv.url("/f"), &dest)
        .size_hint(Some(6 * MIB as u64), true)
        .mirrors(vec![srv.url("/g"), srv.url("/old")]);
    get(&Downloader::new(reqwest::Client::new()).config(quick()).on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
    let reqs = srv.requests.lock().unwrap().clone();
    assert!(reqs.iter().any(|r| r.starts_with("/g ")), "the mirror was used");
    assert!(log.lock().unwrap().iter().any(|e| e.contains("mirror") && e.contains("/old")), "the bad mirror was dropped");
}

#[tokio::test]
async fn a_work_folder_keeps_the_partial_elsewhere() {
    // Slow enough that four connections cannot finish before the cancel at
    // 0.8 s (at 1 MiB/s each they sometimes did, leaving nothing to resume).
    let srv = Server::start(Behaviour { size: 3 * MIB, rate: 512 * 1024, ..Default::default() }).await;
    let dest = tmp("workdir");
    let work = dest.parent().unwrap().join("scratch");
    let dl = Downloader::new(reqwest::Client::new()).config(quick());
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(3 * MIB as u64), true).work_dir(Some(work.clone()));
    let control = Arc::new(Control::default());
    let progress = Arc::new(Progress::default());
    let (c, pr) = (control.clone(), progress.clone());
    // Stop once some bytes have landed (not after a fixed time: on a busy
    // machine that was either before the first byte or after the last).
    tokio::spawn(async move {
        while pr.downloaded() < 256 * 1024 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        c.cancel();
    });
    let _ = dl.download(job.clone(), progress, control).await;
    assert!(downloadcraft_engine::has_partial_in(&dest, Some(&work)));
    assert!(!downloadcraft_engine::has_partial(&dest), "nothing next to the destination");
    let out = get(&dl, job).await.unwrap();
    assert!(out.resumed > 0);
    assert_eq!(std::fs::read(&dest).unwrap(), body(3 * MIB, 1));
    assert!(!downloadcraft_engine::has_partial_in(&dest, Some(&work)));
}

#[tokio::test]
async fn progress_reports_segments_and_connections() {
    let srv = Server::start(Behaviour { size: 4 * MIB, rate: 1024 * 1024, ..Default::default() }).await;
    let dest = tmp("progress");
    let progress = Arc::new(Progress::default());
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(4 * MIB as u64), true);
    let p = progress.clone();
    let watcher = tokio::spawn(async move {
        let mut most = 0;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            most = most.max(p.connections());
        }
        (most, p.segments().len())
    });
    Downloader::new(reqwest::Client::new())
        .config(quick())
        .download(job, progress.clone(), Arc::new(Control::default()))
        .await
        .unwrap();
    let (most, segs) = watcher.await.unwrap();
    assert!(most >= 2 && segs >= 2, "connections {most}, segments {segs}");
    assert_eq!(progress.downloaded(), 4 * MIB as u64);
    assert_eq!(progress.total(), Some(4 * MIB as u64));
}

#[tokio::test]
async fn a_per_ip_connection_limit_is_not_a_dead_link() {
    // Two connections allowed; the others are refused with 403 from the start.
    let srv = Server::start(Behaviour { size: 4 * MIB, refuse_over: 2, rate: 4 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("perip");
    let (log, ev) = recorder();
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(4 * MIB as u64), true);
    get(&Downloader::new(reqwest::Client::new()).config(quick()).on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(4 * MIB, 1));
    assert!(log.lock().unwrap().iter().any(|e| e.contains("ConnectionsReduced")));
}

#[tokio::test]
async fn a_server_that_ignores_extra_connections_is_handled() {
    // Like proof.ovh.net: past 3 connections the server accepts the socket
    // and never answers. Those must not count as failures.
    let srv = Server::start(Behaviour { size: 6 * MIB, hang_over: 3, rate: 2 * MIB as u64, ..Default::default() }).await;
    let dest = tmp("hang");
    let (log, ev) = recorder();
    let cfg = Config { connections: 8, max_retries: 2, ..quick() };
    let job = Job::new(srv.url("/f"), &dest).size_hint(Some(6 * MIB as u64), true);
    get(&Downloader::new(reqwest::Client::new()).config(cfg).on_event(ev), job).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body(6 * MIB, 1));
    assert!(log.lock().unwrap().iter().any(|e| e.contains("ConnectionsReduced")));
}
