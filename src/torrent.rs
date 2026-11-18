//! Torrents and magnet links.
//!
//! The BitTorrent work itself (peers, DHT, trackers, pieces) is done by
//! librqbit (Apache-2.0). This module ties one shared session to the app's
//! queue: a row whose address is a magnet link, a `.torrent` link or a
//! `.torrent` file downloads here, reports its progress through the same
//! [`SharedState`] as any other download, and pauses, resumes and cancels
//! with the row. Files are written straight into the row's save folder
//! (a multi-file torrent into a folder of its own name). A resumed torrent
//! is checked against what is already on disk and continues from there.
//! Seeding stops when the download is complete.

use crate::engine::SharedState;
use anyhow::{anyhow, Context, Result};
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, ListenerOptions, Session, SessionOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Is `url` something this module downloads: a magnet link, a `.torrent`
/// address, or a `.torrent` file on this computer?
pub fn is_torrent(url: &str) -> bool {
    let t = url.trim();
    // `get` (not `[..8]`): a pasted name in Bangla, Arabic, … has no char
    // boundary at byte 8, and slicing there panicked (issue #19).
    if t.len() > 8 && t.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("magnet:?")) {
        return true;
    }
    let path = t.split(['?', '#']).next().unwrap_or("");
    path.to_ascii_lowercase().ends_with(".torrent")
        && (t.starts_with("http://") || t.starts_with("https://") || Path::new(t).is_file())
}

/// A readable name for a magnet link before its metadata arrives (`dn=`).
pub fn magnet_name(url: &str) -> Option<String> {
    let q = url.trim().strip_prefix("magnet:?")?;
    q.split('&')
        .find_map(|kv| kv.strip_prefix("dn="))
        .map(|v| crate::engine::percent_decode_owned(&v.replace('+', " ")))
        .filter(|n| !n.trim().is_empty())
}

static SESSION: tokio::sync::OnceCell<Arc<Session>> = tokio::sync::OnceCell::const_new();

/// The app's one BitTorrent session (listening for peers, with DHT), made on
/// first use. Its own state (the DHT's peers) lives in the settings folder.
async fn session() -> Result<Arc<Session>> {
    SESSION
        .get_or_try_init(|| async {
            let dir = crate::model::config_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("torrent");
            std::fs::create_dir_all(&dir).ok();
            // A fixed port when one is given (server mode in Docker maps it),
            // else any free one.
            let port = std::env::var("DOWNLOAD_CRAFT_TORRENT_PORT").ok().and_then(|v| v.trim().parse::<u16>().ok()).unwrap_or(0);
            let mut listen = ListenerOptions::default();
            listen.listen_addr = (std::net::Ipv6Addr::UNSPECIFIED, port).into();
            // Settings → Network: listen on the chosen adapter (and, on Linux
            // and macOS, bind every peer, tracker and DHT socket to it).
            let bind = crate::netlock::bind_ip().ok().flatten();
            if let Some(ip) = bind {
                listen.listen_addr = (ip, port).into();
                listen.ipv4_only = true;
            }
            let device = (!crate::netlock::chosen().is_empty() && cfg!(not(windows))).then(crate::netlock::chosen);
            let opts = SessionOptions {
                bind_device_name: device,
                listen: Some(listen),
                fastresume: false,
                persistence: None,
                client_name_and_version: Some(format!("DownloadCraft {}", crate::model::APP_VERSION)),
                ..Default::default()
            };
            Session::new_with_opts(dir, opts).await.context("could not start BitTorrent")
        })
        .await
        .cloned()
}

fn source(url: &str) -> Result<AddTorrent<'static>> {
    let t = url.trim();
    if Path::new(t).is_file() {
        AddTorrent::from_local_filename(t).context("could not read the .torrent file")
    } else {
        Ok(AddTorrent::from_url(t.to_string()))
    }
}

/// Takes a torrent out of the session when the download ends in any way,
/// including the task being aborted (Cancel and Stop in the app abort it at
/// once). Without it an aborted download left its torrent running in the
/// shared session: the row said Paused while it kept downloading, and a
/// Resume then met the leftover.
struct Leave {
    session: Arc<Session>,
    id: Option<librqbit::api::TorrentIdOrHash>,
}

impl Leave {
    /// Keep it in the session (sharing after completion).
    fn keep(mut self) {
        self.id = None;
    }
}

impl Drop for Leave {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else { return };
        let session = self.session.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = session.delete(id, false).await;
            });
        }
    }
}

fn set_phase(shared: &SharedState, s: &str) {
    if let Ok(mut p) = shared.phase.lock() {
        if *p != s {
            *p = s.to_string();
        }
    }
}

/// Download the torrent `url` into `dest`'s folder. Returns when every file
/// is complete, or with an error (`cancelled` when the row was stopped).
pub async fn download(url: &str, dest: &Path, shared: Arc<SharedState>, log: &str) -> Result<()> {
    // The kill switch: nothing starts while the chosen adapter is down.
    crate::netlock::bind_ip().map_err(|e| anyhow!(e))?;
    let session = session().await?;
    let folder: PathBuf = dest.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&folder).ok();

    // 1. The metadata (a magnet link asks the peers for it): the torrent's
    //    name, size and whether it is one file or a folder of files.
    set_phase(&shared, "Finding peers");
    crate::engine::job_log_pub(log, format!("torrent: getting the file list for {url}"));
    let listed = tokio::select! {
        r = session.add_torrent(source(url)?, Some(AddTorrentOptions { list_only: true, ..Default::default() })) => r,
        _ = cancelled(&shared) => return Err(anyhow!("cancelled")),
    }
    .map_err(|e| anyhow!("{e:#}"))
    .context("could not read the torrent")?;
    let AddTorrentResponse::ListOnly(meta) = listed else {
        return Err(anyhow!("the torrent did not answer with its file list"));
    };
    let name = meta
        .info
        .name()
        .map(|n| crate::engine::sanitize_filename(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "torrent".to_string());
    let single = meta.info.info().length.is_some();
    // The file list, for "Choose files" (src/torrent_pick.rs), and the
    // files chosen there (all when none were).
    let list: Vec<(String, u64)> = meta.info.iter_file_details().map(|f| (f.filename.to_string(), f.len)).collect();
    crate::torrent_pick::save_files(log, &list);
    let only = crate::torrent_pick::picked(log).map(|v| v.into_iter().filter(|i| *i < list.len()).collect::<Vec<_>>()).filter(|v| !v.is_empty());
    let total: u64 = match &only {
        Some(v) => v.iter().map(|i| list[*i].1).sum(),
        None => list.iter().map(|(_, l)| *l).sum(),
    };
    // Started again (a new choice of files): the old run leaves the session.
    let _ = session.delete(meta.info_hash.into(), false).await;
    let out = if single { folder.clone() } else { folder.join(&name) };
    if let Ok(mut n) = shared.resolved_name.lock() {
        *n = Some(name.clone());
    }
    shared.total.store(total, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    shared.resumable.store(true, Ordering::SeqCst);
    crate::engine::job_log_pub(
        log,
        format!("torrent: {name}, {} file(s), {total} bytes, into {}", meta.info.iter_file_lengths().count(), out.display()),
    );

    // 2. The download itself, from the metadata just read (no second wait
    //    for peers). Files already on disk are checked and kept.
    let added = session
        .add_torrent(
            AddTorrent::from_bytes(meta.torrent_bytes.clone()),
            Some(AddTorrentOptions {
                output_folder: Some(out.to_string_lossy().into_owned()),
                overwrite: true,
                only_files: only.clone(),
                ..Default::default()
            }),
        )
        .await
        .map_err(|e| anyhow!("{e:#}"))
        .context("could not start the torrent")?;
    let (id, handle) = match added {
        AddTorrentResponse::Added(id, h) | AddTorrentResponse::AlreadyManaged(id, h) => (id, h),
        AddTorrentResponse::ListOnly(_) => return Err(anyhow!("the torrent was not started")),
    };
    let leave = Leave { session: session.clone(), id: Some(id.into()) };
    let mut swarm = Swarm::default();

    // 3. Report progress; follow the row's pause, resume and cancel.
    let mut paused = false;
    let result: Result<()> = loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            break Err(anyhow!("cancelled"));
        }
        let want_pause = shared.paused.load(Ordering::SeqCst);
        if want_pause != paused {
            let r = if want_pause { session.pause(&handle).await } else { session.unpause(&handle).await };
            // librqbit refuses to pause a paused torrent (and to start a live
            // one): what counts is the state it is in now, or the row and
            // the torrent could disagree for good.
            paused = match handle.stats().state {
                librqbit::TorrentStatsState::Paused => true,
                librqbit::TorrentStatsState::Live => false,
                _ => {
                    if r.is_ok() {
                        want_pause
                    } else {
                        paused
                    }
                }
            };
        }
        let st = handle.stats();
        if let Some(e) = st.error.clone() {
            break Err(anyhow!("torrent error: {e}"));
        }
        shared.downloaded.store(st.progress_bytes, Ordering::SeqCst);
        if st.total_bytes > 0 {
            shared.total.store(st.total_bytes, Ordering::SeqCst);
        }
        let (speed, peers) = st
            .live
            .as_ref()
            .map(|l| (l.download_speed.as_bytes(), l.snapshot.peer_stats.live))
            .unwrap_or((0, 0));
        shared.speed_bps.store(speed, Ordering::SeqCst);
        if let Ok(mut t) = shared.torrent.lock() {
            *t = Some(swarm.view(&handle, &st, &list, only.as_deref()));
        }
        if st.finished {
            break Ok(());
        }
        let phase = if paused {
            String::new()
        } else if matches!(st.state, librqbit::TorrentStatsState::Initializing { .. }) {
            "Checking files".to_string()
        } else if peers == 0 {
            "Finding peers".to_string()
        } else {
            format!("{peers} peer(s)")
        };
        set_phase(&shared, &phase);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    };
    set_phase(&shared, "");
    if let Ok(mut t) = shared.torrent.lock() {
        *t = None;
    }
    // Done: sharing goes on in the background up to Settings' ratio (0 =
    // none); stopped or failed: the torrent leaves the session at once.
    // Every byte on disk stays for a later resume.
    let ratio: f64 = crate::model::ui_pref_str("seed_ratio", "0").trim().parse().unwrap_or(0.0);
    if result.is_ok() && ratio > 0.0 {
        leave.keep();
        let (session, handle, goal) = (session.clone(), handle.clone(), (total as f64 * ratio) as u64);
        crate::engine::job_log_pub(log, format!("torrent: sharing until {goal} bytes are uploaded (ratio {ratio})"));
        tokio::spawn(async move {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(24 * 3600);
            while handle.stats().uploaded_bytes < goal && std::time::Instant::now() < until {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
            let _ = session.delete(id.into(), false).await;
        });
    } else {
        drop(leave); // out of the session now
    }
    match &result {
        Ok(()) => crate::engine::job_log_pub(log, "torrent: complete".to_string()),
        Err(e) => crate::engine::job_log_pub(log, format!("torrent: stopped: {e:#}")),
    }
    result
}

/// What the progress window's Part Info shows for a torrent: seeds and
/// peers, each connected peer with what it sent and how fast, and each
/// file's progress. Speeds come from the change since the last look.
#[derive(Default)]
struct Swarm {
    last: std::collections::HashMap<String, (u64, std::time::Instant)>,
    speeds: std::collections::HashMap<String, f64>,
}

impl Swarm {
    fn view(
        &mut self,
        handle: &librqbit::ManagedTorrent,
        st: &librqbit::TorrentStats,
        files: &[(String, u64)],
        only: Option<&[usize]>,
    ) -> serde_json::Value {
        let agg = st.live.as_ref().map(|l| &l.snapshot.peer_stats);
        let (seeds, live, connecting, queued, seen) =
            agg.map(|a| (a.live_seeders, a.live, a.connecting, a.queued, a.seen)).unwrap_or_default();
        let now = std::time::Instant::now();
        let mut peers: Vec<(String, String, u64, f64)> = Vec::new();
        if let Some(l) = handle.live() {
            let snap = l.per_peer_stats_snapshot(Default::default());
            for (addr, p) in snap.peers {
                let got = p.counters.fetched_bytes;
                let speed = match self.last.get(&addr).copied() {
                    Some((b, t)) => {
                        let dt = now.duration_since(t).as_secs_f64();
                        if dt >= 0.9 {
                            let v = got.saturating_sub(b) as f64 / dt;
                            self.speeds.insert(addr.clone(), v);
                            self.last.insert(addr.clone(), (got, now));
                        }
                        self.speeds.get(&addr).copied().unwrap_or(0.0)
                    }
                    None => {
                        self.last.insert(addr.clone(), (got, now));
                        0.0
                    }
                };
                peers.push((addr, p.client_name.unwrap_or_default(), got, speed));
            }
        }
        // Fastest first; peers that sent nothing yet last.
        peers.sort_by(|a, b| b.3.total_cmp(&a.3).then(b.2.cmp(&a.2)));
        let known: std::collections::HashSet<String> = peers.iter().map(|p| p.0.clone()).collect();
        self.last.retain(|k, _| known.contains(k));
        self.speeds.retain(|k, _| known.contains(k));
        let file_rows: Vec<serde_json::Value> = files
            .iter()
            .enumerate()
            .filter(|(i, _)| only.is_none_or(|o| o.contains(i)))
            .map(|(i, (name, len))| serde_json::json!([name, len, st.file_progress.get(i).copied().unwrap_or(0)]))
            .collect();
        serde_json::json!({
            "seeds": seeds,
            "peers": live.saturating_sub(seeds),
            "connecting": connecting,
            "queued": queued,
            "seen": seen,
            "up": st.live.as_ref().map(|l| l.upload_speed.as_bytes()).unwrap_or(0),
            "list": peers.iter().take(64).map(|(a, c, g, v)| serde_json::json!([a, c, g, v])).collect::<Vec<_>>(),
            "files": file_rows,
        })
    }
}

async fn cancelled(shared: &SharedState) {
    while !shared.cancelled.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_torrent_links() {
        assert!(is_torrent("magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=ubuntu"));
        assert!(is_torrent("https://example.com/files/linux.iso.torrent"));
        assert!(is_torrent("https://example.com/files/linux.iso.torrent?x=1"));
        assert!(!is_torrent("https://example.com/files/linux.iso"));
        assert!(!is_torrent("magnet:"));
        // Issue #19: non-ASCII text with no char boundary at byte 8.
        assert!(!is_torrent("আমার ফাইল"));
        assert!(!is_torrent("ab মফাইল.zip"));
        assert!(is_torrent("MAGNET:?xt=urn:btih:abc&dn=আমার"));
    }

    #[test]
    fn magnet_display_name() {
        assert_eq!(
            magnet_name("magnet:?xt=urn:btih:abc&dn=Big+Buck+Bunny%20(2008)&tr=udp://x").as_deref(),
            Some("Big Buck Bunny (2008)")
        );
        assert_eq!(magnet_name("magnet:?xt=urn:btih:abc"), None);
    }
}
