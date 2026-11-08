//! P3.3 companions: Drop Basket (`--basket`) and Mini status (`--mini`).
//!
//! - Basket: tiny always-on-top window. Paste a URL + Add, or DROP `.txt`
//!   (one URL per line) / `.url` files onto it — dropped links queue
//!   straight into the shared `queue.json`.
//! - Mini: compact live status (active count, total speed, overall bar) with
//!   Open-main / Pause-all / Resume / Close. Orders travel via `cmd.json`
//!   (`pause_all` / `resume_all`), consumed by the main app.

use crate::model::{
    append_to_queue, apply_rename_template, category_of, config_dir, is_download_url, AppendResult,
    DownloadItem,
};

fn ui_prefs() -> (String, usize, String, bool) {
    // (save_dir, connections, rename_template, category_folders)
    let v: serde_json::Value = config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("ui.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let dir = v
        .get("save_dir")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned())
        });
    let conns = v
        .get("connections_default")
        .and_then(|x| x.as_u64())
        .map(|n| (n as usize).clamp(1, 32))
        .unwrap_or(8);
    let tpl = v
        .get("rename_template")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("{name}")
        .to_string();
    let cat = v
        .get("category_folders")
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    (dir, conns, tpl, cat)
}

fn queue_one(url: &str) -> &'static str {
    let url = url.trim().trim_matches(|c| c == '"' || c == '\'');
    if !is_download_url(url) || url.len() > 2048 {
        return "not a URL";
    }
    let (base, conns, tpl, cat) = ui_prefs();
    let mut name = apply_rename_template(
        &tpl,
        &crate::model::filename_from_url(url),
        url,
    );
    if name.trim().is_empty() {
        name = crate::model::filename_from_url(url);
    }
    let mut dir = std::path::PathBuf::from(&base);
    if cat {
        dir.push(category_of(&name));
    }
    let mut item = DownloadItem::new(
        0,
        url.to_string(),
        crate::engine::sanitize_filename(&name),
        dir.to_string_lossy().to_string(),
        conns,
    );
    match append_to_queue(&mut item) {
        AppendResult::Added(_) => "queued",
        AppendResult::AlreadyQueued(_) => "already queued",
        AppendResult::Error(_) => "queue error",
    }
}

/// URLs inside a dropped file: `.url` (URL= line) or plain text (any token).
pub(crate) fn urls_from_dropped(path: &std::path::Path) -> Vec<String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    if ext == "url" {
        return data
            .lines()
            .filter_map(|l| {
                let t = l.trim();
                t.strip_prefix("URL=")
                    .or_else(|| t.strip_prefix("url="))
                    .map(|u| u.trim().to_string())
            })
            .collect();
    }
    if ext == "txt" || ext == "lnk" || ext.is_empty() {
        return data
            .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .filter(|t| is_download_url(t))
            .map(|t| t.to_string())
            .collect();
    }
    Vec::new()
}

fn read_queue() -> Vec<DownloadItem> {
    config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("queue.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn send_cmd(action: &str) {
    let Some(p) = config_dir().map(|d| d.join("cmd.json")) else {
        return;
    };
    let _guard = crate::model::fs_lock("cmd");
    if !_guard.owned() { return; }
    let mut v: serde_json::Value = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(arr) = v.as_array_mut() {
        arr.push(serde_json::json!({ "action": action }));
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

fn launch_main() {
    let Ok(exe) = crate::platform::launch_path() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008);
    }
    let _ = cmd.spawn();
}

fn centered_window(title: &str, w: f32, h: f32) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id(crate::platform::app_id())
        .with_title(title)
        .with_inner_size([w, h])
        .with_min_inner_size([w - 40.0, h - 40.0])
        .with_always_on_top()
        .with_resizable(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    crate::platform::native_options(viewport)
}

fn center_on_first(ctx: &egui::Context, done: &mut bool, w: f32, h: f32) {
    if *done {
        return;
    }
    // Retries until the monitor geometry is known, then lands in the middle of
    // the work area (taskbar excluded).
    if crate::model::center_window(ctx, egui::vec2(w, h)) {
        *done = true;
    }
}

// ---------------------------------------------------------------- basket ---

struct BasketApp {
    url: String,
    notice: String,
    centered: bool,
    items: Vec<DownloadItem>,
    last: std::time::Instant,
}

impl BasketApp {
    fn refresh(&mut self) {
        self.items = read_queue();
        self.last = std::time::Instant::now();
    }

    fn ingest_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let mut n = 0;
        let mut msgs = Vec::new();
        for p in dropped {
            if p.is_dir() {
                continue;
            }
            for u in urls_from_dropped(&p) {
                let r = queue_one(&u);
                if r == "queued" {
                    n += 1;
                } else if msgs.len() < 3 {
                    msgs.push(format!("{}: {r}", p.display()));
                }
            }
        }
        self.notice = if n > 0 {
            format!("Queued {n} link(s) from dropped files.")
        } else if msgs.is_empty() {
            "No URLs found in dropped files (.txt / .url).".to_string()
        } else {
            msgs.join("; ")
        };
        self.refresh();
    }
}

/// Entry point for `--basket`.
pub fn run_basket() -> eframe::Result<()> {
    let mut app = BasketApp {
        url: String::new(),
        notice: "Paste a URL, or drop .txt / .url files here.".to_string(),
        centered: false,
        items: Vec::new(),
        last: std::time::Instant::now() - std::time::Duration::from_secs(10),
    };
    app.refresh();
    eframe::run_native(
        "Drop Basket",
        centered_window("Drop Basket", 420.0, 520.0),
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            // Spawned as its own process: pick up whatever theme `ui.json`
            // holds (written by the main app / the Settings window).
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for BasketApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        center_on_first(ctx, &mut self.centered, 420.0, 520.0);
        self.ingest_drops(ctx);
        if self.last.elapsed() >= std::time::Duration::from_secs(2) {
            self.refresh();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Drop Basket");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.url)
                        .hint_text("https://…")
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Add").clicked() && !self.url.trim().is_empty() {
                    self.notice = queue_one(&self.url).to_string();
                    if self.notice == "queued" {
                        self.url.clear();
                    }
                    self.refresh();
                }
            });
            ui.small(&self.notice);
            ui.separator();
            ui.label(format!("{} item(s) in queue", self.items.len()));
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let mut rev: Vec<&DownloadItem> = self.items.iter().collect();
                rev.reverse();
                for d in rev.iter().take(12) {
                    ui.horizontal(|ui| {
                        ui.label(d.status.label());
                        ui.monospace(d.filename.clone());
                    });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

// ------------------------------------------------------------------ mini ---

struct MiniApp {
    centered: bool,
    active: usize,
    queued: usize,
    speed: f64,
    frac: f32,
}

/// Entry point for `--mini`.
pub fn run_mini() -> eframe::Result<()> {
    let mut app = MiniApp { centered: false, active: 0, queued: 0, speed: 0.0, frac: 0.0 };
    refresh_mini(&mut app);
    eframe::run_native(
        "Download Craft Mini",
        centered_window("Download Craft Mini", 340.0, 230.0),
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            // Spawned as its own process: pick up whatever theme `ui.json`
            // holds (written by the main app / the Settings window).
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

fn refresh_mini(app: &mut MiniApp) {
    let items = read_queue();
    let snap: serde_json::Value = config_dir()
        .and_then(|p| std::fs::read_to_string(p.join("progress.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let mut active = 0;
    let mut queued = 0;
    let mut speed = 0.0;
    let mut dl = 0u64;
    let mut total = 0u64;
    let mut any_total = false;
    for d in &items {
        match d.status {
            crate::model::Status::Downloading => {
                active += 1;
                let id = d.id.to_string();
                let e = snap.get("items").and_then(|m| m.get(&id));
                let s = e
                    .and_then(|x| x.get("speed"))
                    .and_then(|x| x.as_f64())
                    .unwrap_or(d.speed_bps);
                speed += s;
                let dd = e
                    .and_then(|x| x.get("downloaded"))
                    .and_then(|x| x.as_u64())
                    .unwrap_or(d.downloaded_bytes);
                dl += dd;
                if let Some(t) = e
                    .and_then(|x| x.get("total"))
                    .and_then(|x| x.as_u64())
                    .or(d.total_bytes)
                {
                    total += t;
                    any_total = true;
                }
            }
            crate::model::Status::Queued | crate::model::Status::Paused => queued += 1,
            _ => {}
        }
    }
    app.active = active;
    app.queued = queued;
    app.speed = speed;
    app.frac = if any_total && total > 0 {
        (dl as f32 / total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
}

impl eframe::App for MiniApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        center_on_first(ctx, &mut self.centered, 340.0, 230.0);
        refresh_mini(self);
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("{} active · {} queued", self.active, self.queued));
            ui.label(if self.speed > 0.0 {
                format!("{}/s", crate::model::format_bytes(self.speed as u64))
            } else {
                "-".to_string()
            });
            ui.add(egui::ProgressBar::new(self.frac).desired_height(10.0).show_percentage());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Open main").clicked() {
                    launch_main();
                }
                if ui.button("Pause all").clicked() {
                    send_cmd("pause_all");
                }
                if ui.button("Resume").clicked() {
                    send_cmd("resume_all");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });
    }
}
