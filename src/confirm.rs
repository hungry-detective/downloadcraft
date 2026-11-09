//! IDM-style standalone "Download File Info" dialog.
//!
//! Launched as a separate OS window (always on top) when the browser
//! extension captures a download:
//!   `download_craft.exe --confirm-download --url <url> [--filename ..]
//!    [--referer ..] [--cookie ..] [--mime ..]`
//!
//! Unlike the in-canvas Add modal in the main window, this pops in front of
//! whatever the user is doing — one window per captured download, like IDM.
//! It writes straight to the shared `queue.json` (+ `wake.json` for
//! Start Download), so it works even when the main app is closed.

use crate::app::pl;
use crate::dm::*;
use crate::engine::{self, EngineOpts};
use crate::model::{
    atomic_write, category_of, config_dir, filename_from_url, format_bytes, is_download_url,
    DownloadItem,
};
use std::sync::mpsc::Receiver;

/// Seconds → "3:26" badge for browser-known media durations.
fn format_dur(secs: u64) -> String {
    if secs == 0 || secs >= 86400 {
        return String::new();
    }
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn wake_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("wake.json"))
}

fn ui_path() -> Option<std::path::PathBuf> {
    config_dir().map(|p| p.join("ui.json"))
}

/// Claim (remove) our URL from the native inbox so the main window does not
/// prompt for it too. Runs at dialog open; Cancel therefore discards.
fn claim_inbox(url: &str) -> Option<crate::native_host::InboxItem> {
    let path=crate::native_host::inbox_path()?;
    let guard=crate::model::fs_lock("inbox");
    if !guard.owned() {return None;}
    claim_inbox_at(&path,url)
}
fn claim_inbox_at(path:&std::path::Path,url:&str) -> Option<crate::native_host::InboxItem> {
    let data=std::fs::read_to_string(path).ok()?;
    // Retain unreadable pending captures for recovery.
    let items:Vec<crate::native_host::InboxItem>=serde_json::from_str(&data).ok()?;
    let claimed=items.iter().find(|i|i.url==url)?.clone();
    let rest:Vec<_>=items.into_iter().filter(|i|i.url!=url).collect();
    if rest.is_empty() {std::fs::remove_file(path).ok()?;}
    else {atomic_write(path,serde_json::to_string_pretty(&rest).ok()?.as_bytes()).ok()?;}
    Some(claimed)
}

#[derive(Debug, Default)]
pub(crate) struct UiPrefs {
    pub(crate) save_dir: String,
    pub(crate) connections: usize,
    pub(crate) category_folders: bool,
    pub(crate) user_agent: String,
    pub(crate) proxy_url: String,
    pub(crate) timeout_secs: u64,
    pub(crate) rename_template: String,
    pub(crate) folder_rules: Vec<crate::model::FolderRule>,
    /// Settings -> "Remember the last folders used".
    pub(crate) remember_folders: bool,
    /// Folders used before, newest first (only kept while that is on).
    pub(crate) recent_folders: Vec<String>,
}

pub(crate) fn load_prefs() -> UiPrefs {
    let mut p = UiPrefs {
        connections: 8,
        category_folders: true,
        timeout_secs: 60,
        ..Default::default()
    };
    let v: serde_json::Value = ui_path()
        .and_then(|x| std::fs::read_to_string(x).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    if let Some(s) = v.get("save_dir").and_then(|x| x.as_str()) {
        if !s.trim().is_empty() {
            p.save_dir = s.to_string();
        }
    }
    if p.save_dir.is_empty() && std::env::var_os("DOWNLOAD_CRAFT_DOWNLOADS").is_some_and(|v| !v.is_empty()) {
        p.save_dir = crate::model::default_download_dir().to_string_lossy().into_owned();
    }
    if p.save_dir.is_empty() {
        p.save_dir = dirs::download_dir()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
    }
    if let Some(n) = v.get("connections_default").and_then(|x| x.as_u64()) {
        p.connections = (n as usize).clamp(1, 32);
    }
    if let Some(b) = v.get("category_folders").and_then(|x| x.as_bool()) {
        p.category_folders = b;
    }
    if let Some(s) = v.get("user_agent").and_then(|x| x.as_str()) {
        p.user_agent = s.to_string();
    }
    if let Some(s) = v.get("proxy_url").and_then(|x| x.as_str()) {
        p.proxy_url = s.to_string();
    }
    if let Some(n) = v.get("timeout_secs").and_then(|x| x.as_u64()) {
        p.timeout_secs = n.clamp(5, 600);
    }
    if let Some(s) = v.get("rename_template").and_then(|x| x.as_str()) {
        if !s.trim().is_empty() {
            p.rename_template = s.to_string();
        }
    }
    if p.rename_template.trim().is_empty() {
        p.rename_template = "{name}".to_string();
    }
    if let Some(arr) = v.get("folder_rules").and_then(|x| x.as_array()) {
        let rules: Vec<crate::model::FolderRule> = arr
            .iter()
            .filter_map(|r| {
                Some(crate::model::FolderRule {
                    exts: r.get("exts")?.as_str()?.to_string(),
                    folder: r.get("folder")?.as_str()?.to_string(),
                })
            })
            .take(8)
            .collect();
        if !rules.is_empty() {
            p.folder_rules = rules;
        }
    }
    if p.folder_rules.is_empty() {
        p.folder_rules = crate::model::default_folder_rules();
    }
    p.remember_folders = v.get("remember_folders").and_then(|x| x.as_bool()).unwrap_or(false);
    if p.remember_folders {
        p.recent_folders = v
            .get("recent_folders")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str()).map(str::to_string).collect())
            .unwrap_or_default();
        // FDM-style: start where the last download went.
        if let Some(last) = p.recent_folders.iter().find(|d| std::path::Path::new(d).is_dir()) {
            p.save_dir = last.clone();
        }
    }
    p
}

/// How many folders "Remember the last folders used" keeps.
const RECENT_FOLDERS: usize = 8;

/// `dir` moved to the front of `list`, kept to RECENT_FOLDERS entries.
fn push_recent(list: &[String], dir: &str) -> Vec<String> {
    let dir = dir.trim().trim_end_matches(['\\', '/']).to_string();
    let mut out = vec![dir.clone()];
    out.extend(
        list.iter()
            .filter(|d| !d.trim_end_matches(['\\', '/']).eq_ignore_ascii_case(&dir))
            .cloned(),
    );
    out.truncate(RECENT_FOLDERS);
    out
}

/// Record the folder a download went to (only while the setting is on).
fn save_recent_folder(dir: &str) {
    if dir.trim().is_empty() {
        return;
    }
    let _guard = crate::model::fs_lock("ui-settings");
    if !_guard.owned() {
        return;
    }
    let Some(path) = ui_path() else { return };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(obj) = v.as_object_mut() else { return };
    if !obj.get("remember_folders").and_then(|x| x.as_bool()).unwrap_or(false) {
        return;
    }
    let old: Vec<String> = obj
        .get("recent_folders")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str()).map(str::to_string).collect())
        .unwrap_or_default();
    obj.insert("recent_folders".to_string(), serde_json::json!(push_recent(&old, dir)));
    if let Ok(json) = serde_json::to_string_pretty(&v) {
        let _ = atomic_write(&path, json.as_bytes());
    }
}

#[cfg(test)]
mod recent_folder_tests {
    use super::push_recent;

    #[test]
    fn newest_first_without_duplicates() {
        let l = push_recent(&[], r"D:\Movies");
        let l = push_recent(&l, r"E:\Music");
        let l = push_recent(&l, r"d:\movies\");
        assert_eq!(l, vec![r"d:\movies".to_string(), r"E:\Music".to_string()]);
    }

    #[test]
    fn keeps_eight() {
        let mut l = Vec::new();
        for i in 0..12 {
            l = push_recent(&l, &format!(r"C:\f{i}"));
        }
        assert_eq!(l.len(), 8);
        assert_eq!(l[0], r"C:\f11");
    }
}

fn save_remember_category(value: bool) {
    let Some(path) = ui_path() else { return };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "category_folders".to_string(),
            serde_json::Value::Bool(value),
        );
        if let Ok(json) = serde_json::to_string_pretty(&v) {
            let _ = atomic_write(&path, json.as_bytes());
        }
    }
}

const NOT_RUNNING: &str = "Download Craft isn't running — open it, then click Download again.";
const STARTING: &str = "Starting Download Craft…";
/// How long Download waits for the app it started before giving up.
const START_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// True when the main window is already running (fresh heartbeat).
/// Replaces process-name counting (two dialogs used to look "live").
fn main_app_running() -> bool {
    crate::model::is_main_live()
}

/// Launch the main window detached (fire-and-forget).
fn launch_main_app() {
    let Ok(exe) = crate::platform::launch_path() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS so the GUI outlives this dialog.
        cmd.creation_flags(0x00000008);
    }
    // Its own process group on Linux/macOS: the browser tears down the
    // native host's group, which must not take the app with it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.spawn();
}

/// The download the dialog starts while it is still open (IDM's way): by
/// the time Download is pressed, part of the file is already there.
struct PreStart {
    shared: std::sync::Arc<engine::SharedState>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// The temporary file it downloads to.
    dest: std::path::PathBuf,
}

struct ConfirmApp {
    pre: Option<PreStart>,
    pre_tried: bool,
    url: String,
    filename: String,
    category: String,
    remember_path: bool,
    save_base: String,
    description: String,
    referer: String,
    cookie: String,
    /// Captured request headers of this browser capture (from the inbox).
    headers: std::collections::BTreeMap<String, String>,
    /// The browser's side of the capture (method/body, headers, IP, proxy).
    browser: Option<crate::model::BrowserContext>,
    pair_url: String,
    pair_mime: String,
    /// Page/media MIME from the extension (first hint for naming when the
    /// URL carries no usable name and no probe has run yet).
    page_mime: String,
    /// Media duration in seconds from the extension (best-effort badge).
    duration_secs: u64,
    expects_audio: bool,
    quality_pref: String,
    connections: usize,
    user_agent: String,
    proxy_url: String,
    timeout_secs: u64,
    rename_template: String,
    /// Remembered folders for the path menu (empty when the setting is off).
    recent_folders: Vec<String>,
    folder_rules: Vec<crate::model::FolderRule>,
    probing: bool,
    probe_size: Option<u64>,
    /// Size the browser measured in its own session (`--size`): what a
    /// refused off-session re-probe falls back to instead of "—".
    browser_size: Option<u64>,
    /// The name the dialog made up itself (no `--filename`), while the user
    /// has not changed it: a server/title name may replace exactly that.
    auto_name: Option<String>,
    probe_rx: Option<Receiver<(Option<u64>, Option<String>)>>,
    rt: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    centered: bool,
    show_adv: bool,
    done: bool,
    /// Download was pressed while the app was closed: it was started at this
    /// moment and the download is handed over once it is live (issue #7).
    waiting_app: Option<std::time::Instant>,
    exited: bool,
    notice: String,
    /// State for controls the concept does not show (dropdown + extras).
    cat_open: bool,
    /// Path field's recent-folders menu (same pattern as the category combo).
    recent_open: bool,
    win_h: f32,
    adding_cat: bool,
    new_cat: String,
    custom_cats: Vec<String>,
}

const CATEGORIES: &[&str] = &[
    "Video",
    "Music",
    "Image",
    "Document",
    "Compressed",
    "Apps",
    "Other",
];

// ---------- concept geometry (tokens live in `crate::dm`) ----------
// The capture popup is the same "Add download" card as the in-canvas modal,
// so it reuses `dm`'s tokens and row offsets verbatim: 562x253 at rest, with
// the header's window controls, the size chip and the gear panel in the same
// places.
/// Advanced (gear) panel height: category + description, referer + cookie,
/// connections, remember-path. The concept draws this collapsed.
const PANEL_H: f32 = 178.0;
/// Height of one dim note line inside the panel.
const NOTE_H: f32 = 12.0;

/// Loads Segoe UI — the family the concept asks for — plus a semibold cut for
/// its 600-weight title and primary button. Missing system fonts fall back to
/// the bundled defaults, so this is safe on any machine.
fn setup_popup_fonts(cc: &eframe::CreationContext<'_>) {
    // `true`: this whole window is the concept, so Segoe UI also becomes its
    // default proportional face.
    crate::fonts::install(cc, true);
}

/// Concept "+" square: accent fill, white plus (adds a custom category).
fn plus_square(ui: &mut egui::Ui, rect: egui::Rect, salt: u64) -> egui::Response {
    let r = ui.interact(rect, egui::Id::new(("popup-plus", salt)), egui::Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    p.rect(
        rect,
        DM_RAD,
        if r.hovered() { DM_ACC2() } else { DM_ACC() },
        egui::Stroke::NONE,
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    let st = egui::Stroke::new(2.0, egui::Color32::WHITE);
    p.line_segment([egui::pos2(c.x - 6.0, c.y), egui::pos2(c.x + 6.0, c.y)], st);
    p.line_segment([egui::pos2(c.x, c.y - 6.0), egui::pos2(c.x, c.y + 6.0)], st);
    r
}

/// Floating option list anchored under `resp` — the concept's dropdown body,
/// shared by the category combo and the path field's recent folders, so both
/// menus look and behave identically.
fn options_menu(
    resp: &egui::Response,
    options: &[String],
    open: &mut bool,
    selected: &str,
) -> Option<String> {
    if options.is_empty() {
        *open = false;
        return None;
    }
    let mut picked = None;
    egui::Popup::from_response(resp)
        .open_bool(&mut *open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(6.0)
        .frame(
            egui::Frame::new()
                .fill(pl().dm_card)
                .stroke(egui::Stroke::new(1.0_f32, pl().dm_line))
                .corner_radius(DM_RAD)
                .inner_margin(egui::Margin::symmetric(4, 4)),
        )
        .show(|ui| {
            ui.set_min_width(280.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                    let w = ui.available_width().max(272.0);
                    for o in options {
                        let (r, ir) =
                            ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
                        if ir.hovered() {
                            ui.painter().rect_filled(r, 4.0, pl().dm_hover);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        ui.painter().text(
                            egui::pos2(r.left() + 8.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            elide_left(o, 52),
                            reg(11.0),
                            if o.as_str() == selected { DM_ACC() } else { pl().dm_txt },
                        );
                        if ir.clicked() {
                            picked = Some(o.clone());
                        }
                    }
                });
        });
    if picked.is_some() {
        *open = false;
    }
    picked
}

/// Concept dropdown: field box, current value, trailing caret, option list.
fn combo(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    current: &str,
    options: &[String],
    open: &mut bool,
) -> Option<String> {
    let resp = ui.interact(rect, egui::Id::new(("popup-combo", id)), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    dm_box(p, rect);
    p.text(
        egui::pos2(rect.left() + 13.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        elide_left(current, 26),
        reg(11.0),
        pl().dm_txt,
    );
    dm_icon(
        p,
        egui::Rect::from_center_size(
            egui::pos2(rect.right() - 16.0, rect.center().y),
            egui::Vec2::splat(11.0),
        ),
        DmIcon::Chevron,
        pl().dm_dim,
        2.0,
    );
    if resp.clicked() {
        *open = !*open;
    }
    options_menu(&resp, options, open, current)
}

/// Concept checkbox row: 16px box plus label.
fn check_row(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    checked: bool,
    label: &str,
) -> egui::Response {
    let label = crate::i18n::t(label);
    let r = ui.interact(rect, egui::Id::new(("popup-check", id)), egui::Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let box_r = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - 8.0),
        egui::Vec2::splat(16.0),
    );
    dm_check(ui.painter(), box_r, checked);
    ui.painter().text(
        egui::pos2(box_r.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        reg(11.0),
        if r.hovered() { pl().dm_txt } else { pl().dm_dim },
    );
    r
}

/// Concept slider with its drag handling on the same rect, so painting and
/// input can never disagree. Returns the new fraction when it changed.
fn slider_row(ui: &mut egui::Ui, rect: egui::Rect, id: &str, frac: f32) -> Option<f32> {
    let r = ui.interact(rect, egui::Id::new(("popup-slider", id)), egui::Sense::click_and_drag());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mut out = None;
    if r.dragged() || r.clicked() {
        if let Some(pos) = ui.ctx().pointer_interact_pos() {
            let f = ((pos.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
            if (f - frac).abs() > 0.001 {
                out = Some(f);
            }
        }
    }
    let shown = out.unwrap_or(frac);
    slider(ui.painter(), rect, r.hovered(), shown, rect.left(), rect.width());
    out
}

impl ConfirmApp {
    fn effective_dir(&self) -> String {
        if self.remember_path {
            // Ordered user rules first, category fallback second (mirrors app).
            if let Some(rule) = self
                .folder_rules
                .iter()
                .find(|r| r.matches(&self.filename))
            {
                if !rule.folder.trim().is_empty() {
                    let mut p = std::path::PathBuf::from(&self.save_base);
                    p.push(rule.folder.trim());
                    return p.to_string_lossy().to_string();
                }
            }
            let mut p = std::path::PathBuf::from(&self.save_base);
            p.push(self.category.clone());
            p.to_string_lossy().to_string()
        } else {
            self.save_base.clone()
        }
    }

    /// Recent folders for the path field's caret: the shell folders first,
    /// then whatever the field currently holds (mirrors the main window).
    fn recent_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // "Remember the last folders used": those come first, newest on top.
        for d in &self.recent_folders {
            if std::path::Path::new(d).is_dir() && !out.contains(d) {
                out.push(d.clone());
            }
        }
        for d in [
            dirs::download_dir(),
            dirs::desktop_dir(),
            dirs::document_dir(),
            dirs::picture_dir(),
            dirs::video_dir(),
            dirs::audio_dir(),
        ]
        .into_iter()
        .flatten()
        {
            let s = d.to_string_lossy().to_string();
            if !out.contains(&s) {
                out.push(s);
            }
        }
        let cur = self.save_base.trim().to_string();
        if !cur.is_empty() && !out.contains(&cur) {
            out.push(cur);
        }
        out
    }

    /// Extra one-line facts the concept has no place for; shown under the
    /// hidden extras panel only.
    fn extra_notes(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.pair_url.starts_with("http") {
            v.push("video-only + audio — merged on completion (needs ffmpeg)".to_string());
        }
        if !self.quality_pref.trim().is_empty() {
            v.push(format!("Quality: {}", self.quality_pref.trim()));
        }
        v
    }

    /// Start downloading in the background while the dialog is open, for a
    /// plain file whose size is known (not a video page, a stream or an
    /// audio/video pair: those the app assembles itself). Once only.
    fn maybe_prestart(&mut self) {
        if self.pre_tried || self.probing || self.done {
            return;
        }
        let url = self.url.trim().to_string();
        let plain = is_download_url(&url)
            && self.pair_url.is_empty()
            && self.quality_pref.trim().is_empty()
            && !self.page_mime.contains("text/html")
            && !engine::is_stream_manifest(&url, None, &self.page_mime)
            && !url.contains("youtube.com")
            && !url.contains("youtu.be");
        if !plain || self.probe_size.unwrap_or(0) == 0 {
            return;
        }
        self.pre_tried = true;
        let Some(rt) = self.rt.clone() else { return };
        let name = engine::sanitize_filename(if self.filename.trim().is_empty() { "download" } else { self.filename.trim() });
        let dir = std::env::temp_dir().join("download-craft-pre").join(format!(
            "{}-{}",
            std::process::id(),
            chrono::Local::now().timestamp_millis()
        ));
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let dest = dir.join(name);
        let opts = EngineOpts {
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", crate::model::APP_VERSION)
            } else {
                self.user_agent.clone()
            },
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            referer: self.referer.clone(),
            cookie: self.cookie.clone(),
            captured_headers: engine::CapturedHeaders::for_url(&url, &self.headers),
            browser: self.browser.clone(),
            allow_rename: false,
            ..Default::default()
        };
        let shared = std::sync::Arc::new(engine::SharedState::new());
        let s2 = shared.clone();
        let d2 = dest.clone();
        let conns = self.connections.max(1);
        let handle = rt.spawn(async move { engine::download(url, d2, conns, opts, s2).await });
        self.pre = Some(PreStart { shared, handle, dest });
    }

    /// Stop the background download. `keep_as`: continue it as this file
    /// (its data is moved there; a file that already finished is moved
    /// whole and the return says so). Without it everything is deleted.
    fn stop_prestart(&mut self, keep_as: Option<&std::path::Path>) -> bool {
        let Some(pre) = self.pre.take() else { return false };
        pre.shared.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        // Let the transfer notice the flag and write its journal before the
        // hard abort: aborting mid-write is what leaves partial files for the
        // next run to clean up. The wait is bounded and returns as soon as the
        // task ends — this runs on the dialog's UI thread, where the old
        // blanket 250 ms sleep froze the window on every Download.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        while !pre.handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let finished =
            pre.handle.is_finished() && !engine::has_partial(&pre.dest) && pre.dest.is_file();
        pre.handle.abort();
        let mut whole = false;
        if let Some(to) = keep_as {
            if finished && !to.exists() {
                if let Some(dir) = to.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                whole = std::fs::rename(&pre.dest, to).is_ok()
                    || (std::fs::copy(&pre.dest, to).is_ok() && std::fs::remove_file(&pre.dest).is_ok());
            } else if !finished {
                engine::move_partial(&pre.dest, to);
            }
        }
        engine::remove_partial(&pre.dest);
        if let Some(dir) = pre.dest.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
        whole
    }

    fn start_probe(&mut self) {
        if !is_download_url(&self.url) {
            return;
        }
        let Some(rt) = self.rt.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.probe_rx = Some(rx);
        self.probing = true;
        self.probe_size = None;
        let url = self.url.clone();
        let referer = self.referer.clone();
        // Quality choice feeds the page resolve so the dialog measures the
        // chosen stream's size, not just the page's (IDM shows size upfront).
        let quality_pref = self.quality_pref.trim().to_string();
        // Bare URL guess to correct with the MIME type when the server
        // sends no disposition (P1: `download_123.bin` + video/mp4).
        let guess_name = self.filename.clone();
        let page_mime = self.page_mime.clone();
        let opts = EngineOpts {
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", crate::model::APP_VERSION)
            } else {
                self.user_agent.clone()
            },
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            referer,
            // The browser session claimed from the inbox: without it a
            // login-gated file probes as 403 or as the login page's size.
            cookie: self.cookie.clone(),
            quality_pref,
            captured_headers: engine::CapturedHeaders::for_url(&url, &self.headers),
            browser: self.browser.clone(),
            ..Default::default()
        };
        rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => {
                    let (mut total, _, ctype, mut disp) =
                        engine::probe(&client, &opts, &url).await;
                    // A playlist's length is its text, not the video: show
                    // the duration instead (as IDM does for streams).
                    if engine::is_stream_manifest(&url, ctype.as_deref(), &page_mime) {
                        total = None;
                    }
                    // A YouTube video: the app's own reader names the size
                    // (from the links themselves) and the title, without
                    // the pre-roll wait a download needs.
                    if total.is_none() && crate::engine::youtube_id(&url).is_some()
                        && crate::youtube::own_reader_allowed(&opts)
                    {
                        if let Ok((size, title)) = crate::youtube::peek(&client, &opts, &url).await {
                            total = size;
                            if disp.as_deref().map_or(true, engine::weak_capture_name) {
                                disp = Some(engine::title_filename(
                                    &title,
                                    Some(std::path::Path::new(&guess_name)),
                                ));
                            }
                        }
                    }
                    let is_page = ctype
                        .as_deref()
                        .map(|c| c.contains("text/html"))
                        .unwrap_or(false)
                        || url.contains("youtube.com")
                        || url.contains("youtu.be");
                    if total.is_none() && is_page {
                        let shared =
                            std::sync::Arc::new(engine::SharedState::new());
                        if let Some(hit) =
                            engine::resolve_media(&client, &opts, &url, &shared).await
                        {
                            let (t2, _, _, _) =
                                engine::probe(&client, &opts, &hit.url).await;
                            if total.is_none() {
                                total = t2;
                            }
                            if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                                let (ta, _, _, _) =
                                    engine::probe(&client, &opts, &pair).await;
                                if let (Some(a), Some(b)) = (total, ta) {
                                    total = Some(a.saturating_add(b));
                                } else {
                                    total = total.or(ta);
                                }
                            }
                            if !hit.title.trim().is_empty()
                                && disp.as_deref().map_or(true, engine::weak_capture_name)
                            {
                                disp = Some(engine::title_filename(
                                    &hit.title,
                                    Some(std::path::Path::new(&guess_name)),
                                ));
                            }
                        }
                    }
                    if disp.is_none()
                        && crate::model::is_fallback_filename(&guess_name)
                    {
                        // No disposition and no media title: at least fix the
                        // extension — probed MIME first, extension hint when
                        // the probe itself was refused off-session.
                        let ext = ctype
                            .as_deref()
                            .and_then(crate::model::ext_for_mime)
                            .or_else(|| crate::model::ext_for_mime(&page_mime));
                        if let Some(ext) = ext {
                            disp = Some(guess_name.trim_end_matches(".bin").to_string() + ext);
                        }
                    }
                    (total, disp)
                }
                Err(_) => (None, None),
            };
            let _ = tx.send(out);
        });
    }

    /// Queue the download. `start` = also wake the main app to begin it now
    /// (IDM "Start Download") vs merely queueing ("Download Later").
    fn commit(&mut self, start: bool) {
        // Download needs the main app, which owns the engine. When it is not
        // running, start it and queue nothing until it answers: a
        // queued-and-woken row used to sit silently and then start, with
        // every other one like it, the next time the app opened. `update`
        // commits once the app's heartbeat is live, or says it did not come.
        if start && !main_app_running() {
            if self.waiting_app.is_none() {
                self.waiting_app = Some(std::time::Instant::now());
                self.notice = crate::i18n::t(STARTING).to_string();
                std::thread::spawn(launch_main_app);
            }
            return;
        }
        self.waiting_app = None;
        if self.filename.trim().is_empty() {
            self.filename = filename_from_url(&self.url);
        }
        let mut item = DownloadItem::new(
            0, // id assigned atomically under the queue lock below
            self.url.trim().to_string(),
            engine::sanitize_filename(self.filename.trim()),
            self.effective_dir(),
            self.connections,
        );
        item.referer = self.referer.clone();
        item.cookie = self.cookie.clone();
        item.extra_headers = self.headers.clone();
        item.browser = self.browser.clone();
        item.description = self.description.trim().to_string();
        // Browser-measured size (extension HEAD/Range in the real session):
        // keep it so the engine and progress UI know the total even when a
        // server-side re-probe would 403 off-session.
        item.total_bytes = self.probe_size.filter(|s| *s > 0);
        if self.pair_url.starts_with("http") {
            item.pair_url = self.pair_url.clone();
            item.pair_mime = self.pair_mime.clone();
        }
        item.quality_pref = self.quality_pref.trim().to_string();
        item.duration_secs = self.duration_secs;
        item.expects_audio = self.expects_audio;
        // What the dialog already downloaded continues as this file. If it
        // finished meanwhile, the file is listed as finished.
        if self.pre.is_some() && !item.dest_path().exists() && !engine::has_partial(&item.dest_path()) {
            let dest = item.dest_path();
            // The progress window opens at what is already here, not at 0.
            item.downloaded_bytes = self.pre.as_ref().map_or(0, |p| p.shared.downloaded.load(std::sync::atomic::Ordering::SeqCst));
            if self.stop_prestart(Some(&dest)) {
                let len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                item.total_bytes = Some(len);
                item.downloaded_bytes = len;
                let _ = crate::model::record_finished(&mut item);
                save_remember_category(self.remember_path);
                save_recent_folder(&self.save_base);
                self.done = true;
                return;
            }
        } else {
            self.stop_prestart(None);
        }
        let id = match crate::model::append_to_queue(&mut item) {
            crate::model::AppendResult::Added(id) => {
                save_remember_category(self.remember_path);
                save_recent_folder(&self.save_base);
                id
            }
            crate::model::AppendResult::AlreadyQueued(id) => {
                // Not an error. The URL is already in the list; if the user
                // asked to start it we wake that row, so this is never a dead
                // end. Stay open so the notice is actually read. The notice
                // names what really happens to the row.
                use crate::model::Status;
                self.notice = match crate::model::queued_status(id) {
                    Some(Status::Completed) => format!("Already downloaded (#{id}) — see Finished."),
                    Some(Status::Downloading) => format!("Already downloading (#{id})."),
                    // Download on a row that is waiting: start it and get out of
                    // the way, like any other Download (the progress window
                    // takes over). Only the cases with nothing to start stay
                    // open to explain themselves.
                    _ if start => {
                        self.wake_targets(&[id]);
                        self.done = true;
                        return;
                    }
                    _ => format!("Already in the queue (#{id})."),
                };
                return;
            }
            crate::model::AppendResult::Error(e) => {
                // Never close on a failed write: closing with a stale message
                // is exactly what made this look like "already in queue".
                self.notice = format!("Could not update the queue: {e}");
                return;
            }
        };
        save_remember_category(self.remember_path);
                save_recent_folder(&self.save_base);
        if start {
            self.wake_targets(&[id]);
        }
        self.done = true;
    }

    fn wake_targets(&self, targets: &[u64]) {
        wake_ids(targets);
    }
}

/// Ask the main app to start these ids (consumed on its next poll, or at
/// startup when we launch it below).
pub(crate) fn wake_ids(targets: &[u64]) {
    request_start(targets);
    // tasklist + cold-start spawn can take seconds (debug builds
    // especially) — never block the dialog on it; close at once.
    std::thread::spawn(|| {
        if !main_app_running() {
            launch_main_app();
        }
    });
}

/// Write the start request only (`wake.json`), without starting the main
/// window: for the remote control, whose server is the running window's or
/// the window-less server mode's, and either reads the request itself.
pub(crate) fn request_start(targets: &[u64]) {
    if let Some(p) = wake_path() {
        let _guard = crate::model::fs_lock("wake");
        if !_guard.owned() { return; }
        let mut ids: Vec<u64> = std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("start").cloned())
            .and_then(|v| serde_json::from_value::<Vec<u64>>(v).ok())
            .unwrap_or_default();
        for id in targets {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        if let Ok(json) =
            serde_json::to_string_pretty(&serde_json::json!({ "start": ids }))
        {
            let _ = atomic_write(&p, json.as_bytes());
        }
    }
}

/// Entry point for `--confirm-download`. Never returns normally
/// (runs the dialog window until it closes).
pub fn run() -> eframe::Result<()> {
    // Index-iterated flag parsing: a VALUE that looks like a flag (e.g. a
    // `Content-Disposition` filename of `--referer`) must not shift parsing.
    // Only the token immediately following a known flag is consumed.
    const KNOWN: &[&str] = &[
        "--url",
        "--filename",
        "--referer",
        "--cookie",
        "--mime",
        "--size",
        "--pair-url",
        "--pair-mime",
        "--quality",
        "--dur",
    ];
    let args: Vec<String> = std::env::args().collect();
    let mut map: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(flag) = KNOWN.iter().find(|k| **k == args[i].as_str()) {
            if i + 1 < args.len() {
                map.insert(*flag, args[i + 1].clone());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    let mut get = |name: &str| -> Option<String> { map.remove(name) };
    let url = get("--url").unwrap_or_default();
    if !is_download_url(&url) {
        eprintln!("--confirm-download needs --url <http(s)://...>");
        std::process::exit(2);
    }
    let page_mime = get("--mime").filter(|s| !s.trim().is_empty()).unwrap_or_default();
    // `get` consumes the flag, so read it once.
    let caller_name = get("--filename").filter(|f| !f.trim().is_empty());
    let named_by_caller = caller_name.is_some();
    let filename = caller_name.unwrap_or_else(|| {
        crate::model::filename_from_url_with_mime(&url, Some(page_mime.as_str()))
    });
    let referer = get("--referer").unwrap_or_default();
    let mut cookie = get("--cookie").unwrap_or_default();
    let pair_url = get("--pair-url").filter(|s| s.starts_with("http")).unwrap_or_default();
    let pair_mime = get("--pair-mime").unwrap_or_default();
    let quality_pref = get("--quality").filter(|s| !s.trim().is_empty()).unwrap_or_default();
    // Browser-measured size (menu HEAD/Range): trust it, skip probing.
    let known_size: Option<u64> = get("--size")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0)
        // An older extension sends a playlist's text length as its size.
        .filter(|_| !engine::is_stream_manifest(&url, None, &page_mime));
    // Browser-known media duration in seconds (best-effort badge).
    let duration_secs: u64 = get("--dur")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s > 0 && *s < 86400)
        .unwrap_or(0);

    // Claim the inbox entry first so the main window won't prompt too.
    let mut headers = std::collections::BTreeMap::new();
    let mut browser = None;
    if let Some(item)=claim_inbox(url.trim()) {
        browser = item.browser.clone();
        // The native host leaves credentials in the private inbox, not the
        // process command line where other local users can inspect them.
        if cookie.is_empty() {cookie=item.cookie.unwrap_or_default();}
        headers = item.headers.unwrap_or_default();
    }

    let prefs = load_prefs();
    let category = category_of(&filename).to_string();
    // A row that is still usable (queued/downloading/paused, or finished with
    // its file present) is reported up front. A dead row (failed, or finished
    // with the file gone) is refreshed by the capture instead, so the buttons
    // never dead-end on a stale "already in queue".
    let initial_notice = match crate::model::queued_entry(url.trim(), &quality_pref) {
        Some((id, false)) => format!("Already in the queue (#{id})."),
        _ => String::new(),
    };

    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    let start_name = crate::model::apply_rename_template(&prefs.rename_template, &engine::sanitize_filename(&filename), url.trim());
    let mut app = ConfirmApp {
        auto_name: (!named_by_caller).then(|| start_name.clone()),
        browser_size: known_size,
        url: url.trim().to_string(),
        filename: start_name,
        category,
        remember_path: prefs.category_folders,
        save_base: prefs.save_dir,
        description: String::new(),
        referer,
        cookie,
        headers,
        browser,
        pair_url,
        pair_mime,
        page_mime,
        duration_secs,
        expects_audio: args.iter().any(|s| s == "--expect-audio"),
        quality_pref,
        connections: prefs.connections,
        user_agent: prefs.user_agent,
        proxy_url: prefs.proxy_url,
        timeout_secs: prefs.timeout_secs,
        rename_template: prefs.rename_template,
        folder_rules: prefs.folder_rules,
        recent_folders: if prefs.remember_folders { prefs.recent_folders } else { Vec::new() },
        pre: None,
        pre_tried: false,
        probing: false,
        probe_size: known_size,
        probe_rx: None,
        rt: Some(rt.clone()),
        centered: false,
        show_adv: false,
        done: false,
        waiting_app: None,
        exited: false,
        notice: initial_notice,
        cat_open: false,
        recent_open: false,
        win_h: 0.0,
        adding_cat: false,
        new_cat: String::new(),
        custom_cats: Vec::new(),
    };
    // Prefill filename from server when the extension sent none useful.
    // A browser-known size skips probing entirely (server re-probes of
    // media URLs often 403 off-session).
    if known_size.is_none() {
        app.start_probe();
    }
    let _rt_keepalive = rt;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Add download")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([DM_W, DM_H])
        .with_resizable(false)
        .with_decorations(crate::platform::system_title_bar())
        .with_transparent(!crate::platform::system_title_bar())
        .with_always_on_top();
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Add download",
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            setup_popup_fonts(cc);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for ConfirmApp {
    /// The window is transparent and frameless: the concept's rounded card is
    /// painted by hand, so the surface behind it must stay clear.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Same bulletproofing as the progress window: Close can no-op, so back
    /// it with a delayed exit once the dialog is done.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(text) = crate::i18n::take_font_text() {
            crate::fonts::ensure_text(ctx, &text);
        }
        crate::fonts::ensure_text(ctx, &self.filename);
        crate::fonts::ensure_text(ctx, &self.description);
        // Say up front when the app is not running (Download needs it), and
        // drop the note by itself once the app is started.
        let now = ctx.input(|i| i.time);
        let last = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(egui::Id::new("run-check"), || -10.0f64));
        if now - last >= 2.0 {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("run-check"), now));
            let note = crate::i18n::t(NOT_RUNNING).to_string();
            let running = main_app_running();
            if !running && self.notice.is_empty() && self.waiting_app.is_none() {
                self.notice = note;
            } else if running && self.notice == note {
                self.notice.clear();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        if let Some(since) = self.waiting_app {
            if main_app_running() {
                self.notice.clear();
                self.commit(true);
            } else if since.elapsed() > START_WAIT {
                self.waiting_app = None;
                self.notice = crate::i18n::t(NOT_RUNNING).to_string();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        if self.done || ctx.input(|i| i.viewport().close_requested()) {
            self.stop_prestart(None);
            self.done = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            if !self.exited {
                self.exited = true;
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    std::process::exit(0);
                });
            }
            return;
        }
        self.maybe_prestart();
        // Middle of the work area (taskbar excluded), once the monitor
        // geometry is known; Windows would otherwise cascade the window.
        if !self.centered && crate::model::center_window(ctx, egui::vec2(DM_W, DM_H)) {
            self.centered = true;
        }
        // Collect probe result.
        if let Some(rx) = self.probe_rx.as_ref() {
            while let Ok((sz, nm)) = rx.try_recv() {
                self.probing = false;
                // A refused re-probe (off-session 403) must not erase the
                // size the browser measured in its own session.
                self.probe_size = sz.or(self.browser_size);
                if let Some(n) = nm {
                    // Prefer the server/video title while our name is still a
                    // guess we made (or a last-resort fallback whose timestamp
                    // drifted since dialog open) — never over a name the
                    // extension chose or the user typed.
                    let untouched_guess = self.auto_name.as_deref() == Some(self.filename.as_str());
                    if untouched_guess
                        || self.filename == filename_from_url(&self.url)
                        || crate::model::is_fallback_filename(&self.filename)
                    {
                        self.filename = crate::model::apply_rename_template(
                            &self.rename_template,
                            &n,
                            &self.url,
                        );
                        self.category = category_of(&self.filename).to_string();
                        if untouched_guess {
                            self.auto_name = Some(self.filename.clone());
                        }
                    }
                }
            }
        }
        if self.probing {
            ctx.request_repaint();
        }
        // Grow the window when the gear panel (or a notice) needs room, so the
        // concept surface itself never changes size or position.
        let notes = self.extra_notes();
        let panel_h = if self.show_adv {
            PANEL_H + if notes.is_empty() { 0.0 } else { 4.0 + NOTE_H * notes.len() as f32 }
        } else {
            0.0
        };
        let adv_h = if self.show_adv { panel_h + 14.0 } else { 0.0 };
        let mut want_h = DM_H + adv_h;
        if !self.notice.is_empty() {
            want_h += 16.0;
        }
        if (self.win_h - want_h).abs() > 0.5 {
            self.win_h = want_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(DM_W, want_h)));
        }
        // Esc closes, unless a menu is open (those close on the same key).
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !self.cat_open && !self.recent_open {
            self.done = true;
        }

        let panel = egui::CentralPanel::default().frame(egui::Frame::NONE);
        panel.show(ctx, |ui| {
            // The concept card IS the window: 562x253, 1px #1e293b border,
            // 8px radius, painted edge to edge over the clear surface.
            let card = ui.max_rect();
            let o = card.min;
            let p = ui.painter().clone();
            let at = |x: f32, y: f32, w: f32, h: f32| {
                egui::Rect::from_min_size(egui::pos2(o.x + x, o.y + y), egui::vec2(w, h))
            };
            p.rect(
                card,
                8.0,
                pl().dm_card,
                egui::Stroke::new(1.0, pl().dm_line),
                egui::StrokeKind::Inside,
            );
            // ---- header: mark + title + window controls (drag to move) ----
            dm_mark(&p, at(15.0, 11.0, 18.0, 18.0));
            p.text(
                egui::pos2(o.x + 41.0, o.y + 20.0),
                egui::Align2::LEFT_CENTER,
                "Download File Info",
                semi(13.0),
                pl().dm_txt,
            );
            p.line_segment(
                [
                    egui::pos2(o.x + 1.0, o.y + 39.5),
                    egui::pos2(o.x + DM_W - 1.0, o.y + 39.5),
                ],
                egui::Stroke::new(1.0, pl().dm_line),
            );
            if ui
                .interact(
                    at(0.0, 0.0, DM_W - 100.0, 39.0),
                    egui::Id::new("popup-header"),
                    egui::Sense::click_and_drag(),
                )
                .drag_started()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            let ctl = |x: f32| {
                egui::Rect::from_center_size(
                    egui::pos2(o.x + x, o.y + 20.0),
                    egui::Vec2::splat(12.0),
                )
            };
            let (min_r, max_r, close_r) = (ctl(485.0), ctl(513.0), ctl(541.0));
            let min_b = ui.interact(min_r, egui::Id::new("popup-min"), egui::Sense::click());
            let max_b = ui.interact(max_r, egui::Id::new("popup-max"), egui::Sense::click());
            let close_b = ui.interact(close_r, egui::Id::new("popup-close"), egui::Sense::click());
            for (r, hov) in [
                (min_r, min_b.hovered()),
                (max_r, max_b.hovered()),
                (close_r, close_b.hovered()),
            ] {
                if hov {
                    p.rect_filled(r.expand(4.0), DM_RAD, pl().dm_hover2);
                }
            }
            if min_b.hovered() || max_b.hovered() || close_b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            dm_icon(&p, min_r, DmIcon::Minimize, pl().dm_dim, 2.0);
            dm_icon(&p, max_r, DmIcon::Maximize, pl().dm_dim, 2.0);
            dm_icon(&p, close_r, DmIcon::Close, pl().dm_dim, 2.0);
            if min_b.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            if max_b.clicked() {
                let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            if close_b.clicked() {
                self.done = true;
            }
            // ---- row 1: URL + clipboard (copies the captured link) ----
            let url_box = at(DM_X, DM_ROW1, DM_W_IN, DM_BOX_H);
            let clip_btn = egui::Rect::from_min_size(
                egui::pos2(url_box.max.x - 33.0, url_box.min.y + 5.0),
                egui::Vec2::splat(28.0),
            );
            dm_field(
                ui,
                "popup_url",
                url_box,
                clip_btn.min.x - 6.0,
                &mut self.url,
                "https://…",
                false,
            );
            let clip_b = ui.interact(clip_btn, egui::Id::new("popup-clip"), egui::Sense::click());
            if clip_b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            icon_btn(&p, clip_btn, clip_b.hovered(), DmIcon::Clipboard, 14.0, 1.5, pl().dm_dim);
            if clip_b.clicked() {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    let _ = cb.set_text(self.url.clone());
                }
            }
            // ---- row 2: save folder | size chip ----
            let path_box = at(DM_X, DM_ROW2, DM_PATH_W, DM_BOX_H);
            let caret_btn = egui::Rect::from_min_size(
                egui::pos2(path_box.max.x - 32.0, path_box.min.y + 5.0),
                egui::Vec2::splat(28.0),
            );
            let folder_btn = egui::Rect::from_min_size(
                egui::pos2(caret_btn.min.x - 30.0, caret_btn.min.y),
                egui::Vec2::splat(28.0),
            );
            dm_field(
                ui,
                "popup_dir",
                path_box,
                folder_btn.min.x - 6.0,
                &mut self.save_base,
                "Save folder",
                false,
            );
            if self.remember_path {
                // The field holds the base folder; say where the rule sends it.
                ui.interact(
                    path_box,
                    egui::Id::new("popup_dir_tip"),
                    egui::Sense::hover(),
                )
                .on_hover_text(format!("→ {}", self.effective_dir()));
            }
            let f_b = ui.interact(folder_btn, egui::Id::new("popup-folder"), egui::Sense::click());
            icon_btn(&p, folder_btn, f_b.hovered(), DmIcon::Folder, 14.0, 1.5, pl().dm_dim);
            let c_b = ui.interact(caret_btn, egui::Id::new("popup-caret"), egui::Sense::click());
            icon_btn(&p, caret_btn, c_b.hovered(), DmIcon::Chevron, 10.0, 2.0, pl().dm_dim);
            if f_b.hovered() || c_b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if c_b.clicked() {
                self.recent_open = !self.recent_open;
            }
            let recents = self.recent_dirs();
            if let Some(dir) = options_menu(
                &c_b,
                &recents,
                &mut self.recent_open,
                &self.save_base,
            ) {
                self.save_base = dir;
            }
            if f_b.clicked() {
                let mut dlg = rfd::FileDialog::new();
                if !self.save_base.trim().is_empty() {
                    dlg = dlg.set_directory(&self.save_base);
                }
                if let Some(d) = dlg.pick_folder() {
                    self.save_base = d.to_string_lossy().to_string();
                }
            }
            // size chip: grid mark, size (with duration when known), tick
            let chip = at(DM_X + DM_PATH_W + 8.0, DM_ROW2, DM_SIZE_W, DM_BOX_H);
            dm_box(&p, chip);
            dm_icon(
                &p,
                egui::Rect::from_center_size(
                    egui::pos2(chip.min.x + 21.0, chip.center().y),
                    egui::Vec2::splat(16.0),
                ),
                DmIcon::Grid,
                DM_ACC(),
                0.0,
            );
            let known = !self.probing && self.probe_size.filter(|s| *s > 0).is_some();
            let sz_txt: String = if self.probing {
                "…".to_string()
            } else {
                match self.probe_size.filter(|s| *s > 0) {
                    Some(sz) if self.duration_secs > 0 => {
                        format!("{} • {}", format_dur(self.duration_secs), format_bytes(sz))
                    }
                    Some(sz) => format_bytes(sz),
                    // Streams (HLS/DASH) have no size until assembled; the
                    // duration is the useful fact, as IDM shows it.
                    None if self.duration_secs > 0 => format_dur(self.duration_secs),
                    None => "—".to_string(),
                }
            };
            let g = p.layout_no_wrap(sz_txt, reg(11.0), pl().dm_txt);
            let tw = g.size().x;
            let tx = chip.min.x + 37.0;
            p.galley(egui::pos2(tx, chip.center().y - g.size().y * 0.5), g, pl().dm_txt);
            if known {
                dm_icon(
                    &p,
                    egui::Rect::from_min_size(
                        egui::pos2(tx + tw + 8.0, chip.min.y + 13.0),
                        egui::Vec2::splat(12.0),
                    ),
                    DmIcon::Check,
                    DM_OK,
                    2.5,
                );
            }
            // ---- row 3: filename + refresh + advanced ----
            let file_box = at(DM_X, DM_ROW3, DM_FILE_W, DM_BOX_H);
            let refr_btn = egui::Rect::from_min_size(
                egui::pos2(file_box.max.x + 8.0, file_box.min.y),
                egui::Vec2::splat(38.0),
            );
            let gear_btn = egui::Rect::from_min_size(
                egui::pos2(refr_btn.max.x + 8.0, refr_btn.min.y),
                egui::Vec2::splat(38.0),
            );
            dm_field(
                ui,
                "popup_file",
                file_box,
                file_box.max.x - 13.0,
                &mut self.filename,
                "Filename",
                false,
            );
            let r_b = ui
                .interact(refr_btn, egui::Id::new("popup-refresh"), egui::Sense::click())
                .on_hover_text("Check the size and file name again");
            let g_b = ui
                .interact(gear_btn, egui::Id::new("popup-gear"), egui::Sense::click())
                .on_hover_text(if self.show_adv {
                    "Hide more options"
                } else {
                    "More options: category, description, referer, cookie"
                });
            // Square field-surface buttons, so they read as part of the
            // filename row (bare glyphs looked like stray icons).
            icon_btn_box(&p, refr_btn, r_b.hovered(), DmIcon::Refresh, 14.0, 1.5, pl().dm_dim);
            icon_btn_box(
                &p,
                gear_btn,
                g_b.hovered(),
                DmIcon::Settings,
                14.0,
                1.5,
                if self.show_adv { DM_ACC() } else { pl().dm_dim },
            );
            if r_b.hovered() || g_b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r_b.clicked() {
                self.start_probe();
            }
            if g_b.clicked() {
                self.show_adv = !self.show_adv;
            }
            // ---- gear panel: the rows the concept has no place for ----
            if self.show_adv {
                let panel = at(DM_X, DM_ACT, DM_W_IN, panel_h);
                dm_box(&p, panel);
                let cx = panel.min.x + 13.0;
                let cw = panel.width() - 26.0;
                let half = (cw - 8.0) / 2.0;
                let mut cy = panel.min.y + 13.0;
                let cat_r = egui::Rect::from_min_size(
                    egui::pos2(cx, cy),
                    egui::vec2(half - 34.0, DM_BOX_H),
                );
                let plus_r = egui::Rect::from_min_size(
                    egui::pos2(cx + half - 30.0, cy),
                    egui::vec2(30.0, DM_BOX_H),
                );
                let desc_r = egui::Rect::from_min_size(
                    egui::pos2(cx + half + 8.0, cy),
                    egui::vec2(half, DM_BOX_H),
                );
                let mut cats: Vec<String> = CATEGORIES.iter().map(|c| c.to_string()).collect();
                cats.extend(self.custom_cats.iter().cloned());
                if self.adding_cat {
                    let name_r = egui::Rect::from_min_size(
                        cat_r.min,
                        egui::vec2(cat_r.width() - 48.0, DM_BOX_H),
                    );
                    let add_r = egui::Rect::from_min_size(
                        egui::pos2(cat_r.max.x - 44.0, cy),
                        egui::vec2(44.0, DM_BOX_H),
                    );
                    dm_field(
                        ui,
                        "popup_newcat",
                        name_r,
                        name_r.max.x - 10.0,
                        &mut self.new_cat,
                        "New category",
                        false,
                    );
                    let a_b = ui.interact(add_r, egui::Id::new("popup-catadd"), egui::Sense::click());
                    btn_flat(&p, add_r, a_b.hovered(), "Add");
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if a_b.clicked() || enter {
                        let name = self.new_cat.trim().to_string();
                        if !name.is_empty() {
                            if !self
                                .custom_cats
                                .iter()
                                .any(|c| c.eq_ignore_ascii_case(&name))
                            {
                                self.custom_cats.push(name.clone());
                            }
                            self.category = name;
                            self.new_cat.clear();
                            self.adding_cat = false;
                        }
                    }
                } else if let Some(pick) =
                    combo(ui, cat_r, "cat", &self.category, &cats, &mut self.cat_open)
                {
                    self.category = pick;
                }
                let p_b = plus_square(ui, plus_r, 1);
                if p_b.clicked() {
                    self.adding_cat = !self.adding_cat;
                }
                dm_field(
                    ui,
                    "popup_desc",
                    desc_r,
                    desc_r.max.x - 13.0,
                    &mut self.description,
                    "Description (optional)",
                    false,
                );
                cy += DM_BOX_H + 8.0;
                let ref_r = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(half, DM_BOX_H));
                let cok_r = egui::Rect::from_min_size(
                    egui::pos2(cx + half + 8.0, cy),
                    egui::vec2(half, DM_BOX_H),
                );
                dm_field(
                    ui,
                    "popup_ref",
                    ref_r,
                    ref_r.max.x - 13.0,
                    &mut self.referer,
                    "Referer (optional)",
                    false,
                );
                dm_field(
                    ui,
                    "popup_cookie",
                    cok_r,
                    cok_r.max.x - 13.0,
                    &mut self.cookie,
                    "Cookie (optional)",
                    false,
                );
                cy += DM_BOX_H + 8.0;
                let row = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 30.0));
                p.text(
                    egui::pos2(row.min.x, row.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Connections",
                    reg(11.0),
                    pl().dm_dim,
                );
                let srect = egui::Rect::from_min_size(
                    egui::pos2(row.min.x + 92.0, row.min.y + 5.0),
                    egui::vec2(row.width() - 144.0, 20.0),
                );
                let frac = ((self.connections as f32 - 1.0) / 31.0).clamp(0.0, 1.0);
                if let Some(nf) = slider_row(ui, srect, "conns", frac) {
                    self.connections = ((nf * 31.0).round() as usize + 1).clamp(1, 32);
                }
                p.text(
                    egui::pos2(row.max.x, row.center().y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{}", self.connections),
                    reg(11.0),
                    pl().dm_txt,
                );
                cy += 30.0 + 8.0;
                let rem = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 22.0));
                if check_row(
                    ui,
                    rem,
                    "remember",
                    self.remember_path,
                    "Remember this path (category subfolder)",
                )
                .clicked()
                {
                    self.remember_path = !self.remember_path;
                }
                cy += 22.0;
                for n in &notes {
                    p.text(
                        egui::pos2(cx, cy + 6.0),
                        egui::Align2::LEFT_CENTER,
                        n,
                        reg(11.0),
                        pl().dm_dim,
                    );
                    cy += NOTE_H;
                }
            }
            // ---- actions: Add (queue only) | Download (start) | Cancel ----
            let act_y = DM_ACT + adv_h;
            let add_r = at(DM_X, act_y, 100.0, 36.0);
            let dl_r = egui::Rect::from_min_size(
                egui::pos2(add_r.max.x + 8.0, add_r.min.y),
                egui::vec2(140.0, 36.0),
            );
            let cancel_r = egui::Rect::from_min_size(
                egui::pos2(o.x + DM_W - 17.0 - 100.0, add_r.min.y),
                egui::vec2(100.0, 36.0),
            );
            let a_b = ui.interact(add_r, egui::Id::new("popup-add"), egui::Sense::click());
            let d_b = ui.interact(dl_r, egui::Id::new("popup-start"), egui::Sense::click());
            let x_b = ui.interact(cancel_r, egui::Id::new("popup-cancel"), egui::Sense::click());
            btn_flat(&p, add_r, a_b.hovered(), "Add");
            dm_gradient(&p, dl_r, DM_RAD, DM_ACC(), DM_ACC2());
            if d_b.hovered() {
                p.rect_filled(dl_r, DM_RAD, egui::Color32::from_white_alpha(22));
            }
            p.text(
                dl_r.center(),
                egui::Align2::CENTER_CENTER,
                crate::i18n::t("Download"),
                semi(13.0),
                egui::Color32::WHITE,
            );
            btn_flat(&p, cancel_r, x_b.hovered(), "Cancel");
            if a_b.hovered() || d_b.hovered() || x_b.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if a_b.clicked() {
                self.commit(false);
            }
            if d_b.clicked() {
                self.commit(true);
            }
            if x_b.clicked() {
                self.stop_prestart(None);
                self.done = true;
            }
            // ---- notice (the window grew 16px for it) ----
            if !self.notice.is_empty() {
                let shown: String = self.notice.chars().take(78).collect();
                p.text(
                    egui::pos2(o.x + DM_X, act_y + 36.0 + 10.0),
                    egui::Align2::LEFT_CENTER,
                    shown,
                    reg(11.0),
                    pl().dm_warn,
                );
            }
        });
    }
}

#[cfg(test)]
mod audit_inbox_tests {
    #[test]
    fn claiming_a_capture_returns_private_cookies_and_preserves_other_entries() {
        let dir=std::env::temp_dir().join(format!("dc-inbox-audit-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();let path=dir.join("inbox.json");
        let data=serde_json::json!([
            {"url":"https://example.com/a","received_at":"2026-10-20","cookie":"session=secret"},
            {"url":"https://example.com/b","received_at":"2026-10-20"}
        ]);
        super::atomic_write(&path,serde_json::to_string(&data).unwrap().as_bytes()).unwrap();
        let capture=super::claim_inbox_at(&path,"https://example.com/a").unwrap();
        assert_eq!(capture.cookie.as_deref(),Some("session=secret"));
        let left:Vec<crate::native_host::InboxItem>=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(left.len(),1);assert_eq!(left[0].url,"https://example.com/b");
        std::fs::write(&path,b"{broken").unwrap();
        assert!(super::claim_inbox_at(&path,"https://example.com/b").is_none());
        assert_eq!(std::fs::read(&path).unwrap(),b"{broken");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
