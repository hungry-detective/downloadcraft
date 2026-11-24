//! YouTube subscriptions: a channel or playlist whose new videos download
//! by themselves. Subscribing counts everything already there as seen; the
//! app then looks at start and every six hours, queues what is new (into a
//! folder named after the channel) and says so.
//!
//! Kept in `subscriptions.json` in the settings folder. Listing goes
//! through yt-dlp (`--flat-playlist`), as the Add dialog's playlist picker.

use serde::{Deserialize, Serialize};

const FILE: &str = "subscriptions.json";
/// How often the main window looks for new videos.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// Seen links kept per subscription (newest channels list thousands).
const SEEN_MAX: usize = 20000;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Sub {
    pub url: String,
    pub title: String,
    /// Video links already there or already queued.
    #[serde(default)]
    pub seen: Vec<String>,
    /// Quality for its videos ("" = best, "1080p", "mp3"…).
    #[serde(default)]
    pub quality: String,
    #[serde(default)]
    pub last_check: String,
    /// Videos queued by the last check.
    #[serde(default)]
    pub last_new: usize,
    #[serde(default)]
    pub last_error: String,
}

fn file() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|d| d.join(FILE))
}

pub fn load() -> Vec<Sub> {
    file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(all: &[Sub]) {
    if let (Some(p), Ok(t)) = (file(), serde_json::to_string_pretty(all)) {
        let _ = crate::model::atomic_write(&p, t.as_bytes());
    }
}

/// "Qabas Anime - Videos" -> "Qabas Anime": yt-dlp names a channel's tab.
pub fn clean_title(t: &str) -> String {
    let t = t.trim();
    for tab in [" - Videos", " - Shorts", " - Live", " - Streams", " - Home", " - Playlists"] {
        if let Some(x) = t.strip_suffix(tab) {
            return x.trim().to_string();
        }
    }
    t.to_string()
}

/// A link that can be followed: a channel or a playlist on YouTube.
pub fn followable(url: &str) -> bool {
    crate::engine::is_youtube_list(url)
}

/// The videos in `listed` not seen before, oldest first (so they queue in
/// the order they came out).
pub fn new_videos(sub: &Sub, listed: &[(String, String)]) -> Vec<(String, String)> {
    let seen: std::collections::HashSet<&str> = sub.seen.iter().map(String::as_str).collect();
    let mut v: Vec<(String, String)> = listed.iter().filter(|(u, _)| !seen.contains(u.as_str())).cloned().collect();
    v.reverse();
    v
}

/// Subscribe: list it, and count what is there as seen.
pub async fn subscribe(url: &str, quality: &str, opts: &crate::engine::EngineOpts) -> Result<Sub, String> {
    if !followable(url) {
        return Err("Paste a YouTube channel or playlist link.".into());
    }
    let (title, listed) = crate::engine::playlist_entries(url, opts).await.map_err(|e| format!("{e:#}"))?;
    Ok(Sub {
        url: url.trim().to_string(),
        title: clean_title(&title),
        seen: listed.into_iter().map(|(u, _)| u).collect(),
        quality: quality.to_string(),
        last_check: chrono::Local::now().to_rfc3339(),
        last_new: 0,
        last_error: String::new(),
    })
}

/// Look at every subscription: queue new videos and start them. Returns
/// one line per subscription that had something to say.
pub async fn check_all(opts: &crate::engine::EngineOpts) -> Vec<String> {
    let mut all = load();
    let mut notes = Vec::new();
    for sub in all.iter_mut() {
        match crate::engine::playlist_entries(&sub.url, opts).await {
            Ok((title, listed)) => {
                if !title.trim().is_empty() {
                    sub.title = clean_title(&title);
                }
                let fresh = new_videos(sub, &listed);
                let queued = queue(sub, &fresh);
                sub.seen.extend(fresh.into_iter().map(|(u, _)| u));
                if sub.seen.len() > SEEN_MAX {
                    let cut = sub.seen.len() - SEEN_MAX;
                    sub.seen.drain(..cut);
                }
                sub.last_new = queued;
                sub.last_error.clear();
                if queued > 0 {
                    notes.push(format!("{}: {queued} new video{}", sub.title, if queued == 1 { "" } else { "s" }));
                }
            }
            Err(e) => sub.last_error = format!("{e:#}"),
        }
        sub.last_check = chrono::Local::now().to_rfc3339();
    }
    // A window may have changed the list meanwhile: merge by link.
    let mut now = load();
    for s in all {
        if let Some(n) = now.iter_mut().find(|n| n.url == s.url) {
            *n = s;
        }
    }
    save(&now);
    notes
}

/// Queue (and start) videos into the channel's own folder.
fn queue(sub: &Sub, videos: &[(String, String)]) -> usize {
    let prefs = crate::confirm::load_prefs();
    let folder = std::path::Path::new(&prefs.save_dir).join(crate::engine::sanitize_filename(sub.title.trim()));
    let dir = folder.to_string_lossy().into_owned();
    let ext = match sub.quality.as_str() {
        "mp3" => "mp3",
        "m4a" => "m4a",
        _ => "mp4",
    };
    let mut ids = Vec::new();
    for (url, title) in videos {
        let name = crate::engine::sanitize_filename(&format!("{}.{ext}", title.trim()));
        let mut item = crate::model::DownloadItem::new(0, url.clone(), name, dir.clone(), prefs.connections);
        item.quality_pref = sub.quality.clone();
        if let crate::model::AppendResult::Added(id) = crate::model::append_to_queue(&mut item) {
            ids.push(id);
        }
    }
    if !ids.is_empty() {
        crate::confirm::wake_ids(&ids);
    }
    ids.len()
}

pub fn spawn() {
    let Ok(exe) = crate::platform::launch_path() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--subs");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008);
    }
    let _ = cmd.spawn();
}

// ---------------- the window ----------------

enum Job {
    Subscribe(std::sync::mpsc::Receiver<Result<Sub, String>>),
    Check(std::sync::mpsc::Receiver<Vec<String>>),
}

struct SubsApp {
    all: Vec<Sub>,
    link: String,
    quality: String,
    note: String,
    job: Option<Job>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
}

impl SubsApp {
    fn opts() -> crate::engine::EngineOpts {
        crate::engine::EngineOpts::default()
    }
}

impl eframe::App for SubsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let t = crate::app::pl();
        // A job finished.
        let mut done = false;
        match &self.job {
            Some(Job::Subscribe(rx)) => {
                if let Ok(r) = rx.try_recv() {
                    done = true;
                    match r {
                        Ok(s) => {
                            self.note = format!("Following {} ({} videos there now; new ones download by themselves).", s.title, s.seen.len());
                            self.all = load();
                            self.all.retain(|x| x.url != s.url);
                            self.all.push(s);
                            save(&self.all);
                            self.link.clear();
                        }
                        Err(e) => self.note = e,
                    }
                }
            }
            Some(Job::Check(rx)) => {
                if let Ok(notes) = rx.try_recv() {
                    done = true;
                    self.all = load();
                    self.note = if notes.is_empty() { "No new videos.".into() } else { notes.join(" · ") };
                }
            }
            None => {}
        }
        if done {
            self.job = None;
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        let busy = self.job.is_some();
        egui::TopBottomPanel::top("subs-top").frame(egui::Frame::new().fill(t.bg).inner_margin(12)).show(ctx, |ui| {
            ui.label(egui::RichText::new("YouTube subscriptions").size(17.0).strong());
            ui.label(egui::RichText::new("Follow a channel or playlist: its new videos download by themselves (checked at start and every 6 hours).").color(t.dim));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.link).hint_text("https://www.youtube.com/@channel or a playlist link").desired_width(ui.available_width() - 260.0));
                egui::ComboBox::from_id_salt("subs-q")
                    .width(90.0)
                    .selected_text(if self.quality.is_empty() { "Best".to_string() } else { self.quality.clone() })
                    .show_ui(ui, |ui| {
                        for (k, l) in [("", "Best"), ("1080p", "1080p"), ("720p", "720p"), ("480p", "480p"), ("mp3", "MP3"), ("m4a", "M4A")] {
                            ui.selectable_value(&mut self.quality, k.to_string(), l);
                        }
                    });
                if ui.add_enabled(!busy && !self.link.trim().is_empty(), egui::Button::new("Subscribe")).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (url, q) = (self.link.trim().to_string(), self.quality.clone());
                    self.rt.spawn(async move {
                        let _ = tx.send(subscribe(&url, &q, &SubsApp::opts()).await);
                    });
                    self.job = Some(Job::Subscribe(rx));
                    self.note = "Reading the channel…".into();
                }
                if ui.add_enabled(!busy && !self.all.is_empty(), egui::Button::new("Check now")).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.rt.spawn(async move {
                        let _ = tx.send(check_all(&SubsApp::opts()).await);
                    });
                    self.job = Some(Job::Check(rx));
                    self.note = "Looking for new videos…".into();
                }
            });
            if !self.note.is_empty() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&self.note).color(t.dim));
            }
        });
        let mut remove: Option<String> = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 12 })).show(ctx, |ui| {
            if self.all.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| ui.label(egui::RichText::new("No subscriptions yet.").color(t.dim)));
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for s in &self.all {
                    egui::Frame::new().fill(t.surface).corner_radius(8.0).inner_margin(8).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&s.title).strong());
                                let when = chrono::DateTime::parse_from_rfc3339(&s.last_check)
                                    .map(|d| d.with_timezone(&chrono::Local).format("%d %b, %H:%M").to_string())
                                    .unwrap_or_default();
                                let state = if !s.last_error.is_empty() {
                                    format!("last check {when}: {}", s.last_error)
                                } else {
                                    format!("last check {when}: {} new · quality {}", s.last_new, if s.quality.is_empty() { "best" } else { &s.quality })
                                };
                                ui.label(egui::RichText::new(state).size(11.5).color(if s.last_error.is_empty() { t.dim } else { t.warning }));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add_enabled(!busy, egui::Button::new("Unsubscribe")).clicked() {
                                    remove = Some(s.url.clone());
                                }
                                if ui.button("Open").clicked() {
                                    crate::platform::open_url(&s.url);
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        });
        if let Some(u) = remove {
            self.all.retain(|s| s.url != u);
            save(&self.all);
            self.note = "Unsubscribed (videos already downloaded are kept).".into();
        }
    }
}

/// `--subs`: the subscriptions window.
pub fn run() -> eframe::Result<()> {
    let Some(_slot) = crate::model::claim_window("subs") else { return Ok(()) };
    let rt = std::sync::Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime"));
    let app = SubsApp { all: load(), link: String::new(), quality: String::new(), note: String::new(), job: None, rt };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("YouTube subscriptions — Download Craft")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([760.0, 520.0])
        .with_min_inner_size([560.0, 320.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "YouTube subscriptions",
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

    #[test]
    fn only_videos_not_seen_are_new_and_oldest_first() {
        let sub = Sub { url: "u".into(), seen: vec!["v1".into(), "v2".into()], ..Default::default() };
        // A channel lists its newest first.
        let listed = vec![("v4".into(), "Four".into()), ("v3".into(), "Three".into()), ("v2".into(), "Two".into()), ("v1".into(), "One".into())];
        let fresh = new_videos(&sub, &listed);
        assert_eq!(fresh.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(), vec!["v3", "v4"]);
        assert!(new_videos(&Sub { seen: vec!["v1".into(), "v2".into(), "v3".into(), "v4".into()], ..sub }, &listed).is_empty());
    }

    /// A real channel (network and yt-dlp; `cargo test -- --ignored live_subscribe`).
    #[tokio::test]
    #[ignore]
    async fn live_subscribe() {
        let s = subscribe("https://www.youtube.com/@QabasAnime/videos", "", &crate::engine::EngineOpts::default()).await.unwrap();
        eprintln!("title {:?}, {} videos seen", s.title, s.seen.len());
        assert!(!s.title.is_empty() && !s.seen.is_empty());
    }

    #[test]
    fn channels_and_playlists_can_be_followed() {
        assert!(followable("https://www.youtube.com/playlist?list=PLabc"));
        assert!(followable("https://www.youtube.com/@SomeChannel"));
        assert!(!followable("https://www.youtube.com/watch?v=4tDqtN4t2zw"));
        assert!(!followable("https://example.com/feed"));
        assert_eq!(clean_title("Qabas Anime - Videos"), "Qabas Anime");
        assert_eq!(clean_title("My Playlist"), "My Playlist");
    }
}
