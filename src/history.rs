//! Download history: every finished download, kept when its row is removed
//! from the list, searchable in its own window (Tools → Download history),
//! and the warning "you downloaded this before" (the same link, or a file
//! of the same name and size, or the same contents).
//!
//! Stored as one JSON line per download in `history.jsonl` in the settings
//! folder, newest last, the newest `KEEP` kept.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "history.jsonl";
const KEEP: usize = 5000;
/// Files up to this size get a checksum (in the background) for "the same
/// contents under another name".
const HASH_UP_TO: u64 = 2 << 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Entry {
    pub url: String,
    pub name: String,
    /// Where the file was saved (its full path).
    pub path: String,
    pub size: u64,
    /// When it finished (RFC 3339).
    pub at: String,
    /// SHA-256 of the contents, for files up to 2 GB.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}

impl Entry {
    /// "22 Nov 2026, 14:05", in local time.
    pub fn when(&self) -> String {
        chrono::DateTime::parse_from_rfc3339(&self.at)
            .map(|t| t.with_timezone(&chrono::Local).format("%d %b %Y, %H:%M").to_string())
            .unwrap_or_else(|_| self.at.clone())
    }
}

fn file() -> Option<PathBuf> {
    crate::model::config_dir().map(|d| d.join(FILE))
}

/// Every entry, newest first.
pub fn load() -> Vec<Entry> {
    let Some(p) = file() else { return Vec::new() };
    let text = std::fs::read_to_string(p).unwrap_or_default();
    let mut v: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v
}

fn save_all(newest_first: &[Entry]) {
    let Some(p) = file() else { return };
    let mut text = String::new();
    for e in newest_first.iter().take(KEEP).rev() {
        if let Ok(l) = serde_json::to_string(e) {
            text.push_str(&l);
            text.push('\n');
        }
    }
    let _ = crate::model::atomic_write(&p, text.as_bytes());
}

/// Note a finished download (the checksum is worked out in the background
/// first, for files up to 2 GB). Returns at once.
pub fn record(url: &str, path: &Path, size: u64) {
    let e = Entry {
        url: url.to_string(),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        size,
        at: chrono::Local::now().to_rfc3339(),
        sha256: String::new(),
    };
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new().name("history".into()).spawn(move || {
        let mut e = e;
        if size > 0 && size <= HASH_UP_TO {
            e.sha256 = sha256_of(&path).unwrap_or_default();
        }
        // The same contents as a file downloaded earlier, under another
        // name or in another folder: say so (both are kept).
        if let Some(old) = same_contents_in(&load(), &e) {
            let _ = notify_rust::Notification::new()
                .summary("Download Craft — the same file again")
                .body(&format!("{} has the same contents as {} ({}).", e.name, old.name, old.path))
                .show();
        }
        append(e);
    });
}

fn append(e: Entry) {
    let _guard = crate::model::fs_lock("history");
    let mut all = load();
    // The same file again (a redownload, a re-check): one entry, the newest.
    all.retain(|x| x.path != e.path);
    all.insert(0, e);
    save_all(&all);
}

fn sha256_of(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(format!("{:x}", h.finalize()))
}

/// The link without what does not name the file (`#…`).
fn same_link(a: &str, b: &str) -> bool {
    let cut = |s: &str| s.split('#').next().unwrap_or(s).trim().trim_end_matches('/').to_string();
    !a.trim().is_empty() && cut(a) == cut(b)
}

/// Was this downloaded before? The same link, or a file of the same name
/// and size (when the size is known). The newest match.
pub fn find_in(all: &[Entry], url: &str, name: &str, size: Option<u64>) -> Option<Entry> {
    all.iter()
        .find(|e| {
            same_link(&e.url, url)
                || (!name.trim().is_empty()
                    && e.name.eq_ignore_ascii_case(name.trim())
                    && size.is_some_and(|s| s > 0 && s == e.size))
        })
        .cloned()
}

pub fn find(url: &str, name: &str, size: Option<u64>) -> Option<Entry> {
    find_in(&load(), url, name, size)
}

/// A finished file with the same contents as an earlier one still on disk
/// (another name or folder): that earlier one.
pub fn same_contents_in(all: &[Entry], e: &Entry) -> Option<Entry> {
    if e.sha256.is_empty() {
        return None;
    }
    all.iter().find(|x| x.sha256 == e.sha256 && x.path != e.path && Path::new(&x.path).exists()).cloned()
}

/// The words for "downloaded before" in a dialog.
pub fn before_note(e: &Entry) -> String {
    let there = if Path::new(&e.path).exists() { "" } else { " (the file is no longer there)" };
    format!("Downloaded before on {}: {}{there}", e.when(), e.name)
}

pub fn clear() {
    if let Some(p) = file() {
        let _ = std::fs::remove_file(p);
    }
}

/// Open the history window (a process of its own).
pub fn spawn() {
    let Ok(exe) = crate::platform::launch_path() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--history");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008);
    }
    let _ = cmd.spawn();
}

// ---------------- the window ----------------

struct HistoryApp {
    all: Vec<Entry>,
    filter: String,
    note: String,
    confirm_clear: bool,
}

impl HistoryApp {
    fn shown(&self) -> Vec<&Entry> {
        let f = self.filter.trim().to_lowercase();
        self.all
            .iter()
            .filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f) || e.url.to_lowercase().contains(&f) || e.path.to_lowercase().contains(&f))
            .collect()
    }

    fn download_again(&mut self, e: &Entry) {
        let path = Path::new(&e.path);
        let dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let mut item = crate::model::DownloadItem::new(0, e.url.clone(), e.name.clone(), dir, crate::confirm::load_prefs().connections);
        self.note = match crate::model::append_to_queue(&mut item) {
            crate::model::AppendResult::Added(id) => {
                crate::confirm::wake_ids(&[id]);
                format!("Downloading {} again.", e.name)
            }
            crate::model::AppendResult::AlreadyQueued(_) => format!("{} is already in the list.", e.name),
            crate::model::AppendResult::Error(err) => format!("Could not add it: {err}"),
        };
    }
}

impl eframe::App for HistoryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = crate::app::pl();
        egui::TopBottomPanel::top("history-top").frame(egui::Frame::new().fill(t.bg).inner_margin(12)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Download history").size(17.0).strong());
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("{} finished downloads", self.all.len())).color(t.dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.confirm_clear {
                        if ui.button("Yes, clear it").clicked() {
                            clear();
                            self.all.clear();
                            self.confirm_clear = false;
                            self.note = "History cleared (the files themselves are kept).".into();
                        }
                        if ui.button("Keep it").clicked() {
                            self.confirm_clear = false;
                        }
                        ui.label("Clear the whole history?");
                    } else if ui.button("Clear history").clicked() {
                        self.confirm_clear = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Search by name, link or folder")
                    .desired_width(f32::INFINITY),
            );
            if !self.note.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&self.note).color(t.dim));
            }
        });
        let mut again: Option<Entry> = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 12 })).show(ctx, |ui| {
            let rows = self.shown().into_iter().cloned().collect::<Vec<_>>();
            if rows.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(if self.all.is_empty() { "Nothing yet: finished downloads appear here, and stay when you remove them from the list." } else { "Nothing matches the search." }).color(t.dim));
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 52.0, rows.len(), |ui, range| {
                for e in &rows[range] {
                    egui::Frame::new().fill(t.surface).corner_radius(8.0).inner_margin(8).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let exists = Path::new(&e.path).exists();
                                ui.label(egui::RichText::new(&e.name).strong().color(if exists { t.txt } else { t.dim }));
                                let folder = Path::new(&e.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {}{}",
                                        e.when(),
                                        crate::model::format_bytes(e.size),
                                        folder,
                                        if exists { "" } else { " · file removed" }
                                    ))
                                    .size(11.5)
                                    .color(t.dim),
                                );
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Download again").clicked() {
                                    again = Some(e.clone());
                                }
                                if ui.button("Copy link").clicked() {
                                    ui.ctx().copy_text(e.url.clone());
                                    self.note = "Link copied.".into();
                                }
                                if ui.button("Folder").clicked() {
                                    crate::app::open_path(Path::new(&e.path), true);
                                }
                                if Path::new(&e.path).exists() && ui.button("Open").clicked() {
                                    crate::app::open_path(Path::new(&e.path), false);
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        });
        if let Some(e) = again {
            self.download_again(&e);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// `--history`: the history window.
pub fn run() -> eframe::Result<()> {
    let Some(_slot) = crate::model::claim_window("history") else { return Ok(()) };
    let app = HistoryApp { all: load(), filter: String::new(), note: String::new(), confirm_clear: false };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download history — Download Craft")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([760.0, 560.0])
        .with_min_inner_size([520.0, 320.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Download history",
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            crate::platform::guard_gpu(cc);
            crate::app::setup_fonts(cc);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(url: &str, name: &str, size: u64, sha: &str, path: &str) -> Entry {
        Entry { url: url.into(), name: name.into(), path: path.into(), size, at: "2026-11-22T10:00:00+06:00".into(), sha256: sha.into() }
    }

    #[test]
    fn a_download_seen_before_is_found() {
        let all = vec![e("https://a.example/film.mp4", "film.mp4", 100, "", "C:/d/film.mp4")];
        assert!(find_in(&all, "https://a.example/film.mp4#t=10", "other.mp4", None).is_some(), "same link");
        assert!(find_in(&all, "https://mirror.example/x", "FILM.mp4", Some(100)).is_some(), "same name and size");
        assert!(find_in(&all, "https://mirror.example/x", "film.mp4", Some(99)).is_none(), "another size");
        assert!(find_in(&all, "https://mirror.example/x", "film.mp4", None).is_none(), "size unknown: name alone is not enough");
        assert!(find_in(&all, "", "", None).is_none());
    }

    #[test]
    fn same_contents_under_another_name() {
        let dir = std::env::temp_dir().join(format!("dc-hist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("a.bin");
        std::fs::write(&old, b"x").unwrap();
        let all = vec![e("u1", "a.bin", 1, "abc", old.to_str().unwrap())];
        let new = e("u2", "b.bin", 1, "abc", dir.join("b.bin").to_str().unwrap());
        assert_eq!(same_contents_in(&all, &new).map(|x| x.name), Some("a.bin".into()));
        let other = e("u3", "c.bin", 1, "def", dir.join("c.bin").to_str().unwrap());
        assert!(same_contents_in(&all, &other).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
