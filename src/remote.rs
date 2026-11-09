//! Remote control: a small web server on the local network that a phone (or
//! any browser) uses to see the downloads, add links and magnets, and pause,
//! resume or remove them.
//!
//! - Off until it is switched on (Settings → Remote control, or `--server`).
//! - A device pairs once with a 6-digit code shown in Settings (valid for ten
//!   minutes, one use); it then holds a random token, sent as
//!   `Authorization: Bearer …` with every request. Only the token's SHA-256
//!   is stored. Pairing attempts are rate-limited.
//! - The server speaks to the app through the same shared files the browser
//!   connection and the progress windows use (`queue.json`, `wake.json`,
//!   `cmd.json`), so it works the same with the window open, in the tray, or
//!   with no window at all (server mode, `crate::server`).

use crate::model::{config_dir, DownloadItem, Status};
use anyhow::{Context, Result};
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default port.
pub const DEFAULT_PORT: u16 = 8765;
/// How long a pairing code works.
const CODE_SECS: i64 = 600;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Device {
    pub name: String,
    /// SHA-256 of the device's token (hex).
    pub token_sha: String,
    pub added: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct RemoteFile {
    /// The current pairing code and when it stops working (Unix seconds).
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub code_until: i64,
    #[serde(default)]
    pub devices: Vec<Device>,
}

fn file_path() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("remote.json"))
}

pub fn load() -> RemoteFile {
    file_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

fn save(f: &RemoteFile) {
    if let (Some(p), Ok(json)) = (file_path(), serde_json::to_string_pretty(f)) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = crate::model::atomic_write(&p, json.as_bytes());
    }
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

fn sha_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A fresh pairing code (and the old one stops working).
pub fn new_code() -> String {
    use rand::Rng;
    let code = format!("{:06}", rand::rng().random_range(0..1_000_000u32));
    let mut f = load();
    f.code = code.clone();
    f.code_until = now() + CODE_SECS;
    save(&f);
    code
}

/// The pairing code while it still works, and its seconds left.
pub fn current_code() -> Option<(String, i64)> {
    let f = load();
    let left = f.code_until - now();
    (!f.code.is_empty() && left > 0).then_some((f.code, left))
}

/// Forget a paired device (by its token's hash).
pub fn remove_device(token_sha: &str) {
    let mut f = load();
    f.devices.retain(|d| d.token_sha != token_sha);
    save(&f);
}

/// This computer's address on the local network (what a phone types).
pub fn lan_address() -> Option<String> {
    // No packet is sent: connecting a UDP socket only picks the route.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").or_else(|_| s.connect("10.0.0.1:9")).ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

struct Shared {
    /// Failed pairing attempts in the current minute.
    fails: Mutex<(Instant, u32)>,
}

fn authorized(headers: &HeaderMap) -> bool {
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| t.len() >= 32)
    else {
        return false;
    };
    let h = sha_hex(token);
    load().devices.iter().any(|d| d.token_sha == h)
}

fn deny() -> Response {
    (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "not paired" }))).into_response()
}

#[derive(Deserialize)]
struct PairReq {
    code: String,
    #[serde(default)]
    name: String,
}

async fn pair(State(st): State<Arc<Shared>>, Json(req): Json<PairReq>) -> Response {
    {
        let mut f = st.fails.lock().unwrap_or_else(|e| e.into_inner());
        if f.0.elapsed() > Duration::from_secs(60) {
            *f = (Instant::now(), 0);
        }
        if f.1 >= 5 {
            return (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({ "error": "too many tries; wait a minute" })))
                .into_response();
        }
    }
    let mut file = load();
    let ok = !file.code.is_empty() && file.code_until > now() && req.code.trim() == file.code;
    if !ok {
        let mut f = st.fails.lock().unwrap_or_else(|e| e.into_inner());
        f.1 += 1;
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": "wrong or expired code" }))).into_response();
    }
    use rand::Rng;
    let token: String = (0..32).map(|_| format!("{:02x}", rand::rng().random::<u8>())).collect();
    let name: String = req.name.chars().filter(|c| !c.is_control()).take(40).collect();
    file.devices.push(Device {
        name: if name.trim().is_empty() { "Phone".into() } else { name },
        token_sha: sha_hex(&token),
        added: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
    });
    // One use: the code stops working once a device has paired with it.
    file.code.clear();
    file.code_until = 0;
    save(&file);
    Json(serde_json::json!({ "token": token })).into_response()
}

#[derive(Serialize)]
struct Row {
    id: u64,
    name: String,
    status: &'static str,
    error: String,
    done: u64,
    total: Option<u64>,
    progress: f32,
    speed: f64,
    torrent: bool,
}

fn row(d: &DownloadItem) -> Row {
    let (status, error) = match &d.status {
        Status::Queued => ("queued", String::new()),
        Status::Downloading => ("downloading", String::new()),
        Status::Paused => ("paused", String::new()),
        Status::Completed => ("done", String::new()),
        Status::Failed(e) => ("failed", e.chars().take(160).collect()),
    };
    Row {
        id: d.id,
        name: d.filename.clone(),
        status,
        error,
        done: d.downloaded_bytes,
        total: d.total_bytes,
        progress: d.progress(),
        speed: d.speed_bps,
        torrent: crate::torrent::is_torrent(&d.url),
    }
}

async fn list(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return deny();
    }
    let items = config_dir()
        .map(|d| d.join("queue.json"))
        .and_then(|p| crate::model::read_queue_items(&p).ok())
        .unwrap_or_default();
    let mut rows: Vec<Row> = items.iter().map(row).collect();
    rows.reverse(); // newest first
    Json(serde_json::json!({ "app": crate::model::APP_VERSION, "rows": rows })).into_response()
}

#[derive(Deserialize)]
struct AddReq {
    url: String,
}

async fn add(headers: HeaderMap, Json(req): Json<AddReq>) -> Response {
    if !authorized(&headers) {
        return deny();
    }
    let url = req.url.trim().to_string();
    if !crate::model::is_download_url(&url) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "not a download link or magnet" }))).into_response();
    }
    let prefs = crate::confirm::load_prefs();
    let name = crate::model::filename_from_url(&url);
    let mut item = DownloadItem::new(0, url, crate::engine::sanitize_filename(&name), prefs.save_dir.clone(), prefs.connections);
    match crate::model::append_to_queue(&mut item) {
        crate::model::AppendResult::Added(id) | crate::model::AppendResult::AlreadyQueued(id) => {
            crate::confirm::request_start(&[id]);
            Json(serde_json::json!({ "id": id })).into_response()
        }
        crate::model::AppendResult::Error(e) => {
            (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

#[derive(Deserialize)]
struct ControlReq {
    #[serde(default)]
    id: Option<u64>,
    action: String,
}

async fn control(headers: HeaderMap, Json(req): Json<ControlReq>) -> Response {
    if !authorized(&headers) {
        return deny();
    }
    let r = match req.action.as_str() {
        "start" => match req.id {
            Some(id) => {
                crate::confirm::request_start(&[id]);
                Ok(())
            }
            None => Err("no id".to_string()),
        },
        a @ ("pause" | "resume" | "cancel" | "remove" | "pause_all" | "resume_all") => {
            crate::native_host::app_control(req.id, a)
        }
        _ => Err("unknown action".to_string()),
    };
    match r {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))).into_response(),
    }
}

async fn page() -> Html<&'static str> {
    Html(include_str!("../assets/remote.html"))
}

/// Serve until `stop` turns true. Listens on every address of this computer
/// (the phone is another device on the network).
pub async fn serve(port: u16, mut stop: tokio::sync::watch::Receiver<bool>) -> Result<()> {
    let app = Router::new()
        .route("/", get(page))
        .route("/api/pair", post(pair))
        .route("/api/list", get(list))
        .route("/api/add", post(add))
        .route("/api/control", post(control))
        .with_state(Arc::new(Shared { fails: Mutex::new((Instant::now(), 0)) }));
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .with_context(|| format!("port {port} is in use or blocked"))?;
    if https_on() {
        return serve_tls(listener, app, stop).await;
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            while !*stop.borrow() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .context("remote control server stopped")
}

/// Settings → Remote control → "Use HTTPS" (or `DOWNLOAD_CRAFT_HTTPS=1`).
pub fn https_on() -> bool {
    crate::model::ui_pref_bool("remote_https", false) || std::env::var("DOWNLOAD_CRAFT_HTTPS").is_ok_and(|v| v == "1")
}

/// This computer's certificate for the page (made once, then kept, so the
/// phone sees the same one): (certificate DER, private key DER).
fn certificate() -> Result<(Vec<u8>, Vec<u8>)> {
    let dir = config_dir().context("no settings folder")?;
    let (cp, kp) = (dir.join("remote-cert.der"), dir.join("remote-key.der"));
    if let (Ok(c), Ok(k)) = (std::fs::read(&cp), std::fs::read(&kp)) {
        return Ok((c, k));
    }
    let mut names = vec!["localhost".to_string(), "download-craft".to_string()];
    if let Some(ip) = lan_address() {
        names.push(ip);
    }
    let ck = rcgen::generate_simple_self_signed(names).context("could not make a certificate")?;
    let (c, k) = (ck.cert.der().to_vec(), ck.key_pair.serialize_der());
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(&cp, &c)?;
    std::fs::write(&kp, &k)?;
    Ok((c, k))
}

/// The certificate's SHA-256 fingerprint, as the phone's browser shows it.
pub fn fingerprint() -> Option<String> {
    let (c, _) = certificate().ok()?;
    let h = Sha256::digest(&c);
    Some(h.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
}

async fn serve_tls(listener: tokio::net::TcpListener, app: Router, mut stop: tokio::sync::watch::Receiver<bool>) -> Result<()> {
    use tokio_rustls::rustls;
    let (cert, key) = certificate()?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS")?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(cert)],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(key)),
        )
        .context("the certificate")?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    loop {
        let (tcp, _) = tokio::select! {
            r = listener.accept() => match r { Ok(x) => x, Err(_) => continue },
            _ = stop.changed() => return Ok(()),
        };
        if *stop.borrow() {
            return Ok(());
        }
        let (acceptor, app) = (acceptor.clone(), app.clone());
        tokio::spawn(async move {
            let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await else { return };
            let svc = hyper_util::service::TowerToHyperService::new(app);
            let _ = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                .serve_connection(hyper_util::rt::TokioIo::new(tls), svc)
                .await;
        });
    }
}

/// The running server of this process (the main window's), if any.
pub struct Handle {
    pub port: u16,
    /// Served over HTTPS (a change restarts it).
    pub https: bool,
    stop: tokio::sync::watch::Sender<bool>,
}

impl Handle {
    pub fn start(rt: &tokio::runtime::Runtime, port: u16) -> Handle {
        let (tx, rx) = tokio::sync::watch::channel(false);
        rt.spawn(async move {
            if let Err(e) = serve(port, rx).await {
                eprintln!("remote control: {e:#}");
            }
        });
        Handle { port, https: https_on(), stop: tx }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_hex() {
        let h = sha_hex("abc");
        assert_eq!(h.len(), 64);
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn no_token_is_not_authorized() {
        let mut h = HeaderMap::new();
        assert!(!authorized(&h));
        h.insert(axum::http::header::AUTHORIZATION, "Bearer short".parse().unwrap());
        assert!(!authorized(&h));
    }
}
