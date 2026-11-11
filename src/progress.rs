//! IDM-style standalone per-download progress window.
//!
//! Launched as a separate window when a download starts from a confirm dialog
//! or the main Add dialog: `download_craft.exe --progress <id>`.
//!
//! The surface is the card drawn in `Downloading gui concepts.html` — frameless
//! and transparent, so that rounded card *is* the window: title bar with the
//! IDM percentage prefix, the Info/Speed/Settings segmented tabs, the info
//! rows, the accent progress bar and the Part Info strip + table. Every offset
//! below is that layout in egui space, origin = card top-left, card border
//! included (the concept renders 522 wide, 553 tall at its six sample parts).
//!
//! The main app (which owns the engine tasks) publishes live numbers to
//! `progress.json` several times a second; this window polls that file +
//! `queue.json` and draws
//! them. Pause/Resume/Cancel go back through `cmd.json`, polled by the main
//! app. The window is a view, not a control: closing it (or minimising it)
//! leaves the transfer running, and only the explicit Pause/Cancel buttons
//! change what the download does.

use crate::app::pl;
use crate::dm::*;
use crate::model::{config_dir, format_bytes, DownloadItem, Status};
use std::time::{Duration, Instant};
fn queue_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("queue.json"))
}

fn progress_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("progress.json"))
}

fn cmd_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("cmd.json"))
}

fn read_queue() -> Vec<DownloadItem> {
    queue_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[derive(Debug, Default, Clone)]
struct Snap {
    downloaded: u64,
    total: Option<u64>,
    speed: f64,
    resumable: bool,
    live: bool,
    /// Stage the transfer is in while it is not moving bytes (yt-dlp's
    /// "Getting info"/"Merging"); empty for the built-in engine.
    phase: String,
    /// The resolution the fetched stream really has ("1280×720"), straight
    /// from the helper's chosen format. Empty while unknown.
    resolution: String,
    /// Where the server is and how far (empty until measured).
    server: String,
    parts: Vec<(u64, u64, u64)>, // (start, end, done)
    /// (index, count) while a helper fetches a fragmented stream, which has
    /// no size until it is done: the bar and Part Info count fragments.
    fragments: Option<(u64, u64)>,
}

fn read_snap(id: u64) -> Snap {
    let mut out = Snap::default();
    let data = progress_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    if data.is_empty() {
        return out;
    }
    let v: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
    let e = match v.get("items").and_then(|m| m.get(&id.to_string())) {
        Some(e) => e,
        None => return out,
    };
    out.live = true;
    out.downloaded = e.get("downloaded").and_then(|x| x.as_u64()).unwrap_or(0);
    out.total = e.get("total").and_then(|x| x.as_u64());
    out.speed = e.get("speed").and_then(|x| x.as_f64()).unwrap_or(0.0);
    out.resumable = e.get("resumable").and_then(|x| x.as_bool()).unwrap_or(false);
    out.phase = e
        .get("phase")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    out.resolution = e
        .get("resolution")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    out.server = e.get("server").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if let Some(arr) = e.get("parts").and_then(|x| x.as_array()) {
        for p in arr.iter().take(32) {
            let n = |i: usize| p.get(i).and_then(|x| x.as_u64()).unwrap_or(0);
            out.parts.push((n(0), n(1), n(2)));
        }
    }
    if let Some(f) = e.get("fragments").and_then(|x| x.as_array()) {
        let n = |i: usize| f.get(i).and_then(|x| x.as_u64()).unwrap_or(0);
        if n(1) > 0 {
            out.fragments = Some((n(0).min(n(1)), n(1)));
        }
    }
    out
}

fn read_ui_prefs() -> (String, String) {
    let v: serde_json::Value = config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let lim = v
        .get("speed_limit_kbps")
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
        .to_string();
    let fin = v
        .get("finish_action")
        .and_then(|x| x.as_str())
        .unwrap_or("nothing")
        .to_string();
    (lim, fin)
}

/// This download's own speed limit (KB/s, 0 = none).
fn send_cmd_limit(id: u64, kbps: u64) {
    let Some(p) = cmd_path() else { return };
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() { return; }
    let mut val: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = val.as_array_mut() {
        arr.push(serde_json::json!({ "action": "limit_one", "id": id, "kbps": kbps }));
        if let Ok(json) = serde_json::to_string_pretty(&val) {
            let _ = crate::model::atomic_write(&p, json.as_bytes());
        }
    }
}

/// Command with a payload (`limit` kbps, `finish` action).
fn send_cmd_kv(action: &str, k: &str, v: serde_json::Value) {
    let Some(p) = cmd_path() else { return };
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() { return; }
    let mut val: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = val.as_array_mut() {
        let mut obj = serde_json::Map::new();
        obj.insert("action".to_string(), serde_json::Value::String(action.to_string()));
        obj.insert(k.to_string(), v);
        arr.push(serde_json::Value::Object(obj));
        if let Ok(json) = serde_json::to_string_pretty(&val) {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}
/// Set once the user has closed the window, so the Close it sends is not
/// intercepted again.
static CLOSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn send_cmd(id: u64, action: &str) {
    let Some(p) = cmd_path() else { return };
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() { return; }
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = v.as_array_mut() {
        arr.retain(|e| !(e.get("id").and_then(|x| x.as_u64()) == Some(id)));
        arr.push(serde_json::json!({ "id": id, "action": action }));
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
}

/// Open a file/folder with the OS default handler on any platform.
fn open_path(p: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(p).spawn();
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(p).spawn();
    }
}

fn fmt_eta(left: u64, speed: f64) -> String {
    if speed <= 1.0 {
        return "-".to_string();
    }
    let secs = (left as f64 / speed) as u64;
    if secs >= 3600 {
        format!("{} h {} m left", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} m {} s left", secs / 60, secs % 60)
    } else {
        format!("{} s left", secs)
    }
}

/// Media length as IDM words it: "1 h 49 min", "6 min 23 s", "19 s".
fn fmt_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{secs} s")
    }
}

// ================= concept geometry =================
// `Downloading gui concepts.html` (measured in Chrome, border-box), tightened:
// the concept spaced its info rows 27 px apart and padded every block, which
// left the card both wide and airy. Same layout, denser rhythm.
//   card     470 wide (468 content + 2x1px border), 361 + 22n tall
//   header   1..38     mark 18 at (15,10), title 14/500 at x=41, controls 14
//   tabs     38..72    trough (15,46,440,26), three 145.33x22 segments
//   info     72..265   eight 13px rows at y=84 step 22, right edge 455
//   progress 265..273  track (15,266,440,7), r4, accent gradient fill
//   parts    273..     rule, Part Info row (h15 at y=293), strip (y=315,h9),
//                      table header (y=331,h20), rows at y=351 step 22, pad 10

/// Concept card width, border-box.
const PG_W: f32 = 470.0;
/// Card height with no part list: title + tabs + info + progress + rule.
const PG_MIN_H: f32 = 286.0;
/// Card height with the part list collapsed but its header row still shown
/// (the chevron and the Pause/Close actions must stay reachable).
const PG_PART_HEAD_H: f32 = 320.0;
/// Dead state (the item left the queue): title + notice + Close.
const PG_DEAD_H: f32 = 148.0;

const HDR_H: f32 = 38.0;
const MARK_X: f32 = 15.0;
const MARK_Y: f32 = 10.0;
const MARK_S: f32 = 18.0;
const TITLE_X: f32 = 41.0;
/// Vertical centre of the header (and of its two window controls).
const HDR_CY: f32 = 19.0;
const CTL_X1: f32 = 416.0;
const CTL_X2: f32 = 446.0;

const SEG_X: f32 = 15.0;
const SEG_Y: f32 = 44.0;
const SEG_W: f32 = 440.0;
const SEG_H: f32 = 24.0;
const SEG_PAD: f32 = 2.0;
const TAB_H: f32 = 20.0;
const TAB_ICON: f32 = 11.0;
/// Gap between the two neighbouring tabs of the strip.
const TAB_GAP: f32 = 3.0;

/// Content gutter (the concept's 15px padding inside the 1px border).
const PAD_X: f32 = 15.0;
/// Content right edge.
const IN_R: f32 = 455.0;
const INFO_Y: f32 = 84.0;
const ROW_STEP: f32 = 22.0;
const ROW_H: f32 = 15.0;

const PROG_Y: f32 = 266.0;
const PROG_W: f32 = 440.0;
// A bold bar: the first thing the eye finds in the window.
const PROG_H: f32 = 11.0;

const PART_Y: f32 = 284.0;
const HEAD_Y: f32 = 293.0;
const HEAD_H: f32 = 15.0;
const STRIP_Y: f32 = 315.0;
const STRIP_H: f32 = 9.0;
const TBL_HDR_Y: f32 = 331.0;
const TBL_HDR_H: f32 = 20.0;
const TBL_Y: f32 = 351.0;
const TBL_STEP: f32 = 22.0;
const TBL_PAD: f32 = 4.0;
const FOOT_PAD: f32 = 10.0;
/// Table columns: # | Status (flex) | Downloaded | Total, both right on IN_R.
const COL_NUM_W: f32 = 24.0;
const COL_DL_X: f32 = IN_R - 170.0;
const COL_TOT_X: f32 = IN_R - 80.0;

/// A soft white highlight that fades out to both sides. Its own mesh: the
/// shared gradient helper paints opaque colours, which turned the fading
/// ends of this highlight into a black band.
fn sheen(p: &egui::Painter, r: egui::Rect) {
    let clear = egui::Color32::from_white_alpha(0);
    let mid = egui::Color32::from_white_alpha(80);
    let xs = [r.min.x, r.center().x, r.max.x];
    let cols = [clear, mid, clear];
    let mut mesh = egui::Mesh::default();
    for (x, c) in xs.iter().zip(cols) {
        for y in [r.min.y, r.max.y] {
            mesh.vertices.push(egui::epaint::Vertex { pos: egui::pos2(*x, y), uv: egui::epaint::WHITE_UV, color: c });
        }
    }
    // Two quads: left half (0,1,2,3) and right half (2,3,4,5).
    for q in [0u32, 2] {
        mesh.add_triangle(q, q + 1, q + 2);
        mesh.add_triangle(q + 1, q + 3, q + 2);
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Part rows the card grows to; beyond this the table scrolls.
const MAX_PART_ROWS: usize = 6;

/// Card height with `n` part rows — 493 at the concept's six samples.
fn card_h(n: usize) -> f32 {
    361.0 + TBL_STEP * n as f32
}

/// The concept's tabs, plus the per-download log.
const TABS: [(DmIcon, &str); 4] = [
    (DmIcon::Info, "Info"),
    (DmIcon::Chart, "Speed"),
    (DmIcon::Sliders, "Settings"),
    (DmIcon::Doc, "Log"),
];

/// Card-local rect (origin = the card's top-left, border included).
fn at(o: egui::Pos2, x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(o.x + x, o.y + y), egui::vec2(w, h))
}

/// One concept info row: dim 13px label at the gutter, value right on IN_R.
fn info_row(
    p: &egui::Painter,
    o: egui::Pos2,
    i: usize,
    label: &str,
    value: String,
    col: egui::Color32,
    font: egui::FontId,
) {
    let label = crate::i18n::t(label);
        let y = o.y + INFO_Y + ROW_STEP * i as f32;
        let lw = crate::shaped::width(p, label, &reg(13.0));
        crate::shaped::draw(
            p,
            egui::pos2(o.x + PAD_X, y),
            egui::Align2::LEFT_TOP,
            label,
            reg(13.0),
            pl().dm_lbl,
        );
        // The value can be a filename in any script, so it is drawn shaped.
        let v = elide_mid(p, &value, font.clone(), IN_R - PAD_X - lw - 12.0);
        crate::shaped::draw(
            p,
            egui::pos2(o.x + IN_R, y),
            egui::Align2::RIGHT_TOP,
            &v,
            font,
            col,
        );
    }

/// One compact label/value row (the concept's `.kv`): a dim label on the
/// gutter, the value right on the content edge. Denser than the Info tab's
/// rows, which stay exactly as they are.
fn kv_row(
    p: &egui::Painter,
    o: egui::Pos2,
    y: f32,
    label: &str,
    value: String,
    col: egui::Color32,
    font: egui::FontId,
) {
    let label = crate::i18n::t(label);
    let lf = reg(12.0);
    let lw = crate::shaped::width(p, label, &lf);
    crate::shaped::draw(
        p,
        egui::pos2(o.x + PAD_X, y),
        egui::Align2::LEFT_TOP,
        label,
        lf,
        pl().dm_lbl,
    );
    let v = elide_mid(p, &value, font.clone(), IN_R - PAD_X - lw - 12.0);
    crate::shaped::draw(
        p,
        egui::pos2(o.x + IN_R, y),
        egui::Align2::RIGHT_TOP,
        &v,
        font,
        col,
    );
}

struct ProgressApp {
    id: u64,
    /// Speed samples for the Speed tab graph (newest last).
    speed_hist: std::collections::VecDeque<f64>,
    /// Tail of this download's log (Log tab), re-read once a second.
    log_lines: Vec<String>,
    log_at: Instant,
    /// Set whenever the card's height changes; cleared once the window has been
    /// re-centred, so a growing part list never drifts off the middle.
    recenter: bool,
    last_poll: Instant,
    item: Option<DownloadItem>,
    snap: Snap,
    /// 0 Info, 1 Speed, 2 Settings — the concept's tabs.
    tab: u8,
    limit_input: String,
    limit_known: Option<u64>,
    /// The speed-limit field is in MB/s (else KB/s).
    limit_mb: bool,
    finish_sel: String,
    /// Last height sent to the OS, so the window is only resized on change.
    win_h: f32,
    /// Chevron state of the part section.
    parts_open: bool,
    /// The engine label the transfer started with, and when it was first
    /// seen: shown in Status for a few seconds.
    engine_seen: Option<(String, Instant)>,
    /// Part-table scroll offset in pixels (long part lists).
    scroll: f32,
    /// The transfer finished and the "Download complete" dialog took over:
    /// this window closes itself.
    handoff: bool,
    /// Theme key last applied: a switch in the main app repaints this window
    /// while it stays open (separate process, so ui.json is the channel).
    theme_seen: String,
    theme_check: Instant,
}

impl ProgressApp {
    fn refresh(&mut self) {
        let items = read_queue();
        self.item = items.into_iter().find(|d| d.id == self.id);
        self.snap = read_snap(self.id);
        if self.engine_seen.is_none()
            && (self.snap.phase == crate::engine::PHASE_APP_ENGINE || self.snap.phase == crate::engine::PHASE_YTDLP)
        {
            self.engine_seen = Some((self.snap.phase.clone(), Instant::now()));
        }
        // Speed graph: the last ~30 s (one sample per refresh).
        self.speed_hist.push_back(if self.snap.live { self.snap.speed.max(0.0) } else { 0.0 });
        while self.speed_hist.len() > 120 {
            self.speed_hist.pop_front();
        }
        if self.tab == 3 && self.log_at.elapsed() >= Duration::from_secs(1) {
            self.log_at = Instant::now();
            self.log_lines = crate::engine::job_log_path(&self.id.to_string())
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|t| t.lines().rev().take(13).map(str::to_string).collect::<Vec<_>>().into_iter().rev().collect())
                .unwrap_or_default();
        }
        // Queue file lags live counters: prefer the snapshot while active.
        // Keep displayed progress monotonic across retry legs (see app).
        if self.snap.live {
            if let Some(it) = self.item.as_mut() {
                // Only a *known* snapshot total may replace the row's size: the
                // helper reports none between the legs of a merged transfer, and
                // adopting that blank flipped the window to "Unknown".
                if let Some(t) = self.snap.total.filter(|t| *t > 0) {
                    if it.total_bytes != Some(t) {
                        it.total_bytes = Some(t);
                        it.downloaded_bytes = self.snap.downloaded;
                    }
                }
                it.downloaded_bytes = it.downloaded_bytes.max(self.snap.downloaded);
                // Never show more downloaded than the size (a retried range can
                // overshoot the engine's own total).
                if let Some(t) = it.total_bytes {
                    it.downloaded_bytes = it.downloaded_bytes.min(t);
                }
                it.speed_bps = self.snap.speed;
            }
        }
        self.last_poll = Instant::now();
        // A finished transfer hands over to the standalone "Download complete"
        // dialog, so this window has done its job (IDM closes it the same way).
        // With that dialog switched off it stays put, which keeps its
        // Open file / Open folder actions reachable.
        self.handoff = !self.snap.live
            && matches!(self.item.as_ref().map(|d| &d.status), Some(Status::Completed))
            && crate::model::ui_pref_bool("show_complete", true);
    }

    /// Close bulletproofing: viewport Close is async and has been observed
    /// to no-op; back it with a delayed process exit (harmless if the
    /// window already closed).
    fn close_soon(ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            std::process::exit(0);
        });
    }

    /// The user closed the window (its X, Close, Alt+F4): the view goes away,
    /// the transfer does not. Closing a detail window must never change what
    /// the download is doing — Pause and Cancel are the explicit actions for
    /// that. The automatic close after completion goes through `close_soon`
    /// directly.
    fn close_by_user(&self, ctx: &egui::Context) {
        if CLOSING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        Self::close_soon(ctx);
    }

    /// Progress as a 0..1 fraction: bytes of the size, or fragments of a
    /// stream that has no size yet (unknown and neither = 0).
    fn frac(&self, item: &DownloadItem) -> f32 {
        match (item.total_bytes, self.snap.fragments) {
            (Some(t), _) if t > 0 => (item.downloaded_bytes as f32 / t as f32).clamp(0.0, 1.0),
            (_, Some((i, n))) if self.snap.live => (i as f32 / n as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Time left from the fragment pace when the stream has no size: the
    /// bytes so far over the share of fragments done gives the whole.
    fn fragment_eta(&self, item: &DownloadItem) -> Option<String> {
        let (i, n) = self.snap.fragments?;
        if i == 0 || item.downloaded_bytes == 0 {
            return None;
        }
        let whole = item.downloaded_bytes as f64 * n as f64 / i as f64;
        Some(fmt_eta((whole as u64).saturating_sub(item.downloaded_bytes), item.speed_bps))
    }
}

/// Entry point for `--progress <id>`.
pub fn run() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let id: u64 = args
        .iter()
        .position(|a| a == "--progress")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("--progress needs <id>");
            std::process::exit(2);
        });
    // One window per download: a second start (Resume, Retry, a Redownload
    // while one is open) must not stack duplicates, while a window the user
    // closed must be replaceable straight away. Windows drops the claim the
    // moment this process ends, so nothing has to clean up after a crash.
    let Some(_slot) = crate::model::claim_window(&format!("progress_{id}")) else {
        return Ok(());
    };
    // Seed controls from shared settings (global limit + finish action).
    let (limit_input, finish_sel) = read_ui_prefs();
    let mut app = ProgressApp {
        speed_hist: Default::default(),
        log_lines: Vec::new(),
        log_at: Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now),
        id,
        recenter: true,
        last_poll: Instant::now() - Duration::from_secs(10),
        item: None,
        snap: Snap::default(),
        tab: 0,
        limit_input,
        limit_known: None,
        limit_mb: false,
        finish_sel,
        win_h: 0.0,
        parts_open: true,
        engine_seen: None,
        scroll: 0.0,
        handoff: false,
        theme_seen: String::new(),
        theme_check: Instant::now(),
    };
    app.refresh();
    let title = app
        .item
        .as_ref()
        .map(|d| d.filename.clone())
        .unwrap_or_else(|| format!("Download #{id}"));

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(crate::platform::app_id())
        .with_title(title)
        .with_inner_size([PG_W, PG_MIN_H])
        .with_resizable(false)
        .with_decorations(crate::platform::system_title_bar())
        .with_transparent(!crate::platform::system_title_bar())
        .with_always_on_top();
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Download Craft",
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            crate::platform::guard_gpu(cc);
            // `true`: this whole window is the concept, so Segoe UI also
            // becomes its default proportional face.
            crate::fonts::install(cc, true);
            // Standalone process: follow the theme saved in `ui.json`.
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for ProgressApp {
    /// Transparent and frameless: the concept's rounded card is painted by
    /// hand, so the surface behind it must stay clear.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(text) = crate::i18n::take_font_text() {
            crate::fonts::ensure_text(ctx, &text);
        }
        if let Some(it) = &self.item {
            crate::fonts::ensure_text(ctx, &it.filename);
        }
        // Alt+F4 / the taskbar's Close: same as the window's own X.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.handoff
            && !CLOSING.load(std::sync::atomic::Ordering::SeqCst)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_by_user(ctx);
        }
        // Theme switches made in the main app while this window is open: the
        // process only read ui.json at start-up, so poll the resolved key
        // (which also catches OS flips under "system").
        if self.theme_check.elapsed() >= Duration::from_secs(1) {
            self.theme_check = Instant::now();
            let theme = crate::app::stored_theme();
            let key = format!("{}:{}", theme, crate::app::theme_is_dark(ctx, &theme));
            if key != self.theme_seen {
                self.theme_seen = key;
                crate::app::apply_theme(ctx, &theme);
            }
        }
        // Fast enough that the speed readout visibly moves while a transfer
        // runs: the app republishes progress.json about ten times a second.
        if self.last_poll.elapsed() >= Duration::from_millis(250) {
            self.refresh();
        }
        if self.handoff {
            Self::close_soon(ctx);
        }
        // The app republishes progress ~4 times a second: no point drawing
        // more often than that.
        ctx.request_repaint_after(Duration::from_millis(250));

        // The card follows the concept's own rules: one 25px row per part,
        // everything else fixed. A monitor-sized cap keeps a 32-connection
        // list usable — the table scrolls instead of leaving the screen.
        //
        // Every tab carries the part section (its header row holds the state
        // actions), so the height no longer depends on the tab.
        // A fragmented stream shows one row (see `fragment_parts`).
        let parts = self.snap.parts.len().max(self.snap.fragments.map_or(0, |_| 1));
        let want_h = match &self.item {
            None => PG_DEAD_H,
            // The header row is always there: it carries the state actions
            // (Pause/Resume/Cancel, and Open file/folder + Close once the
            // transfer is done), so a finished download is not a dead end
            // even though the engine drops the part snapshot at the end.
            // Six rows at most: more connections scroll inside the table
            // instead of making the window taller.
            Some(_) if parts > 0 && self.parts_open => card_h(parts.min(MAX_PART_ROWS)),
            Some(_) => PG_PART_HEAD_H,
        };
        let want_h = match crate::model::work_area_rect(ctx) {
            Some(w) => want_h.min((w.height() - 40.0).max(PG_MIN_H)),
            None => want_h,
        };
        if (self.win_h - want_h).abs() > 0.5 {
            self.win_h = want_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(PG_W, want_h)));
            // The card grows as the part rows appear: re-centre on every size
            // change so the whole window stays in the middle of the screen
            // (taskbar excluded) instead of sliding behind it.
            self.recenter = true;
        }
        if self.recenter && crate::model::center_window(ctx, egui::vec2(PG_W, want_h)) {
            self.recenter = false;
        }

        let item = self.item.clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let o = ui.max_rect().min;
                match item.as_ref() {
                    Some(it) => self.paint(ui, it, o),
                    None => self.paint_dead(ui, o),
                }
            });
    }
}

impl ProgressApp {
    /// Card background, title bar and the tab strip — every state shares it.
    fn chrome(&mut self, ui: &mut egui::Ui, o: egui::Pos2, title: &str) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        p.rect(
            ui.max_rect(),
            8.0,
            pl().dm_card,
            egui::Stroke::new(1.0, pl().dm_line),
            egui::StrokeKind::Inside,
        );
        dm_mark(&p, at(o, MARK_X, MARK_Y, MARK_S, MARK_S));
        // The title carries the item's name, so it goes through the shaper too.
        crate::shaped::draw(
            &p,
            egui::pos2(o.x + TITLE_X, o.y + HDR_CY),
            egui::Align2::LEFT_CENTER,
            title,
            semi(14.0),
            pl().dm_txt,
        );
        // The bar drags the window, like the capture popup's header.
        if ui
            .interact(
                at(o, 0.0, 0.0, PG_W - 100.0, HDR_H),
                egui::Id::new("pg-header"),
                egui::Sense::click_and_drag(),
            )
            .drag_started()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        for (i, cx, icon, id, tip) in [
            (0usize, CTL_X1, DmIcon::Minimize, "pg-min", "Minimise"),
            (1, CTL_X2, DmIcon::Close, "pg-close", "Close"),
        ] {
            let r = egui::Rect::from_center_size(
                egui::pos2(o.x + cx, o.y + HDR_CY),
                egui::Vec2::splat(14.0),
            );
            let b = ui
                .interact(r.expand(5.0), egui::Id::new(id), egui::Sense::click())
                .on_hover_text(tip);
            if b.hovered() {
                p.rect_filled(r.expand(5.0), DM_RAD, pl().dm_hover2);
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            dm_icon(&p, r, icon, if b.hovered() { pl().dm_txt } else { pl().dm_dim }, 2.0);
            if b.clicked() {
                if i == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                } else {
                    self.close_by_user(&ctx);
                }
            }
        }
    }

    /// The concept's segmented control: an inset trough with a hairline
    /// border, and the three tabs as separate pills inside it (the active one
    /// is the accent pill, the others are plain until hovered).
    fn tabs(&mut self, ui: &mut egui::Ui, o: egui::Pos2) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        // No trough: the tabs sit on the window's own background, with a
        // hairline under the row and an accent underline under the open tab.
        let base_y = o.y + SEG_Y + SEG_H;
        p.line_segment(
            [egui::pos2(o.x + 1.0, base_y - 0.5), egui::pos2(o.x + PG_W - 1.0, base_y - 0.5)],
            egui::Stroke::new(1.0_f32, pl().dm_line),
        );
        let inner = SEG_W - 2.0 * SEG_PAD;
        let tw = (inner - TAB_GAP * (TABS.len() as f32 - 1.0)) / TABS.len() as f32;
        for (i, (icon, name)) in TABS.iter().enumerate() {
            let x = SEG_X + SEG_PAD + i as f32 * (tw + TAB_GAP);
            let r = at(o, x, SEG_Y + SEG_PAD, tw, TAB_H);
            let b = ui.interact(r, egui::Id::new(("pg-tab", i)), egui::Sense::click());
            let sel = self.tab as usize == i;
            if sel {
                p.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(r.min.x + 6.0, base_y - 2.0), egui::pos2(r.max.x - 6.0, base_y)),
                    1.0,
                    DM_ACC2(),
                );
            }
            if b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let col = if sel { pl().dm_txt } else if b.hovered() { pl().dm_txt.gamma_multiply(0.8) } else { pl().dm_lbl };
            let font = if sel { semi(11.0) } else { reg(11.0) };
            let gw = p
                .layout_no_wrap((*name).to_string(), font.clone(), col)
                .size()
                .x;
            let x0 = r.center().x - (TAB_ICON + 5.0 + gw) / 2.0;
            dm_icon(
                &p,
                egui::Rect::from_center_size(
                    egui::pos2(x0 + TAB_ICON / 2.0, r.center().y),
                    egui::Vec2::splat(TAB_ICON),
                ),
                *icon,
                col,
                1.8,
            );
            p.text(
                egui::pos2(x0 + TAB_ICON + 5.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                (*name).to_string(),
                font,
                col,
            );
            if b.clicked() {
                self.tab = i as u8;
            }
        }
    }

    /// The card for a live queue item: chrome, tabs, the active tab's body,
    /// the progress bar and the part section. The part section belongs to the
    /// card, not to the Info tab: every tab keeps its collapse toggle and the
    /// state actions.
    fn paint(&mut self, ui: &mut egui::Ui, item: &DownloadItem, o: egui::Pos2) {
        let p = ui.painter().clone();
        let frac = self.frac(item);
        let pct = (frac * 100.0).floor() as u64;
        // IDM's title: "<pct>%-<name>" (the design shows 39%-Stories-…).
        let raw_title = if item.total_bytes.unwrap_or(0) > 0 || frac > 0.0 {
            format!("{pct}%-{}", item.filename)
        } else {
            item.filename.clone()
        };
        let title = elide_mid(&p, &raw_title, semi(14.0), CTL_X1 - 34.0 - TITLE_X);
        self.chrome(ui, o, &title);
        self.tabs(ui, o);
        let downloading = self.snap.live
            && matches!(item.status, Status::Downloading | Status::Queued);
        match self.tab {
            1 => self.body_speed(ui, o, item, downloading),
            2 => self.body_settings(ui, o),
            3 => self.body_log(ui, o),
            _ => self.body_info(ui, o, item, downloading),
        }
        self.progress_bar(ui, o, item, frac);
        self.parts(ui, o, item, downloading);
    }

    /// The item left the queue (removed or cancelled elsewhere).
    fn paint_dead(&mut self, ui: &mut egui::Ui, o: egui::Pos2) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let title = format!("Download #{}", self.id);
        self.chrome(ui, o, &title);
        p.text(
            egui::pos2(o.x + PAD_X, o.y + 66.0),
            egui::Align2::LEFT_TOP,
            "No longer in the queue.",
            reg(13.0),
            pl().dm_dim,
        );
        let r = at(o, PAD_X, 96.0, 92.0, 32.0);
        let b = ui.interact(r, egui::Id::new("pg-dead-close"), egui::Sense::click());
        btn_flat(&p, r, b.hovered(), "Close");
        if b.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if b.clicked() {
            Self::close_soon(&ctx);
        }
    }

    /// True while a helper is in a stage that moves no bytes yet (YouTube's
    /// player challenge, a merge). The size may well be known already — the
    /// extension measured it before the transfer started — so "nothing has
    /// arrived" is the signal, not "size unknown".
    fn preparing(&self) -> bool {
        self.snap.live
            && self.snap.downloaded == 0
            && !self.snap.phase.is_empty()
            && !crate::engine::phase_is_downloading(&self.snap.phase)
    }

    /// The stage a live transfer is in before its first byte, worded for the
    /// Status row on its own ("Reading YouTube", "Waiting 5s for YouTube",
    /// "yt-dlp fallback — getting info"), or "Getting info" while nothing
    /// named one yet. `None` once bytes move: the status label says it.
    fn stage_text(&self) -> Option<String> {
        if !self.snap.live {
            return None;
        }
        // Which engine downloads, for its first seconds: green for the app's
        // own, blue for yt-dlp.
        if let Some((label, at)) = &self.engine_seen {
            if at.elapsed() < std::time::Duration::from_secs(4) {
                return Some(label.clone());
            }
        }
        if self.snap.downloaded > 0 {
            return None;
        }
        Some(match self.snap.phase.as_str() {
            "" | "Downloading" => "Getting info".to_string(),
            p if crate::engine::phase_is_downloading(p) => "Getting info".to_string(),
            p => p.to_string(),
        })
    }

    /// The Status row's colour for an engine label (`None` = the usual).
    fn stage_color(&self, text: &str) -> Option<egui::Color32> {
        match text {
            crate::engine::PHASE_APP_ENGINE => Some(egui::Color32::from_rgb(0x34, 0xd3, 0x99)),
            crate::engine::PHASE_YTDLP => Some(egui::Color32::from_rgb(0x60, 0xa5, 0xfa)),
            _ => None,
        }
    }

    /// Concept Info tab: the detail rows, laid out in order. The resolution row
    /// is the one that comes and goes: a ZIP, a PDF or a photo has no frame
    /// size, and a "Resolution: -" row on a file that has no frames at all
    /// reads as a bug rather than as information.
    fn body_info(&mut self, ui: &mut egui::Ui, o: egui::Pos2, item: &DownloadItem, downloading: bool) {
        let p = ui.painter().clone();
        let status_col = match &item.status {
            Status::Completed => DM_OK,
            Status::Failed(_) => pl().dm_warn,
            Status::Paused | Status::Queued => DM_YEL,
            _ => pl().dm_txt,
        };
        // Rows are placed by their position, so the ones below a skipped row
        // move up instead of leaving a hole.
        let mut i = 0usize;
        let mut row = |label: &str, value: String, col: egui::Color32, font: egui::FontId| {
            let at = i;
            i += 1;
            info_row(&p, o, at, label, value, col, font);
        };
        row("Name:", item.filename.clone(), pl().dm_txt, reg(13.0));
        // A helper in a stage that moves no bytes yet (YouTube's player
        // challenge, a merge) is named here: "Downloading" with 0 of "-" and
        // no speed reads as a hang, and this can last ten seconds.
        let stage = if downloading { self.stage_text() } else { None };
        let stage_col = stage.as_deref().and_then(|s| self.stage_color(s)).unwrap_or(status_col);
        row(
            "Status:",
            stage.unwrap_or_else(|| item.status.label()),
            stage_col,
            reg(13.0),
        );
        // Live value while a video is arriving, the recorded one otherwise.
        // Empty means the file has no frames (or none ffprobe could read), and
        // then the row is left out entirely.
        let resolution = if !self.snap.resolution.is_empty() {
            self.snap.resolution.clone()
        } else {
            item.resolution.clone()
        };
        if !resolution.is_empty() {
            row("Resolution:", resolution, pl().dm_txt, reg(13.0));
        }
        // A stream has no size until it is done: IDM says so and shows the
        // duration instead of a number that changes with every fragment.
        let size = match item.total_bytes {
            Some(t) => format_bytes(t),
            None if item.duration_secs > 0 => format!("Unknown ({})", fmt_duration(item.duration_secs)),
            None if self.snap.fragments.is_some() => "Unknown (stream)".into(),
            None => "-".into(),
        };
        row("Size:", size, pl().dm_txt, reg(13.0));
        row(
            "Downloaded:",
            format_bytes(item.downloaded_bytes),
            pl().dm_txt,
            reg(13.0),
        );
        row(
            "Speed:",
            if downloading && item.speed_bps > 0.0 {
                format!("{}/s", format_bytes(item.speed_bps as u64))
            } else {
                "-".into()
            },
            pl().dm_txt,
            reg(13.0),
        );
        row(
            "Remaining Time:",
            match item.total_bytes {
                _ if matches!(item.status, Status::Paused) => "Paused".into(),
                Some(t) if downloading => fmt_eta(t.saturating_sub(item.downloaded_bytes), item.speed_bps),
                None if downloading => self.fragment_eta(item).unwrap_or_else(|| "-".into()),
                _ => "-".into(),
            },
            pl().dm_txt,
            reg(13.0),
        );
        let res = item.resumable || self.snap.resumable;
        // The server's place and distance share this line (src/geo.rs).
        let yes_no = if res { "Yes" } else { "No" };
        let server = self.snap.server.trim();
        row(
            if server.is_empty() { "Resume Support:" } else { "Resume · Server:" },
            if server.is_empty() { yes_no.into() } else { format!("{yes_no}  ·  {server}") },
            if res { DM_OK } else { pl().dm_lbl },
            if res { semi(13.0) } else { reg(13.0) },
        );
        // The status row carries the full server error as hover text (the
        // design's single line can only hold the short form).
        if let Status::Failed(e) = &item.status {
            ui.interact(
                at(o, PAD_X, INFO_Y + ROW_STEP, IN_R - PAD_X, ROW_H),
                egui::Id::new("pg-status"),
                egui::Sense::hover(),
            )
            .on_hover_text(e.clone());
        }
    }

    /// Concept Speed tab: thread count, live rate and the global cap.
    ///
    /// The rows keep the right edge of the Info tab but sit on the concept's
    /// denser rhythm; below them the cap gets a compact field and a compact
    /// accent button, with the two hint lines that explain it.
    fn body_speed(&mut self, ui: &mut egui::Ui, o: egui::Pos2, item: &DownloadItem, downloading: bool) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let live_conns = self.snap.parts.iter().filter(|(s, e, d)| s + d <= *e).count();
        kv_row(
            &p,
            o,
            84.0,
            "Connections:",
            if downloading && live_conns > 0 {
                format!("up to {}, {} parts left (the engine adapts)", item.connections, live_conns)
            } else {
                format!("up to {}", item.connections)
            },
            pl().dm_txt,
            reg(12.0),
        );
        kv_row(
            &p,
            o,
            111.0,
            "Current Speed:",
            if downloading && item.speed_bps > 0.0 {
                format!("{}/s", format_bytes(item.speed_bps as u64))
            } else {
                "-".into()
            },
            pl().dm_txt,
            reg(12.0),
        );

        // ---- speed limit: one row — label, number, unit, Apply ----
        let row_y = 136.0;
        p.text(
            egui::pos2(o.x + PAD_X, o.y + row_y + 15.0),
            egui::Align2::LEFT_CENTER,
            "This download:",
            reg(12.0),
            pl().dm_lbl,
        );
        let field = at(o, PAD_X + 104.0, row_y, 132.0, 30.0);
        dm_field(ui, "pg_limit", field, field.max.x - 10.0, &mut self.limit_input, "No limit", false);
        // KB/s | MB/s, so the number means what it says.
        let seg = at(o, PAD_X + 244.0, row_y, 104.0, 30.0);
        p.rect_filled(seg, DM_RAD, pl().dm_line);
        for (k, label) in ["KB/s", "MB/s"].iter().enumerate() {
            let r = egui::Rect::from_min_size(egui::pos2(seg.min.x + 3.0 + k as f32 * 49.0, seg.min.y + 3.0), egui::vec2(49.0, 24.0));
            let on = self.limit_mb == (k == 1);
            let resp = ui.interact(r, egui::Id::new(("pg-unit", k)), egui::Sense::click());
            if on {
                p.rect_filled(r, DM_RAD - 1.0, DM_ACC().gamma_multiply(0.9));
            } else if resp.hovered() {
                p.rect_filled(r, DM_RAD - 1.0, pl().dm_hover2);
            }
            p.text(r.center(), egui::Align2::CENTER_CENTER, *label, semi(11.0), if on { egui::Color32::WHITE } else { pl().dm_dim });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.limit_mb = k == 1;
            }
        }
        let apply = at(o, IN_R - 76.0, row_y, 76.0, 30.0);
        let ab = ui.interact(apply, egui::Id::new("pg-apply"), egui::Sense::click());
        dm_gradient(&p, apply, DM_RAD, DM_ACC(), DM_ACC2());
        p.text(apply.center(), egui::Align2::CENTER_CENTER, "Apply", semi(12.0), egui::Color32::WHITE);
        if ab.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if ab.clicked() {
            // Empty or 0 = no limit; decimals allowed ("1.5" MB/s).
            let txt = self.limit_input.trim().replace(',', ".");
            let v: f64 = if txt.is_empty() { 0.0 } else { txt.parse().unwrap_or(-1.0) };
            if v >= 0.0 {
                let kbps = (v * if self.limit_mb { 1024.0 } else { 1.0 }).round() as u64;
                send_cmd_limit(self.id, kbps);
                self.limit_known = Some(kbps);
            } else {
                self.limit_known = None;
            }
        }
        let fmt_cap = |k: u64| {
            if k >= 1024 {
                format!("{:.1} MB/s", k as f64 / 1024.0).replace(".0 MB", " MB")
            } else {
                format!("{k} KB/s")
            }
        };
        let note = match self.limit_known {
            Some(0) => "No limit — downloads use the full speed.".to_string(),
            Some(k) => format!("Every download is limited to {} in all (applied now).", fmt_cap(k)),
            None => "Limits every download in all · empty or 0 = no limit.".to_string(),
        };
        p.text(egui::pos2(o.x + PAD_X, o.y + row_y + 40.0), egui::Align2::LEFT_CENTER, note, reg(10.5), pl().dm_dim);

        // ---- speed over the last half minute ----
        let g = at(o, PAD_X, 192.0, PROG_W, 66.0);
        p.rect_filled(g, 6.0, pl().dm_trough);
        p.rect_stroke(g, 6.0, egui::Stroke::new(1.0, pl().dm_line), egui::StrokeKind::Inside);
        let plot = egui::Rect::from_min_max(egui::pos2(g.left() + 8.0, g.top() + 18.0), egui::pos2(g.right() - 8.0, g.bottom() - 6.0));
        for k in 1..=2 {
            let y = plot.top() + plot.height() * k as f32 / 3.0;
            p.hline(plot.x_range(), y, egui::Stroke::new(1.0, pl().dm_line.gamma_multiply(0.7)));
        }
        p.text(egui::pos2(g.left() + 8.0, g.top() + 9.0), egui::Align2::LEFT_CENTER, "Speed · last 30 s", reg(9.5), pl().dm_dim);
        let peak = self.speed_hist.iter().cloned().fold(0.0_f64, f64::max);
        if peak > 0.0 && self.speed_hist.len() > 1 {
            let n = self.speed_hist.len();
            let top = peak * 1.1;
            let pts: Vec<egui::Pos2> = self
                .speed_hist
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    egui::pos2(
                        plot.left() + plot.width() * i as f32 / (n - 1) as f32,
                        plot.bottom() - plot.height() * (*v / top) as f32,
                    )
                })
                .collect();
            // A soft fill under the line: one quad (two triangles) per step.
            let mut mesh = egui::Mesh::default();
            let fill_top = DM_ACC().gamma_multiply(0.32);
            let fill_bot = DM_ACC().gamma_multiply(0.02);
            for w in pts.windows(2) {
                let base = mesh.vertices.len() as u32;
                mesh.colored_vertex(w[0], fill_top);
                mesh.colored_vertex(w[1], fill_top);
                mesh.colored_vertex(egui::pos2(w[1].x, plot.bottom()), fill_bot);
                mesh.colored_vertex(egui::pos2(w[0].x, plot.bottom()), fill_bot);
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
            p.add(egui::Shape::mesh(mesh));
            p.add(egui::Shape::line(pts.clone(), egui::Stroke::new(1.8, DM_ACC())));
            if let Some(last) = pts.last() {
                p.circle_filled(*last, 3.5, DM_ACC());
                p.circle_stroke(*last, 5.5, egui::Stroke::new(1.0, DM_ACC().gamma_multiply(0.4)));
            }
            let now = self.speed_hist.back().copied().unwrap_or(0.0);
            p.text(
                egui::pos2(g.right() - 8.0, g.top() + 9.0),
                egui::Align2::RIGHT_CENTER,
                format!("now {}/s  ·  peak {}/s", format_bytes(now as u64), format_bytes(peak as u64)),
                reg(9.5),
                pl().dm_txt,
            );
        } else {
            p.text(plot.center(), egui::Align2::CENTER_CENTER, "The graph appears while it downloads", reg(10.0), pl().dm_dim);
        }
    }

    /// The engine's log for this download: connections, retries, fallbacks.
    fn body_log(&mut self, ui: &mut egui::Ui, o: egui::Pos2) {
        let p = ui.painter().clone();
        if self.log_lines.is_empty() {
            p.text(
                egui::pos2(o.x + PAD_X, o.y + 86.0),
                egui::Align2::LEFT_TOP,
                "Nothing logged yet. Each start, connection, retry and fallback of this download shows up here.",
                reg(10.5),
                pl().dm_dim,
            );
            return;
        }
        let font = egui::FontId::monospace(10.0);
        for (i, line) in self.log_lines.iter().enumerate() {
            let shown = elide_mid(&p, line, font.clone(), PROG_W);
            p.text(
                egui::pos2(o.x + PAD_X, o.y + 80.0 + i as f32 * 13.5),
                egui::Align2::LEFT_TOP,
                shown,
                font.clone(),
                pl().dm_txt,
            );
        }
    }

    /// Concept Settings tab: what happens when the queue drains.
    ///
    /// The four choices are cards in a two-column grid (radio, icon, label);
    /// the picked one is outlined in the accent and tinted with it. The block
    /// sits in the middle of the pane, like the concept's centred pane.
    fn body_settings(&mut self, ui: &mut egui::Ui, o: egui::Pos2) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        // The block sits in the middle of the pane, like the concept's
        // centred pane: equal air above and below, no dead band.
        let y0 = 104.0;
        p.text(
            egui::pos2(o.x + PAD_X, o.y + y0),
            egui::Align2::LEFT_TOP,
            "When the queue drains:",
            reg(11.0),
            pl().dm_lbl,
        );
        p.text(
            egui::pos2(o.x + PAD_X, o.y + y0 + 19.0),
            egui::Align2::LEFT_TOP,
            "Choose what Download Craft does once the last item in the queue finishes.",
            reg(10.0),
            pl().dm_dim,
        );
        // value, label, Material glyph
        const OPTS: [(&str, &str, &str); 4] = [
            ("nothing", "Nothing", "\u{e15d}"),
            ("shutdown", "Shut down", "\u{e8ac}"),
            ("sleep", "Sleep", "\u{ef44}"),
            ("hibernate", "Hibernate", "\u{eb3b}"),
        ];
        let grid_y = o.y + y0 + 38.0;
        let cw = (PROG_W - 6.0) / 2.0;
        for (i, (val, label, glyph)) in OPTS.iter().enumerate() {
            let col = (i % 2) as f32;
            let row = (i / 2) as f32;
            let r = egui::Rect::from_min_size(
                egui::pos2(o.x + PAD_X + col * (cw + 6.0), grid_y + row * 39.0),
                egui::vec2(cw, 33.0),
            );
            let b = ui.interact(r, egui::Id::new(("pg-fin", i)), egui::Sense::click());
            let sel = self.finish_sel == *val;
            p.rect_filled(
                r,
                7.0,
                if sel {
                    pl().dm_line
                } else if b.hovered() {
                    pl().dm_hover2
                } else {
                    pl().dm_line
                },
            );
            if sel {
                // The accent tint sits on top of the card fill, so the card
                // keeps its own surface under the colour.
                p.rect_filled(
                    r,
                    7.0,
                    egui::Color32::from_rgba_unmultiplied(DM_ACC().r(), DM_ACC().g(), DM_ACC().b(), 42),
                );
            }
            p.rect_stroke(
                r,
                7.0,
                egui::Stroke::new(1.0_f32, if sel { DM_ACC() } else { pl().dm_box_bd }),
                egui::StrokeKind::Inside,
            );
            if b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            // radio
            let rc = egui::pos2(r.min.x + 15.0, r.center().y);
            if sel {
                p.circle_filled(rc, 6.0, DM_ACC());
                p.circle_filled(rc, 2.2, egui::Color32::WHITE);
            } else {
                p.circle_stroke(rc, 6.0, egui::Stroke::new(1.0_f32, pl().dm_box_bd));
            }
            // icon + label
            let icol = if sel { DM_ACC() } else { pl().dm_dim };
            p.text(
                egui::pos2(r.min.x + 28.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                (*glyph).to_string(),
                crate::fonts::icon(13.0),
                icol,
            );
            p.text(
                egui::pos2(r.min.x + 48.0, r.center().y),
                egui::Align2::LEFT_CENTER,
                (*label).to_string(),
                if sel { semi(11.0) } else { reg(11.0) },
                if sel { pl().dm_txt } else { pl().dm_dim },
            );
            if b.clicked() {
                self.finish_sel = (*val).to_string();
                send_cmd_kv("finish", "value", serde_json::Value::from(*val));
            }
        }
        p.text(
            egui::pos2(o.x + PAD_X, o.y + y0 + 123.0),
            egui::Align2::LEFT_TOP,
            "Only applies when the queue runs out of items — active downloads are not interrupted.",
            reg(10.0),
            pl().dm_lbl,
        );
    }

    /// The accent progress bar (every tab keeps it: it is the "still
    /// running" signal), with the exact numbers as hover text.
    fn progress_bar(&mut self, ui: &mut egui::Ui, o: egui::Pos2, item: &DownloadItem, frac: f32) {
        let p = ui.painter().clone();
        let track = at(o, PAD_X, PROG_Y, PROG_W, PROG_H);
        p.rect_filled(track, PROG_H / 2.0, pl().dm_line);
        // Before a single byte has arrived the bar sweeps a segment instead of
        // sitting empty: yt-dlp resolves YouTube's player challenge for several
        // seconds first, and a frozen bar is indistinguishable from a stalled
        // download. (The size is often already known from the browser probe, so
        // "no bytes yet" — not "no size" — is the signal.)
        let preparing = self.preparing();
        if preparing {
            let t = ui.input(|i| i.time) as f32;
            let span = PROG_W * 0.32;
            // ~2.5s per sweep: detectable without being a strobe at the
            // window's 150ms repaint cadence.
            let left = (t * 0.4).fract() * (PROG_W + span) - span;
            let l = (track.min.x + left).max(track.min.x);
            let r = (track.min.x + left + span).min(track.max.x);
            if r - l > 1.0 {
                dm_gradient_x(
                    &p,
                    egui::Rect::from_min_size(egui::pos2(l, track.min.y), egui::vec2(r - l, PROG_H)),
                    PROG_H / 2.0,
                    DM_ACC(),
                    DM_ACC2(),
                );
            }
        } else if let Some(parts) = self
            .part_spans(item)
            .filter(|_| crate::model::ui_pref_bool("segmented_progress_bar", false))
        {
            // Optional (Settings -> Progress windows), off by default: the
            // Part Info strip below always shows the parts. Part by part, as IDM draws it (issue #10): each connection's
            // bytes fill its own stretch of the bar, so every part moving at
            // once is visible, and a part that finished stands complete.
            let live = self.snap.live && !matches!(item.status, Status::Paused | Status::Completed | Status::Failed(_));
            let t = ui.input(|i| i.time) as f32;
            for (i, (x0, x1, f)) in parts.iter().enumerate() {
                let l = track.min.x + x0 * PROG_W;
                let r = track.min.x + x1 * PROG_W;
                if i > 0 {
                    p.line_segment(
                        [egui::pos2(l, track.min.y + 1.0), egui::pos2(l, track.max.y - 1.0)],
                        egui::Stroke::new(1.0, pl().dm_trough),
                    );
                }
                let w = (r - l) * f;
                if w < 0.5 {
                    continue;
                }
                let fill = egui::Rect::from_min_size(egui::pos2(l, track.min.y), egui::vec2(w, PROG_H));
                dm_gradient_x(&p, fill, 2.0, DM_ACC(), DM_ACC2());
                if live && *f < 1.0 {
                    let clip = p.with_clip_rect(fill.intersect(p.clip_rect()));
                    let step = 12.0;
                    let off = (t * 28.0) % step;
                    let mut x = fill.min.x - PROG_H - step + off;
                    while x < fill.max.x {
                        clip.add(egui::Shape::convex_polygon(
                            vec![
                                egui::pos2(x, fill.max.y),
                                egui::pos2(x + step / 2.0, fill.max.y),
                                egui::pos2(x + step / 2.0 + PROG_H, fill.min.y),
                                egui::pos2(x + PROG_H, fill.min.y),
                            ],
                            egui::Color32::from_white_alpha(34),
                            egui::Stroke::NONE,
                        ));
                        x += step;
                    }
                }
            }
            if live {
                ui.ctx().request_repaint_after(Duration::from_millis(40));
            }
        } else if frac > 0.0 {
            let w = (PROG_W * frac).max(PROG_H);
            let fill = egui::Rect::from_min_size(track.min, egui::vec2(w, PROG_H));
            let live = self.snap.live && !matches!(item.status, Status::Paused | Status::Completed | Status::Failed(_));
            if live {
                // Soft glow under the filled part.
                p.rect_filled(fill.expand(2.0), PROG_H / 2.0 + 2.0, DM_ACC2().gamma_multiply(0.18));
            }
            dm_gradient_x(&p, fill, PROG_H / 2.0, DM_ACC(), DM_ACC2());
            if live {
                // Diagonal stripes that keep moving and a light sheen that
                // sweeps across: the bar reads as busy even when the
                // percentage creeps.
                let t = ui.input(|i| i.time) as f32;
                let clip = p.with_clip_rect(fill.intersect(p.clip_rect()));
                let step = 12.0;
                let off = (t * 28.0) % step;
                let mut x = fill.min.x - PROG_H - step + off;
                while x < fill.max.x {
                    clip.add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(x, fill.max.y),
                            egui::pos2(x + step / 2.0, fill.max.y),
                            egui::pos2(x + step / 2.0 + PROG_H, fill.min.y),
                            egui::pos2(x + PROG_H, fill.min.y),
                        ],
                        egui::Color32::from_white_alpha(34),
                        egui::Stroke::NONE,
                    ));
                    x += step;
                }
                let sw = (w * 0.35).clamp(24.0, 120.0);
                let sx = fill.min.x - sw + ((t / 1.6).fract()) * (w + sw * 2.0);
                sheen(&clip, egui::Rect::from_min_size(egui::pos2(sx, fill.min.y), egui::vec2(sw, PROG_H)));
                ui.ctx().request_repaint_after(Duration::from_millis(40));
            }
        }
        ui.interact(
            egui::Rect::from_min_max(
                track.min - egui::vec2(0.0, 6.0),
                track.max + egui::vec2(0.0, 8.0),
            ),
            egui::Id::new("pg-bar"),
            egui::Sense::hover(),
        )
        .on_hover_text(if preparing && !self.snap.phase.is_empty() {
            format!("{} — no data on the wire yet", self.snap.phase)
        } else {
            format!(
                "{} of {} ({:.0}%)",
                format_bytes(item.downloaded_bytes),
                item.total_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "unknown size".into()),
                frac * 100.0
            )
        });
    }

    /// The bar's parts as (start, end, filled), each a fraction of the file:
    /// `None` for a single-stream download, which keeps the one filling bar.
    fn part_spans(&self, item: &DownloadItem) -> Option<Vec<(f32, f32, f32)>> {
        if self.snap.parts.len() < 2 || matches!(item.status, Status::Completed) {
            return None;
        }
        let total = self
            .snap
            .parts
            .iter()
            .map(|(_, e, _)| e + 1)
            .max()
            .unwrap_or(0)
            .max(item.total_bytes.unwrap_or(0));
        if total == 0 {
            return None;
        }
        let tf = total as f32;
        Some(
            self.snap
                .parts
                .iter()
                .map(|(s, e, d)| {
                    let len = (e + 1).saturating_sub(*s).max(1);
                    (*s as f32 / tf, (e + 1) as f32 / tf, (*d as f32 / len as f32).clamp(0.0, 1.0))
                })
                .collect(),
        )
    }

    /// Part Info section: rule, header row with the state actions, the part
    /// strip and the per-part table (scrolled when it outgrows the card).
    fn parts(&mut self, ui: &mut egui::Ui, o: egui::Pos2, item: &DownloadItem, downloading: bool) {
        let ctx = ui.ctx().clone();
        let p = ui.painter().clone();
        let done = matches!(item.status, Status::Completed);
        let failed = matches!(item.status, Status::Failed(_));
        let paused = matches!(item.status, Status::Paused);
        // A live transfer always has parts; a finished/failed one has none
        // (the engine drops the snapshot), which must not cost the header row
        // its actions.
        // A helper fetching a fragmented stream has no byte ranges, only a
        // fragment count: that is its part list (IDM shows one too).
        let frags = self.snap.fragments.filter(|_| self.snap.parts.is_empty());
        let has_parts = !self.snap.parts.is_empty() || frags.is_some();
        p.line_segment(
            [
                egui::pos2(o.x + 1.0, o.y + PART_Y + 0.5),
                egui::pos2(o.x + PG_W - 1.0, o.y + PART_Y + 0.5),
            ],
            egui::Stroke::new(1.0, pl().dm_line),
        );
        let chev = egui::Rect::from_center_size(
            egui::pos2(o.x + 25.0, o.y + HEAD_Y + HEAD_H / 2.0),
            egui::Vec2::splat(12.0),
        );
        let cb = ui
            .interact(chev.expand(6.0), egui::Id::new("pg-chev"), egui::Sense::click());
        let cb = if has_parts {
            cb.on_hover_text(if self.parts_open {
                "Hide the part list"
            } else {
                "Show the part list"
            })
        } else {
            cb
        };
        if cb.hovered() && has_parts {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        dm_icon(
            &p,
            chev,
            if self.parts_open {
                DmIcon::ChevronUp
            } else {
                DmIcon::Chevron
            },
            if cb.hovered() { pl().dm_txt } else { pl().dm_dim },
            2.0,
        );
        p.text(
            egui::pos2(o.x + 37.0, o.y + HEAD_Y + HEAD_H / 2.0),
            egui::Align2::LEFT_CENTER,
            "Part Info",
            reg(12.0),
            pl().dm_dim,
        );
        if cb.clicked() && has_parts {
            self.parts_open = !self.parts_open;
        }
        // Right cluster, right-aligned on the content edge: the concept's
        // [Pause] [Close], swapping in whatever this state can actually do.
        let actions: Vec<(Option<DmIcon>, &str, &str)> = if done {
            vec![
                (Some(DmIcon::Doc), "Open file", "file"),
                (Some(DmIcon::Folder), "Open folder", "dir"),
                (None, "Close", "close"),
            ]
        } else if failed {
            vec![
                (Some(DmIcon::Folder), "Open folder", "dir"),
                (None, "Close", "close"),
            ]
        } else if paused {
            // The row is already stopped: Cancel just puts the window away
            // (the title bar's × does the same).
            vec![
                (Some(DmIcon::Play), "Resume", "resume"),
                (None, "Cancel", "close"),
            ]
        } else if !downloading {
            vec![(None, "Cancel", "cancel")]
        } else {
            // Pause and Cancel only, as IDM: Cancel stops the transfer (the
            // partial file is kept, so it can be resumed from the list) and
            // closes the window, which made a separate Close the same button.
            vec![
                (Some(DmIcon::Pause), "Pause", "pause"),
                (None, "Cancel", "stop"),
            ]
        };
        let mut right = IN_R;
        for (icon, label, act) in actions.iter().rev() {
            let gw = p
                .layout_no_wrap((*label).to_string(), reg(12.0), pl().dm_dim)
                .size()
                .x;
            let iw = if icon.is_some() { 17.0 } else { 0.0 };
            let w = iw + gw;
            let r = egui::Rect::from_min_size(
                egui::pos2(o.x + right - w, o.y + HEAD_Y - 4.0),
                egui::vec2(w, HEAD_H + 8.0),
            );
            let b = ui.interact(r, egui::Id::new(("pg-act", act)), egui::Sense::click());
            let col = if b.hovered() { pl().dm_txt } else { pl().dm_dim };
            if b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if let Some(ic) = icon {
                dm_icon(
                    &p,
                    egui::Rect::from_center_size(
                        egui::pos2(r.min.x + 6.0, r.center().y),
                        egui::Vec2::splat(12.0),
                    ),
                    *ic,
                    col,
                    2.0,
                );
            }
            p.text(
                egui::pos2(r.min.x + iw, r.center().y),
                egui::Align2::LEFT_CENTER,
                (*label).to_string(),
                reg(12.0),
                col,
            );
            if b.clicked() {
                match *act {
                    "pause" => send_cmd(self.id, "pause"),
                    "resume" => send_cmd(self.id, "resume"),
                    "cancel" => send_cmd(self.id, "cancel"),
                    "stop" => {
                        send_cmd(self.id, "cancel");
                        CLOSING.store(true, std::sync::atomic::Ordering::SeqCst);
                        Self::close_soon(&ctx);
                    }
                    "close" => self.close_by_user(&ctx),
                    "file" => open_path(&item.dest_path()),
                    "dir" => {
                        if let Some(dir) = item.dest_path().parent() {
                            open_path(dir);
                        }
                    }
                    _ => {}
                }
            }
            right -= w + 18.0;
        }
        // No snapshot (finished, failed or not yet started): the header row
        // above *is* the whole section — it still holds the state actions.
        if !has_parts || !self.parts_open {
            return;
        }
        if let Some((i, n)) = frags {
            self.fragment_parts(&p, o, i, n, downloading);
            return;
        }

        // ---- part strip: one segment per part, as wide as its byte span ----
        let strip = at(o, PAD_X, STRIP_Y, PROG_W, STRIP_H);
        let n = self.snap.parts.len();
        let lens: Vec<u64> = self
            .snap
            .parts
            .iter()
            .map(|(s, e, _)| e.saturating_sub(*s) + 1)
            .collect();
        let sum = lens.iter().sum::<u64>().max(1);
        let corner = |first: bool, last: bool| egui::CornerRadius {
            nw: if first { 2 } else { 0 },
            sw: if first { 2 } else { 0 },
            ne: if last { 2 } else { 0 },
            se: if last { 2 } else { 0 },
        };
        let mut x = strip.min.x;
        for (i, (_, _, d)) in self.snap.parts.iter().enumerate() {
            let len = lens[i];
            let w = strip.width() * (len as f32 / sum as f32);
            let seg = egui::Rect::from_min_size(egui::pos2(x, strip.min.y), egui::vec2(w, STRIP_H));
            let f = if len > 0 {
                (*d as f32 / len as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Green finished/receiving, amber while it has nothing yet (the
            // concept's "Connecting..."), slate otherwise.
            let base = if f >= 1.0 {
                DM_OK
            } else if f > 0.0 || !downloading {
                pl().dm_box_bd
            } else {
                DM_YEL
            };
            p.rect_filled(seg, corner(i == 0, i + 1 == n), base);
            if f > 0.0 && f < 1.0 {
                p.rect_filled(
                    egui::Rect::from_min_size(seg.min, egui::vec2(seg.width() * f, STRIP_H)),
                    corner(i == 0, false),
                    DM_OK,
                );
            }
            x += w;
        }

        // ---- table: header row, then one row per part ----
        let hdr_y = o.y + TBL_HDR_Y + 4.0;
        p.text(
            egui::pos2(o.x + PAD_X, hdr_y),
            egui::Align2::LEFT_TOP,
            "#",
            reg(10.0),
            pl().dm_head,
        );
        p.text(
            egui::pos2(o.x + PAD_X + COL_NUM_W, hdr_y),
            egui::Align2::LEFT_TOP,
            "Status",
            reg(10.0),
            pl().dm_head,
        );
        p.text(
            egui::pos2(o.x + COL_DL_X + 90.0, hdr_y),
            egui::Align2::RIGHT_TOP,
            "Downloaded",
            reg(10.0),
            pl().dm_head,
        );
        p.text(
            egui::pos2(o.x + COL_TOT_X + 80.0, hdr_y),
            egui::Align2::RIGHT_TOP,
            "Total",
            reg(10.0),
            pl().dm_head,
        );
        p.line_segment(
            [
                egui::pos2(o.x + PAD_X, o.y + TBL_HDR_Y + TBL_HDR_H - 0.5),
                egui::pos2(o.x + IN_R, o.y + TBL_HDR_Y + TBL_HDR_H - 0.5),
            ],
            egui::Stroke::new(1.0, pl().dm_line),
        );
        let rows_top = o.y + TBL_Y;
        let rows_bottom = (ui.max_rect().max.y - 1.0 - FOOT_PAD).max(rows_top);
        let rows_rect = egui::Rect::from_min_max(
            egui::pos2(o.x + 1.0, rows_top),
            egui::pos2(o.x + PG_W - 1.0, rows_bottom),
        );
        let table_h = TBL_STEP * n as f32;
        let max_scroll = (table_h - rows_rect.height()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let rows = ui.interact(rows_rect, egui::Id::new("pg-rows"), egui::Sense::hover());
        if rows.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                self.scroll = (self.scroll - dy).clamp(0.0, max_scroll);
            }
        }
        let pc = p.with_clip_rect(rows_rect);
        for (i, (_, _, d)) in self.snap.parts.iter().enumerate() {
            let y = rows_top + TBL_STEP * i as f32 - self.scroll;
            if y > rows_bottom || y + TBL_STEP < rows_top {
                continue;
            }
            let len = lens[i];
            let f = if len > 0 {
                (*d as f32 / len as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (status, col) = if len > 0 && *d >= len {
                ("Finished", DM_OK)
            } else if downloading && f > 0.0 {
                ("Receiving Data", DM_OK)
            } else if downloading {
                ("Connecting...", DM_YEL)
            } else if f > 0.0 {
                ("Incomplete", pl().dm_dim)
            } else {
                ("Waiting", pl().dm_lbl)
            };
            pc.text(
                egui::pos2(o.x + PAD_X, y + TBL_PAD),
                egui::Align2::LEFT_TOP,
                (i + 1).to_string(),
                reg(10.0),
                pl().dm_lbl,
            );
            pc.text(
                egui::pos2(o.x + PAD_X + COL_NUM_W, y + TBL_PAD),
                egui::Align2::LEFT_TOP,
                status,
                reg(10.0),
                col,
            );
            pc.text(
                egui::pos2(o.x + COL_DL_X + 90.0, y + TBL_PAD),
                egui::Align2::RIGHT_TOP,
                // The concept leaves the cell blank while a part has nothing.
                if *d > 0 { format_bytes(*d) } else { String::new() },
                reg(10.0),
                pl().dm_dim,
            );
            pc.text(
                egui::pos2(o.x + COL_TOT_X + 80.0, y + TBL_PAD),
                egui::Align2::RIGHT_TOP,
                // A stream segment not yet fetched has no size to show (its
                // placeholder slot is one byte wide).
                if len <= 1 && *d == 0 { String::new() } else { format_bytes(len) },
                reg(10.0),
                pl().dm_dim,
            );
            if i + 1 < n {
                pc.line_segment(
                    [
                        egui::pos2(o.x + 1.0, y + TBL_STEP - 0.5),
                        egui::pos2(o.x + PG_W - 1.0, y + TBL_STEP - 0.5),
                    ],
                    egui::Stroke::new(1.0, pl().dm_trough),
                );
            }
        }
        if max_scroll > 0.0 {
            let bar_h = (rows_rect.height() * rows_rect.height() / table_h).max(24.0);
            let t = self.scroll / max_scroll;
            let y = rows_top + (rows_rect.height() - bar_h) * t;
            // The same thin, see-through handle as the main list's bars.
            p.rect_filled(
                egui::Rect::from_min_size(egui::pos2(o.x + PG_W - 7.0, y), egui::vec2(4.0, bar_h)),
                2.0,
                pl().dm_dim.gamma_multiply(if rows.hovered() { 0.55 } else { 0.30 }),
            );
        }
    }

    /// Part Info for a fragmented stream: the strip fills with the fragments
    /// done, and one row says how many of how many.
    fn fragment_parts(&self, p: &egui::Painter, o: egui::Pos2, i: u64, n: u64, downloading: bool) {
        let strip = at(o, PAD_X, STRIP_Y, PROG_W, STRIP_H);
        p.rect_filled(strip, 2.0, if downloading && i == 0 { DM_YEL } else { pl().dm_box_bd });
        let f = (i as f32 / n.max(1) as f32).clamp(0.0, 1.0);
        if f > 0.0 {
            p.rect_filled(
                egui::Rect::from_min_size(strip.min, egui::vec2(strip.width() * f, STRIP_H)),
                2.0,
                DM_OK,
            );
        }
        let hdr_y = o.y + TBL_HDR_Y + 4.0;
        for (x, align, text) in [
            (o.x + PAD_X, egui::Align2::LEFT_TOP, "#"),
            (o.x + PAD_X + COL_NUM_W, egui::Align2::LEFT_TOP, "Status"),
            (o.x + COL_DL_X + 90.0, egui::Align2::RIGHT_TOP, "Fragments"),
            (o.x + COL_TOT_X + 80.0, egui::Align2::RIGHT_TOP, "Total"),
        ] {
            p.text(egui::pos2(x, hdr_y), align, text, reg(10.0), pl().dm_head);
        }
        p.line_segment(
            [
                egui::pos2(o.x + PAD_X, o.y + TBL_HDR_Y + TBL_HDR_H - 0.5),
                egui::pos2(o.x + IN_R, o.y + TBL_HDR_Y + TBL_HDR_H - 0.5),
            ],
            egui::Stroke::new(1.0, pl().dm_line),
        );
        let y = o.y + TBL_Y;
        let (status, col) = if i >= n {
            ("Finished", DM_OK)
        } else if downloading && i > 0 {
            ("Receiving fragments", DM_OK)
        } else if downloading {
            ("Connecting...", DM_YEL)
        } else {
            ("Incomplete", pl().dm_dim)
        };
        p.text(egui::pos2(o.x + PAD_X, y + TBL_PAD), egui::Align2::LEFT_TOP, "1", reg(10.0), pl().dm_lbl);
        p.text(egui::pos2(o.x + PAD_X + COL_NUM_W, y + TBL_PAD), egui::Align2::LEFT_TOP, status, reg(10.0), col);
        p.text(
            egui::pos2(o.x + COL_DL_X + 90.0, y + TBL_PAD),
            egui::Align2::RIGHT_TOP,
            i.to_string(),
            reg(10.0),
            pl().dm_dim,
        );
        p.text(
            egui::pos2(o.x + COL_TOT_X + 80.0, y + TBL_PAD),
            egui::Align2::RIGHT_TOP,
            n.to_string(),
            reg(10.0),
            pl().dm_dim,
        );
    }
}
