//! About & Updates — its own small window (`download_craft.exe --about`),
//! so it is never clipped by the main window.
//!
//! The main window owns updating (checking, downloading, installing,
//! restarting). It writes what it is doing to `update_state.json`; this
//! window draws that, and its buttons go back through `cmd.json`
//! (`check_update`, `self_update`, `restart_update`, `restart_when_idle`).
//! What the newest release brings comes from `release_info.json`, written by
//! the update check. The four looks: up to date, update available,
//! installing, ready to restart.

use crate::app::pl;
use crate::dm::*;
use crate::fonts::{reg, semi};
use crate::model::{claim_window, config_dir, format_bytes};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const AW: f32 = 520.0;
/// Opening height; the window then fits itself to its content.
const AH: f32 = 560.0;
const REPO: &str = "https://github.com/hungry-detective/downloadcraft";

/// Keep what the About window shows about the newest release: its tag, page
/// and the first few headline points of its notes.
pub fn save_release_info(v: &serde_json::Value, newer: bool) {
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let highlights: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("- "))
        .map(|l| {
            let l = l.trim_start_matches("- ").replace("**", "");
            // "Updates itself: when a new release…" → "Updates itself".
            let head = l.split(": ").next().unwrap_or(&l).trim().to_string();
            if head.chars().count() > 90 { head.chars().take(88).collect::<String>() + "…" } else { head }
        })
        .take(3)
        .collect();
    let info = serde_json::json!({
        "tag": v.get("tag_name").and_then(|t| t.as_str()).unwrap_or(""),
        "url": v.get("html_url").and_then(|t| t.as_str()).unwrap_or(""),
        "highlights": highlights,
        "newer": newer,
    });
    if let Some(p) = config_dir().map(|d| d.join("release_info.json")) {
        let _ = crate::model::atomic_write(&p, info.to_string().as_bytes());
    }
}

pub fn spawn() {
    let Ok(exe) = crate::platform::launch_path() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--about");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008);
    }
    let _ = cmd.spawn();
}

/// A button's order for the main window (its `cmd.json`).
pub(crate) fn send(action: &str) {
    let Some(p) = config_dir().map(|d| d.join("cmd.json")) else { return };
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() {
        return;
    }
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = v.as_array_mut() {
        arr.push(serde_json::json!({ "action": action }));
        let _ = crate::model::atomic_write(&p, v.to_string().as_bytes());
    }
}

#[derive(Default, Clone)]
pub(crate) struct UpdState {
    pub(crate) phase: String,
    pub(crate) status: String,
    pub(crate) size: u64,
    pub(crate) running: u64,
}

#[derive(Default, Clone)]
pub(crate) struct Release {
    pub(crate) tag: String,
    pub(crate) url: String,
    pub(crate) highlights: Vec<String>,
    pub(crate) newer: bool,
}

fn read_json(name: &str) -> Option<serde_json::Value> {
    let p = config_dir()?.join(name);
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

pub(crate) fn read_state() -> UpdState {
    let Some(v) = read_json("update_state.json") else { return UpdState::default() };
    UpdState {
        phase: v.get("phase").and_then(|x| x.as_str()).unwrap_or("idle").into(),
        status: v.get("status").and_then(|x| x.as_str()).unwrap_or("").into(),
        size: v.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        running: v.get("running").and_then(|x| x.as_u64()).unwrap_or(0),
    }
}

pub(crate) fn read_release() -> Release {
    let Some(v) = read_json("release_info.json") else { return Release::default() };
    Release {
        tag: v.get("tag").and_then(|x| x.as_str()).unwrap_or("").trim_start_matches('v').into(),
        url: v.get("url").and_then(|x| x.as_str()).unwrap_or("").into(),
        highlights: v
            .get("highlights")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|h| h.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        newer: v.get("newer").and_then(|x| x.as_bool()).unwrap_or(false),
    }
}

/// "Downloading update… 45%" → 0.45.
fn percent(status: &str) -> Option<f32> {
    let i = status.find('%')?;
    let digits: String = status[..i].chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse::<f32>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))
}

struct AboutApp {
    state: UpdState,
    release: Release,
    last_read: Instant,
    helpers: Vec<(&'static str, String, bool)>,
    helpers_rx: Option<Receiver<Vec<(&'static str, String, bool)>>>,
    auto: bool,
    placed: bool,
    /// A button was pressed: show it at once, before the main window answers.
    pending: Option<(&'static str, Instant)>,
    /// The height last asked of the window.
    win_h: f32,
}

fn close_soon(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        std::process::exit(0);
    });
}

pub fn run() -> eframe::Result<()> {
    let Some(_slot) = claim_window("about") else {
        return Ok(());
    };
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::setup::helper_lines());
    });
    let app = AboutApp {
        state: read_state(),
        release: read_release(),
        last_read: Instant::now(),
        helpers: Vec::new(),
        helpers_rx: Some(rx),
        auto: crate::model::ui_pref_bool("update_auto", true),
        placed: false,
        pending: None,
        win_h: AH,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("About Download Craft")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([AW, AH])
        .with_resizable(false)
        .with_decorations(crate::platform::system_title_bar())
        .with_transparent(!crate::platform::system_title_bar());
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        crate::model::APP_NAME,
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            crate::fonts::install(cc, true);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for AboutApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_read.elapsed() >= Duration::from_millis(400) {
            self.last_read = Instant::now();
            let s = read_state();
            // The main window has answered the button: its state wins again.
            if let Some((_, at)) = self.pending {
                if s.phase != self.state.phase || at.elapsed() > Duration::from_secs(5) {
                    self.pending = None;
                }
            }
            self.state = s;
            self.release = read_release();
        }
        if let Some(rx) = &self.helpers_rx {
            if let Ok(h) = rx.try_recv() {
                self.helpers = h;
                self.helpers_rx = None;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        if !self.placed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if crate::model::center_window(ctx, egui::vec2(AW, AH)) {
                self.placed = true;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_soon(ctx);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| self.paint(ui));
    }
}

/// A filled (primary) or outline button; true when clicked.
fn button(ui: &egui::Ui, p: &egui::Painter, r: egui::Rect, id: &str, label: &str, primary: bool, enabled: bool) -> bool {
    let t = pl();
    let resp = ui.interact(r, egui::Id::new(("about-btn", id)), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let hov = resp.hovered() && enabled;
    if primary {
        let fill = if hov { t.primary.gamma_multiply(0.85) } else { t.primary };
        p.rect_filled(r, 8.0, fill.gamma_multiply(if enabled { 1.0 } else { 0.5 }));
        p.text(r.center(), egui::Align2::CENTER_CENTER, label, semi(13.0), egui::Color32::WHITE);
    } else {
        if hov {
            p.rect_filled(r, 8.0, t.surface_lt);
        }
        p.rect_stroke(r, 8.0, egui::Stroke::new(1.0, t.card_bd), egui::StrokeKind::Inside);
        p.text(r.center(), egui::Align2::CENTER_CENTER, label, semi(12.5), t.txt);
    }
    enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn label_width(p: &egui::Painter, s: &str, f: &egui::FontId) -> f32 {
    crate::shaped::width(p, s, f)
}

impl AboutApp {
    fn press(&mut self, action: &'static str, shows: &'static str) {
        send(action);
        self.pending = Some((shows, Instant::now()));
    }

    fn paint(&mut self, ui: &mut egui::Ui) {
        let t = pl();
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.rect(full.shrink(0.5), 14.0, t.modal_bg, egui::Stroke::new(1.0, t.card_bd), egui::StrokeKind::Inside);
        let o = full.min;
        let x0 = o.x + 26.0;
        let w = full.width() - 52.0;
        let ok = DM_OK;

        // ---- title bar ----
        let bar = egui::Rect::from_min_size(o, egui::vec2(full.width(), 50.0));
        let drag = ui.interact(bar.with_max_x(bar.max.x - 50.0), egui::Id::new("about-drag"), egui::Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        p.text(egui::pos2(x0 - 6.0, o.y + 25.0), egui::Align2::LEFT_CENTER, "About", semi(14.0), t.txt);
        let close = egui::Rect::from_center_size(egui::pos2(full.max.x - 27.0, o.y + 25.0), egui::vec2(30.0, 30.0));
        let cr = ui.interact(close, egui::Id::new("about-x"), egui::Sense::click()).on_hover_text("Close");
        if cr.hovered() {
            p.rect_filled(close, 7.0, t.surface_lt);
        }
        dm_icon(&p, close.shrink(9.0), DmIcon::Close, if cr.hovered() { t.txt } else { t.dim }, 2.0);
        if cr.clicked() {
            close_soon(&ctx);
        }
        p.hline(full.x_range(), bar.max.y, egui::Stroke::new(1.0, t.card_bd));

        // ---- the app ----
        let mut y = bar.max.y + 22.0;
        let tile = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(60.0, 60.0));
        p.rect(tile, 16.0, t.primary.gamma_multiply(0.14), egui::Stroke::new(1.0, t.primary.gamma_multiply(0.45)), egui::StrokeKind::Inside);
        let c = tile.center();
        p.circle_stroke(c, 13.5, egui::Stroke::new(2.4, t.primary));
        p.line_segment([egui::pos2(c.x, c.y - 7.0), egui::pos2(c.x, c.y + 5.0)], egui::Stroke::new(2.4, t.primary));
        p.line_segment([egui::pos2(c.x - 5.0, c.y), egui::pos2(c.x, c.y + 5.5)], egui::Stroke::new(2.4, t.primary));
        p.line_segment([egui::pos2(c.x + 5.0, c.y), egui::pos2(c.x, c.y + 5.5)], egui::Stroke::new(2.4, t.primary));
        p.text(egui::pos2(tile.max.x + 16.0, y + 20.0), egui::Align2::LEFT_CENTER, "Download Craft", semi(21.0), t.txt);
        p.text(
            egui::pos2(tile.max.x + 16.0, y + 44.0),
            egui::Align2::LEFT_CENTER,
            format!("Version {} · Free and open source · MIT", crate::model::APP_VERSION),
            reg(12.5),
            t.dim,
        );
        y = tile.max.y + 20.0;

        // ---- the update card ----
        let phase = self.pending.map(|(s, _)| s.to_string()).unwrap_or_else(|| self.state.phase.clone());
        let next = if self.release.tag.is_empty() { "the new version".to_string() } else { self.release.tag.clone() };
        let card_h = match phase.as_str() {
            "available" => 128.0 + 20.0 * self.release.highlights.len() as f32,
            "installing" => 196.0,
            "ready" => if self.state.running > 0 { 150.0 } else { 120.0 },
            _ => 128.0,
        };
        let card = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, card_h));
        let (fill, bd) = match phase.as_str() {
            "available" | "installing" => (t.primary.gamma_multiply(0.10), t.primary.gamma_multiply(0.45)),
            "ready" => (ok.gamma_multiply(0.08), ok.gamma_multiply(0.40)),
            _ => (t.surface, t.card_bd),
        };
        p.rect(card, 12.0, fill, egui::Stroke::new(1.0, bd), egui::StrokeKind::Inside);
        let ix = card.min.x + 18.0;
        let ic = egui::pos2(ix + 18.0, card.min.y + 36.0);
        let title_x = ix + 48.0;
        let spin = |p: &egui::Painter, c: egui::Pos2| {
            let a = ui.input(|i| i.time) as f32 * 300.0;
            dm_arc(p, c, 9.0, a, a + 260.0, egui::Stroke::new(2.4, t.primary));
        };
        match phase.as_str() {
            "available" => {
                p.circle_filled(ic, 18.0, t.primary.gamma_multiply(0.22));
                p.text(ic, egui::Align2::CENTER_CENTER, crate::app::MI_ARROW_DOWN, crate::fonts::icon(20.0), t.primary);
                p.text(egui::pos2(title_x, card.min.y + 27.0), egui::Align2::LEFT_CENTER, format!("Version {next} is ready"), semi(15.0), t.txt);
                let size = if self.state.size > 0 { format!("{} · ", format_bytes(self.state.size)) } else { String::new() };
                p.text(egui::pos2(title_x, card.min.y + 47.0), egui::Align2::LEFT_CENTER, format!("{size}your settings and downloads list are kept"), reg(12.0), t.dim);
                let mut hy = card.min.y + 78.0;
                for h in &self.release.highlights {
                    let line = crate::app::truncate_to_width(&p, &format!("•  {h}"), reg(12.5), t.txt, card.width() - 36.0);
                    p.text(egui::pos2(ix, hy), egui::Align2::LEFT_CENTER, line, reg(12.5), t.txt);
                    hy += 20.0;
                }
                let by = card.max.y - 50.0;
                if button(ui, &p, egui::Rect::from_min_size(egui::pos2(ix, by), egui::vec2(124.0, 36.0)), "update", "Update now", true, true) {
                    self.press("self_update", "installing");
                }
                if !self.release.url.is_empty() {
                    let lr = egui::Rect::from_min_size(egui::pos2(ix + 140.0, by), egui::vec2(150.0, 36.0));
                    let resp = ui.interact(lr, egui::Id::new("about-notes"), egui::Sense::click());
                    p.text(egui::pos2(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, "Full release notes", reg(12.5), if resp.hovered() { t.txt } else { t.primary });
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        crate::platform::open_url(&self.release.url);
                    }
                }
            }
            "installing" => {
                let frac = percent(&self.state.status);
                p.text(egui::pos2(ix, card.min.y + 26.0), egui::Align2::LEFT_CENTER, format!("Updating to {next}"), semi(15.0), t.txt);
                if let Some(f) = frac {
                    p.text(egui::pos2(card.max.x - 18.0, card.min.y + 26.0), egui::Align2::RIGHT_CENTER, format!("{}%", (f * 100.0).round()), reg(12.5), t.dim);
                }
                let track = egui::Rect::from_min_size(egui::pos2(ix, card.min.y + 48.0), egui::vec2(card.width() - 36.0, 8.0));
                p.rect_filled(track, 4.0, t.bg);
                let installing = self.state.status.contains("Installing") || self.state.status.contains("Checked");
                let f = if installing { 1.0 } else { frac.unwrap_or(0.02) };
                p.rect_filled(egui::Rect::from_min_size(track.min, egui::vec2(track.width() * f.max(0.02), 8.0)), 4.0, t.primary);
                let steps = [
                    ("Download", installing),
                    ("Check it is the genuine release", installing),
                    ("Replace the app's files", false),
                    ("Restart into the new version", false),
                ];
                let current = if installing { 2 } else { 0 };
                for (k, (label, done)) in steps.iter().enumerate() {
                    let sy = card.min.y + 80.0 + k as f32 * 22.0;
                    let dc = egui::pos2(ix + 8.0, sy);
                    if *done {
                        dm_icon(&p, egui::Rect::from_center_size(dc, egui::vec2(12.0, 12.0)), DmIcon::Check, ok, 2.6);
                    } else if k == current {
                        spin(&p, dc);
                    } else {
                        p.circle_stroke(dc, 6.0, egui::Stroke::new(1.5, t.dim.gamma_multiply(0.6)));
                    }
                    p.text(egui::pos2(ix + 24.0, sy), egui::Align2::LEFT_CENTER, *label, reg(12.5), if *done || k == current { t.txt } else { t.dim });
                }
                p.text(egui::pos2(ix, card.max.y - 18.0), egui::Align2::LEFT_CENTER, "You can keep downloading meanwhile. If anything goes wrong, nothing is changed.", reg(11.5), t.dim);
            }
            "ready" => {
                p.circle_filled(ic, 18.0, ok.gamma_multiply(0.18));
                dm_icon(&p, egui::Rect::from_center_size(ic, egui::vec2(16.0, 16.0)), DmIcon::Check, ok, 2.8);
                p.text(egui::pos2(title_x, card.min.y + 27.0), egui::Align2::LEFT_CENTER, format!("{next} is installed"), semi(15.0), t.txt);
                p.text(egui::pos2(title_x, card.min.y + 47.0), egui::Align2::LEFT_CENTER, "Restart to start using it.", reg(12.0), t.dim);
                let mut by = card.min.y + 70.0;
                if self.state.running > 0 {
                    let n = self.state.running;
                    let s = format!("{n} download{} still running — they carry on after the restart.", if n == 1 { " is" } else { "s are" });
                    p.text(egui::pos2(ix, by + 4.0), egui::Align2::LEFT_CENTER, s, reg(12.0), t.txt);
                    by += 26.0;
                }
                if button(ui, &p, egui::Rect::from_min_size(egui::pos2(ix, by + 4.0), egui::vec2(124.0, 36.0)), "restart", "Restart now", true, true) {
                    self.press("restart_update", "ready");
                }
                if self.state.running > 0 {
                    let lw = label_width(&p, "When downloads finish", &semi(12.5)) + 32.0;
                    if button(ui, &p, egui::Rect::from_min_size(egui::pos2(ix + 134.0, by + 4.0), egui::vec2(lw, 36.0)), "later", "When downloads finish", false, true) {
                        send("restart_when_idle");
                    }
                }
            }
            "checking" => {
                spin(&p, ic);
                p.text(egui::pos2(title_x, card.min.y + 27.0), egui::Align2::LEFT_CENTER, "Checking for updates…", semi(15.0), t.txt);
                p.text(egui::pos2(title_x, card.min.y + 47.0), egui::Align2::LEFT_CENTER, "Asking GitHub for the newest release", reg(12.0), t.dim);
                self.auto_row(ui, &p, card);
            }
            _ => {
                let failed = phase == "failed";
                let checked = phase == "uptodate";
                if failed {
                    p.circle_filled(ic, 18.0, t.warning.gamma_multiply(0.18));
                    p.text(ic, egui::Align2::CENTER_CENTER, "!", semi(16.0), t.warning);
                } else if checked {
                    p.circle_filled(ic, 18.0, ok.gamma_multiply(0.18));
                    dm_icon(&p, egui::Rect::from_center_size(ic, egui::vec2(16.0, 16.0)), DmIcon::Check, ok, 2.8);
                } else {
                    p.circle_stroke(ic, 16.0, egui::Stroke::new(1.6, t.dim));
                }
                let (title, sub) = if failed {
                    ("Update failed".to_string(), self.state.status.clone())
                } else if checked {
                    ("You're up to date".to_string(), format!("{} is the newest version", crate::model::APP_VERSION))
                } else {
                    ("Updates".to_string(), "Not checked yet this session".to_string())
                };
                p.text(egui::pos2(title_x, card.min.y + 27.0), egui::Align2::LEFT_CENTER, title, semi(15.0), t.txt);
                let sub = crate::app::truncate_to_width(&p, &sub, reg(12.0), t.dim, card.width() - 48.0 - 150.0);
                p.text(egui::pos2(title_x, card.min.y + 47.0), egui::Align2::LEFT_CENTER, sub, reg(12.0), if failed { t.warning } else { t.dim });
                let label = if checked { "Check again" } else if failed { "Try again" } else { "Check now" };
                let bw = label_width(&p, label, &semi(12.5)) + 32.0;
                if button(ui, &p, egui::Rect::from_min_size(egui::pos2(card.max.x - 18.0 - bw, card.min.y + 18.0), egui::vec2(bw, 36.0)), "check", label, false, true) {
                    self.press("check_update", "checking");
                }
                self.auto_row(ui, &p, card);
            }
        }
        y = card.max.y + 22.0;

        // ---- helpers ----
        p.text(egui::pos2(x0, y), egui::Align2::LEFT_CENTER, "HELPERS", semi(11.0), t.dim);
        y += 14.0;
        let rows = if self.helpers.is_empty() {
            vec![("yt-dlp", "Checking…".to_string(), false), ("FFmpeg", "Checking…".to_string(), false), ("Deno", "Checking…".to_string(), false)]
        } else {
            self.helpers.clone()
        };
        let box_r = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, 42.0 * rows.len() as f32));
        p.rect_stroke(box_r, 12.0, egui::Stroke::new(1.0, t.card_bd), egui::StrokeKind::Inside);
        for (k, (name, detail, ready)) in rows.iter().enumerate() {
            let ry = y + 21.0 + k as f32 * 42.0;
            if k > 0 {
                p.hline(box_r.x_range(), y + k as f32 * 42.0, egui::Stroke::new(1.0, t.card_bd));
            }
            p.text(egui::pos2(x0 + 16.0, ry), egui::Align2::LEFT_CENTER, *name, reg(13.0), t.txt);
            let (txt, col) = if *ready { (detail.clone(), ok) } else if detail == "Checking…" { (detail.clone(), t.dim) } else { ("Not installed — Help → Get ready".to_string(), DM_YEL) };
            p.text(egui::pos2(box_r.max.x - 16.0, ry), egui::Align2::RIGHT_CENTER, txt, reg(12.0), col);
        }
        y = box_r.max.y + 20.0;

        // ---- links ----
        let mut lx = x0;
        for (label, url) in [
            ("GitHub", REPO.to_string()),
            ("Release notes", format!("{REPO}/releases")),
            ("Report a problem", format!("{REPO}/issues")),
            ("Licenses", format!("{REPO}/blob/master/NOTICE.md")),
        ] {
            let lw = label_width(&p, label, &reg(12.5));
            let r = egui::Rect::from_min_size(egui::pos2(lx, y - 9.0), egui::vec2(lw, 18.0));
            let resp = ui.interact(r, egui::Id::new(("about-link", label)), egui::Sense::click());
            p.text(egui::pos2(lx, y), egui::Align2::LEFT_CENTER, label, reg(12.5), if resp.hovered() { t.txt } else { t.primary });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                crate::platform::open_url(&url);
            }
            lx += lw + 20.0;
        }

        // Fit the window: the links row, then the footer, no empty space.
        let want = (y - o.y) + 26.0 + 62.0;
        if (want - self.win_h).abs() > 1.0 {
            self.win_h = want;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(AW, want)));
        }

        // ---- footer ----
        let fy = full.max.y - 32.0;
        p.hline(full.x_range(), fy - 24.0, egui::Stroke::new(1.0, t.card_bd));
        p.text(egui::pos2(x0, fy), egui::Align2::LEFT_CENTER, "Made with Rust + egui", reg(11.5), t.dim);
        if button(ui, &p, egui::Rect::from_min_size(egui::pos2(full.max.x - 26.0 - 96.0, fy - 17.0), egui::vec2(96.0, 34.0)), "close", "Close", true, true) {
            close_soon(&ctx);
        }
    }

    /// "Update automatically" switch, at the bottom of the update card.
    fn auto_row(&mut self, ui: &egui::Ui, p: &egui::Painter, card: egui::Rect) {
        let t = pl();
        let ix = card.min.x + 18.0;
        p.hline(egui::Rangef::new(ix, card.max.x - 18.0), card.min.y + 72.0, egui::Stroke::new(1.0, t.card_bd));
        let row = egui::Rect::from_min_max(egui::pos2(ix, card.min.y + 76.0), egui::pos2(card.max.x - 18.0, card.max.y - 6.0));
        let resp = ui.interact(row, egui::Id::new("about-auto"), egui::Sense::click());
        p.text(egui::pos2(ix, row.center().y - 8.0), egui::Align2::LEFT_CENTER, "Update automatically", reg(13.0), t.txt);
        p.text(egui::pos2(ix, row.center().y + 10.0), egui::Align2::LEFT_CENTER, "Installs new versions in the background; restarts when no download is running.", reg(11.0), t.dim);
        let sw = egui::Rect::from_min_size(egui::pos2(row.max.x - 34.0, row.center().y - 9.0), egui::vec2(34.0, 18.0));
        p.rect_filled(sw, 9.0, if self.auto { t.primary } else { t.surface_lt });
        let kx = if self.auto { sw.max.x - 9.0 } else { sw.min.x + 9.0 };
        p.circle_filled(egui::pos2(kx, sw.center().y), 7.0, egui::Color32::WHITE);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.auto = !self.auto;
            crate::model::ui_pref_set_bool("update_auto", self.auto);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_release_highlights() {
        assert_eq!(percent("Downloading update… 45%"), Some(0.45));
        assert_eq!(percent("Checked. Installing…"), None);
        let v = serde_json::json!({
            "tag_name": "v0.9.12",
            "html_url": "https://h/r",
            "body": "# Download Craft 0.9.12\n\n**App**\n- **Updates itself**: when a new release…\n- **Ready on first start**: yt-dlp…\n- Plain point\n- Fourth\n"
        });
        // Written to the config folder; parse the same way as a check.
        let body = v["body"].as_str().unwrap();
        let firsts: Vec<&str> = body.lines().filter(|l| l.trim().starts_with("- ")).take(3).collect();
        assert_eq!(firsts.len(), 3);
    }
}
