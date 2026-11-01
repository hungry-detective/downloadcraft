//! Command-line interface for Download Craft.
//!
//! Shares the queue file (`%APPDATA%/download_craft/queue.json`) and settings
//! (`ui.json`) with the GUI, so both front-ends manage the same list.
//!
//! Note: downloads run in the process that starts them. `download`/`get`
//! run the engine here in the terminal; they cannot pause a transfer that
//! is running inside the GUI window (and vice versa).

use crate::engine::{download, EngineOpts, SharedState};
use crate::model::{filename_from_url, format_bytes, is_download_url, DownloadItem};
use clap::{Parser, Subcommand};
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, Parser)]
#[command(name = "download_craft", version = crate::model::APP_VERSION, about = "Download Craft — download manager (CLI)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Queue a URL (and optionally start it right away)
    Add {
        url: String,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long, default_value_t = 8)]
        connections: usize,
        /// Queue only, do not start downloading
        #[arg(long, action = clap::ArgAction::SetTrue)]
        no_start: bool,
        /// Expected SHA-256 hex digest (verified after download)
        #[arg(long)]
        checksum: Option<String>,
        /// Replace the destination file if it already exists
        /// (default is to save a numbered copy instead)
        #[arg(long, action = clap::ArgAction::SetTrue)]
        overwrite: bool,
    },
    /// Check media helpers or install the app-managed copies (no GUI required)
    Helpers {
        #[arg(long)] install_ytdlp: bool,
        #[arg(long)] install_ffmpeg: bool,
        /// Install the app's own JavaScript runtime (portable Deno) for yt-dlp
        #[arg(long)] install_runtime: bool,
        /// Compare with the latest releases and update the app's own copies
        /// (what Settings → Plugins does when opened)
        #[arg(long)] update: bool,
    },
    /// Register this app as a browser's native messaging host
    RegisterBrowser { browser:String },
    /// Show queued downloads
    List {
        #[arg(long)]
        json: bool,
    },
    /// Download a queued item by id (runs here until done)
    Download {
        id: u64,
    },
    /// One-shot download without touching the queue
    Get {
        url: String,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(short, long, default_value_t = 8)]
        connections: usize,
        /// Expected SHA-256 hex digest (verified after download)
        #[arg(long)]
        checksum: Option<String>,
        /// Preferred YouTube quality (e.g. 720p, 1080p60): the engine
        /// resolves the highest stream at or below it, merging video+audio
        /// for adaptive-only qualities (needs ffmpeg for those)
        #[arg(long)]
        quality: Option<String>,
    },
    /// List the files inside a remote ZIP without downloading it
    /// (reads only its table of contents with a Range request)
    Peek {
        url: String,
    },
    /// Remove an item from the queue (keeps the file)
    Remove {
        id: u64,
    },
    /// Re-queue a finished/failed item from zero and download it
    Redownload {
        id: u64,
    },
    /// Remove all finished items from the queue
    Clear,
    /// Queue every http(s) URL found in a text file
    Import {
        file: String,
    },
    /// Write all queued URLs to a text file
    Export {
        file: String,
    },
    /// Totals by status
    Status,
    /// Build the system tray icon and keep it alive briefly (self-test)
    #[cfg(windows)]
    TrayTest,
}

fn queue_path() -> std::path::PathBuf {
    crate::model::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("queue.json")
}

fn load_queue() -> anyhow::Result<Vec<DownloadItem>> {
    crate::model::read_queue_items(&queue_path()).map_err(anyhow::Error::msg)
}
fn save_queue(items: &[DownloadItem]) -> anyhow::Result<()> {
    let path=queue_path();
    if let Some(parent)=path.parent() { std::fs::create_dir_all(parent)?; }
    crate::model::atomic_write(&path,serde_json::to_string_pretty(items)?.as_bytes())?;
    Ok(())
}

/// Global network settings shared with the GUI (ui.json).
fn global_opts() -> EngineOpts {
    let mut o = EngineOpts {
        user_agent: crate::platform::default_user_agent().to_string(),
        timeout_secs: 60,
        max_retries: 5,
        ..Default::default()
    };
    if let Some(cfg) = crate::model::config_dir().map(|p| p.join("ui.json")) {
        if let Ok(data) = std::fs::read_to_string(cfg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                let g = |k: &str| v.get(k);
                if let Some(s) = g("user_agent").and_then(|x| x.as_str()) {
                    if !s.is_empty() {
                        o.user_agent = s.to_string();
                    }
                }
                if let Some(n) = g("timeout_secs").and_then(|x| x.as_u64()) {
                    o.timeout_secs = n;
                }
                if let Some(n) = g("max_retries").and_then(|x| x.as_u64()) {
                    o.max_retries = n as u32;
                }
                if let Some(s) = g("proxy_url").and_then(|x| x.as_str()) {
                    o.proxy_url = s.to_string();
                }
            }
        }
    }
    o
}

fn opts_for(item: &DownloadItem, limit_bps: u64) -> EngineOpts {
    let mut o = global_opts();
    o.speed_limit_bps = limit_bps;
    o.timeout_secs = item.eff_timeout(o.timeout_secs);
    o.max_retries = item.eff_retries(o.max_retries);
    o.referer = item.referer.clone();
    o.cookie = item.cookie.clone();
    o.captured_headers = crate::engine::CapturedHeaders::for_url(&item.url, &item.extra_headers);
    o.browser = item.browser.clone();
    o.log_key = item.id.to_string();
    o.mirrors = item.mirrors.clone();
    o.username = item.username.clone();
    o.password = item.password.clone();
    o.expected_sha256 = item.checksum_sha256.clone();
    o.expected_duration_secs = item.duration_secs;
    o.require_audio = item.expects_audio || !item.pair_url.is_empty();
    o.overwrite_existing = item.overwrite_existing;
    o.quality_pref = item.quality_pref.clone();
    o
}

pub fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Commands::RegisterBrowser {browser} => {
            println!("{}",crate::app::install_native_host(&browser).map_err(anyhow::Error::msg)?);
            Ok(())
        }

        Commands::Helpers {install_ytdlp,install_ffmpeg,install_runtime,update} => {
            if install_ytdlp {crate::engine::ytdlp_refresh_blocking()?;}
            if install_ffmpeg {crate::helpers::install_ffmpeg()?;}
            if install_runtime {crate::engine::deno_install_blocking()?;}
            if update {
                for line in [crate::engine::ytdlp_update_check_blocking(),crate::engine::ffmpeg_update_check_blocking()] {
                    if !line.is_empty() {eprintln!("{line}");}
                }
            }
            let yt=crate::engine::ytdlp_info();let ff=crate::engine::ffmpeg_info();
            println!("{}",serde_json::json!({"yt_dlp":yt.path,"yt_dlp_version":yt.version,"yt_dlp_managed":yt.managed,"ffmpeg":ff.path,"ffmpeg_version":ff.version,"ffprobe":ff.probe_path,"ffmpeg_managed":ff.managed,"js_runtime":yt.js_runtime}));
            Ok(())
        }

        Commands::Add {
            url,
            dir,
            name,
            connections,
            no_start,
            checksum,
            overwrite,
        } => {
            let _guard = crate::model::fs_lock("queue");
            anyhow::ensure!(_guard.owned(), "Queue is busy; retry shortly");
            if !is_download_url(&url) {
                anyhow::bail!("not an http(s) URL: {url}");
            }
            let csum = crate::model::normalize_sha256(checksum.as_deref().unwrap_or(""));
            if !csum.is_empty() && !crate::model::is_valid_sha256(&csum) {
                anyhow::bail!("invalid --checksum (want 64 hex chars)");
            }
            let mut items = load_queue()?;
            if items.iter().any(|d| d.url == url) {
                println!("already queued");
                return Ok(());
            }
            let fname = name.unwrap_or_else(|| filename_from_url(&url));
            let dir = dir.unwrap_or_else(|| {
                dirs::download_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned())
            });
            let dest = std::path::Path::new(&dir).join(&fname);
            // Same file, fresh URL: refresh in place (see append_to_queue).
            if let Some(pos) = crate::model::find_dest_clash(&items, &dest) {
                let id = items[pos].id;
                if items[pos].url != url {
                    items[pos].url = url;
                    items[pos].downloaded_bytes = 0;
                    crate::model::drop_resume_state(&dest);
                }
                items[pos].speed_bps = 0.0;
                items[pos].status = crate::model::Status::Queued;
                save_queue(&items)?;
                println!("refreshed #{id} {fname}");
                drop(_guard);
                if !no_start {
                    run_id(id)?;
                }
                return Ok(());
            }
            let id = crate::model::next_queue_id(&queue_path(), &items);
            let mut item = DownloadItem::new(id, url, fname.clone(), dir, connections);
            item.checksum_sha256 = csum;
            item.overwrite_existing = overwrite;
            items.push(item);
            save_queue(&items)?;
            println!("queued #{id} {fname}");
            drop(_guard);
            if !no_start {
                run_id(id)?;
            }
            Ok(())
        }
        Commands::List { json } => {
            let items = load_queue()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
                return Ok(());
            }
            println!(
                "{:<6} {:<9} {:>10} {:>5}  {}",
                "ID", "STATUS", "SIZE", "PCT", "FILE"
            );
            for d in &items {
                let pct = (d.progress() * 100.0) as u32;
                println!(
                    "{:<6} {:<9} {:>10} {:>4}%  {}",
                    d.id,
                    d.status.label(),
                    d.total_bytes.map(format_bytes).unwrap_or_else(|| "?".into()),
                    pct,
                    d.filename
                );
            }
            Ok(())
        }
        Commands::Download { id } => run_id(id),
        Commands::Get {
            url,
            output,
            connections,
            checksum,
            quality,
        } => {
            if !is_download_url(&url) {
                anyhow::bail!("not an http(s) URL: {url}");
            }
            let csum = crate::model::normalize_sha256(checksum.as_deref().unwrap_or(""));
            if !csum.is_empty() && !crate::model::is_valid_sha256(&csum) {
                anyhow::bail!("invalid --checksum (want 64 hex chars)");
            }
            let dest = match output {
                Some(o) => std::path::PathBuf::from(o),
                None => dirs::download_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(filename_from_url(&url)),
            };
            let mut o = global_opts();
            o.speed_limit_bps = 0;
            o.allow_rename = false;
            o.expected_sha256 = csum;
            o.quality_pref = quality.filter(|s| !s.trim().is_empty()).unwrap_or_default();
            run_engine(url, dest, connections, o)?;
            Ok(())
        }
        Commands::Peek { url } => {
            if !is_download_url(&url) {
                anyhow::bail!("not an http(s) URL: {url}");
            }
            let opts = global_opts();
            let client = opts.build_client()?;
            let request: downloadcraft_engine::RequestFn = Arc::new(move |u: &str| opts.get(&client, u));
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let entries = rt.block_on(downloadcraft_engine::zip::list_remote(&request, &url))?;
            let total: u64 = entries.iter().map(|e| e.size).sum();
            for e in &entries {
                if e.is_dir {
                    println!("{:>12}  {}", "", e.name);
                } else {
                    println!("{:>12}  {}", format_bytes(e.size), e.name);
                }
            }
            println!("{} entries, {} unpacked", entries.len(), format_bytes(total));
            Ok(())
        }
        Commands::Remove { id } => {
            let _guard = crate::model::fs_lock("queue");
            anyhow::ensure!(_guard.owned(), "Queue is busy; retry shortly");
            let mut items = load_queue()?;
            let n = items.len();
            items.retain(|d| d.id != id);
            if items.len() == n {
                anyhow::bail!("no such id: {id}");
            }
            save_queue(&items)?;
            println!("removed #{id}");
            Ok(())
        }
        Commands::Redownload { id } => {
            let _guard = crate::model::fs_lock("queue");
            anyhow::ensure!(_guard.owned(), "Queue is busy; retry shortly");
            let mut items = load_queue()?;
            let d = items
                .iter_mut()
                .find(|d| d.id == id)
                .ok_or_else(|| anyhow::anyhow!("no such id: {id}"))?;
            // Replace semantics (same as the GUI redownload): drop the old
            // file so the engine does not auto-rename to `name (1).ext`.
            let dest = d.dest_path();
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            d.downloaded_bytes = 0;
            d.total_bytes = None;
            d.speed_bps = 0.0;
            d.status = crate::model::Status::Queued;
            save_queue(&items)?;
            drop(_guard);
            run_id(id)
        }
        Commands::Clear => {
            let _guard = crate::model::fs_lock("queue");
            anyhow::ensure!(_guard.owned(), "Queue is busy; retry shortly");
            let mut items = load_queue()?;
            let n = items.len();
            items.retain(|d| !matches!(d.status, crate::model::Status::Completed));
            save_queue(&items)?;
            println!("cleared {} finished item(s)", n - items.len());
            Ok(())
        }
        Commands::Import { file } => {
            let _guard = crate::model::fs_lock("queue");
            anyhow::ensure!(_guard.owned(), "Queue is busy; retry shortly");
            let data = std::fs::read_to_string(&file)?;
            let mut items = load_queue()?;
            let mut n = 0;
            for l in crate::model::parse_url_list(&data) {
                let t = l.url.as_str();
                if !items.iter().any(|d| d.url == t) {
                    let id = crate::model::next_queue_id(&queue_path(), &items);
                    let dir = dirs::download_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
                    let name = if l.filename.is_empty() { filename_from_url(t) } else { crate::engine::sanitize_filename(&l.filename) };
                    let mut item = DownloadItem::new(id, t.to_string(), name, dir, 8);
                    item.referer = l.referer.clone();
                    item.cookie = l.cookie.clone();
                    items.push(item);
                    n += 1;
                }
            }
            save_queue(&items)?;
            println!("imported {n} URL(s)");
            Ok(())
        }
        Commands::Export { file } => {
            let items = load_queue()?;
            let body: String = items
                .iter()
                .map(|d| d.url.clone())
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&file, body)?;
            println!("exported {} URL(s) to {file}", items.len());
            Ok(())
        }
        Commands::Status => {
            let items = load_queue()?;
            let mut by: std::collections::HashMap<String, usize> = Default::default();
            let mut bytes = 0u64;
            for d in &items {
                *by.entry(d.status.label()).or_default() += 1;
                bytes += d.downloaded_bytes;
            }
            println!("items: {}", items.len());
            let mut keys: Vec<_> = by.keys().collect();
            keys.sort();
            for k in keys {
                println!("  {k}: {}", by[k]);
            }
            println!("downloaded total: {}", format_bytes(bytes));
            Ok(())
        }
        #[cfg(windows)]
        Commands::TrayTest => {
            let img = image::load_from_memory(include_bytes!("../assets/icon.png"))?.to_rgba8();
            let (w, h) = (img.width(), img.height());
            let icon = tray_icon::Icon::from_rgba(img.into_raw(), w, h)?;
            let menu = tray_icon::menu::Menu::new();
            let show =
                tray_icon::menu::MenuItem::with_id("tray-show", "Show Download Craft", true, None);
            let exit = tray_icon::menu::MenuItem::with_id("tray-exit", "Exit", true, None);
            menu.append_items(&[&show, &exit])?;
            let _tray = tray_icon::TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("Download Craft (test)")
                .with_icon(icon)
                .build()?;
            println!("tray icon created — look in the system tray overflow (▲), 15s…");
            std::thread::sleep(std::time::Duration::from_secs(15));
            println!("tray test done");
            Ok(())
        }
    }
}

fn run_id(id: u64) -> anyhow::Result<()> {
    let items = load_queue()?;
    let item = items
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| anyhow::anyhow!("no such id: {id}"))?
        .clone();
    let dest = item.dest_path();
    let mut opts = opts_for(&item, 0);
    // A row still under a placeholder name (`download_123.bin`, as `add`
    // names a page link) takes the real one, a YouTube video's title
    // included, as the app's own runs do.
    opts.allow_rename = crate::model::is_fallback_filename(&item.filename) || !item.filename.contains('.');
    let (got_bytes, got_total, got_name) = run_engine(item.url.clone(), dest, item.connections, opts)?;
    // Re-read under the lock so a completed CLI job retains concurrent adds.
    let _guard=crate::model::fs_lock("queue");
    anyhow::ensure!(_guard.owned(), "Download finished but queue is busy; retry updating its status");
    let mut items = load_queue()?;
    if let Some(d) = items.iter_mut().find(|d| d.id == id) {
        d.status = crate::model::Status::Completed;
        d.speed_bps = 0.0;
        d.downloaded_bytes = got_bytes;
        if let Some(n) = got_name {
            d.filename = n;
        }
        if got_total.is_some() {
            d.total_bytes = got_total;
        }
        save_queue(&items)?;
    }
    Ok(())
}

/// Run the engine synchronously with a console progress line.
/// Returns (bytes_downloaded, total_if_known).
fn run_engine(
    url: String,
    dest: std::path::PathBuf,
    connections: usize,
    opts: EngineOpts,
) -> anyhow::Result<(u64, Option<u64>, Option<String>)> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let shared = Arc::new(SharedState::new());
    let s2 = shared.clone();
    let name = dest
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| url.clone());
    let handle = rt.spawn(async move {
        download(url, dest, connections.clamp(1, 32), opts, s2).await
    });
    let t0 = std::time::Instant::now();
    let mut last = 0u64;
    let mut last_t = t0;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let dl = shared.downloaded.load(Ordering::SeqCst);
        let now = std::time::Instant::now();
        let el = now.duration_since(last_t).as_secs_f64();
        let spd = if el > 0.0 { (dl.saturating_sub(last)) as f64 / el } else { 0.0 };
        last = dl;
        last_t = now;
        if shared.has_total.load(Ordering::SeqCst) {
            let total = shared.total.load(Ordering::SeqCst);
            let pct = if total > 0 {
                (dl as f64 / total as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let bar_n = (pct / 5.0) as usize;
            eprint!(
                "\r[{:<20}] {:>5.1}% {} / {} @ {}/s  {}",
                "#".repeat(bar_n.min(20)),
                pct,
                format_bytes(dl),
                format_bytes(total),
                format_bytes(spd as u64),
                truncate(&name, 40),
            );
        } else {
            // No size yet: during yt-dlp's extraction phase nothing is moving,
            // so name the stage instead of printing a bare "0 B".
            let phase = shared.phase_now();
            if dl == 0 && !phase.is_empty() {
                eprint!(
                    "\r{}…  {}                    ",
                    phase,
                    truncate(&name, 40)
                );
            } else {
                eprint!(
                    "\r{} downloaded @ {}/s  {}",
                    format_bytes(dl),
                    format_bytes(spd as u64),
                    truncate(&name, 40)
                );
            }
        }
        use std::io::Write;
        let _ = std::io::stderr().flush();
        if handle.is_finished() {
            break;
        }
    }
    eprintln!();
    let r = rt.block_on(handle)?;
    r.map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let name = shared.resolved_name.lock().ok().and_then(|mut g| g.take());
    if let Some(n) = &name {
        println!("saved as: {n}");
    }
    println!("done in {:.1}s", t0.elapsed().as_secs_f64());
    let total = if shared.has_total.load(Ordering::SeqCst) {
        Some(shared.total.load(Ordering::SeqCst))
    } else {
        None
    };
    Ok((shared.downloaded.load(Ordering::SeqCst), total, name))
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let cut = s
        .char_indices()
        .take_while(|(i, _)| *i < n)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    format!("{}…", &s[..cut])
}
