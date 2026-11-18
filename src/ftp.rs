//! FTP backend (P3): `ftp://[user:pass@]host[:port]/path` downloads
//! with PASV binary transfers and REST resume, bound to the same `.dcpart`
//! + `.dcmeta` resume contract as HTTP (`RESUME` offsets from another URL
//! are discarded, never resumed into this file).
//!
//! Clean-room RFC 959 subset: control connection
//! (USER/PASS/TYPE I/SIZE/PASV/REST/RETR/QUIT) + one PASV data connection
//! per transfer. FTPS (RFC 4217) wraps both in TLS: `ftpes://` (and
//! `ftps://` on a port other than 990) is explicit (`AUTH TLS` on the plain
//! port), `ftps://` on 990 or with no port is implicit. Both protect the data
//! connection too (`PBSZ 0`, `PROT P`); there is never a silent fallback to
//! plain text. SFTP is a different protocol and not supported.
//!
//! Only `download_ftp()` is public; the tiny line protocol + fixture
//! server live here for unit tests.

use crate::engine::{io_message, mutex_guard, resume_offset, EngineOpts, RateLimiter, SharedState};
use anyhow::{anyhow, Result};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt};

/// Control-connection halves and a data connection, plain or TLS alike.
type CtlR = tokio::io::BufReader<Box<dyn AsyncRead + Unpin + Send>>;
type CtlW = Box<dyn AsyncWrite + Unpin + Send>;
type DataStream = Box<dyn AsyncRead + Unpin + Send>;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tls {
    None,
    /// `AUTH TLS` on the plain control connection.
    Explicit,
    /// TLS from the first byte (port 990).
    Implicit,
}

struct FtpTarget {
    host: String,
    port: u16,
    user: String,
    pass: String,
    path: String,
    tls: Tls,
}

fn parse_target(url: &str) -> Result<FtpTarget> {
    let u = url::Url::parse(url).map_err(|_| anyhow!("bad ftp URL: {url}"))?;
    let tls = match u.scheme() {
        "ftp" => Tls::None,
        "ftpes" => Tls::Explicit,
        "ftps" if matches!(u.port(), None | Some(990)) => Tls::Implicit,
        "ftps" => Tls::Explicit,
        _ => return Err(anyhow!("not an ftp URL: {url}")),
    };
    let host = u.host_str().unwrap_or("").to_string();
    if host.is_empty() {
        return Err(anyhow!("ftp URL has no host: {url}"));
    }
    // The URL parser hands back percent-encoded text; the server wants the
    // real names (`ftp://h/My%20File.zip` is the file "My File.zip").
    Ok(FtpTarget {
        host,
        port: u.port().unwrap_or(if tls == Tls::Implicit { 990 } else { 21 }),
        user: if u.username().is_empty() {
            "anonymous".to_string()
        } else {
            percent_decode(u.username())
        },
        pass: u.password().map(percent_decode).unwrap_or_else(|| "anonymous@".to_string()),
        path: if u.path().is_empty() { "/".to_string() } else { percent_decode(u.path()) },
        tls,
    })
}

/// One TLS setup for every FTPS connection: the shared session cache lets a
/// data connection resume the control connection's session, which servers
/// such as vsftpd (`require_ssl_reuse`) insist on.
fn tls_connector() -> tokio_rustls::TlsConnector {
    static CFG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    let cfg = CFG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(c);
        }
        #[cfg(test)]
        if let Some(extra) = tests::TEST_ROOT.get() {
            let _ = roots.add(extra.clone());
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        Arc::new(
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    });
    tokio_rustls::TlsConnector::from(cfg.clone())
}

async fn tls_wrap(t: &FtpTarget, tcp: tokio::net::TcpStream) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let name = rustls::pki_types::ServerName::try_from(t.host.clone())
        .map_err(|_| anyhow!("FTPS: {} is not a valid server name", t.host))?;
    tls_connector().connect(name, tcp).await.map_err(|e| {
        let m = e.to_string();
        if m.contains("certificate") || m.contains("UnknownIssuer") {
            anyhow!("FTPS: the server's certificate is not trusted ({m})")
        } else {
            anyhow!("FTPS: TLS handshake failed ({m})")
        }
    })
}

fn split_boxed<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(s: S) -> (CtlR, CtlW) {
    let (rd, wr) = tokio::io::split(s);
    (tokio::io::BufReader::new(Box::new(rd) as Box<dyn AsyncRead + Unpin + Send>), Box::new(wr))
}

/// Connect the control connection, TLS first when the URL asks for it, and
/// read the greeting.
async fn connect_control(t: &FtpTarget, connect_to: u64) -> Result<(CtlR, CtlW)> {
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(connect_to), ftp_tcp(&t.host, t.port))
        .await
        .map_err(|_| anyhow!("ftp connect timed out ({connect_to}s) — resuming"))?
        .map_err(|e| anyhow!("ftp connect: {e}"))?;
    let _ = tcp.set_nodelay(true);
    match t.tls {
        Tls::None => {
            let (mut r, w) = split_boxed(tcp);
            greeting(&mut r).await?;
            Ok((r, w))
        }
        Tls::Implicit => {
            let (mut r, w) = split_boxed(tls_wrap(t, tcp).await?);
            greeting(&mut r).await?;
            Ok((r, w))
        }
        Tls::Explicit => {
            // The greeting and AUTH happen in the clear; the server sends
            // nothing after 234 until the handshake, so the buffer is empty.
            let mut br = tokio::io::BufReader::new(tcp);
            greeting(&mut br).await?;
            br.get_mut().write_all(b"AUTH TLS\r\n").await.map_err(|e| anyhow!(io_message(&e)))?;
            let (code, text) = read_reply(&mut br).await?;
            if code != 234 {
                return Err(anyhow!("FTPS: the server does not offer TLS ({code} {text})"));
            }
            Ok(split_boxed(tls_wrap(t, br.into_inner()).await?))
        }
    }
}

async fn greeting<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<()> {
    let (code, text) = read_reply(r).await?;
    if !(220..300).contains(&code) {
        return Err(anyhow!("ftp server refused ({code} {text})"));
    }
    Ok(())
}

/// How the data connection is made: passive (we connect to the server, as
/// PASV names it) or active (the server connects to us after PORT).
enum DataPlan {
    Passive(tokio::net::TcpStream),
    Active(tokio::net::TcpListener),
}

impl DataPlan {
    /// The data connection, once RETR was accepted.
    async fn connected(self) -> Result<tokio::net::TcpStream> {
        match self {
            DataPlan::Passive(s) => Ok(s),
            DataPlan::Active(l) => {
                let (s, _) = tokio::time::timeout(std::time::Duration::from_secs(30), l.accept())
                    .await
                    .map_err(|_| anyhow!("ftp server never connected back (active mode) — turn PASV mode on in Options > Proxy / Socks"))?
                    .map_err(|e| anyhow!("ftp data accept: {e}"))?;
                let _ = s.set_nodelay(true);
                Ok(s)
            }
        }
    }
}

/// Options -> Proxy / Socks: "Use FTP in PASV mode" (on by default). Off,
/// the server connects back to us (PORT); some old servers only do that.
async fn prepare_data(t: &FtpTarget, w: &mut CtlW, r: &mut CtlR) -> Result<DataPlan> {
    // Through a proxy only passive mode works (the server cannot reach us).
    if crate::model::ui_pref_bool("ftp_pasv", true) || ftp_proxy().is_some() {
        let (code, text) = cmd(w, r, "PASV").await?;
        if !(200..300).contains(&code) {
            return Err(anyhow!("ftp PASV refused ({code} {text})"));
        }
        return Ok(DataPlan::Passive(open_data(t, &text).await?));
    }
    // Our address as the server sees it on this network (the route to it).
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| anyhow!("ftp active mode: {e}"))?;
    probe.connect((t.host.as_str(), t.port)).map_err(|e| anyhow!("ftp active mode: {e}"))?;
    let ip = match probe.local_addr().map_err(|e| anyhow!("ftp active mode: {e}"))?.ip() {
        std::net::IpAddr::V4(a) => a,
        std::net::IpAddr::V6(_) => return Err(anyhow!("ftp active mode needs IPv4 — turn PASV mode on")),
    };
    let listener = tokio::net::TcpListener::bind((ip, 0)).await.map_err(|e| anyhow!("ftp active mode: {e}"))?;
    let port = listener.local_addr().map_err(|e| anyhow!("ftp active mode: {e}"))?.port();
    let o = ip.octets();
    let (code, text) = cmd(w, r, &format!("PORT {},{},{},{},{},{}", o[0], o[1], o[2], o[3], port >> 8, port & 0xff)).await?;
    if !(200..300).contains(&code) {
        return Err(anyhow!("ftp PORT refused ({code} {text}) — turn PASV mode on in Options > Proxy / Socks"));
    }
    Ok(DataPlan::Active(listener))
}

/// The data connection for a PASV reply. Some servers behind NAT announce a
/// private or empty address: then the control connection's host is used.
async fn open_data(t: &FtpTarget, pasv_text: &str) -> Result<tokio::net::TcpStream> {
    let (ip, port) = parse_pasv(pasv_text)?;
    let host = match ip.parse::<std::net::Ipv4Addr>() {
        Ok(a) if a.is_unspecified() || (a.is_private() && !host_is_private(&t.host)) => t.host.clone(),
        _ => ip,
    };
    let s = ftp_tcp(&host, port).await.map_err(|e| anyhow!("ftp data connect: {e}"))?;
    let _ = s.set_nodelay(true);
    Ok(s)
}

/// The proxy FTP goes through: Options -> Proxy / Socks with "ftp" ticked
/// under the manual proxy. (kind, host, port, user, password)
fn ftp_proxy() -> Option<(String, String, u16, String, String)> {
    use crate::model::{ui_pref_bool, ui_pref_str};
    if !ui_pref_bool("proxy_ftp", false) || ui_pref_str("proxy_mode", "system") != "manual" {
        return None;
    }
    let host = ui_pref_str("proxy_host", "").trim().to_string();
    let port = ui_pref_str("proxy_port", "").trim().parse::<u16>().ok()?;
    if host.is_empty() {
        return None;
    }
    Some((ui_pref_str("proxy_kind", "http"), host, port, ui_pref_str("proxy_user", ""), ui_pref_str("proxy_pass", "")))
}

/// A TCP connection to (host, port), through the FTP proxy when one is set:
/// SOCKS 4/5 directly, an HTTP(S) proxy by a CONNECT tunnel.
async fn ftp_tcp(host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    let Some((kind, ph, pp, user, pass)) = ftp_proxy() else {
        return tokio::net::TcpStream::connect((host, port)).await;
    };
    let other = |e: String| std::io::Error::other(e);
    if kind.starts_with("socks4") {
        let s = tokio_socks::tcp::Socks4Stream::connect((ph.as_str(), pp), (host, port)).await.map_err(|e| other(format!("SOCKS 4 proxy: {e}")))?;
        return Ok(s.into_inner());
    }
    if kind.starts_with("socks") {
        let s = if user.is_empty() {
            tokio_socks::tcp::Socks5Stream::connect((ph.as_str(), pp), (host, port)).await
        } else {
            tokio_socks::tcp::Socks5Stream::connect_with_password((ph.as_str(), pp), (host, port), &user, &pass).await
        }
        .map_err(|e| other(format!("SOCKS 5 proxy: {e}")))?;
        return Ok(s.into_inner());
    }
    // An HTTP proxy: ask it for a tunnel to the FTP server.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect((ph.as_str(), pp)).await?;
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if !user.is_empty() {
        let token = crate::model::base64_encode(format!("{user}:{pass}").as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
        if s.read(&mut b).await? == 0 {
            break;
        }
        head.push(b[0]);
    }
    let line = String::from_utf8_lossy(&head);
    let ok = line.split_whitespace().nth(1).is_some_and(|c| c.starts_with('2'));
    if !ok {
        return Err(other(format!("the proxy refused a tunnel to {host}:{port} ({})", line.lines().next().unwrap_or("").trim())));
    }
    Ok(s)
}

/// Many FTPS servers close the data connection without TLS's close_notify;
/// that is the end of the file, not an error (the size check still catches
/// a short file).
fn data_eof(e: std::io::Error) -> std::io::Result<usize> {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        Ok(0)
    } else {
        Err(e)
    }
}

fn host_is_private(host: &str) -> bool {
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(a) => a.is_private() || a.is_loopback(),
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// After the server accepted RETR: TLS on the data connection for FTPS.
async fn start_data(t: &FtpTarget, tcp: tokio::net::TcpStream) -> Result<DataStream> {
    Ok(match t.tls {
        Tls::None => Box::new(tcp),
        _ => Box::new(tls_wrap(t, tcp).await?),
    })
}

/// `%XX` escapes only (a URL path keeps a literal `+`, unlike form data).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read one reply (single- or multi-line `code-…code …`) from the control
/// connection. Returns (code, full text).
async fn read_reply<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<(u32, String)> {
    let mut text = String::new();
    let mut code = 0u32;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.map_err(|e| anyhow!("ftp control: {e}"))? == 0 {
            return Err(anyhow!("ftp server closed the connection ({text})"));
        }
        text.push_str(line.trim_end());
        text.push(' ');
        if line.len() >= 4 {
            if let Ok(c) = line[..3].parse::<u32>() {
                code = code.max(c);
                if line.as_bytes().get(3) == Some(&b' ') {
                    break;
                }
            }
        }
        if text.len() > 16 * 1024 {
            return Err(anyhow!("ftp server sent an overlong reply"));
        }
    }
    Ok((code, text.trim().to_string()))
}

async fn cmd<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin>(w: &mut W, r: &mut R, line: &str) -> Result<(u32, String)> {
    w.write_all(line.as_bytes()).await.map_err(|e| anyhow!(io_message(&e)))?;
    w.write_all(b"\r\n").await.map_err(|e| anyhow!(io_message(&e)))?;
    w.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    read_reply(r).await
}

fn parse_pasv(text: &str) -> Result<(String, u16)> {
    let s = text.find('(').and_then(|a| text[a + 1..].find(')').map(|b| text[a + 1..a + 1 + b].to_string()));
    let s = s.ok_or_else(|| anyhow!("ftp server sent an unparseable PASV reply"))?;
    let nums: Vec<u16> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if nums.len() != 6 {
        return Err(anyhow!("ftp server sent an unparseable PASV reply"));
    }
    Ok((
        format!("{}.{}.{}.{}", nums[0], nums[1], nums[2], nums[3]),
        nums[4] * 256 + nums[5],
    ))
}

/// Download `url` to `dest` over plain FTP with REST resume. `limiter` is
/// the shared token bucket (global speed cap honored like HTTP legs).
/// Download `url` over FTP: several parts at once when the server tells the
/// size (and allows the logins), otherwise one connection with REST resume.
pub async fn download_ftp(
    url: &str,
    dest: &std::path::PathBuf,
    opts: &EngineOpts,
    shared: Arc<SharedState>,
    limiter: Arc<RateLimiter>,
    connections: usize,
) -> Result<()> {
    // A single-connection partial from an earlier run continues as it was.
    if connections > 1 && !dest.with_extension("dcpart").exists() {
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        match try_segmented_ftp(url, dest, connections, opts, &shared, &limiter).await {
            Ok(true) => {
                if !opts.expected_sha256.trim().is_empty() {
                    if let Err(e) = crate::engine::verify_checksum(dest, &opts.expected_sha256).await {
                        let _ = tokio::fs::remove_file(dest).await;
                        return Err(e);
                    }
                }
                return Ok(());
            }
            Ok(false) => {}
            Err(e) if shared.cancelled.load(Ordering::SeqCst) || e.to_string() == "cancelled" => return Err(e),
            // The server would not take several logins (or a part kept
            // failing): one connection, from the start.
            Err(_) => {
                let _ = tokio::fs::remove_file(dest.with_extension("dcftp")).await;
                let _ = tokio::fs::remove_file(dest.with_extension("dcftpparts")).await;
                shared.downloaded.store(0, Ordering::SeqCst);
            }
        }
    }
    download_ftp_single(url, dest, opts, shared, limiter).await
}

async fn download_ftp_single(
    url: &str,
    dest: &std::path::PathBuf,
    opts: &EngineOpts,
    shared: Arc<SharedState>,
    limiter: Arc<RateLimiter>,
) -> Result<()> {
    let t = parse_target(url)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let connect_to = opts.timeout_secs.max(5).min(120);
    let stall_secs = opts.timeout_secs.max(15).min(120);
    let max_retries = opts.max_retries.max(1);

    // Control connection with a connect timeout (a blackhole host must not
    // hang the queue forever), login, binary mode.
    let (mut r, mut w) = ftp_login(&t, connect_to).await?;
    // SIZE is optional (some servers hide it): unknown total still streams.
    let mut total: Option<u64> = None;
    let (code, text) = cmd(&mut w, &mut r, &format!("SIZE {}", t.path)).await?;
    if code == 213 {
        total = text.split_whitespace().last().and_then(|s| s.parse().ok());
    }
    if let Some(tot) = total {
        shared.total.store(tot, Ordering::SeqCst);
        shared.has_total.store(true, Ordering::SeqCst);
    }
    // Bind resume state BEFORE touching bytes (P0 contract shared w/ HTTP).
    let mut start_at = resume_offset(dest, "dcpart", url, total, 1);
    {
        let mut totals = mutex_guard(&shared.part_total);
        let mut dones = mutex_guard(&shared.part_done);
        totals.clear();
        dones.clear();
        totals.push((0, total.unwrap_or(0)));
        dones.push(start_at);
    }
    shared.downloaded.store(start_at, Ordering::SeqCst);

    let tmp = dest.with_extension("dcpart");
    let mut stalled = 0u32;
    // The "REST rejected → restart from zero" fallback runs once per job.
    let mut restarted_from_zero = false;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if shared.cancelled.load(Ordering::SeqCst) {
            let _ = cmd(&mut w, &mut r, "QUIT").await;
            return Err(anyhow!("cancelled"));
        }
        crate::engine::wait_if_paused(&shared).await?;
        let len_before = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
        match transfer_once(&t, &mut w, &mut r, &tmp, len_before, stall_secs, &shared, &limiter).await {
            Ok(_) => {
                // Clean end-of-transfer: accept only when SIZE is met (or
                // unknown). A clean-but-short close resumes like a cut.
                let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if let Some(tot) = total {
                    if tot > 0 && got < tot {
                        if got > len_before {
                            stalled = 0;
                        } else {
                            stalled = stalled.saturating_add(1);
                        }
                        if stalled >= max_retries || attempt >= max_retries * 20 {
                            let _ = cmd(&mut w, &mut r, "QUIT").await;
                            return Err(anyhow!(
                                "connection ended early (got {got} of {tot} bytes) — resuming [{attempt} tries]"
                            ));
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
                        continue;
                    }
                }
                break;
            }
            Err(e) => {
                if shared.cancelled.load(Ordering::SeqCst) || e.to_string() == "cancelled" {
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(e);
                }
                let msg = e.to_string();
                if msg.contains("REST rejected") {
                    // Server forgot our offset (or never resumes): one clean
                    // restart from zero, never a second.
                    // `start_at || len_before` alone let every later cut
                    // restart again (the restart writes bytes): a server that
                    // never resumes looped forever instead of failing.
                    if (start_at > 0 || len_before > 0) && !restarted_from_zero {
                        restarted_from_zero = true;
                        let _ = tokio::fs::remove_file(&tmp).await;
                        shared.downloaded.store(0, Ordering::SeqCst);
                        if let Ok(mut g) = shared.part_done.lock() {
                            if let Some(slot) = g.first_mut() {
                                *slot = 0;
                            }
                        }
                        start_at = 0;
                        stalled = 0;
                        // Rebind the sidecar to the fresh start.
                        crate::engine::resume_offset(dest, "dcpart", url, total, 1);
                        continue;
                    }
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(e);
                }
                let len_after = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
                if len_after > len_before {
                    stalled = 0;
                } else {
                    stalled = stalled.saturating_add(1);
                }
                if stalled >= max_retries || attempt >= max_retries * 20 {
                    let _ = cmd(&mut w, &mut r, "QUIT").await;
                    return Err(anyhow!("{e:#} [{attempt} tries]"));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
            }
        }
        // Completion check against SIZE when the server gave one.
        if let Some(tot) = total {
            let got = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
            if tot > 0 && got >= tot {
                break;
            }
            if tot == 0 {
                break;
            }
        }
    }
    let _ = cmd(&mut w, &mut r, "QUIT").await;
    tokio::fs::rename(&tmp, dest).await.map_err(|e| anyhow!(io_message(&e)))?;
    if !opts.expected_sha256.trim().is_empty() {
        if let Err(e) = crate::engine::verify_checksum(dest, &opts.expected_sha256).await {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(e);
        }
    }
    Ok(())
}

/// One PASV+REST+RETR round ending in a clean transfer reply.
/// Mid-stream cuts surface as errors so the retry loop resumes them.
async fn transfer_once(
    t: &FtpTarget,
    w: &mut CtlW,
    r: &mut CtlR,
    tmp: &std::path::PathBuf,
    offset: u64,
    stall_secs: u64,
    shared: &Arc<SharedState>,
    limiter: &Arc<RateLimiter>,
) -> Result<bool> {
    // Open the data connection as soon as PASV names it, before RETR: some
    // servers (vsftpd, ProFTPD) wait for it before sending "150", while we
    // used to wait for "150" before connecting, so both sides waited.
    let plan = prepare_data(t, w, r).await?;
    if offset > 0 {
        let (code, text) = cmd(w, r, &format!("REST {offset}")).await?;
        if !(300..400).contains(&code) {
            return Err(anyhow!("REST rejected ({code} {text}) — resuming from zero"));
        }
    }
    let (code, text) = cmd(w, r, &format!("RETR {}", t.path)).await?;
    if !(100..200).contains(&code) {
        return Err(anyhow!("ftp retrieve refused ({code} {text})"));
    }
    let data_tcp = plan.connected().await?;
    let mut data = start_data(t, data_tcp).await?;
    #[allow(clippy::suspicious_open_options)]
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tmp)
        .await
        .map_err(|e| anyhow!(io_message(&e)))?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 32 * 1024];
    loop {
        crate::engine::wait_if_paused(shared).await?;
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        let n = tokio::time::timeout(std::time::Duration::from_secs(stall_secs), data.read(&mut buf))
            .await
            .map_err(|_| anyhow!("stalled (no data for {stall_secs}s) — resuming"))?
            .or_else(data_eof)
            .map_err(|e| anyhow!("ftp data: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).await.map_err(|e| anyhow!(io_message(&e)))?;
        shared.downloaded.fetch_add(n as u64, Ordering::SeqCst);
        if let Ok(mut g) = shared.part_done.lock() {
            if let Some(slot) = g.first_mut() {
                *slot = slot.saturating_add(n as u64);
            }
        }
        limiter.consume(n as u64).await;
    }
    file.flush().await.map_err(|e| anyhow!(io_message(&e)))?;
    drop(file);
    drop(data);
    // End-of-transfer reply (226/250/2xx), guarded so a silent server
    // can't hang the queue. A 426/550 here means the bytes above are
    // suspect — surface it instead of finalizing.
    let (code, text) = tokio::time::timeout(std::time::Duration::from_secs(stall_secs), read_reply(r))
        .await
        .map_err(|_| anyhow!("stalled (no end-of-transfer reply for {stall_secs}s) — resuming"))??;
    if (200..300).contains(&code) {
        return Ok(true);
    }
    Err(anyhow!("ftp transfer aborted ({code} {text}) — resuming"))
}

// ---- segmented FTP (IDM's FTPSession: several logins, one part each) --------

/// Log in on a fresh control connection and switch to binary mode.
async fn ftp_login(t: &FtpTarget, connect_to: u64) -> Result<(CtlR, CtlW)> {
    let (mut r, mut w) = connect_control(t, connect_to).await?;
    let (code, _) = cmd(&mut w, &mut r, &format!("USER {}", t.user)).await?;
    if code == 331 || code == 332 {
        let (code, text) = cmd(&mut w, &mut r, &format!("PASS {}", t.pass)).await?;
        if !(200..300).contains(&code) {
            let _ = cmd(&mut w, &mut r, "QUIT").await;
            return Err(anyhow!("ftp login refused ({code} {text}) — check the username/password in the Add dialog"));
        }
    } else if !(200..300).contains(&code) {
        return Err(anyhow!("ftp login refused ({code}) — check the username/password"));
    }
    if t.tls != Tls::None {
        // Protect the data connections too (RFC 4217).
        let _ = cmd(&mut w, &mut r, "PBSZ 0").await?;
        let (code, text) = cmd(&mut w, &mut r, "PROT P").await?;
        if !(200..300).contains(&code) {
            return Err(anyhow!("FTPS: the server refused encrypted data connections ({code} {text})"));
        }
    }
    let (code, text) = cmd(&mut w, &mut r, "TYPE I").await?;
    if !(200..300).contains(&code) {
        return Err(anyhow!("ftp server refused binary mode ({code} {text})"));
    }
    Ok((r, w))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FtpParts {
    url: String,
    total: u64,
    /// [start, end (inclusive), done]
    parts: Vec<(u64, u64, u64)>,
}

/// One part: its own login, REST to where it stands, RETR, read exactly its
/// range, write it at its offset. Retries on its own; fails only after
/// `max_retries` attempts in a row without progress.
#[allow(clippy::too_many_arguments)]
async fn ftp_part(
    t: Arc<FtpTarget>,
    data: std::path::PathBuf,
    idx: usize,
    start: u64,
    end: u64,
    done: Arc<std::sync::atomic::AtomicU64>,
    opts: EngineOpts,
    shared: Arc<SharedState>,
    limiter: Arc<RateLimiter>,
) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let len = end - start + 1;
    let connect_to = opts.timeout_secs.clamp(5, 120);
    let stall_secs = opts.timeout_secs.clamp(15, 120);
    let mut stalled = 0u32;
    while done.load(Ordering::SeqCst) < len {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        crate::engine::wait_if_paused(&shared).await?;
        let before = done.load(Ordering::SeqCst);
        let attempt: Result<()> = async {
            let (mut r, mut w) = ftp_login(&t, connect_to).await?;
            let plan = prepare_data(&t, &mut w, &mut r).await?;
            let at = start + done.load(Ordering::SeqCst);
            if at > 0 {
                let (code, text) = cmd(&mut w, &mut r, &format!("REST {at}")).await?;
                if !(300..400).contains(&code) {
                    return Err(anyhow!("REST rejected ({code} {text})"));
                }
            }
            let (code, text) = cmd(&mut w, &mut r, &format!("RETR {}", t.path)).await?;
            if !(100..200).contains(&code) {
                return Err(anyhow!("ftp retrieve refused ({code} {text})"));
            }
            let data_tcp = plan.connected().await?;
            let mut sock = start_data(&t, data_tcp).await?;
            let mut file = tokio::fs::OpenOptions::new().write(true).open(&data).await.map_err(|e| anyhow!(io_message(&e)))?;
            file.seek(std::io::SeekFrom::Start(at)).await?;
            let mut buf = vec![0u8; 64 * 1024];
            while done.load(Ordering::SeqCst) < len {
                crate::engine::wait_if_paused(&shared).await?;
                let want = ((len - done.load(Ordering::SeqCst)) as usize).min(buf.len());
                let n = tokio::time::timeout(std::time::Duration::from_secs(stall_secs), sock.read(&mut buf[..want]))
                    .await
                    .map_err(|_| anyhow!("stalled (no data for {stall_secs}s)"))?
                    .or_else(data_eof)
            .map_err(|e| anyhow!("ftp data: {e}"))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n]).await.map_err(|e| anyhow!(io_message(&e)))?;
                done.fetch_add(n as u64, Ordering::SeqCst);
                shared.downloaded.fetch_add(n as u64, Ordering::SeqCst);
                if let Ok(mut g) = shared.part_done.lock() {
                    if let Some(slot) = g.get_mut(idx) {
                        *slot = done.load(Ordering::SeqCst);
                    }
                }
                limiter.consume(n as u64).await;
            }
            file.flush().await?;
            // The part is complete (or the server stopped): close the data
            // connection early and leave; the server's 426 is expected.
            drop(sock);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), cmd(&mut w, &mut r, "QUIT")).await;
            Ok(())
        }
        .await;
        let after = done.load(Ordering::SeqCst);
        if after >= len {
            return Ok(());
        }
        if after > before {
            stalled = 0;
        } else {
            stalled += 1;
            if stalled >= opts.max_retries.max(1) {
                return Err(attempt.err().unwrap_or_else(|| anyhow!("ftp part {idx} made no progress")));
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1 << stalled.min(4))).await;
    }
    Ok(())
}

/// Several parts at once when the server tells the size and it is worth it.
/// `Ok(false)` = not applicable (no size, small file): use one connection.
async fn try_segmented_ftp(
    url: &str,
    dest: &std::path::PathBuf,
    connections: usize,
    opts: &EngineOpts,
    shared: &Arc<SharedState>,
    limiter: &Arc<RateLimiter>,
) -> Result<bool> {
    let t = Arc::new(parse_target(url)?);
    let (mut r, mut w) = ftp_login(&t, opts.timeout_secs.clamp(5, 120)).await?;
    let (code, text) = cmd(&mut w, &mut r, &format!("SIZE {}", t.path)).await?;
    let _ = cmd(&mut w, &mut r, "QUIT").await;
    let total: u64 = if code == 213 { text.split_whitespace().last().and_then(|s| s.parse().ok()).unwrap_or(0) } else { 0 };
    const MIN_PART: u64 = 2 * 1024 * 1024;
    if total < 2 * MIN_PART {
        return Ok(false);
    }
    let n = (connections.min(8) as u64).min(total / MIN_PART).max(2);
    let data = dest.with_extension("dcftp");
    let side = dest.with_extension("dcftpparts");
    // Resume a previous segmented run of the same file.
    let previous: Option<FtpParts> = std::fs::read_to_string(&side)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|p: &FtpParts| p.url == url && p.total == total && data.is_file());
    let parts: Vec<(u64, u64, u64)> = match previous {
        Some(p) => p.parts,
        None => {
            let f = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(&data).map_err(|e| anyhow!(io_message(&e)))?;
            f.set_len(total).map_err(|e| anyhow!(io_message(&e)))?;
            let chunk = total / n;
            (0..n).map(|i| (i * chunk, if i + 1 == n { total - 1 } else { (i + 1) * chunk - 1 }, 0)).collect()
        }
    };
    shared.total.store(total, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    shared.resumable.store(true, Ordering::SeqCst);
    shared.downloaded.store(parts.iter().map(|p| p.2).sum(), Ordering::SeqCst);
    *mutex_guard(&shared.part_total) = parts.iter().map(|p| (p.0, p.1)).collect();
    *mutex_guard(&shared.part_done) = parts.iter().map(|p| p.2).collect();
    let dones: Vec<Arc<std::sync::atomic::AtomicU64>> = parts.iter().map(|p| Arc::new(std::sync::atomic::AtomicU64::new(p.2))).collect();
    let save = {
        let (side, url, dones, parts) = (side.clone(), url.to_string(), dones.clone(), parts.clone());
        move || {
            let p = FtpParts { url: url.clone(), total, parts: parts.iter().zip(&dones).map(|(p, d)| (p.0, p.1, d.load(Ordering::SeqCst))).collect() };
            if let Ok(j) = serde_json::to_string(&p) {
                let _ = std::fs::write(&side, j);
            }
        }
    };
    let mut set = tokio::task::JoinSet::new();
    for (i, p) in parts.iter().enumerate() {
        set.spawn(ftp_part(t.clone(), data.clone(), i, p.0, p.1, dones[i].clone(), opts.clone(), shared.clone(), limiter.clone()));
    }
    let mut failed: Option<anyhow::Error> = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3));
    loop {
        tokio::select! {
            r = set.join_next() => match r {
                None => break,
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) => { failed.get_or_insert(e); }
                Some(Err(e)) => { failed.get_or_insert(anyhow!("ftp part: {e}")); }
            },
            _ = tick.tick() => save(),
        }
    }
    save();
    if let Some(e) = failed {
        return Err(e);
    }
    tokio::fs::rename(&data, dest).await.map_err(|e| anyhow!(io_message(&e)))?;
    let _ = std::fs::remove_file(&side);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncRead, AsyncWrite};
    use super::*;

    #[test]
    fn url_parts_are_percent_decoded() {
        let t = parse_target("ftp://us%40er:p%20w@h.example/My%20File+1.zip").unwrap();
        assert_eq!(t.user, "us@er");
        assert_eq!(t.pass, "p w");
        assert_eq!(t.path, "/My File+1.zip", "a URL path keeps a literal '+'");
        assert_eq!(percent_decode("bad%zz%4"), "bad%zz%4");
    }

    /// The test server's certificate, trusted by `tls_connector` in tests.
    pub(super) static TEST_ROOT: std::sync::OnceLock<rustls::pki_types::CertificateDer<'static>> = std::sync::OnceLock::new();

    /// A TLS server setup for "localhost" whose certificate the client trusts.
    fn test_tls() -> Arc<rustls::ServerConfig> {
        static CFG: std::sync::OnceLock<Arc<rustls::ServerConfig>> = std::sync::OnceLock::new();
        CFG.get_or_init(|| {
            let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
            let cert = ck.cert.der().clone();
            let key = rustls::pki_types::PrivateKeyDer::Pkcs8(ck.key_pair.serialize_der().into());
            let _ = TEST_ROOT.set(cert.clone());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            Arc::new(
                rustls::ServerConfig::builder_with_provider(provider)
                    .with_safe_default_protocol_versions()
                    .unwrap()
                    .with_no_client_auth()
                    .with_single_cert(vec![cert], key)
                    .unwrap(),
            )
        })
        .clone()
    }

    type SrvR = tokio::io::BufReader<Box<dyn AsyncRead + Unpin + Send>>;
    type SrvW = Box<dyn AsyncWrite + Unpin + Send>;

    fn srv_split<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(s: S) -> (SrvR, SrvW) {
        let (rd, wr) = tokio::io::split(s);
        (tokio::io::BufReader::new(Box::new(rd) as Box<dyn AsyncRead + Unpin + Send>), Box::new(wr))
    }

    async fn send(w: &mut SrvW, s: &str) {
        let _ = w.write_all(s.as_bytes()).await;
        let _ = w.flush().await;
    }

    /// Minimal in-process FTP server (localhost only): one canned file,
    /// optional login gate, REST offsets, PASV data sends. With `tls` it is
    /// an FTPS server: explicit (`AUTH TLS`) or, with `implicit`, TLS from the
    /// first byte; `PROT P` encrypts the data connections.
    async fn spawn_ftp(body: Vec<u8>, want_user: Option<(String, String)>) -> std::net::SocketAddr {
        spawn_ftp_tls(body, want_user, None, false).await
    }

    async fn spawn_ftp_tls(
        body: Vec<u8>,
        want_user: Option<(String, String)>,
        tls: Option<Arc<rustls::ServerConfig>>,
        implicit: bool,
    ) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(body);
        tokio::spawn(async move {
            loop {
                let Ok((ctrl, _)) = listener.accept().await else { break };
                let body = body.clone();
                let want = want_user.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    let acceptor = tls.clone().map(tokio_rustls::TlsAcceptor::from);
                    let (mut r, mut w) = match (&acceptor, implicit) {
                        (Some(acc), true) => match acc.accept(ctrl).await {
                            Ok(s) => srv_split(s),
                            Err(_) => return,
                        },
                        _ => srv_split(ctrl),
                    };
                    send(&mut w, "220 dc-test ftp\r\n").await;
                    let pending: Arc<std::sync::Mutex<(Option<tokio::net::TcpListener>, u64)>> =
                        Arc::new(std::sync::Mutex::new((None, 0)));
                    let mut prot_p = false;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                            break;
                        }
                        let t = line.trim().to_string();
                        let (verb, arg) = t.split_once(' ').map(|(a, b)| (a.to_ascii_uppercase(), b.to_string())).unwrap_or((t.to_ascii_uppercase(), String::new()));
                        match verb.as_str() {
                            "AUTH" => {
                                let Some(acc) = acceptor.clone() else {
                                    send(&mut w, "502 no TLS here\r\n").await;
                                    continue;
                                };
                                send(&mut w, "234 go ahead\r\n").await;
                                // Rejoin the halves of the plain socket and
                                // start TLS on it.
                                let rd = r.into_inner();
                                let joined = tokio::io::join(rd, w);
                                let Ok(s) = acc.accept(joined).await else { return };
                                (r, w) = srv_split(s);
                            }
                            "PBSZ" => send(&mut w, "200 pbsz ok\r\n").await,
                            "PROT" => {
                                prot_p = arg.eq_ignore_ascii_case("P");
                                send(&mut w, "200 prot ok\r\n").await;
                            }
                            "USER" => {
                                if let Some((u, _)) = &want {
                                    if arg != *u {
                                        send(&mut w, "530 bad user\r\n").await;
                                        continue;
                                    }
                                }
                                send(&mut w, "331 ok, need pass\r\n").await;
                            }
                            "PASS" => {
                                if let Some((_, p)) = &want {
                                    if arg != *p {
                                        send(&mut w, "530 login incorrect\r\n").await;
                                        continue;
                                    }
                                }
                                send(&mut w, "230 logged in\r\n").await;
                            }
                            "TYPE" => send(&mut w, "200 type set\r\n").await,
                            "SIZE" => {
                                if arg == "/file.bin" {
                                    send(&mut w, &format!("213 {}\r\n", body.len())).await;
                                } else {
                                    send(&mut w, "550 not found\r\n").await;
                                }
                            }
                            "PASV" => {
                                match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                                    Ok(dl) => {
                                        let p = dl.local_addr().unwrap().port();
                                        if let Ok(mut g) = pending.lock() {
                                            g.0 = Some(dl);
                                        }
                                        send(&mut w, &format!("227 Entering Passive Mode (127,0,0,1,{}, {})\r\n", p / 256, p % 256)).await;
                                    }
                                    Err(_) => send(&mut w, "425 no data conn\r\n").await,
                                }
                            }
                            "REST" => {
                                match arg.parse::<u64>() {
                                    Ok(n) => {
                                        if let Ok(mut g) = pending.lock() {
                                            g.1 = n;
                                        }
                                        send(&mut w, "350 restart at offset\r\n").await;
                                    }
                                    Err(_) => send(&mut w, "501 bad offset\r\n").await,
                                }
                            }
                            "RETR" => {
                                if arg != "/file.bin" {
                                    send(&mut w, "550 not found\r\n").await;
                                    continue;
                                }
                                let (dl, off) = match pending.lock().map(|mut g| (g.0.take(), g.1)) {
                                    Ok(v) => v,
                                    Err(_) => (None, 0),
                                };
                                let Some(dl) = dl else {
                                    send(&mut w, "425 use PASV first\r\n").await;
                                    continue;
                                };
                                let body = body.clone();
                                let data_tls = if prot_p { acceptor.clone() } else { None };
                                tokio::spawn(async move {
                                    if let Ok((ds, _)) = dl.accept().await {
                                        let part = &body[off.min(body.len() as u64) as usize..];
                                        match data_tls {
                                            Some(acc) => {
                                                if let Ok(mut s) = acc.accept(ds).await {
                                                    let _ = s.write_all(part).await;
                                                    let _ = s.shutdown().await;
                                                }
                                            }
                                            None => {
                                                let mut ds = ds;
                                                let _ = ds.write_all(part).await;
                                            }
                                        }
                                    }
                                });
                                send(&mut w, "150 opening data\r\n").await;
                                // Give the data connection time to deliver.
                                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                send(&mut w, "226 done\r\n").await;
                            }
                            "QUIT" => {
                                send(&mut w, "221 bye\r\n").await;
                                break;
                            }
                            _ => send(&mut w, "502 nah\r\n").await,
                        }
                    }
                });
            }
        });
        addr
    }

    fn ftp_opts() -> EngineOpts {
        EngineOpts { timeout_secs: 10, max_retries: 3, user_agent: "dc-test".into(), ..Default::default() }
    }

    async fn dl(url: &str, dest: &std::path::PathBuf) -> Result<()> {
        let shared = Arc::new(SharedState::new());
        let lim = Arc::new(RateLimiter::new(0));
        download_ftp(url, dest, &ftp_opts(), shared, lim, 1).await
    }

    #[tokio::test]
    async fn ftp_downloads_in_parts() {
        let body: Vec<u8> = (0..9_000_000u32).map(|i| (i.wrapping_mul(7) % 251) as u8).collect();
        let addr = spawn_ftp(body.clone(), None).await;
        let dir = std::env::temp_dir().join(format!("dc_ftpseg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("seg.bin");
        let shared = Arc::new(SharedState::new());
        download_ftp(&format!("ftp://{addr}/file.bin"), &dest, &ftp_opts(), shared.clone(), Arc::new(RateLimiter::new(0)), 4)
            .await
            .expect("segmented ftp");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(mutex_guard(&shared.part_total).len(), 4, "four parts");
        assert!(!dest.with_extension("dcftpparts").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ftp_full_and_resume() {
        let body: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let addr = spawn_ftp(body.clone(), None).await;
        let dir = std::env::temp_dir().join(format!("dc_ftp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = format!("ftp://{addr}/file.bin");
        // 1. Full download, byte-exact.
        let dest = dir.join("full.bin");
        dl(&url, &dest).await.expect("ftp full");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 2. Resume with a bound prefix continues, not restarts.
        let dest = dir.join("resume.bin");
        std::fs::write(dest.with_extension("dcpart"), &body[..50_000]).unwrap();
        dl(&url, &dest).await.expect("ftp resume");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // 3. Stale prefix bound to another URL is discarded, never resumed
        // (garbage prefix + real suffix would corrupt the file).
        let dest = dir.join("stale.bin");
        std::fs::write(dest.with_extension("dcpart"), vec![0xFFu8; 50_000]).unwrap();
        let foreign = format!("ftp://{addr}/foreign.bin");
        crate::engine::resume_offset(&dest, "dcpart", &foreign, Some(body.len() as u64), 1);
        dl(&url, &dest).await.expect("ftp stale discard");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ftp_login_refused() {
        let body: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        let addr = spawn_ftp(body, Some(("tester".into(), "s3cret".into()))).await;
        let dir = std::env::temp_dir().join(format!("dc_ftpauth_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Wrong password → loud login error, no retry storm, no file.
        let dest = dir.join("auth.bin");
        let err = dl(&format!("ftp://bob:wrong@{addr}/file.bin"), &dest).await.expect_err("bad login");
        assert!(format!("{err:#}").contains("login"), "got: {err:#}");
        assert!(!dest.exists());
        // Right credentials → exact file.
        let dest = dir.join("auth2.bin");
        dl(&format!("ftp://tester:s3cret@{addr}/file.bin"), &dest).await.expect("good login");
        assert_eq!(std::fs::read(&dest).unwrap().len(), 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ftps_urls_pick_the_tls_mode() {
        assert_eq!(parse_target("ftp://h/f").unwrap().tls, Tls::None);
        let t = parse_target("ftps://h/f").unwrap();
        assert_eq!((t.tls, t.port), (Tls::Implicit, 990));
        assert_eq!(parse_target("ftps://h:21/f").unwrap().tls, Tls::Explicit);
        let t = parse_target("ftpes://h/f").unwrap();
        assert_eq!((t.tls, t.port), (Tls::Explicit, 21));
        assert!(parse_target("sftp://h/f").is_err());
    }

    #[tokio::test]
    async fn ftps_explicit_and_implicit_download() {
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let dir = std::env::temp_dir().join(format!("dc_ftps_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Explicit: AUTH TLS on the plain port; resume included.
        let addr = spawn_ftp_tls(body.clone(), None, Some(test_tls()), false).await;
        let url = format!("ftpes://localhost:{}/file.bin", addr.port());
        let dest = dir.join("explicit.bin");
        dl(&url, &dest).await.expect("explicit ftps");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        let dest = dir.join("explicit-resume.bin");
        std::fs::write(dest.with_extension("dcpart"), &body[..70_000]).unwrap();
        dl(&url, &dest).await.expect("explicit ftps resume");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        // Implicit: TLS from the first byte.
        let addr = spawn_ftp_tls(body.clone(), None, Some(test_tls()), true).await;
        // (`ftps://` on a port other than 990 means explicit, so the target
        // is built directly to reach a test port.)
        let t = FtpTarget { host: "localhost".into(), port: addr.port(), user: "anonymous".into(), pass: "x".into(), path: "/file.bin".into(), tls: Tls::Implicit };
        let (mut r, mut w) = ftp_login(&t, 10).await.expect("implicit login");
        // Several encrypted connections at once (segmented FTPS).
        let big: Vec<u8> = (0..9_000_000u32).map(|i| (i.wrapping_mul(13) % 241) as u8).collect();
        let seg = spawn_ftp_tls(big.clone(), None, Some(test_tls()), false).await;
        let dest = dir.join("segmented.bin");
        let shared = Arc::new(SharedState::new());
        download_ftp(&format!("ftpes://localhost:{}/file.bin", seg.port()), &dest, &ftp_opts(), shared.clone(), Arc::new(RateLimiter::new(0)), 3)
            .await
            .expect("segmented ftps");
        assert_eq!(std::fs::read(&dest).unwrap(), big);
        assert_eq!(mutex_guard(&shared.part_total).len(), 3);
        let (code, _) = cmd(&mut w, &mut r, "SIZE /file.bin").await.unwrap();
        assert_eq!(code, 213);
        // A server without TLS is refused, never downgraded.
        let plain = spawn_ftp(body, None).await;
        let err = dl(&format!("ftpes://localhost:{}/file.bin", plain.port()), &dir.join("no.bin")).await.expect_err("no tls");
        assert!(format!("{err:#}").contains("does not offer TLS"), "got {err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
