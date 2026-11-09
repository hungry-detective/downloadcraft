//! "Choose files" for a torrent: a small window (`--torrent-files <id>`)
//! listing the torrent's files with tick boxes. What is saved is read when
//! the torrent next starts; a running one starts again at once with it.
//!
//! The torrent module writes the file list (`torrent/files-<id>.json`) when
//! it reads the torrent's metadata; the choice is `torrent/pick-<id>.json`.

use crate::app::pl;
use crate::model::config_dir;

fn dir() -> Option<std::path::PathBuf> {
    config_dir().map(|d| d.join("torrent"))
}

/// The torrent's files: (path inside the torrent, size).
pub fn files(id: &str) -> Vec<(String, u64)> {
    dir()
        .and_then(|d| std::fs::read_to_string(d.join(format!("files-{id}.json"))).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_files(id: &str, list: &[(String, u64)]) {
    if let (Some(d), Ok(json)) = (dir(), serde_json::to_string(list)) {
        let _ = std::fs::create_dir_all(&d);
        let _ = crate::model::atomic_write(&d.join(format!("files-{id}.json")), json.as_bytes());
    }
}

/// The files chosen (their indexes), or `None` for all of them.
pub fn picked(id: &str) -> Option<Vec<usize>> {
    dir()
        .and_then(|d| std::fs::read_to_string(d.join(format!("pick-{id}.json"))).ok())
        .and_then(|s| serde_json::from_str::<Vec<usize>>(&s).ok())
        .filter(|v| !v.is_empty())
}

fn save_pick(id: &str, v: &[usize]) {
    if let (Some(d), Ok(json)) = (dir(), serde_json::to_string(v)) {
        let _ = std::fs::create_dir_all(&d);
        let _ = crate::model::atomic_write(&d.join(format!("pick-{id}.json")), json.as_bytes());
    }
}

struct PickApp {
    id: String,
    list: Vec<(String, u64)>,
    on: Vec<bool>,
    note: String,
}

pub fn spawn(id: u64) {
    if let Ok(exe) = crate::platform::launch_path() {
        let _ = std::process::Command::new(exe).arg("--torrent-files").arg(id.to_string()).spawn();
    }
}

pub fn run(id: &str) -> eframe::Result<()> {
    let list = files(id);
    let chosen = picked(id);
    let on = (0..list.len()).map(|i| chosen.as_ref().is_none_or(|c| c.contains(&i))).collect();
    let note = if list.is_empty() {
        "The file list arrives once the torrent has started (it reads it from the peers). Start it, then open this again.".into()
    } else {
        String::new()
    };
    let app = PickApp { id: id.to_string(), list, on, note };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Torrent files")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([560.0, 440.0]);
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

impl eframe::App for PickApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::bottom("pick_actions").frame(egui::Frame::new().fill(pl().surface).inner_margin(12)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let chosen: Vec<usize> = self.on.iter().enumerate().filter(|(_, o)| **o).map(|(i, _)| i).collect();
                let size: u64 = chosen.iter().map(|i| self.list[*i].1).sum();
                ui.label(format!("{} of {} file(s), {}", chosen.len(), self.list.len(), crate::model::format_bytes(size)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = !chosen.is_empty();
                    if ui.add_enabled(ok, egui::Button::new("Save")).clicked() {
                        let all = chosen.len() == self.list.len();
                        save_pick(&self.id, if all { &[] } else { &chosen });
                        // A running torrent starts again with the new choice.
                        if let Ok(id) = self.id.parse::<u64>() {
                            let _ = crate::native_host::app_control(Some(id), "restart");
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("None").clicked() {
                        self.on.iter_mut().for_each(|o| *o = false);
                    }
                    if ui.button("All").clicked() {
                        self.on.iter_mut().for_each(|o| *o = true);
                    }
                });
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pl().bg).inner_margin(12)).show(ctx, |ui| {
            if !self.note.is_empty() {
                ui.label(&self.note);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (i, (name, size)) in self.list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.on[i], name.as_str());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(crate::model::format_bytes(*size)).color(pl().dim));
                        });
                    });
                }
            });
        });
    }
}
