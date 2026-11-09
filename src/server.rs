//! Server mode: `download_craft --server [--port N]`.
//!
//! Download Craft without a window, for a NAS, a home server or Docker: it
//! runs the queue by itself (every queued download starts, a few at a time)
//! and serves the remote control's page, so downloads are added and watched
//! from a phone or any browser on the network. Settings, the queue and the
//! paired devices live in the usual folder, or in `DOWNLOAD_CRAFT_DATA`;
//! files go to the save folder, or `DOWNLOAD_CRAFT_DOWNLOADS`.

use crate::engine::SharedState;
use crate::model::{config_dir, DownloadItem, Status};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

struct Task {
    shared: Arc<SharedState>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// Bytes and time at the last speed sample.
    last: (u64, std::time::Instant),
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Run until the process is stopped (Ctrl+C, `docker stop`).
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let port = arg_value(args, "--port")
        .or_else(|| std::env::var("DOWNLOAD_CRAFT_PORT").ok())
        .or_else(|| Some(crate::model::ui_pref_str("remote_port", "")))
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(crate::remote::DEFAULT_PORT);
    let max = crate::model::ui_pref_str("max_concurrent", "")
        .parse::<usize>()
        .ok()
        .or_else(|| std::env::var("DOWNLOAD_CRAFT_MAX").ok().and_then(|v| v.parse().ok()))
        .unwrap_or(3)
        .clamp(1, 16);
    let data = config_dir().ok_or_else(|| anyhow::anyhow!("no settings folder"))?;
    std::fs::create_dir_all(&data)?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    rt.spawn(async move {
        if let Err(e) = crate::remote::serve(port, stop_rx).await {
            eprintln!("remote control: {e:#}");
            std::process::exit(2);
        }
    });
    let code = crate::remote::new_code();
    let host = crate::remote::lan_address().unwrap_or_else(|| "this-computer".into());
    println!("Download Craft {} — server mode", crate::model::APP_VERSION);
    println!("  settings and queue: {}", data.display());
    println!("  downloads:          {}", crate::confirm::load_prefs().save_dir);
    let scheme = if crate::remote::https_on() { "https" } else { "http" };
    println!("  open on a phone:    {scheme}://{host}:{port}/");
    if crate::remote::https_on() {
        if let Some(fp) = crate::remote::fingerprint() {
            println!("  certificate:        {fp}");
        }
    }
    println!("  pairing code:       {code}   (works once, for ten minutes)");
    println!("  a new code:         download_craft --server-code");
    // Held for the life of the process: dropping it stops the web server.
    let _stop = stop_tx;
    // Rows left "Downloading" by a stop are queued again.
    requeue_interrupted();
    let mut tasks: HashMap<u64, Task> = HashMap::new();
    loop {
        tick(&rt, &mut tasks, max);
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Print a fresh pairing code (for a server already running: they share the
/// settings folder).
pub fn print_code() {
    println!("{}", crate::remote::new_code());
}

fn requeue_interrupted() {
    let _g = crate::model::fs_lock("queue");
    if let Ok(mut items) = crate::cli::load_queue() {
        let mut changed = false;
        for d in items.iter_mut() {
            if matches!(d.status, Status::Downloading) {
                d.status = Status::Queued;
                changed = true;
            }
        }
        if changed {
            let _ = crate::cli::save_queue(&items);
        }
    }
}

/// Orders from the phone page (and any other front-end): `cmd.json` and
/// `wake.json`, claimed once each.
fn take_json(name: &str) -> Option<serde_json::Value> {
    let p = config_dir()?.join(name);
    let _g = crate::model::fs_lock(name.trim_end_matches(".json"));
    if !_g.owned() {
        return None;
    }
    let data = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    serde_json::from_str(data.trim_start_matches('\u{feff}')).ok()
}

fn tick(rt: &tokio::runtime::Runtime, tasks: &mut HashMap<u64, Task>, max: usize) {
    let mut start: Vec<u64> = Vec::new();
    let mut set_status: Vec<(u64, Status)> = Vec::new();
    let mut remove: Vec<u64> = Vec::new();
    if let Some(serde_json::Value::Array(cmds)) = take_json("cmd.json") {
        for c in cmds {
            let id = c.get("id").and_then(|x| x.as_u64());
            match (c.get("action").and_then(|x| x.as_str()).unwrap_or(""), id) {
                ("pause", Some(id)) => {
                    if let Some(t) = tasks.get(&id) {
                        t.shared.paused.store(true, Ordering::SeqCst);
                    }
                    set_status.push((id, Status::Paused));
                }
                ("resume", Some(id)) => match tasks.get(&id) {
                    // Still running (paused in place): it just carries on.
                    Some(t) => {
                        t.shared.paused.store(false, Ordering::SeqCst);
                        set_status.push((id, Status::Downloading));
                    }
                    None => {
                        start.push(id);
                        set_status.push((id, Status::Queued));
                    }
                },
                ("cancel", Some(id)) => {
                    if let Some(t) = tasks.get(&id) {
                        t.shared.cancelled.store(true, Ordering::SeqCst);
                    }
                    set_status.push((id, Status::Paused));
                }
                ("remove", Some(id)) => {
                    if let Some(t) = tasks.remove(&id) {
                        t.shared.cancelled.store(true, Ordering::SeqCst);
                    }
                    remove.push(id);
                }
                ("pause_all", _) => {
                    for (id, t) in tasks.iter() {
                        t.shared.paused.store(true, Ordering::SeqCst);
                        set_status.push((*id, Status::Paused));
                    }
                }
                ("resume_all", _) => {
                    for t in tasks.values() {
                        t.shared.paused.store(false, Ordering::SeqCst);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(v) = take_json("wake.json") {
        if let Some(ids) = v.get("start").and_then(|x| x.as_array()) {
            start.extend(ids.iter().filter_map(|x| x.as_u64()));
        }
    }

    let _g = crate::model::fs_lock("queue");
    if !_g.owned() {
        return;
    }
    let Ok(mut items) = crate::cli::load_queue() else { return };
    let before = serde_json::to_string(&items).unwrap_or_default();
    items.retain(|d| !remove.contains(&d.id));
    for (id, st) in &set_status {
        if let Some(d) = items.iter_mut().find(|d| d.id == *id) {
            if !matches!(d.status, Status::Completed) {
                d.status = st.clone();
            }
        }
    }
    for id in &start {
        if let Some(d) = items.iter_mut().find(|d| d.id == *id) {
            if matches!(d.status, Status::Paused | Status::Failed(_)) && !tasks.contains_key(id) {
                d.status = Status::Queued;
            }
        }
    }
    // Progress of the running ones; the finished ones end.
    let mut ended: Vec<u64> = Vec::new();
    for (id, t) in tasks.iter_mut() {
        let Some(d) = items.iter_mut().find(|d| d.id == *id) else {
            t.shared.cancelled.store(true, Ordering::SeqCst);
            ended.push(*id);
            continue;
        };
        let dl = t.shared.downloaded.load(Ordering::SeqCst);
        d.downloaded_bytes = dl;
        if t.shared.has_total.load(Ordering::SeqCst) {
            d.total_bytes = Some(t.shared.total.load(Ordering::SeqCst));
        }
        if let Ok(g) = t.shared.resolved_name.lock() {
            if let Some(n) = g.clone() {
                d.filename = n;
            }
        }
        let helper = t.shared.speed_bps.load(Ordering::SeqCst);
        let secs = t.last.1.elapsed().as_secs_f64().max(0.2);
        d.speed_bps = if helper > 0 { helper as f64 } else { dl.saturating_sub(t.last.0) as f64 / secs };
        t.last = (dl, std::time::Instant::now());
        if t.handle.is_finished() {
            ended.push(*id);
            d.speed_bps = 0.0;
            if t.shared.cancelled.load(Ordering::SeqCst) {
                if !matches!(d.status, Status::Completed) {
                    d.status = Status::Paused;
                }
            } else {
                let err = t.shared.error.lock().ok().and_then(|g| g.clone());
                let landed = crate::model::size_on_disk(&d.dest_path());
                d.status = match err {
                    None if landed > 0 => {
                        d.downloaded_bytes = landed;
                        d.total_bytes = Some(landed);
                        Status::Completed
                    }
                    None => Status::Failed("nothing arrived".into()),
                    Some(e) => Status::Failed(e),
                };
            }
        }
    }
    for id in ended {
        tasks.remove(&id);
    }
    // Start what is queued, oldest first, up to the limit.
    let free = max.saturating_sub(tasks.len());
    let to_start: Vec<DownloadItem> = items
        .iter()
        .filter(|d| matches!(d.status, Status::Queued) && !tasks.contains_key(&d.id))
        .take(free)
        .cloned()
        .collect();
    for item in to_start {
        let shared = Arc::new(SharedState::new());
        let mut opts = crate::cli::opts_for(&item, 0);
        opts.allow_rename = crate::model::is_fallback_filename(&item.filename) || !item.filename.contains('.');
        let (url, dest, conns, s2) = (item.url.clone(), item.dest_path(), item.connections, shared.clone());
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let handle = rt.spawn(async move {
            let r = crate::engine::download(url, dest, conns, opts, s2.clone()).await;
            if let Err(e) = &r {
                s2.set_error(format!("{e:#}"));
            }
            r
        });
        if let Some(d) = items.iter_mut().find(|d| d.id == item.id) {
            d.status = Status::Downloading;
        }
        println!("start #{}: {}", item.id, item.filename);
        tasks.insert(item.id, Task { shared, handle, last: (item.downloaded_bytes, std::time::Instant::now()) });
    }
    if serde_json::to_string(&items).unwrap_or_default() != before {
        let _ = crate::cli::save_queue(&items);
    }
}
