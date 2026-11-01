//! Standalone Settings window (`--settings`): a separate OS window.
//!
//! The layout follows `settings_window.html`: a 640-wide window with an icon
//! rail (160 px), a scrolling content pane, and a Save/Close footer. Colours
//! come from the active app palette (`app::pl`), so the window follows
//! System/Dark/Light exactly like the rest of the app; geometry, type sizes
//! and icons follow the concept.
//!
//! It edits the shared `ui.json` with read-modify-write merging (only its
//! own keys are overlaid, so concurrent main-app saves elsewhere survive).
//! The main app watches `ui.json` mtime and reloads — no restart needed.

use crate::app::pl;
use crate::fonts::{reg, semi};
use crate::model::{default_folder_rules, FolderRule, SpeedWindow};
use egui::{Align2, Color32, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

/// Rail entries: Material Icons glyph + label. The concept's YouTube pane is
/// the helper ("plugin") page, so it is labelled Plugins.
/// The first field is the `app::line_icon` key: the same outline strokes as
/// the main window's sidebar, so both follow the theme the same way. A
/// single icon-font glyph (Appearance's ring) is drawn from the font as before.
const PANES: &[(&str, &str)] = &[
    ("\u{e3dc}", "Appearance"), // filter_center_focus
    ("Engine", "Download Engine"),
    ("Plugins", "Plugins"),
    ("Browser", "Browser Integration"),
    ("Proxy", "Proxy / Socks"),
    ("Rules", "Categories & Rules"),
    ("Sounds", "Sounds"),
    ("Updates", "Updates"),
];

/// Rail index of the Plugins pane (the helper's status is probed when it is
/// first shown, not at startup: asking a frozen build for its version costs
/// about a second).
const PANE_YTDLP: usize = 2;

#[derive(Debug, Clone)]
struct Settings {
    // Appearance
    theme: String,
    /// Interface language code ("en", or a lang/<code>.json file).
    language: String,
    close_to_tray: bool,
    start_with_windows: bool,
    /// Keep the OS title bar (`platform::system_title_bar`).
    system_title_bar: bool,
    /// Extract finished archives, which kinds, where, and delete after.
    extract_auto: bool,
    extract_types: String,
    extract_into_folder: bool,
    extract_delete: bool,
    /// Pop the standalone "Download complete" dialog when a transfer ends.
    show_complete: bool,
    /// The list's column titles without their icons.
    hide_col_icons: bool,
    /// Main window zoom, in percent (`ui_scale` in ui.json).
    ui_scale: u32,
    /// Interface font: a name from `fonts::UI_FONTS`, a .ttf path, or empty
    /// for the default (`ui_font` in ui.json).
    ui_font: String,
    // Proxy page: "none" | "system" | "pac" | "manual". Saved as the one
    // `proxy_url` value the engine reads (see `engine::proxy_choice`).
    proxy_mode: String,
    proxy_pac: String,
    proxy_kind: String,
    proxy_host: String,
    proxy_port: String,
    proxy_user: String,
    proxy_pass: String,
    proxy_http: bool,
    proxy_https: bool,
    proxy_from_browser: bool,
    /// Sounds page: (event key, on, wav file; empty = the system sound).
    sounds_ev: Vec<(&'static str, bool, String)>,
    // Engine
    save_dir: String,
    category_folders: bool,
    connections_default: usize,
    speed_limit_kbps: u64,
    user_agent: String,
    proxy_url: String,
    timeout_secs: u64,
    max_retries: u32,
    antivirus_cmd: String,
    finish_action: String,
    speed_windows_on: bool,
    speed_windows: Vec<SpeedWindow>,
    // Rules
    rename_template: String,
    folder_rules: Vec<FolderRule>,
    /// YouTube helper: use a yt-dlp found on PATH instead of the copy this app
    /// fetches and refreshes itself.
    ytdlp_prefer_path: bool,
    /// Where the helper comes from: "managed" (this app's own copy), "path"
    /// (the first yt-dlp on PATH) or "custom" (an executable the user picked).
    ytdlp_source: String,
    /// The executable picked for "custom"; empty until one is chosen.
    ytdlp_custom_path: String,
    /// ffmpeg: "managed" (fetched on demand), "path", or "custom".
    ffmpeg_source: String,
    /// The ffmpeg executable picked for "custom"; empty until chosen.
    ffmpeg_custom_path: String,
    /// Max simultaneous connections per host across all downloads (0 = off).
    per_host_conns: usize,
    /// Where unfinished downloads are kept (empty = next to the file).
    temp_dir: String,
    /// Play a sound when a download finishes or fails.
    sounds: bool,
    /// Download quota: at most this many MB (0 = off) per `quota_hours`.
    quota_mb: String,
    quota_hours: String,
    /// yt-dlp: save subtitles next to the video, in these languages.
    subtitles: bool,
    subtitle_langs: String,
    // Browser capture (pushed to the extension over its live connection)
    capture_auto: bool,
    /// File extensions to take over from the browser ("zip exe mp4 ..");
    /// empty = every download.
    capture_types: String,
    /// Sites whose downloads stay in the browser (comma separated).
    capture_skip_sites: String,
    /// Smaller files stay in the browser (KB, as text while editing).
    capture_min_kb: String,
    /// Key held while clicking a link to leave it to the browser:
    /// "alt", "shift" or "off".
    capture_skip_key: String,
    /// Browser notification when a download sent from it finishes.
    notify_done: bool,
    /// When the app cannot download a video, the extension downloads it in
    /// the browser instead.
    browser_fallback: bool,
    /// YouTube engine order: "auto", "native" or "ytdlp".
    youtube_engine: String,
    /// Saved site logins (kept in logins.json, passwords protected).
    logins: Vec<crate::model::SiteLogin>,
    // Updates
    update_check_on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            language: "en".to_string(),
            close_to_tray: cfg!(windows),
            system_title_bar: crate::platform::frame_unsupported(),
            ui_scale: 100,
            ui_font: String::new(),
            proxy_mode: "system".into(),
            proxy_pac: String::new(),
            proxy_kind: "http".into(),
            proxy_host: String::new(),
            proxy_port: String::new(),
            proxy_user: String::new(),
            proxy_pass: String::new(),
            proxy_http: true,
            proxy_https: true,
            proxy_from_browser: true,
            sounds_ev: SOUND_EVENTS.iter().map(|(k, _, on)| (*k, *on, String::new())).collect(),
            start_with_windows: cfg!(windows),
            extract_auto: false,
            extract_types: "zip rar 7z".to_string(),
            extract_into_folder: true,
            extract_delete: false,
            show_complete: true,
            hide_col_icons: false,
            save_dir: dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned()),
            category_folders: true,
            connections_default: 8,
            speed_limit_kbps: 0,
            user_agent: String::new(),
            proxy_url: String::new(),
            timeout_secs: 60,
            max_retries: 5,
            antivirus_cmd: String::new(),
            finish_action: "nothing".to_string(),
            speed_windows_on: false,
            speed_windows: vec![SpeedWindow {
                start: "02:00".to_string(),
                end: "06:00".to_string(),
                limit_kbps: 0,
            }],
            rename_template: "{name}".to_string(),
            folder_rules: default_folder_rules(),
            ytdlp_prefer_path: false,
            ytdlp_source: "managed".to_string(),
            ytdlp_custom_path: String::new(),
            ffmpeg_source: "managed".to_string(),
            ffmpeg_custom_path: String::new(),
            per_host_conns: 0,
            temp_dir: String::new(),
            sounds: false,
            quota_mb: "0".into(),
            quota_hours: "1".into(),
            subtitles: false,
            subtitle_langs: "en.*".into(),
            capture_auto: true,
            capture_types: String::new(),
            capture_skip_sites: String::new(),
            capture_min_kb: "100".to_string(),
            capture_skip_key: "alt".to_string(),
            notify_done: true,
            browser_fallback: true,
            youtube_engine: "auto".to_string(),
            logins: Vec::new(),
            update_check_on: true,
        }
    }
}

fn ui_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|p| p.join("ui.json"))
}

fn load() -> Settings {
    let mut s = Settings::default();
    let v: serde_json::Value = ui_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let g = |k: &str| v.get(k);
    let str_in = |k: &str, dst: &mut String| {
        if let Some(x) = g(k).and_then(|x| x.as_str()) {
            if !x.trim().is_empty() {
                *dst = x.to_string();
            }
        }
    };
    str_in("theme", &mut s.theme);
    str_in("language", &mut s.language);
    str_in("save_dir", &mut s.save_dir);
    str_in("user_agent", &mut s.user_agent);
    str_in("proxy_url", &mut s.proxy_url);
    str_in("antivirus_cmd", &mut s.antivirus_cmd);
    str_in("finish_action", &mut s.finish_action);
    str_in("rename_template", &mut s.rename_template);
    if let Some(p) = g("ffmpeg_custom_path").and_then(|v|v.as_str()) { s.ffmpeg_custom_path=p.to_string(); }
    if let Some(b) = g("close_to_tray").and_then(|x| x.as_bool()) {
        s.close_to_tray = cfg!(windows) && b;
    }
    load_proxy(&mut s, &v);
    str_in("ui_font", &mut s.ui_font);
    for ev in s.sounds_ev.iter_mut() {
        // Before the Sounds page one switch covered finished and failed.
        let legacy = if ev.0 == "complete" || ev.0 == "failed" { g("sounds").and_then(|x| x.as_bool()) } else { None };
        if let Some(b) = g(&format!("sound_{}", ev.0)).and_then(|x| x.as_bool()).or(legacy) {
            ev.1 = b;
        }
        if let Some(f) = g(&format!("sound_{}_file", ev.0)).and_then(|x| x.as_str()) {
            ev.2 = f.to_string();
        }
    }
    if let Some(n) = g("ui_scale").and_then(|x| x.as_u64()) {
        s.ui_scale = n.clamp(80, 175) as u32;
    }
    if let Some(b) = g("system_title_bar").and_then(|x| x.as_bool()) {
        s.system_title_bar = b;
    }
    if let Some(b) = g("start_with_windows").and_then(|x| x.as_bool()) {
        s.start_with_windows = cfg!(windows) && b;
    }
    if let Some(b) = g("extract_auto").and_then(|x| x.as_bool()) {
        s.extract_auto = b;
    }
    if let Some(x) = g("extract_types").and_then(|x| x.as_str()) {
        s.extract_types = x.to_string();
    }
    if let Some(b) = g("extract_into_folder").and_then(|x| x.as_bool()) {
        s.extract_into_folder = b;
    }
    if let Some(b) = g("extract_delete").and_then(|x| x.as_bool()) {
        s.extract_delete = b;
    }
    if let Some(b) = g("hide_col_icons").and_then(|x| x.as_bool()) {
        s.hide_col_icons = b;
    }
    if let Some(b) = g("show_complete").and_then(|x| x.as_bool()) {
        s.show_complete = b;
    }
    if let Some(b) = g("category_folders").and_then(|x| x.as_bool()) {
        s.category_folders = b;
    }
    if let Some(b) = g("speed_windows_on").and_then(|x| x.as_bool()) {
        s.speed_windows_on = b;
    }
    if let Some(b) = g("update_check_on").and_then(|x| x.as_bool()) {
        s.update_check_on = b;
    }
    if let Some(b) = g("ytdlp_prefer_path").and_then(|x| x.as_bool()) {
        s.ytdlp_prefer_path = b;
    }
    // Installs from before the three-way choice only carry the checkbox.
    match g("ytdlp_source").and_then(|x| x.as_str()) {
        Some(v @ ("managed" | "path" | "custom")) => s.ytdlp_source = v.to_string(),
        _ => {
            s.ytdlp_source = if s.ytdlp_prefer_path { "path" } else { "managed" }.to_string();
        }
    }
    if let Some(x) = g("ytdlp_custom_path").and_then(|x| x.as_str()) {
        s.ytdlp_custom_path = x.to_string();
    }
    match g("ffmpeg_source").and_then(|x| x.as_str()) {
        Some(v @ ("managed" | "path" | "custom")) => s.ffmpeg_source = v.to_string(),
        // Same migration as the engine: an older install that already has
        // ffmpeg on PATH keeps using it rather than fetching a second copy.
        _ => {
            s.ffmpeg_source = if crate::engine::ffmpeg_on_path().is_some() {
                "path".to_string()
            } else {
                "managed".to_string()
            }
        }
    }
    if let Some(x) = g("ffmpeg_custom_path").and_then(|x| x.as_str()) {
        s.ffmpeg_custom_path = x.to_string();
    }
    s.logins = crate::model::site_logins();
    if let Some(x) = g("temp_dir").and_then(|x| x.as_str()) {
        s.temp_dir = x.to_string();
    }
    if let Some(b) = g("sounds").and_then(|x| x.as_bool()) {
        s.sounds = b;
    }
    if let Some(n) = g("quota_mb").and_then(|x| x.as_u64()) {
        s.quota_mb = n.to_string();
    }
    if let Some(n) = g("quota_hours").and_then(|x| x.as_u64()) {
        s.quota_hours = n.to_string();
    }
    if let Some(b) = g("subtitles").and_then(|x| x.as_bool()) {
        s.subtitles = b;
    }
    if let Some(x) = g("subtitle_langs").and_then(|x| x.as_str()) {
        s.subtitle_langs = x.to_string();
    }
    if let Some(b) = g("capture_auto").and_then(|x| x.as_bool()) {
        s.capture_auto = b;
    }
    if let Some(x) = g("capture_types").and_then(|x| x.as_str()) {
        s.capture_types = x.to_string();
    }
    if let Some(x) = g("capture_skip_sites").and_then(|x| x.as_str()) {
        s.capture_skip_sites = x.to_string();
    }
    if let Some(n) = g("capture_min_kb").and_then(|x| x.as_u64()) {
        s.capture_min_kb = n.to_string();
    }
    match g("capture_skip_key").and_then(|x| x.as_str()) {
        Some(v @ ("alt" | "shift" | "off")) => s.capture_skip_key = v.to_string(),
        _ => {}
    }
    if let Some(b) = g("notify_done").and_then(|x| x.as_bool()) {
        s.notify_done = b;
    }
    if let Some(b) = g("browser_fallback").and_then(|x| x.as_bool()) {
        s.browser_fallback = b;
    }
    match g("youtube_engine").and_then(|x| x.as_str()) {
        Some(v @ ("auto" | "native" | "ytdlp")) => s.youtube_engine = v.to_string(),
        _ => {}
    }
    if let Some(n) = g("per_host_conns").and_then(|x| x.as_u64()) {
        s.per_host_conns = (n as usize).clamp(0, 64);
    }
    if let Some(n) = g("connections_default").and_then(|x| x.as_u64()) {
        s.connections_default = (n as usize).clamp(1, 32);
    }
    if let Some(n) = g("speed_limit_kbps").and_then(|x| x.as_u64()) {
        s.speed_limit_kbps = n;
    }
    if let Some(n) = g("timeout_secs").and_then(|x| x.as_u64()) {
        s.timeout_secs = n.clamp(5, 600);
    }
    if let Some(n) = g("max_retries").and_then(|x| x.as_u64()) {
        s.max_retries = (n as u32).clamp(1, 20);
    }
    if let Some(arr) = g("folder_rules").and_then(|x| x.as_array()) {
        let rules: Vec<FolderRule> = arr
            .iter()
            .filter_map(|r| {
                Some(FolderRule {
                    exts: r.get("exts")?.as_str()?.to_string(),
                    folder: r.get("folder")?.as_str()?.to_string(),
                })
            })
            .take(8)
            .collect();
        if !rules.is_empty() {
            s.folder_rules = rules;
        }
    }
    if let Some(arr) = g("speed_windows").and_then(|x| x.as_array()) {
        let wins: Vec<SpeedWindow> = arr
            .iter()
            .filter_map(|r| {
                Some(SpeedWindow {
                    start: r.get("start")?.as_str()?.to_string(),
                    end: r.get("end")?.as_str()?.to_string(),
                    limit_kbps: r.get("limit_kbps")?.as_u64()?,
                })
            })
            .take(6)
            .collect();
        if !wins.is_empty() {
            s.speed_windows = wins;
        }
    }
    s
}

/// "zip, .EXE  mp4" -> "zip exe mp4": lowercase extensions, one space apart.
/// Sound events: (key, label, on by default).
const SOUND_EVENTS: &[(&str, &str, bool)] = &[
    ("complete", "Download complete", true),
    ("failed", "Download failed", true),
    ("queue_start", "Queue processing started", false),
    ("queue_done", "Queue processing finished", false),
];

/// The proxy page's fields, from their own keys or, for settings written
/// before the page existed, from the single `proxy_url`.
fn load_proxy(s: &mut Settings, v: &serde_json::Value) {
    let g = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    if let Some(mode) = g("proxy_mode") {
        s.proxy_mode = mode;
        s.proxy_pac = g("proxy_pac").unwrap_or_default();
        s.proxy_kind = g("proxy_kind").unwrap_or_else(|| "http".into());
        s.proxy_host = g("proxy_host").unwrap_or_default();
        s.proxy_port = g("proxy_port").unwrap_or_default();
        s.proxy_user = g("proxy_user").unwrap_or_default();
        s.proxy_pass = g("proxy_pass").unwrap_or_default();
        if let Some(list) = g("proxy_protocols") {
            s.proxy_http = list.split(',').any(|p| p == "http");
            s.proxy_https = list.split(',').any(|p| p == "https");
        }
    } else {
        match crate::engine::proxy_choice(&s.proxy_url) {
            crate::engine::ProxyChoice::System => s.proxy_mode = "system".into(),
            crate::engine::ProxyChoice::Direct => s.proxy_mode = "none".into(),
            crate::engine::ProxyChoice::Pac(u) => {
                s.proxy_mode = "pac".into();
                s.proxy_pac = u;
            }
            crate::engine::ProxyChoice::Manual(u) => {
                s.proxy_mode = "manual".into();
                fill_manual(s, &u);
            }
        }
    }
    if let Some(b) = v.get("proxy_from_browser").and_then(|x| x.as_bool()) {
        s.proxy_from_browser = b;
    }
}

/// Split "socks5://user:pass@host:port" into the manual fields.
fn fill_manual(s: &mut Settings, url: &str) {
    let url = if url.contains("://") { url.to_string() } else { format!("http://{url}") };
    let Ok(u) = reqwest::Url::parse(&url) else { return };
    s.proxy_kind = if u.scheme().starts_with("socks") { "socks5".into() } else { "http".into() };
    s.proxy_host = u.host_str().unwrap_or("").to_string();
    s.proxy_port = u.port().map(|p| p.to_string()).unwrap_or_default();
    s.proxy_user = url_decode(u.username());
    s.proxy_pass = url_decode(u.password().unwrap_or(""));
}

fn url_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default()
}

/// The `proxy_url` value the engine reads, built from the proxy page.
fn proxy_field(s: &Settings) -> String {
    match s.proxy_mode.as_str() {
        "none" => "direct".into(),
        "pac" if !s.proxy_pac.trim().is_empty() => format!("pac:{}", s.proxy_pac.trim()),
        "manual" if !s.proxy_host.trim().is_empty() => {
            let scheme = if s.proxy_kind == "socks5" { "socks5h" } else { "http" };
            let enc = |x: &str| url::form_urlencoded::byte_serialize(x.as_bytes()).collect::<String>();
            let login = if s.proxy_user.is_empty() {
                String::new()
            } else if s.proxy_pass.is_empty() {
                format!("{}@", enc(&s.proxy_user))
            } else {
                format!("{}:{}@", enc(&s.proxy_user), enc(&s.proxy_pass))
            };
            let port = s.proxy_port.trim();
            let port = if port.is_empty() { if s.proxy_kind == "socks5" { "1080" } else { "8080" } } else { port };
            format!("{scheme}://{login}{}:{port}", s.proxy_host.trim())
        }
        _ => String::new(),
    }
}

fn normalize_types(raw: &str) -> String {
    let mut seen = Vec::new();
    for t in raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let t = t.trim().trim_start_matches('.').to_ascii_lowercase();
        if !t.is_empty() && t.len() <= 12 && t.chars().all(|c| c.is_ascii_alphanumeric()) && !seen.contains(&t) {
            seen.push(t);
        }
    }
    seen.join(" ")
}

/// Read-modify-write: overlay our keys onto whatever is on disk now.
fn save(s: &Settings) -> bool {
    let _guard=crate::model::fs_lock("ui-settings");
    if !_guard.owned() { return false; }
    let Some(path) = ui_path() else { return false };
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = match v.as_object_mut() {
        Some(o) => o,
        None => return false,
    };
    let set = |o: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: serde_json::Value| {
        o.insert(k.to_string(), v);
    };
    set(obj, "theme", s.theme.clone().into());
    set(obj, "language", s.language.clone().into());
    set(obj, "close_to_tray", s.close_to_tray.into());
    set(obj, "start_with_windows", s.start_with_windows.into());
    set(obj, "system_title_bar", s.system_title_bar.into());
    set(obj, "ui_scale", s.ui_scale.into());
    set(obj, "ui_font", s.ui_font.trim().into());
    set(obj, "extract_auto", s.extract_auto.into());
    set(obj, "extract_types", s.extract_types.clone().into());
    set(obj, "extract_into_folder", s.extract_into_folder.into());
    set(obj, "extract_delete", s.extract_delete.into());
    set(obj, "ffmpeg_custom_path", s.ffmpeg_custom_path.clone().into());
    set(obj, "show_complete", s.show_complete.into());
    set(obj, "hide_col_icons", s.hide_col_icons.into());
    set(obj, "save_dir", s.save_dir.clone().into());
    set(obj, "category_folders", s.category_folders.into());
    set(obj, "connections_default", (s.connections_default as u64).into());
    set(obj, "speed_limit_kbps", s.speed_limit_kbps.into());
    set(obj, "user_agent", s.user_agent.clone().into());
    set(obj, "proxy_url", proxy_field(s).into());
    set(obj, "proxy_mode", s.proxy_mode.clone().into());
    set(obj, "proxy_pac", s.proxy_pac.trim().into());
    set(obj, "proxy_kind", s.proxy_kind.clone().into());
    set(obj, "proxy_host", s.proxy_host.trim().into());
    set(obj, "proxy_port", s.proxy_port.trim().into());
    set(obj, "proxy_user", s.proxy_user.clone().into());
    set(obj, "proxy_pass", s.proxy_pass.clone().into());
    let protocols: Vec<&str> = [("http", s.proxy_http), ("https", s.proxy_https)].iter().filter(|p| p.1).map(|p| p.0).collect();
    set(obj, "proxy_protocols", protocols.join(",").into());
    set(obj, "proxy_from_browser", s.proxy_from_browser.into());
    for (k, on, file) in &s.sounds_ev {
        set(obj, &format!("sound_{k}"), (*on).into());
        set(obj, &format!("sound_{k}_file"), file.trim().into());
    }
    set(obj, "timeout_secs", s.timeout_secs.into());
    set(obj, "max_retries", s.max_retries.into());
    set(obj, "antivirus_cmd", s.antivirus_cmd.clone().into());
    set(obj, "finish_action", s.finish_action.clone().into());
    set(obj, "speed_windows_on", s.speed_windows_on.into());
    set(obj, "rename_template", s.rename_template.clone().into());
    set(obj, "ytdlp_prefer_path", s.ytdlp_prefer_path.into());
    set(obj, "ytdlp_source", s.ytdlp_source.clone().into());
    set(obj, "ytdlp_custom_path", s.ytdlp_custom_path.clone().into());
    set(obj, "ffmpeg_source", s.ffmpeg_source.clone().into());
    set(obj, "ffmpeg_custom_path", s.ffmpeg_custom_path.clone().into());
    set(obj, "per_host_conns", (s.per_host_conns as u64).into());
    set(obj, "temp_dir", s.temp_dir.trim().to_string().into());
    if s.logins != crate::model::site_logins() {
        crate::model::save_site_logins(&s.logins);
    }
    // Kept for older builds: on when any download sound is.
    set(obj, "sounds", s.sounds_ev.iter().any(|e| e.1 && (e.0 == "complete" || e.0 == "failed")).into());
    set(obj, "quota_mb", s.quota_mb.trim().parse::<u64>().unwrap_or(0).into());
    set(obj, "quota_hours", s.quota_hours.trim().parse::<u64>().unwrap_or(1).clamp(1, 720).into());
    set(obj, "subtitles", s.subtitles.into());
    set(obj, "subtitle_langs", s.subtitle_langs.trim().to_string().into());
    set(obj, "capture_auto", s.capture_auto.into());
    set(obj, "capture_types", normalize_types(&s.capture_types).into());
    set(obj, "capture_skip_sites", s.capture_skip_sites.trim().to_string().into());
    set(obj, "capture_min_kb", s.capture_min_kb.trim().parse::<u64>().unwrap_or(0).min(10_000_000).into());
    set(obj, "capture_skip_key", s.capture_skip_key.clone().into());
    set(obj, "notify_done", s.notify_done.into());
    set(obj, "browser_fallback", s.browser_fallback.into());
    set(obj, "youtube_engine", s.youtube_engine.clone().into());
    set(obj, "update_check_on", s.update_check_on.into());
    set(obj, "folder_rules", serde_json::to_value(&s.folder_rules).unwrap_or_default());
    set(
        obj,
        "speed_windows",
        serde_json::to_value(&s.speed_windows).unwrap_or_default(),
    );
    serde_json::to_string_pretty(&v)
        .ok()
        .and_then(|j| crate::model::atomic_write(&path, j.as_bytes()).ok())
        .is_some()
}

struct SettingsApp {
    pane: usize,
    s: Settings,
    centered: bool,
    notice: String,
    browser_msg: String,
    /// Last theme actually applied, so picking one in Appearance repaints
    /// this window at once (and "system" keeps tracking the OS).
    theme_applied: String,
    /// YouTube helper status. Both asking it for a version and re-fetching it
    /// run a process (or a download), so they happen on a worker thread and
    /// the pane shows the last answer.
    ytdlp: Option<crate::engine::YtdlpInfo>,
    ytdlp_msg: String,
    ytdlp_busy: bool,
    /// The app's own JavaScript runtime (Deno) being installed: the answer
    /// is the installed path or the reason it failed.
    deno_rx: Option<std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>>,
    ytdlp_rx: Option<
        std::sync::mpsc::Receiver<(
            String,
            crate::engine::YtdlpInfo,
            crate::engine::FfmpegInfo,
        )>,
    >,
    /// ffmpeg is only used to merge separate video+audio streams; its card
    /// says whether there is one, and how new it is.
    ffmpeg: Option<crate::engine::FfmpegInfo>,
    ffmpeg_msg: String,
    ffmpeg_busy: bool,
    /// The tools install (run on the yt-dlp channel) is writing ffmpeg.
    ffmpeg_installing: bool,
    ffmpeg_rx: Option<std::sync::mpsc::Receiver<(String, crate::engine::FfmpegInfo)>>,
    /// Which plugin cards are expanded (yt-dlp starts open, like the concept).
    plugins_open: [bool; 2],
    // Open/closed state of the Download-Engine cards. The pane is long, so every
    // card starts collapsed: the five heads stay readable at a glance and each
    // section opens on demand.
    open_storage: bool,
    /// Browser Integration -> Capture card expanded.
    open_capture: bool,
    /// Plugins -> YouTube card expanded.
    open_youtube: bool,
    open_perf: bool,
    open_network: bool,
    open_complete: bool,
    open_windows: bool,
    /// Appearance cards (the concept opens both by default).
    open_theme: bool,
    open_window: bool,
    /// Browser Integration cards (Chromium family, Firefox) expanded.
    browser_open: [bool; 2],
    /// Native host registered per browser, asked when the pane first opens
    /// (and again after a repair).
    host_status: Option<Vec<(&'static str, bool)>>,
    /// Updates page cards expanded.
    open_updates: bool,
    open_version: bool,
}

/// Probe the helper, and optionally re-download it first, off the UI thread.
/// ffmpeg is asked in the same pass (it also runs a process).
fn ytdlp_probe(
    force: bool,
) -> std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo, crate::engine::FfmpegInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut msg = String::new();
        if force {
            msg = match crate::engine::ytdlp_refresh_blocking() {
                Ok(p) => format!("Fetched the newest build into {}", p.display()),
                Err(e) => format!("Re-download failed: {e:#}"),
            };
        }
        let info = crate::engine::ytdlp_info();
        let ffmpeg = crate::engine::ffmpeg_info();
        let _ = tx.send((msg, info, ffmpeg));
    });
    rx
}

/// Probe ffmpeg, and optionally re-fetch the app's own copy first, off the UI
/// thread (both run a process; the fetch downloads ~60 MB).
fn ffmpeg_probe(
    force: bool,
) -> std::sync::mpsc::Receiver<(String, crate::engine::FfmpegInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut msg = String::new();
        if force {
            msg = match crate::engine::ffmpeg_refresh_blocking() {
                Ok(p) => format!("Fetched ffmpeg into {}", p.display()),
                Err(e) => format!("Re-download failed: {e:#}"),
            };
        }
        let info = crate::engine::ffmpeg_info();
        let _ = tx.send((msg, info));
    });
    rx
}

/// Status plus a latest-release check (and in-place update of the app's own
/// copy), off the UI thread.
fn ytdlp_update_probe(
) -> std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo, crate::engine::FfmpegInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let msg = crate::engine::ytdlp_update_check_blocking();
        let _ = tx.send((msg, crate::engine::ytdlp_info(), crate::engine::ffmpeg_info()));
    });
    rx
}

fn ffmpeg_update_probe() -> std::sync::mpsc::Receiver<(String, crate::engine::FfmpegInfo)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let msg = crate::engine::ffmpeg_update_check_blocking();
        let _ = tx.send((msg, crate::engine::ffmpeg_info()));
    });
    rx
}

/// The native "open file" dialog, for picking a yt-dlp executable.
fn pick_executable() -> Option<std::path::PathBuf> {
    pick_tool("yt-dlp")
}

fn pick_tool(name:&str) -> Option<std::path::PathBuf> {
    let dialog=rfd::FileDialog::new().set_title(format!("Pick {name}"));
    #[cfg(windows)] let dialog=dialog.add_filter("Executable", &["exe"]);
    dialog.pick_file().filter(|p|crate::platform::executable_file(p))
}
fn ffmpeg_install_probe() -> std::sync::mpsc::Receiver<(String,crate::engine::YtdlpInfo,crate::engine::FfmpegInfo)> {
    let (tx,rx)=std::sync::mpsc::channel();
    std::thread::spawn(move||{
        let message=match crate::helpers::install_ffmpeg(){Ok(_)=>"FFmpeg and ffprobe installed and ready.".into(),Err(e)=>format!("FFmpeg install failed: {e:#}")};
        let _=tx.send((message,crate::engine::ytdlp_info(),crate::engine::ffmpeg_info()));
    });rx
}

/// The native "open file" dialog, for picking an ffmpeg executable.
fn pick_ffmpeg() -> Option<std::path::PathBuf> {
    let dialog = rfd::FileDialog::new().set_title("Pick an ffmpeg executable");
    #[cfg(windows)]
    let dialog = dialog.add_filter("Executable", &["exe"]);
    dialog.pick_file().filter(|p| crate::platform::executable_file(p))
}

fn install_one(browser: &str) -> String {
    match crate::app::install_native_host(browser) {
        Ok(manifest) => format!("Installed {browser} host ✓ ({manifest})"),
        Err(e) => format!("Install failed: {e}"),
    }
}

/// Entry point for `--settings`.
pub fn run() -> eframe::Result<()> {
    // `--pane <n>` opens a page directly (e.g. 2 = Plugins).
    let args: Vec<String> = std::env::args().collect();
    let pane = args
        .iter()
        .position(|a| a == "--pane")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|p| *p < PANES.len())
        .unwrap_or(0);
    let app = SettingsApp {
        pane,
        s: load(),
        centered: false,
        notice: String::new(),
        browser_msg: String::new(),
        theme_applied: String::new(),
        ytdlp: None,
        ytdlp_msg: String::new(),
        ytdlp_busy: false,
        deno_rx: None,
        ytdlp_rx: None,
        ffmpeg: None,
        ffmpeg_msg: String::new(),
        ffmpeg_busy: false,
        ffmpeg_installing: false,
        ffmpeg_rx: None,
        plugins_open: [true, false],
        open_storage: false,
        open_capture: true,
        open_youtube: true,
        open_perf: false,
        open_network: false,
        open_complete: false,
        open_windows: false,
        open_theme: true,
        open_window: true,
        browser_open: [true, false],
        host_status: None,
        open_updates: true,
        open_version: true,
    };
    // Separate OS window (own process via `--settings`): never embedded in
    // the main GUI. 640 wide is the concept's window width.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download Craft Settings")
        .with_inner_size([640.0, 520.0])
        .with_min_inner_size([600.0, 430.0])
        .with_resizable(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Settings",
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(|cc| {
            crate::app::setup_fonts(cc);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(text) = crate::i18n::take_font_text() {
            crate::fonts::ensure_text(ctx, &text);
        }
        // Register the native handle once so the title bar can follow the
        // theme (this is its own process, same helper as the main window).
        {
            use raw_window_handle::HasWindowHandle;
            if let Ok(handle) = frame.window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(w) = handle.as_raw() {
                    crate::tray::set_hwnd(w.hwnd.get());
                }
            }
        }
        let resolved = format!(
            "{}:{}",
            self.s.theme,
            crate::app::theme_is_dark(ctx, &self.s.theme)
        );
        if self.theme_applied != resolved {
            crate::app::apply_theme(ctx, &self.s.theme);
            crate::tray::theme_caption(crate::app::theme_is_dark(ctx, &self.s.theme));
            self.theme_applied = resolved;
        }
        // Come to the front while placing: opened from the main window, the
        // new process otherwise stayed behind it (as the complete dialog did).
        if !self.centered {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if !self.centered && crate::model::center_window(ctx, egui::vec2(640.0, 520.0)) {
            self.centered = true;
        }
        // The helpers' status is asked for when the pane is first opened,
        // together with a check against their latest releases (the app's own
        // copies are updated in place; the user's copies are only reported).
        if self.pane == PANE_YTDLP && self.ytdlp.is_none() && self.ytdlp_rx.is_none() {
            self.ytdlp_rx = Some(ytdlp_update_probe());
            self.ytdlp_busy = true;
            if self.ffmpeg_rx.is_none() {
                self.ffmpeg_rx = Some(ffmpeg_update_probe());
                self.ffmpeg_busy = true;
            }
        }
        if let Some(rx) = &self.deno_rx {
            if let Ok(res) = rx.try_recv() {
                self.deno_rx = None;
                self.ytdlp_msg = match res {
                    Ok(_) => "JavaScript runtime installed (Deno) — YouTube's player challenges are solved now.".into(),
                    Err(e) => format!("Could not install the JavaScript runtime: {e}"),
                };
                // Re-read the status so the Runtime chip shows it.
                if self.ytdlp_rx.is_none() {
                    self.ytdlp_rx = Some(ytdlp_probe(false));
                    self.ytdlp_busy = true;
                }
            }
        }
        if let Some(rx) = &self.ytdlp_rx {
            if let Ok((msg, info, ffmpeg)) = rx.try_recv() {
                if !msg.is_empty() {
                    self.ytdlp_msg = msg;
                }
                self.ytdlp = Some(info);
                self.ffmpeg = Some(ffmpeg);
                self.ytdlp_rx = None;
                self.ytdlp_busy = false;
                self.ffmpeg_installing = false;
            }
        }
        if let Some(rx) = &self.ffmpeg_rx {
            if let Ok((msg, info)) = rx.try_recv() {
                if !msg.is_empty() {
                    self.ffmpeg_msg = msg;
                }
                self.ffmpeg = Some(info);
                self.ffmpeg_rx = None;
                self.ffmpeg_busy = false;
            }
        }

        let t = tok();
        // One manual composition instead of three nested panels: the concept
        // has a fixed rail border, a full-width footer rule and a 20 px content
        // inset, all of which are easier to hit exactly with rects.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let foot_h = 49.0;
                let rail_w = 168.0;
                let foot = Rect::from_min_max(
                    egui::pos2(full.left(), full.bottom() - foot_h),
                    full.max,
                );
                let rail =
                    Rect::from_min_max(full.min, egui::pos2(full.left() + rail_w, foot.top()));
                let body = Rect::from_min_max(
                    egui::pos2(rail.right(), full.top()),
                    egui::pos2(full.right(), foot.top()),
                );

                let p = ui.painter();
                p.rect_filled(full, 0.0, t.bg);
                p.vline(rail.right(), rail.y_range(), Stroke::new(1.0, t.border));
                p.hline(full.x_range(), foot.top(), Stroke::new(1.0, t.border));

                // ---- rail ----
                let mut rail_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rail.shrink(8.0))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                rail_ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
                for (i, (glyph, name)) in PANES.iter().enumerate() {
                    if rail_item(&mut rail_ui, glyph, name, self.pane == i) {
                        self.pane = i;
                    }
                }

                // ---- footer ----
                let mut foot_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(foot.shrink2(Vec2::new(14.0, 10.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                foot_ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                if button(&mut foot_ui, "Save", false).clicked() {
                    let saved=save(&self.s);
                    if saved && self.ytdlp_rx.is_none() {self.ytdlp_rx=Some(ytdlp_probe(false));self.ytdlp_busy=true;}
                    self.notice = if saved {
                        "Saved ✓ (the main app picks it up live)".to_string()
                    } else {
                        "Could not write settings.".to_string()
                    };
                }
                if !self.notice.is_empty() {
                    foot_ui.add_space(4.0);
                    txt(&mut foot_ui, self.notice.as_str(), reg(10.0), t.muted);
                }
                foot_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if button(ui, "Close", false).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                // ---- content ----
                let mut body_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(body.shrink2(Vec2::new(20.0, 16.0)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                body_ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                let pane = self.pane;
                egui::ScrollArea::vertical()
                    .id_salt(("settings_scroll", pane))
                    .auto_shrink([false, false])
                    .show(&mut body_ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
                        match pane {
                            0 => self.pane_appearance(ui),
                            1 => self.pane_engine(ui),
                            PANE_YTDLP => self.pane_plugins(ui),
                            3 => self.pane_browser(ui),
                            4 => self.pane_proxy(ui),
                            5 => self.pane_rules(ui),
                            6 => self.pane_sounds(ui),
                            _ => self.pane_updates(ui),
                        }
                        ui.add_space(16.0);
                    });
            });

        // The plugin probe runs on a worker thread: keep repainting while it
        // is pending so its answer appears without a mouse move.
        if self.ytdlp_busy || self.ffmpeg_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
}

// ===================== concept tokens + primitives =====================

/// The concept's token set, read from the live palette so the window follows
/// System/Dark/Light with every other surface. Slate/blue hex values from the
/// mockup map onto the app's nearest equivalent.
struct Tok {
    /// Window, rail and content background (`--bg`).
    bg: Color32,
    /// Cards and plain buttons (`--bg-secondary`, `--btn-bg`).
    card: Color32,
    /// Text fields and value boxes (`--bg-input`).
    input: Color32,
    /// Hover wash (`--bg-hover`).
    hover: Color32,
    /// Plain/secondary button fill (`--btn-bg`). A real step off the card, so a
    /// secondary button reads as a button instead of stray text: the mockup
    /// pairs `--btn-bg` with `--btn-border` on every non-primary `.btn`.
    btn: Color32,
    /// Plain button border (`--btn-border`).
    btn_bd: Color32,
    /// Plain button hover fill: the next step up from `btn`.
    btn_hover: Color32,
    text: Color32,
    text2: Color32,
    muted: Color32,
    border: Color32,
    accent: Color32,
    accent_hi: Color32,
    /// Toggle/slider trough when off (`--toggle-off`).
    track: Color32,
}

fn tok() -> Tok {
    let p = pl();
    let dark = p.txt.r() > 128;
    let card = if dark { p.surface_lt } else { p.bg };
    Tok {
        bg: p.surface,
        card,
        input: if dark { p.central_bg } else { p.bg },
        hover: if dark { p.dm_hover2 } else { p.surface_lt },
        btn: blend(card, p.txt, 0.08),
        btn_bd: blend(card, p.txt, 0.22),
        btn_hover: blend(card, p.txt, 0.18),
        text: p.txt,
        text2: p.dim,
        muted: if dark { p.dm_lbl } else { p.dm_head },
        border: p.card_bd,
        accent: p.primary,
        accent_hi: if dark {
            blend(p.primary, Color32::WHITE, 0.18)
        } else {
            blend(p.primary, Color32::BLACK, 0.12)
        },
        track: p.track,
    }
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Translucent accent wash for active rows and brand tiles.
fn wash(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn txt(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(
        egui::Label::new(egui::RichText::new(s.into()).font(font).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(Sense::hover()),
    )
}

fn wrapped(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32) -> Response {
    ui.add(
        egui::Label::new(egui::RichText::new(s.into()).font(font).color(color))
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Wrap),
    )
}

/// `h2` of the concept: 16 px semibold with a 16 px gap below.
fn h2(ui: &mut Ui, s: &str) {
    let s = crate::i18n::t(s);
    txt(ui, s, semi(16.0), tok().text);
    ui.add_space(16.0);
}

/// Setting label: 12 px, secondary colour.
fn lbl(ui: &mut Ui, s: &str) {
    let s = crate::i18n::t(s);
    txt(ui, s, reg(12.0), tok().text2);
}

/// 10 px muted hint.
fn hint(ui: &mut Ui, s: &str) {
    let s = crate::i18n::t(s);
    wrapped(ui, s, reg(10.0), tok().muted);
}

/// Plain button: `--btn-bg` / `--btn-border`, 5 px radius, 6×16 padding.
fn button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let label = crate::i18n::t(label);
    let t = tok();
    let enabled = ui.is_enabled();
    let fg = if primary {
        Color32::WHITE
    } else if enabled {
        t.text2
    } else {
        t.muted
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let size = Vec2::new(
        galley.size().x + 32.0,
        (galley.size().y + 12.0).max(28.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered() && enabled;
    let (bg, bd) = if primary {
        (
            if hov { t.accent_hi } else { t.accent },
            if hov { t.accent_hi } else { t.accent },
        )
    } else {
        (if hov { t.btn_hover } else { t.btn }, t.btn_bd)
    };
    // A hovered secondary button also brightens its label, so the highlight is
    // unmistakable even where the fill step is small.
    let galley = if !primary && hov {
        ui.painter().layout_no_wrap(label.to_owned(), reg(11.0), t.text)
    } else {
        galley
    };
    let p = ui.painter();
    p.rect(
        rect,
        5.0,
        bg,
        Stroke::new(1.0, bd),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Square icon button (`--btn-icon`, 30×30 / `--rule-btn`, 28×28).
fn icon_sq(ui: &mut Ui, glyph: &str, size: f32, radius: f32) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    p.rect(
        rect,
        radius,
        if resp.hovered() { t.btn_hover } else { t.btn },
        Stroke::new(1.0, t.btn_bd),
        StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        crate::fonts::icon(size * 0.42),
        if resp.hovered() { t.text } else { t.text2 },
    );
    resp
}

/// Rule row button: bordered up/down, borderless delete with a red hover.
fn rule_btn(ui: &mut Ui, glyph: &str, danger: bool) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if danger {
        let red = Color32::from_rgb(0xef, 0x44, 0x44);
        if resp.hovered() {
            p.rect_filled(rect, 4.0, wash(red, 24));
        }
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            crate::fonts::icon(12.0),
            if resp.hovered() { red } else { t.muted },
        );
    } else {
        p.rect(
            rect,
            4.0,
            if resp.hovered() { t.btn_hover } else { t.btn },
            Stroke::new(1.0, t.btn_bd),
            StrokeKind::Inside,
        );
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            crate::fonts::icon(12.0),
            if resp.hovered() { t.text } else { t.text2 },
        );
    }
    resp
}

/// Frameless editor inside a painted box (`.input`): 5 px radius, 1 px border,
/// accent border while focused.
fn field(ui: &mut Ui, buf: &mut String, width: Option<f32>, hint: &str) -> Response {
    let t = tok();
    let total = width.unwrap_or_else(|| ui.available_width()).max(60.0);
    let text_w = (total - 20.0).max(20.0);
    let out = egui::Frame::new()
        .fill(t.input)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(text_w);
            ui.add(
                egui::TextEdit::singleline(buf)
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .font(reg(11.0))
                    .text_color(t.text)
                    .hint_text(egui::RichText::new(hint).font(reg(11.0)).color(t.muted))
                    .desired_width(text_w)
                    .vertical_align(egui::Align::Center),
            )
        });
    if out.inner.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            5.0,
            Stroke::new(1.0, t.accent),
            StrokeKind::Inside,
        );
    }
    out.inner
}

/// `.checkbox`: 14×14 box (12×12 compact), accent fill + tick when checked.
/// `full` stretches the click row to the pane width, otherwise it shrinks.
fn checkbox(
    ui: &mut Ui,
    on: &mut bool,
    label: &str,
    box_size: f32,
    font_size: f32,
    full: bool,
) -> bool {
    let label = crate::i18n::t(label);
    let t = tok();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(font_size), t.text);
    let w = if full {
        ui.available_width().max(80.0)
    } else {
        box_size + 8.0 + galley.size().x + 2.0
    };
    let h = box_size.max(galley.size().y).max(16.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let br = Rect::from_min_size(
        egui::pos2(rect.left() + 1.0, rect.center().y - box_size / 2.0),
        Vec2::splat(box_size),
    );
    let p = ui.painter();
    let border = if *on {
        t.accent
    } else if resp.hovered() {
        t.text2
    } else {
        // A bare `t.border` is a hairline tuned for the pane background; on a
        // card fill the two values are within one step of each other and the
        // box disappears, so an off box gets a real control border (the
        // concept's `--btn-border`) and the concept's input fill.
        blend(t.border, t.text2, 0.35)
    };
    p.rect(
        br,
        3.0,
        if *on { t.accent } else { t.input },
        Stroke::new(1.5, border),
        StrokeKind::Inside,
    );
    if *on {
        let c = br.center();
        let s = box_size / 14.0;
        let tick = Stroke::new(1.8, Color32::WHITE);
        p.line_segment(
            [c + Vec2::new(-4.5, 0.0) * s, c + Vec2::new(-1.5, 3.0) * s],
            tick,
        );
        p.line_segment(
            [c + Vec2::new(-1.5, 3.0) * s, c + Vec2::new(4.5, -3.5) * s],
            tick,
        );
    }
    p.galley(
        egui::pos2(br.right() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        t.text,
    );
    if resp.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

// ---------- Appearance: theme tiles + switch row ----------

/// Which preview a theme tile shows.
#[derive(Clone, Copy)]
enum TileKind {
    System,
    Dark,
    Light,
}

/// Signed side of the directed line `a`→`b` (positive = egui's left half-plane).
fn line_side(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * (p.y - a.y) - dy * (p.x - a.x)
}

/// Sutherland–Hodgman clip of the rect `r` to one half-plane of `a`→`b`. The
/// result is convex, so it goes straight into `Shape::convex_polygon`.
fn clip_half(r: Rect, a: egui::Pos2, b: egui::Pos2, keep_pos: bool) -> Vec<egui::Pos2> {
    let s = |p: egui::Pos2| {
        let v = line_side(p, a, b);
        if keep_pos {
            v
        } else {
            -v
        }
    };
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    let mut out = Vec::with_capacity(6);
    for i in 0..4 {
        let cur = pts[i];
        let nxt = pts[(i + 1) % 4];
        let (sc, sn) = (s(cur), s(nxt));
        if sc >= 0.0 {
            out.push(cur);
        }
        if (sc > 0.0 && sn < 0.0) || (sc < 0.0 && sn > 0.0) {
            let t = sc / (sc - sn);
            out.push(egui::pos2(
                cur.x + (nxt.x - cur.x) * t,
                cur.y + (nxt.y - cur.y) * t,
            ));
        }
    }
    out
}

/// Fill `rect` with `color`, optionally clipped to a half-plane.
fn face_rect(
    p: &egui::Painter,
    rect: Rect,
    color: Color32,
    clip: Option<(egui::Pos2, egui::Pos2, bool)>,
) {
    match clip {
        None => {
            p.rect_filled(rect, 0.0, color);
        }
        Some((a, b, kp)) => {
            let pts = clip_half(rect, a, b, kp);
            if pts.len() >= 3 {
                p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
            }
        }
    }
}

/// One mini application window (the mock's `.tp`): head bar with three dots, a
/// sidebar and four content lines, the last one accent. `clip` carves it to one
/// half of the System tile's diagonal.
fn draw_face(
    p: &egui::Painter,
    r: Rect,
    light: bool,
    clip: Option<(egui::Pos2, egui::Pos2, bool)>,
) {
    let c = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    let (bg, head, hbd, side, sbd, line, dot) = if light {
        (
            0xffffff, 0xf8fafc, 0xe2e8f0, 0xeef2f7, 0xe2e8f0, 0xdbe3ec, 0xcbd5e1,
        )
    } else {
        (
            0x0f172a, 0x0b1220, 0x1b2637, 0x1e293b, 0x0b1220, 0x334155, 0x334155,
        )
    };
    let accent = c(0x3b82f6);

    face_rect(p, r, c(bg), clip);
    let head_h = (r.height() * 0.17).clamp(7.0, 12.0);
    face_rect(
        p,
        Rect::from_min_max(r.min, egui::pos2(r.max.x, r.min.y + head_h)),
        c(head),
        clip,
    );
    face_rect(
        p,
        Rect::from_min_max(
            egui::pos2(r.min.x, r.min.y + head_h - 1.0),
            egui::pos2(r.max.x, r.min.y + head_h),
        ),
        c(hbd),
        clip,
    );
    let side_w = r.width() * 0.24;
    face_rect(
        p,
        Rect::from_min_max(
            egui::pos2(r.min.x, r.min.y + head_h),
            egui::pos2(r.min.x + side_w, r.max.y),
        ),
        c(side),
        clip,
    );
    face_rect(
        p,
        Rect::from_min_max(
            egui::pos2(r.min.x + side_w, r.min.y + head_h),
            egui::pos2(r.min.x + side_w + 1.0, r.max.y),
        ),
        c(sbd),
        clip,
    );
    // Head dots are circles, so a clipped tile only keeps the ones fully inside.
    for i in 0..3 {
        let dc = egui::pos2(r.min.x + 4.0 + i as f32 * 4.0, r.min.y + head_h / 2.0);
        let keep = match clip {
            None => true,
            Some((a, b, kp)) => {
                let v = line_side(dc, a, b);
                if kp {
                    v > 1.5
                } else {
                    v < -1.5
                }
            }
        };
        if keep {
            p.circle_filled(dc, 1.7, c(dot));
        }
    }
    let main_x = r.min.x + side_w + 4.0;
    let main_w = (r.max.x - 5.0 - main_x).max(6.0);
    let top = r.min.y + head_h + 4.5;
    let step = ((r.max.y - 4.0 - top) / 4.0).max(3.0);
    for (i, wr) in [0.60_f32, 0.90, 0.75, 0.45].iter().enumerate() {
        let lr = Rect::from_min_size(
            egui::pos2(main_x, top + step * i as f32),
            Vec2::new((main_w * wr).max(4.0), 2.5),
        );
        face_rect(p, lr, if i == 3 { accent } else { c(line) }, clip);
    }
}

/// One theme tile (the mock's `.theme-tile`): a mini-window preview, the name
/// under it and an accent check badge when it is the active theme.
fn theme_tile(
    ui: &mut Ui,
    salt: &str,
    w: f32,
    h: f32,
    kind: TileKind,
    name: &str,
    active: bool,
) -> Response {
    let t = tok();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let resp = ui
        .interact(rect, ui.id().with(("theme_tile", salt)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    let pad = 4.0;
    p.rect(
        rect,
        8.0,
        t.input,
        Stroke::new(
            1.0,
            if active {
                t.accent
            } else if resp.hovered() {
                t.muted
            } else {
                t.border
            },
        ),
        StrokeKind::Inside,
    );
    if active {
        p.rect_stroke(rect, 8.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    // Preview box (`.tp`): a dark base so a clipped corner never flashes white.
    let prev = Rect::from_min_size(
        egui::pos2(rect.min.x + pad, rect.min.y + pad),
        Vec2::new(w - pad * 2.0, (h - pad * 2.0 - 19.0).max(24.0)),
    );
    p.rect_filled(prev, 5.0, Color32::from_rgb(0x0f, 0x17, 0x2a));
    let inner = prev.shrink(1.0);
    match kind {
        TileKind::System => {
            // Light left of the diagonal, dark right of it.
            let a = egui::pos2(inner.min.x + inner.width() * 0.62, inner.min.y);
            let b = egui::pos2(inner.min.x + inner.width() * 0.38, inner.max.y);
            draw_face(p, inner, true, None);
            draw_face(p, inner, false, Some((a, b, false)));
        }
        TileKind::Dark => draw_face(p, inner, false, None),
        TileKind::Light => draw_face(p, inner, true, None),
    }
    p.rect_stroke(prev, 5.0, Stroke::new(1.0, t.border), StrokeKind::Inside);
    if active {
        let bc = egui::pos2(prev.max.x - 8.0, prev.min.y + 8.0);
        p.circle_filled(bc, 7.5, t.accent);
        let s = Stroke::new(1.6, Color32::WHITE);
        p.line_segment([bc + Vec2::new(-3.2, -0.2), bc + Vec2::new(-1.0, 2.2)], s);
        p.line_segment([bc + Vec2::new(-1.0, 2.2), bc + Vec2::new(3.4, -2.6)], s);
    }
    p.text(
        egui::pos2(rect.min.x + pad + 3.0, rect.max.y - pad - 2.0),
        Align2::LEFT_BOTTOM,
        name,
        reg(10.5),
        if active { t.text } else { t.text2 },
    );
    resp
}

/// A settings row with its label + hint on the left and a toggle on the right
/// (the mock's `.opt-row`). Only the switch is clickable.
fn switch_row(ui: &mut Ui, salt: &str, on: &mut bool, label: &str, hint: &str) -> bool {
    let (label, hint) = (crate::i18n::t(label), crate::i18n::t(hint));
    let t = tok();
    let w = ui.available_width().max(120.0);
    let sw = 34.0;
    let text_w = (w - sw - 14.0).max(80.0);
    let lg = ui
        .painter()
        .layout_no_wrap(label.to_owned(), reg(11.5), t.text);
    let hg = ui
        .painter()
        .layout(hint.to_owned(), reg(9.5), t.muted, text_w);
    let h = (lg.size().y + 3.0 + hg.size().y).max(26.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let track = Rect::from_min_size(
        egui::pos2(rect.max.x - sw, rect.center().y - 9.5),
        Vec2::new(sw, 19.0),
    );
    let id = ui.id().with(("switch", salt));
    let resp = ui
        .interact(
            Rect::from_min_max(
                egui::pos2(track.min.x - 4.0, rect.min.y),
                egui::pos2(rect.max.x, rect.max.y),
            ),
            id,
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let anim = ui.ctx().animate_bool_with_time(id, *on, 0.15);
    let p = ui.painter();
    let top = rect.center().y - h / 2.0;
    p.galley(egui::pos2(rect.min.x, top), lg.clone(), t.text);
    p.galley(
        egui::pos2(rect.min.x, top + lg.size().y + 3.0),
        hg,
        t.muted,
    );
    p.rect_filled(track, 9.5, blend(t.track, t.accent, anim));
    p.circle_filled(
        egui::pos2(track.left() + 9.5 + 15.0 * anim, track.center().y),
        7.0,
        Color32::WHITE,
    );
    if resp.clicked() {
        *on = !*on;
        return true;
    }
    false
}

// ---------- Download Engine: cards + field rows ----------

/// `--border-soft`: the card body's hairline, a step off the card fill.
fn soft_line(t: &Tok) -> Color32 {
    blend(t.card, t.border, 0.55)
}

/// A collapsible card (`.card`): one painted head that owns the open state and
/// carries the chevron, then the body behind a hairline.
///
/// Everything is built inside a scope salted with the card title: `Frame` hands
/// every one of its content Uis the same id salt (`Id::from("child")`), so
/// without it the implicit ids of identical widgets in two cards (the head, a
/// checkbox) would collide and a click on one card would hit its twin.
fn card(
    ui: &mut Ui,
    title: &str,
    badge: Option<&str>,
    open: &mut bool,
    body: impl FnOnce(&mut Ui),
) {
    let t = tok();
    let mut scope = ui.new_child(egui::UiBuilder::new().id_salt(("engine_card", title)));
    let title = crate::i18n::t(title);
    egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(9.0)
        .inner_margin(egui::Margin::ZERO)
        .outer_margin(egui::Margin {
            bottom: 6,
            ..Default::default()
        })
        .show(&mut scope, |ui| {
            let w = ui.available_width();
            let (head, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(
                    head,
                    egui::CornerRadius {
                        nw: 9,
                        ne: 9,
                        sw: 0,
                        se: 0,
                    },
                    t.hover,
                );
            }
            let tg = p.layout_no_wrap(title.to_owned(), semi(11.5), t.text);
            p.galley(
                egui::pos2(head.left() + 11.0, head.center().y - tg.size().y / 2.0),
                tg.clone(),
                t.text,
            );
            if let Some(b) = badge {
                let bg = p.layout_no_wrap(b.to_owned(), reg(9.0), t.muted);
                let chip = Rect::from_min_size(
                    egui::pos2(
                        head.left() + 11.0 + tg.size().x + 6.0,
                        head.center().y - 6.5,
                    ),
                    Vec2::new(bg.size().x + 12.0, 13.0),
                );
                p.rect(
                    chip,
                    6.5,
                    t.input,
                    Stroke::new(1.0, t.border),
                    StrokeKind::Inside,
                );
                p.galley(chip.center() - bg.size() / 2.0, bg, t.muted);
            }
            p.text(
                egui::pos2(head.right() - 12.0, head.center().y),
                Align2::CENTER_CENTER,
                if *open { "\u{e5ce}" } else { "\u{e5cf}" }, // expand_less / expand_more
                crate::fonts::icon(13.0),
                t.muted,
            );
            if resp.clicked() {
                *open = !*open;
            }
            if *open {
                let top = ui.cursor().min.y;
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    top,
                    Stroke::new(1.0, soft_line(&t)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 11,
                        right: 11,
                        top: 8,
                        bottom: 9,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        body(ui);
                    });
            }
        });
    ui.advance_cursor_after_rect(scope.min_rect());
}

/// One `.field`: a 138 px label column, the control column beside it and an
/// optional hint under the control. Returns the Ui the control goes into.
fn field_row(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    badge: Option<&str>,
    hint_text: Option<&str>,
) -> Ui {
    let (label, hint_text) = (crate::i18n::t(label), hint_text.map(crate::i18n::t));
    let t = tok();
    let full = ui.available_width();
    let label_w = 138.0_f32.min((full * 0.42).max(92.0));
    let gap = 11.0;
    let ctrl_w = (full - label_w - gap).max(60.0);
    let ctrl_h = 28.0;
    let hint = hint_text.map(|h| ui.painter().layout(h.to_owned(), reg(9.5), t.muted, ctrl_w));
    let hint_h = hint.as_ref().map_or(0.0, |g| g.size().y);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(full, ctrl_h + if hint_h > 0.0 { 4.0 + hint_h } else { 0.0 }),
        Sense::hover(),
    );
    let p = ui.painter();
    let lg = p.layout_no_wrap(label.to_owned(), reg(11.0), t.text2);
    let ly = if hint_h > 0.0 {
        rect.top() + 7.0
    } else {
        rect.top() + (ctrl_h - lg.size().y) / 2.0
    };
    p.galley(egui::pos2(rect.left(), ly), lg.clone(), t.text2);
    if let Some(b) = badge {
        let bg = p.layout_no_wrap(b.to_owned(), reg(9.0), t.muted);
        let chip = Rect::from_min_size(
            egui::pos2(rect.left() + lg.size().x + 6.0, ly + lg.size().y / 2.0 - 6.5),
            Vec2::new(bg.size().x + 12.0, 13.0),
        );
        p.rect(
            chip,
            6.5,
            t.input,
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );
        p.galley(chip.center() - bg.size() / 2.0, bg, t.muted);
    }
    if let Some(g) = &hint {
        p.galley(
            egui::pos2(rect.left() + label_w + gap, rect.top() + ctrl_h + 4.0),
            g.clone(),
            t.muted,
        );
    }
    ui.add_space(7.0);
    ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("field", id_salt))
            .max_rect(Rect::from_min_size(
                egui::pos2(rect.left() + label_w + gap, rect.top()),
                Vec2::new(ctrl_w, ctrl_h),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    )
}

/// `.input`: 6 px radius, 1 px border, accent ring while focused.
fn input_box(
    ui: &mut Ui,
    id: &str,
    buf: &mut String,
    w: f32,
    mono: bool,
    center: bool,
    hint: &str,
) -> Response {
    let t = tok();
    let font = if mono {
        egui::FontId::monospace(10.0)
    } else {
        reg(11.0)
    };
    let text_w = (w - 18.0).max(16.0);
    let out = egui::Frame::new()
        .fill(t.input)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.set_width(text_w);
            ui.add(
                egui::TextEdit::singleline(buf)
                    .id(egui::Id::new(id))
                    .frame(false)
                    .margin(egui::Margin::ZERO)
                    .font(font.clone())
                    .text_color(t.text)
                    .horizontal_align(if center {
                        egui::Align::Center
                    } else {
                        egui::Align::Min
                    })
                    .hint_text(egui::RichText::new(hint).font(font).color(t.muted))
                    .desired_width(text_w)
                    .vertical_align(egui::Align::Center),
            )
        });
    if out.inner.has_focus() {
        ui.painter().rect_stroke(
            out.response.rect,
            6.0,
            Stroke::new(1.0, t.accent),
            StrokeKind::Inside,
        );
    }
    out.inner
}

/// `.slider-row`: the 4 px slider plus its editable value box (two-way).
fn slider_row(ui: &mut Ui, salt: &str, value: &mut f64, min: f64, max: f64) -> bool {
    let t = tok();
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
    let box_rect = Rect::from_min_size(
        egui::pos2(rect.right() - 52.0, rect.top()),
        Vec2::new(52.0, 26.0),
    );
    let track_area = Rect::from_min_max(
        egui::pos2(rect.left() + 2.0, rect.top()),
        egui::pos2(box_rect.left() - 10.0, rect.bottom()),
    );
    let resp = ui.interact(
        track_area,
        ui.id().with(("srow", salt)),
        Sense::click_and_drag(),
    );
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(ptr) = resp.interact_pointer_pos() {
            let frac =
                ((ptr.x - track_area.left()) / track_area.width().max(1.0)).clamp(0.0, 1.0) as f64;
            *value = (min + frac * (max - min)).round();
            changed = true;
        }
    }
    let frac = (((*value - min) / (max - min)).clamp(0.0, 1.0)) as f32;
    let cy = rect.center().y;
    let track = Rect::from_min_max(
        egui::pos2(track_area.left(), cy - 2.0),
        egui::pos2(track_area.right(), cy + 2.0),
    );
    let p = ui.painter();
    p.rect_filled(track, 2.0, t.track);
    if frac > 0.0 {
        p.rect_filled(
            Rect::from_min_max(
                track.min,
                egui::pos2(track.left() + track.width() * frac, track.max.y),
            ),
            2.0,
            t.accent,
        );
    }
    let knob = egui::pos2(track.left() + track.width() * frac, cy);
    if resp.hovered() || resp.dragged() {
        p.circle_filled(knob, 9.0, wash(t.accent, 36));
    }
    p.circle_filled(knob, 7.0, t.accent);
    p.circle_stroke(knob, 7.0, Stroke::new(2.0, t.card));
    // An editable box, so a big jump needs no dragging.
    let mut buf = format!("{}", *value as i64);
    let mut box_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("srownum", salt))
            .max_rect(box_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let r = input_box(&mut box_ui, &format!("srownum{salt}"), &mut buf, 52.0, false, true, "");
    if r.lost_focus() || r.changed() {
        if let Ok(n) = buf.trim().parse::<f64>() {
            let n = n.clamp(min, max);
            if n != *value {
                *value = n;
                changed = true;
            }
        }
    }
    changed
}

/// `.segmented`: option group in an inset trough, accent-filled selection.
fn segmented(ui: &mut Ui, salt: &str, options: &[&str], current: &mut String) -> bool {
    let t = tok();
    let pad = 2.0;
    let gap = 3.0;
    let seg_h = 24.0;
    let gals: Vec<std::sync::Arc<egui::Galley>> = options
        .iter()
        .map(|o| {
            ui.painter()
                .layout_no_wrap((*o).to_owned(), reg(10.5), t.text2)
        })
        .collect();
    let widths: Vec<f32> = gals.iter().map(|g| g.size().x + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + gap * (options.len() as f32 - 1.0) + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, seg_h + pad * 2.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        8.0,
        t.input,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    let mut changed = false;
    let mut x = rect.left() + pad;
    for (i, o) in options.iter().enumerate() {
        let seg = Rect::from_min_size(
            egui::pos2(x, rect.top() + pad),
            Vec2::new(widths[i], seg_h),
        );
        let resp = ui
            .interact(seg, ui.id().with(("seg", salt, i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let active = current == o;
        if active {
            p.rect_filled(seg, 6.0, t.accent);
        } else if resp.hovered() {
            p.rect_filled(seg, 6.0, wash(t.text, 10));
        }
        let fg = if active {
            Color32::WHITE
        } else if resp.hovered() {
            t.text
        } else {
            t.text2
        };
        let g = p.layout_no_wrap((*o).to_owned(), reg(10.5), fg);
        p.galley(seg.center() - g.size() / 2.0, g, fg);
        if resp.clicked() && !active {
            *current = (*o).to_string();
            changed = true;
        }
        x += widths[i] + gap;
    }
    changed
}

/// `.btn-dashed` with a leading icon (the "Add window" button).
fn btn_dashed(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let t = tok();
    let (iw, tw) = {
        let p = ui.painter();
        (
            p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), t.text2)
                .size()
                .x,
            p.layout_no_wrap(label.to_owned(), reg(11.0), t.text2)
                .size()
                .x,
        )
    };
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(iw + 6.0 + tw + 22.0, 26.0),
        Sense::click(),
    );
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if hov { t.accent_hi } else { t.text2 };
    let p = ui.painter();
    dashed_rect(
        p,
        rect,
        6.0,
        Stroke::new(1.0, if hov { t.accent } else { t.btn_bd }),
    );
    let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg);
    let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let x = rect.center().x - (iw + 6.0 + tw) / 2.0;
    p.galley(egui::pos2(x, rect.center().y - ig.size().y / 2.0), ig, fg);
    p.galley(
        egui::pos2(x + iw + 6.0, rect.center().y - tg.size().y / 2.0),
        tg,
        fg,
    );
    resp
}

/// One `.window-row`: start — end, cap, unit, delete. Returns "delete me".
fn window_row(ui: &mut Ui, idx: usize, w: &mut SpeedWindow) -> bool {
    let t = tok();
    let full = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(full, 28.0), Sense::hover());
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("swrow", idx))
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 7.0;
    input_box(&mut row, "sw_start", &mut w.start, 60.0, true, true, "02:00");
    txt(&mut row, "—", reg(11.0), t.muted);
    input_box(&mut row, "sw_end", &mut w.end, 60.0, true, true, "06:00");
    let mut cap = w.limit_kbps.to_string();
    input_box(&mut row, "sw_cap", &mut cap, 52.0, false, true, "0");
    if let Ok(n) = cap.trim().parse::<u64>() {
        w.limit_kbps = n;
    }
    txt(&mut row, "KB/s", reg(10.0), t.muted);
    let del = rule_btn(&mut row, "\u{e5cd}", true).clicked();
    ui.add_space(6.0);
    del
}

/// Sidebar entry: 14 px icon + 12 px label, 6 px radius, accent active row.
fn rail_item(ui: &mut Ui, icon: &str, label: &str, active: bool) -> bool {
    let label = crate::i18n::t(label);
    let t = tok();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if active {
        p.rect_filled(rect, 6.0, wash(t.accent, 30));
    } else if resp.hovered() {
        p.rect_filled(rect, 6.0, t.hover);
    }
    let col = if active { t.accent } else { t.text2 };
    if icon.chars().count() == 1 {
        p.text(egui::pos2(rect.left() + 10.0, rect.center().y), Align2::LEFT_CENTER, icon, crate::fonts::icon(14.0), col);
    } else {
        crate::app::line_icon(p, egui::pos2(rect.left() + 17.0, rect.center().y), 15.0, icon, col);
    }
    p.text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if active { semi(11.5) } else { reg(11.5) },
        col,
    );
    resp.clicked()
}

// ===================== Plugins card pieces =====================

/// Platform brands the helper understands. `follows_text` marks logos that
/// are black/white by design (they invert with the theme); plain ASCII glyphs
/// ("X", "in", …) are drawn as brand initials because Material Icons has no
/// such logo, and are never asked of the icon font (which would box them).
const PLATFORMS: &[(&str, &str, Color32, bool)] = &[
    ("YouTube", "\u{f06a}", Color32::from_rgb(0xff, 0x00, 0x00), false), // smart_display
    ("Facebook", "\u{f234}", Color32::from_rgb(0x18, 0x77, 0xf2), false),
    ("TikTok", "\u{e405}", Color32::from_rgb(0x00, 0x00, 0x00), true), // music_note
    ("Instagram", "\u{e412}", Color32::from_rgb(0xe1, 0x30, 0x6c), false), // photo_camera
    ("X/Twitter", "X", Color32::from_rgb(0x00, 0x00, 0x00), true),
    ("Twitch", "\u{e04b}", Color32::from_rgb(0x91, 0x46, 0xff), false), // videocam
    ("Reddit", "\u{eaa0}", Color32::from_rgb(0xff, 0x45, 0x00), false),
    ("Discord", "\u{ea28}", Color32::from_rgb(0x58, 0x65, 0xf2), false), // sports_esports
    ("Vimeo", "V", Color32::from_rgb(0x1a, 0xb7, 0xea), false),
    ("Snapchat", "\u{ea6e}", Color32::from_rgb(0xe3, 0xc4, 0x00), false),
    ("Dailymotion", "d", Color32::from_rgb(0x00, 0xb8, 0xa9), false),
    ("LinkedIn", "in", Color32::from_rgb(0x0a, 0x66, 0xc2), false),
    ("SoundCloud", "\u{e2bd}", Color32::from_rgb(0xff, 0x55, 0x00), false), // cloud
];

/// Closing tile of the platform grid: the size of the extractor list, so the
/// grid does not read as "these thirteen sites and nothing else".
const MORE_SITES: &str = "1,000+";

/// What the YouTube helper fetches (the card's tick list).
const YTDLP_CAPS: [&str; 6] = [
    "YouTube: videos, audio, playlists, channels",
    "Facebook: public videos and Reels",
    "TikTok, Instagram, Vimeo, Reddit",
    "X (Twitter), LinkedIn and 1,000+ more",
    "Up to 1080p and 4K (needs ffmpeg)",
    "Subtitles, thumbnails and metadata",
];

/// Why ffmpeg is worth having (its own card's tick list).
const FFMPEG_CAPS: [&str; 4] = [
    "Merges separate video and audio streams",
    "Required for 1080p and 4K on YouTube",
    "Remuxes and converts containers",
    "Embeds subtitles and cover art",
];

/// Platform tiles in the concept's `repeat(7, 1fr)` grid: 30 px tiles spread
/// edge to edge, 8 px names, closed by a dashed "1,000+ more" tile.
fn platform_grid(ui: &mut Ui) {
    let t = tok();
    let tiles = PLATFORMS.len() + 1; // + the closing "1,000+" tile
    let gap = 4.0; // `.platform-grid { gap: 6px 4px }`
    let avail = ui.available_width();
    let cols = 7.min(tiles).max(1);
    let col_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).max(24.0);
    let rows = tiles.div_ceil(cols);
    // The closing tile's name is two lines ("1,000+" / "more"), so the row
    // holding it is taller. Everything else is one 8 px line.
    let tall_row = (tiles - 1) / cols;
    let row_h = |r: usize| {
        if r == tall_row {
            30.0 + 3.0 + 19.0
        } else {
            30.0 + 3.0 + 10.0
        }
    };
    let total: f32 = (0..rows).map(row_h).sum::<f32>() + 6.0 * (rows as f32 - 1.0).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, total), Sense::hover());
    let p = ui.painter();
    let mut y = rect.top();
    for r in 0..rows {
        for c in 0..cols {
            let i = r * cols + c;
            if i >= tiles {
                break;
            }
            let cx = rect.left() + c as f32 * (col_w + gap) + col_w / 2.0;
            let tile = Rect::from_center_size(egui::pos2(cx, y + 15.0), Vec2::splat(30.0));
            if i == PLATFORMS.len() {
                // Dashed outline + plus: the extractor list does not stop here.
                dashed_rect(p, tile, 8.0, Stroke::new(1.0, t.btn_bd));
                p.text(
                    tile.center(),
                    Align2::CENTER_CENTER,
                    "\u{e145}", // add
                    crate::fonts::icon(16.0),
                    t.muted,
                );
                p.text(
                    egui::pos2(cx, tile.bottom() + 3.0),
                    Align2::CENTER_TOP,
                    MORE_SITES,
                    reg(8.0),
                    t.muted,
                );
                p.text(
                    egui::pos2(cx, tile.bottom() + 11.5),
                    Align2::CENTER_TOP,
                    "more",
                    reg(8.0),
                    t.muted,
                );
                continue;
            }
            let (name, glyph, brand, follows_text) = PLATFORMS[i];
            let col = if follows_text { t.text } else { brand };
            p.rect_filled(tile, 8.0, wash(col, 30));
            // Brand initials are ASCII ("X", "V", "d", "in"); everything else is
            // a Material Icons codepoint. Asking the icon font for a letter
            // would paint a .notdef box, so the two are never mixed up.
            p.text(
                tile.center(),
                Align2::CENTER_CENTER,
                glyph,
                if glyph.is_ascii() {
                    semi(if glyph.len() > 1 { 11.0 } else { 13.0 })
                } else {
                    crate::fonts::icon(16.0)
                },
                col,
            );
            p.text(
                egui::pos2(cx, tile.bottom() + 3.0),
                Align2::CENTER_TOP,
                name,
                reg(8.0),
                t.muted,
            );
        }
        y += row_h(r) + 6.0;
    }
}

/// Dashed rounded outline (the concept's `1px dashed` "and 1,000+ more" tile).
fn dashed_rect(p: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let (dash, gap) = (3.0, 2.0);
    // Straight runs, inset so the dashes sit inside the rounded corner.
    let r = Rect::from_min_max(
        egui::pos2(rect.left() + 2.0 + radius * 0.15, rect.top() + 0.5),
        egui::pos2(rect.right() - 2.0 - radius * 0.15, rect.bottom() - 0.5),
    );
    let mut x = r.left();
    while x < r.right() {
        let x2 = (x + dash).min(r.right());
        p.line_segment([egui::pos2(x, r.top()), egui::pos2(x2, r.top())], stroke);
        p.line_segment([egui::pos2(x, r.bottom()), egui::pos2(x2, r.bottom())], stroke);
        x += dash + gap;
    }
    let mut y = r.top();
    while y < r.bottom() {
        let y2 = (y + dash).min(r.bottom());
        p.line_segment([egui::pos2(r.left(), y), egui::pos2(r.left(), y2)], stroke);
        p.line_segment([egui::pos2(r.right(), y), egui::pos2(r.right(), y2)], stroke);
        y += dash + gap;
    }
}

/// Two-column tick list (`.feat-list`): a green vector check then 10 px text.
/// Both columns are pre-wrapped so neither can push the other around.
fn capabilities(ui: &mut Ui, caps: &[&str]) {
    let t = tok();
    let green = ok_green();
    let avail = ui.available_width();
    let gap = 14.0;
    let col_w = ((avail - gap) / 2.0).max(60.0);
    let marker = 11.0; // tick + air
    let rows = caps.len().div_ceil(2);
    let galleys: Vec<std::sync::Arc<egui::Galley>> = {
        let p = ui.painter();
        caps.iter()
            .map(|c| p.layout((*c).to_owned(), reg(10.0), t.text2, (col_w - marker).max(24.0)))
            .collect()
    };
    let heights: Vec<f32> = (0..rows)
        .map(|r| {
            (0..2)
                .filter_map(|c| galleys.get(r * 2 + c))
                .map(|g| g.size().y)
                .fold(0.0, f32::max)
        })
        .collect();
    let total: f32 = heights.iter().sum::<f32>() + 4.0 * (rows as f32 - 1.0).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, total), Sense::hover());
    let p = ui.painter();
    let mut y = rect.top();
    for r in 0..rows {
        for c in 0..2 {
            let Some(g) = galleys.get(r * 2 + c) else {
                continue;
            };
            let x = rect.left() + c as f32 * (col_w + gap);
            // The check is a polyline, not a glyph: nothing can go .notdef.
            let tick = Stroke::new(1.4, green);
            let ty = y + 2.0;
            p.line_segment(
                [egui::pos2(x + 1.0, ty + 4.0), egui::pos2(x + 3.6, ty + 6.6)],
                tick,
            );
            p.line_segment(
                [egui::pos2(x + 3.6, ty + 6.6), egui::pos2(x + 8.6, ty + 1.0)],
                tick,
            );
            p.galley(egui::pos2(x + marker, y), g.clone(), t.text2);
        }
        y += heights[r] + 4.0;
    }
}

// ---------- pieces shared by the plugin cards ----------

/// The concept's success green (ticks, "Recommended", the active dot).
fn ok_green() -> Color32 {
    Color32::from_rgb(0x22, 0xc5, 0x5e)
}

/// Section heading (`.p-label`): 9 px semibold, uppercased, 7 px of air below.
fn sec_head(ui: &mut Ui, s: &str) {
    txt(ui, s.to_uppercase(), semi(9.0), tok().muted);
    ui.add_space(7.0);
}

/// The green "Recommended" chip (`.pill-rec`).
fn rec_pill(ui: &mut Ui, label: &str) {
    let green = ok_green();
    let g = ui
        .painter()
        .layout_no_wrap(label.to_uppercase(), semi(8.0), green);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 12.0, 14.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 7.0, wash(green, 40));
    p.galley(rect.center() - g.size() / 2.0, g, green);
}

/// Inline code chip (`code`): the palette's inset surface.
fn code_chip(ui: &mut Ui, s: &str) {
    let t = tok();
    let g = ui
        .painter()
        .layout_no_wrap(s.to_owned(), egui::FontId::monospace(9.0), t.text2);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 10.0, 16.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        4.0,
        t.input,
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - g.size() / 2.0, g, t.text2);
}

/// `.btn` with a leading Material icon. Sizes are measured with the resting
/// colour (identical metrics), so hover changes ink only, never layout.
fn btn_icon(ui: &mut Ui, glyph: &str, label: &str, primary: bool) -> Response {
    let t = tok();
    let fg0 = if primary { Color32::WHITE } else { t.text2 };
    let gap = 6.0;
    let (iw, tw) = {
        let p = ui.painter();
        let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg0);
        let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg0);
        (ig.size().x, tg.size().x)
    };
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(iw + gap + tw + 28.0, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if !primary && hov { t.text } else { fg0 };
    let (bg, bd) = if primary {
        (if hov { t.accent_hi } else { t.accent }, t.accent)
    } else if hov {
        (t.btn_hover, t.btn_bd)
    } else {
        (t.btn, t.btn_bd)
    };
    let p = ui.painter();
    p.rect(rect, 6.0, bg, Stroke::new(1.0, bd), StrokeKind::Inside);
    let ig = p.layout_no_wrap(glyph.to_owned(), crate::fonts::icon(12.0), fg);
    let tg = p.layout_no_wrap(label.to_owned(), reg(11.0), fg);
    let x = rect.center().x - (iw + gap + tw) / 2.0;
    p.galley(egui::pos2(x, rect.center().y - ig.size().y / 2.0), ig, fg);
    p.galley(
        egui::pos2(x + iw + gap, rect.center().y - tg.size().y / 2.0),
        tg,
        fg,
    );
    resp
}

/// `.btn-mini`: the compact button used inside the location options.
fn btn_mini(ui: &mut Ui, label: &str) -> Response {
    let t = tok();
    let w = {
        let g = ui
            .painter()
            .layout_no_wrap(label.to_owned(), reg(10.0), t.text2);
        g.size().x + 20.0
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hov = resp.hovered();
    let fg = if hov { t.text } else { t.text2 };
    let p = ui.painter();
    p.rect(
        rect,
        5.0,
        if hov { t.btn_hover } else { t.btn },
        Stroke::new(1.0, t.btn_bd),
        StrokeKind::Inside,
    );
    let g = p.layout_no_wrap(label.to_owned(), reg(10.0), fg);
    p.galley(rect.center() - g.size() / 2.0, g, fg);
    resp
}

/// The identity row every plugin card shares (`.plugin-head`): brand tile ·
/// name · version chip, then right-aligned meta and the open/closed chevron.
/// `icon` is an `app::line_icon` key: the same outline strokes as the main
/// window's sidebar, in the text colour, so it reads in dark and light alike.
fn plugin_head(
    ui: &mut Ui,
    w: f32,
    icon: &str,
    name: &str,
    version: &str,
    meta: &str,
    open: bool,
) -> Response {
    let t = tok();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 58.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 11.0, wash(t.text, 8));
    }
    let tile = Rect::from_min_size(
        egui::pos2(rect.left() + 13.0, rect.center().y - 17.0),
        Vec2::splat(34.0),
    );
    p.rect(tile, 9.0, wash(t.text, 10), Stroke::new(1.0, t.border), StrokeKind::Inside);
    crate::app::line_icon(p, tile.center(), 18.0, icon, if open { t.accent } else { t.text });
    // name, with the version chip right after it
    let name_g = p.layout_no_wrap(name.to_owned(), semi(13.0), t.text);
    let nx = tile.right() + 11.0;
    p.galley(
        egui::pos2(nx, rect.center().y - name_g.size().y / 2.0),
        name_g.clone(),
        t.text,
    );
    let ver_g = p.layout_no_wrap(version.to_owned(), egui::FontId::monospace(9.0), t.accent);
    let chip = Rect::from_min_size(
        egui::pos2(nx + name_g.size().x + 8.0, rect.center().y - 9.0),
        Vec2::new(ver_g.size().x + 16.0, 18.0),
    );
    p.rect(
        chip,
        9.0,
        wash(t.accent, 30),
        Stroke::new(1.0, t.border),
        StrokeKind::Inside,
    );
    p.galley(chip.center() - ver_g.size() / 2.0, ver_g, t.accent);
    // chevron at the far right, meta to its left
    p.text(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        if open { "\u{e5ce}" } else { "\u{e5cf}" }, // expand_less / expand_more
        crate::fonts::icon(16.0),
        t.muted,
    );
    let meta_g = p.layout_no_wrap(meta.to_owned(), reg(10.0), t.muted);
    p.galley(
        egui::pos2(
            rect.right() - 30.0 - meta_g.size().x,
            rect.center().y - meta_g.size().y / 2.0,
        ),
        meta_g,
        t.muted,
    );
    resp
}

/// One `.loc-opt` radio row: title (+ optional chip), a path/monospace sub-line
/// and an optional inline button. Returns `(row picked, button pressed)`.
#[allow(clippy::too_many_arguments)]
fn loc_option(
    ui: &mut Ui,
    selected: bool,
    title: &str,
    badge: Option<&str>,
    sub: &str,
    mono: bool,
    action: Option<&str>,
) -> (bool, bool) {
    let t = tok();
    let w = ui.available_width();
    let mut action_hit = false;
    let frame = egui::Frame::new()
        .fill(if selected { wash(t.accent, 26) } else { t.bg })
        .stroke(Stroke::new(1.0, if selected { t.accent } else { t.border }))
        .corner_radius(8.0)
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 8,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(w - 22.0);
            ui.spacing_mut().item_spacing.x = 9.0;
            ui.horizontal_top(|ui| {
                // Painted radio: the filled dot is the whole "active" signal.
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                let p = ui.painter();
                let c = egui::pos2(r.center().x, r.center().y + 1.0);
                p.circle_stroke(
                    c,
                    6.0,
                    Stroke::new(1.4, if selected { t.accent } else { t.btn_bd }),
                );
                if selected {
                    p.circle_filled(c, 3.2, t.accent);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        txt(ui, title, semi(11.0), t.text);
                        if let Some(b) = badge {
                            rec_pill(ui, b);
                        }
                    });
                    if mono {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(sub)
                                    .font(egui::FontId::monospace(9.0))
                                    .color(t.muted),
                            )
                            .selectable(false),
                        );
                    } else {
                        wrapped(ui, sub, reg(9.0), t.muted);
                    }
                });
                if let Some(label) = action {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if btn_mini(ui, label).clicked() {
                            action_hit = true;
                        }
                    });
                }
            });
        });
    let resp = frame
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp.clicked(), action_hit)
}

/// Status line: a green (or amber) dot and what yt-dlp/ffmpeg will actually do.
fn loc_status(ui: &mut Ui, text: &str, warn: bool) {
    let t = tok();
    let col = if warn { pl().warning } else { ok_green() };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        let p = ui.painter();
        let c = r.center();
        p.circle_filled(c, 5.0, wash(col, 40)); // the concept's glow ring
        p.circle_filled(c, 3.0, col);
        txt(ui, text, reg(10.0), if warn { col } else { t.text2 });
    });
}

/// `.runtime-note`: "Runtime `node` — solves YouTube's player challenge",
/// right-aligned in the action row.
fn runtime_note(ui: &mut Ui, runtime: Option<&str>) {
    let t = tok();
    let (tail, warn) = runtime_tail(runtime);
    let fg = if warn { pl().warning } else { t.muted };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        txt(ui, tail, reg(9.0), fg);
        if let Some(r) = runtime {
            code_chip(ui, r);
        }
        txt(ui, "Runtime", reg(9.0), fg);
    });
}

/// Trailing words of the runtime note, plus whether they need the warning ink.
fn runtime_tail(runtime: Option<&str>) -> (&'static str, bool) {
    match runtime {
        Some(_) => ("— solves YouTube's player challenge", false),
        None => ("— needs Node.js 22+ or Deno 2.3+", true),
    }
}

/// How much room the runtime note needs, so the actions row can keep it beside
/// the buttons or wrap it onto its own line instead of overlapping them.
fn runtime_note_width(ui: &Ui, runtime: Option<&str>) -> f32 {
    let (tail, _) = runtime_tail(runtime);
    let p = ui.painter();
    let word = p
        .layout_no_wrap("Runtime".to_owned(), reg(9.0), Color32::WHITE)
        .size()
        .x;
    let tail_w = p
        .layout_no_wrap(tail.to_owned(), reg(9.0), Color32::WHITE)
        .size()
        .x;
    // code_chip pads its monospace label by 10 px.
    let chip = runtime.map_or(0.0, |r| {
        p.layout_no_wrap(r.to_owned(), egui::FontId::monospace(9.0), Color32::WHITE)
            .size()
            .x
            + 10.0
    });
    word + chip + tail_w + 10.0 // the two 5 px gaps
}

impl SettingsApp {
    // ---------- Appearance ----------
    fn pane_appearance(&mut self, ui: &mut Ui) {
        h2(ui, "Appearance");

        hint(
            ui,
            "Fonts, radius and accent stay identical — only the palette changes.",
        );
        ui.add_space(10.0);

        // ---- Theme: three mini-window tiles (the concept's visual picker) ----
        let current = self.s.theme.clone();
        let mut picked: Option<String> = None;
        let mut open_theme = self.open_theme;
        card(ui, "Theme", None, &mut open_theme, |ui| {
            let gap = 8.0;
            let tile_w = ((ui.available_width() - gap * 2.0) / 3.0).max(72.0);
            let tile_h = 92.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (key, name, kind) in [
                    ("system", "System", TileKind::System),
                    ("dark", "Dark", TileKind::Dark),
                    ("light", "Light", TileKind::Light),
                ] {
                    if theme_tile(ui, key, tile_w, tile_h, kind, name, current == key).clicked() {
                        picked = Some(key.to_string());
                    }
                }
            });
        });
        self.open_theme = open_theme;
        if let Some(k) = picked {
            self.s.theme = k;
        }

        // ---- Download list ----
        let mut open_list = true;
        let mut hide_icons = self.s.hide_col_icons;
        card(ui, "Download list", None, &mut open_list, |ui| {
            switch_row(
                ui,
                "hide_col_icons",
                &mut hide_icons,
                "Hide column icons",
                "The list's column titles show text only (Name, Size, Status, Speed, Time left, Date).",
            );
        });
        // Applies at once (no Save needed): the main window reloads it live.
        if hide_icons != self.s.hide_col_icons {
            crate::model::ui_pref_set_bool("hide_col_icons", hide_icons);
        }
        self.s.hide_col_icons = hide_icons;

        // ---- Interface size: the main window's text, icons and spacing ----
        let mut open_size = true;
        let scale = &mut self.s.ui_scale;
        card(ui, "Interface size", None, &mut open_size, |ui| {
            egui::ComboBox::from_id_salt("ui_scale")
                .selected_text(format!("{scale}%"))
                .show_ui(ui, |ui| {
                    for pct in [90u32, 100, 110, 125, 150, 175] {
                        ui.selectable_value(scale, pct, format!("{pct}%"));
                    }
                });
            ui.add_space(4.0);
            hint(ui, "Makes the main window's text larger or smaller. Applies when you press Save.");
        });

        // ---- Font: the interface typeface ----
        let mut open_font = true;
        let font = &mut self.s.ui_font;
        card(ui, "Font", None, &mut open_font, |ui| {
            let shown = if font.trim().is_empty() {
                "Default (Segoe UI)".to_string()
            } else {
                std::path::Path::new(font.trim())
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| font.clone())
            };
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("ui_font")
                    .selected_text(shown)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(font, String::new(), "Default (Segoe UI)");
                        for (name, _, _) in crate::fonts::UI_FONTS.iter().skip(1) {
                            ui.selectable_value(font, name.to_string(), *name);
                        }
                    });
                ui.add_space(8.0);
                if button(ui, "Other font file…", false).clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Font", &["ttf", "otf"]).pick_file() {
                        *font = p.to_string_lossy().to_string();
                    }
                }
            });
            ui.add_space(4.0);
            hint(ui, "Takes effect the next time Download Craft starts. Scripts the font lacks (Bengali, Arabic…) still use their own fonts.");
        });

        // ---- Language: one lang/<code>.json per language ----
        let mut open_lang = true;
        let langs = crate::i18n::available();
        let lang = &mut self.s.language;
        card(ui, "Language", None, &mut open_lang, |ui| {
            let shown = langs.iter().find(|(c, _)| c == lang).map(|(_, n)| n.clone()).unwrap_or_else(|| lang.clone());
            egui::ComboBox::from_id_salt("language")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (code, name) in &langs {
                        ui.selectable_value(lang, code.clone(), name);
                    }
                });
            ui.add_space(4.0);
            hint(ui, "Takes effect the next time Download Craft starts. To add a language, put <code>.json in the lang folder next to the program; missing texts are listed in lang/<code>.missing.json in the settings folder.");
        });

        // ---- Window behaviour ----
        #[cfg(windows)]
        {
            let mut tray = self.s.close_to_tray;
            let mut startup = self.s.start_with_windows;
            let mut os_caption = self.s.system_title_bar;
            let mut open_window = self.open_window;
            card(ui, "Window", None, &mut open_window, |ui| {
                switch_row(
                    ui,
                    "close_to_tray",
                    &mut tray,
                    "Minimize to system tray instead of quitting",
                    "Closing the window keeps downloads running in the background.",
                );
                ui.add_space(6.0);
                switch_row(
                    ui,
                    "start_with_windows",
                    &mut startup,
                    "Start with Windows",
                    "Opens in the tray when you sign in, from wherever this copy is now (a moved folder is followed).",
                );
                ui.add_space(6.0);
                switch_row(
                    ui,
                    "system_title_bar",
                    &mut os_caption,
                    "Use the Windows title bar",
                    "For systems where buttons react above where you point (custom or stripped Windows builds). Takes effect the next time Download Craft starts.",
                );
            });
            self.open_window = open_window;
            self.s.close_to_tray = tray;
            self.s.start_with_windows = startup;
            self.s.system_title_bar = os_caption;
        }
    }

    // ---------- Download Engine ----------
    fn pane_engine(&mut self, ui: &mut Ui) {
        h2(ui, "Download Engine");
        hint(ui, "How downloads are saved, fetched, and finished.");
        ui.add_space(10.0);
        let s = &mut self.s;

        // ---- Storage ----
        card(ui, "Storage", None, &mut self.open_storage, |ui| {
            let mut c = field_row(ui, "save_dir", "Save folder", None, None);
            let iw = (c.available_width() - 34.0).max(60.0);
            input_box(
                &mut c,
                "engine_save_dir",
                &mut s.save_dir,
                iw,
                true,
                false,
                if cfg!(windows){"C:\\Downloads"}else{"~/Downloads"},
            );
            c.add_space(6.0);
            if icon_sq(&mut c, "\u{e2c7}", 28.0, 6.0)
                .on_hover_text("Browse…")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    s.save_dir = p.to_string_lossy().to_string();
                }
            }
            let mut c = field_row(ui, "temp_dir", "Temporary folder", None, Some("Unfinished downloads wait here (a fast disk), then move to the save folder. Empty = next to the file."));
            let iw = (c.available_width() - 34.0).max(60.0);
            input_box(&mut c, "engine_temp_dir", &mut s.temp_dir, iw, true, false, "next to the file");
            c.add_space(6.0);
            if icon_sq(&mut c, "\u{e2c7}", 28.0, 6.0).on_hover_text("Browse…").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    s.temp_dir = p.to_string_lossy().to_string();
                }
            }
            ui.add_space(6.0);
            let mut c = field_row(ui, "quota", "Download quota", None, Some("For capped connections: downloads pause once this much arrived within the hours on the right, and resume by themselves. 0 = off."));
            input_box(&mut c, "quota_mb_in", &mut s.quota_mb, 80.0, true, false, "0");
            c.add_space(6.0);
            c.label(egui::RichText::new("MB per").font(reg(11.0)).color(tok().text2));
            c.add_space(6.0);
            input_box(&mut c, "quota_h_in", &mut s.quota_hours, 50.0, true, false, "1");
            c.add_space(6.0);
            c.label(egui::RichText::new("hours").font(reg(11.0)).color(tok().text2));
            ui.add_space(1.0);
            checkbox(
                ui,
                &mut s.category_folders,
                "Sort into category subfolders",
                14.0,
                11.5,
                true,
            );
            ui.add_space(1.0);
            checkbox(
                ui,
                &mut s.show_complete,
                "Show the \u{201c}Download complete\u{201d} dialog",
                14.0,
                11.5,
                true,
            );
        });

        // ---- Performance ----
        card(ui, "Performance", None, &mut self.open_perf, |ui| {
            let mut c = field_row(ui, "connections", "Default connections", None, None);
            let mut conn = s.connections_default as f64;
            if slider_row(&mut c, "connections", &mut conn, 1.0, 32.0) {
                s.connections_default = conn.round() as usize;
            }
            let mut c = field_row(
                ui,
                "speed_limit",
                "Speed limit KB/s",
                None,
                Some("0 = unlimited"),
            );
            let mut lim = s.speed_limit_kbps as f64;
            if slider_row(&mut c, "speed_limit", &mut lim, 0.0, 10000.0) {
                s.speed_limit_kbps = lim.round() as u64;
            }
            let mut c = field_row(
                ui,
                "per_host",
                "Connections per server",
                None,
                Some("0 = no extra cap"),
            );
            let mut per_host = s.per_host_conns as f64;
            if slider_row(&mut c, "per_host", &mut per_host, 0.0, 32.0) {
                s.per_host_conns = per_host.round() as usize;
            }
        });

        // ---- Network ----
        card(ui, "Network", None, &mut self.open_network, |ui| {
            let mut c = field_row(ui, "ua", "User-Agent", None, None);
            let w = c.available_width();
            input_box(
                &mut c,
                "engine_ua",
                &mut s.user_agent,
                w,
                true,
                false,
                "default (Download Craft)",
            );
            let mut c = field_row(ui, "timeout", "Timeout", Some("sec"), None);
            let mut timeout = s.timeout_secs as f64;
            if slider_row(&mut c, "timeout", &mut timeout, 5.0, 600.0) {
                s.timeout_secs = timeout.round() as u64;
            }
            let mut c = field_row(ui, "retries", "Retries", None, None);
            let mut retries = s.max_retries as f64;
            if slider_row(&mut c, "retries", &mut retries, 1.0, 20.0) {
                s.max_retries = retries.round() as u32;
            }
        });

        // ---- On completion ----
        card(ui, "On completion", None, &mut self.open_complete, |ui| {
            let mut c = field_row(
                ui,
                "av",
                "After-download command",
                None,
                Some("Executable and arguments; {file} is passed as the file path. Shell operators are not supported."),
            );
            let w = c.available_width();
            input_box(
                &mut c,
                "engine_av",
                &mut s.antivirus_cmd,
                w,
                true,
                false,
                if cfg!(windows) { r#""C:\Program Files\Defender\MpCmdRun.exe" -Scan -ScanType 3 -File {file}"# } else { r#"clamscan "{file}""# },
            );
            let mut c = field_row(ui, "finish", "When a download finishes", None, None);
            segmented(
                &mut c,
                "finish",
                if cfg!(target_os="macos") { &["nothing", "shutdown", "sleep"] } else { &["nothing", "shutdown", "sleep", "hibernate"] },
                &mut s.finish_action,
            );
            ui.add_space(8.0);
            switch_row(
                ui,
                "extract_auto",
                &mut s.extract_auto,
                "Extract archives when they finish",
                "ZIP, RAR (also multi-part) and 7z. Password-protected archives are left as they are. Right-click a finished archive → Extract works either way.",
            );
            if s.extract_auto {
                ui.add_space(6.0);
                let mut c = field_row(ui, "extract_types", "Archive types", None, None);
                c.horizontal(|ui| {
                    for (kind, key) in [("ZIP", "zip"), ("RAR", "rar"), ("7z", "7z")] {
                        let had = s.extract_types.split_whitespace().any(|k| k.eq_ignore_ascii_case(key));
                        let mut on = had;
                        checkbox(ui, &mut on, kind, 14.0, 11.5, true);
                        if on != had {
                            let kinds: Vec<&str> = ["zip", "rar", "7z"]
                                .into_iter()
                                .filter(|k| {
                                    if *k == key {
                                        on
                                    } else {
                                        s.extract_types.split_whitespace().any(|x| x.eq_ignore_ascii_case(k))
                                    }
                                })
                                .collect();
                            s.extract_types = kinds.join(" ");
                        }
                        ui.add_space(14.0);
                    }
                });
                let mut c = field_row(ui, "extract_where", "Put the files", None, None);
                let mut place = if s.extract_into_folder { "in a new folder".to_string() } else { "next to the archive".to_string() };
                segmented(&mut c, "extract_where", &["in a new folder", "next to the archive"], &mut place);
                s.extract_into_folder = place == "in a new folder";
                ui.add_space(6.0);
                switch_row(
                    ui,
                    "extract_delete",
                    &mut s.extract_delete,
                    "Delete the archive afterwards",
                    "Only when every file came out; an archive with files that were already there is kept.",
                );
            }
        });

        // ---- Speed windows ----
        card(ui, "Speed windows", None, &mut self.open_windows, |ui| {
            checkbox(
                ui,
                &mut s.speed_windows_on,
                "Enable speed windows",
                14.0,
                11.5,
                true,
            );
            ui.add_space(4.0);
            hint(
                ui,
                "While a window is active its cap overrides the global limit (0 = full speed).",
            );
            ui.add_space(8.0);
            let mut del: Option<usize> = None;
            for (i, w) in s.speed_windows.iter_mut().enumerate().take(6) {
                if window_row(ui, i, w) {
                    del = Some(i);
                }
            }
            if let Some(i) = del {
                s.speed_windows.remove(i);
            }
            if btn_dashed(ui, "\u{e145}", "Add window").clicked() && s.speed_windows.len() < 6 {
                s.speed_windows.push(SpeedWindow {
                    start: "02:00".to_string(),
                    end: "06:00".to_string(),
                    limit_kbps: 0,
                });
            }
        });
    }

    // ---------- Plugins (helpers, one card each) ----------
    fn pane_plugins(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Plugins");
        hint(
            ui,
            "Each plugin is its own card. Name, version and date sit on one row.",
        );
        ui.add_space(12.0);

        self.plugin_card_ytdlp(ui);
        ui.add_space(10.0);
        self.plugin_card_ffmpeg(ui);
        ui.add_space(10.0);
        let s = &mut self.s;
        card(ui, "YouTube", None, &mut self.open_youtube, |ui| {
            sec_head(ui, "Which engine goes first");
            let mut pick = match s.youtube_engine.as_str() {
                "native" => "App first",
                "ytdlp" => "yt-dlp first",
                _ => "Auto",
            }
            .to_string();
            ui.add_space(8.0);
            checkbox(ui, &mut s.subtitles, "Save subtitles next to the video", 14.0, 11.5, true);
            if s.subtitles {
                let mut c = field_row(ui, "sub_langs", "Languages", None, Some("yt-dlp language codes, comma separated: en.*, bn, all"));
                let w = c.available_width();
                input_box(&mut c, "sub_langs_in", &mut s.subtitle_langs, w, true, false, "en.*");
            }
            ui.add_space(8.0);
            if segmented(ui, "yt_engine", &["Auto", "App first", "yt-dlp first"], &mut pick) {
                s.youtube_engine = match pick.as_str() {
                    "App first" => "native",
                    "yt-dlp first" => "ytdlp",
                    _ => "auto",
                }
                .to_string();
            }
            ui.add_space(4.0);
            wrapped(
                ui,
                "Auto and App first read YouTube with the app itself, every quality up to 4K, on its own connections; yt-dlp takes over for what it cannot read (a sign-in, an age check, a live stream) and makes Audio only (MP3). yt-dlp first always asks yt-dlp. Whichever goes first, the other takes over when it fails, and a download sent from the browser falls back to the extension's own browser download last.",
                reg(9.5),
                tok().muted,
            );
        });
        ui.add_space(14.0);

        // `.empty-note`: what the catalogue grows into.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add(
                egui::Label::new(
                    egui::RichText::new("\u{e88e}") // info
                        .font(crate::fonts::icon(12.0))
                        .color(t.muted),
                )
                .selectable(false),
            );
            wrapped(
                ui,
                "More plugins will appear here as they are added. Each plugin gets its own card with info, actions, and options.",
                reg(9.0),
                t.muted,
            );
        });
    }

    /// yt-dlp: the card that starts open (`plugins_open[0]`).
    fn plugin_card_ytdlp(&mut self, ui: &mut Ui) {
        let t = tok();
        let info = self.ytdlp.clone();
        let msg = self.ytdlp_msg.clone();
        let busy = self.ytdlp_busy;
        let open = self.plugins_open[0];
        let source = self.s.ytdlp_source.clone();
        let custom = self.s.ytdlp_custom_path.clone();
        let managed_dir = crate::engine::ytdlp_managed_path();

        let pill = match info.as_ref().and_then(|i| i.version.as_deref()) {
            Some(v) => format!("v{v}"),
            None if busy => "checking…".to_string(),
            None => "unknown".to_string(),
        };
        let size = info
            .as_ref()
            .and_then(|i| i.size)
            .map(crate::model::format_bytes)
            .unwrap_or_else(|| "—".to_string());
        let meta = match (&info, busy) {
            (_, true) => "checking…".to_string(),
            (None, _) => "not checked yet".to_string(),
            (Some(i), _) if i.managed => format!("own copy · {size}"),
            (Some(_), _) => format!("from PATH · {size}"),
        };
        let custom_missing = !custom.trim().is_empty()
            && !crate::platform::executable_file(std::path::Path::new(custom.trim()));

        let w = ui.available_width();
        let mut want_open = open;
        let mut set_source: Option<&'static str> = None;
        let mut set_custom: Option<String> = None;
        let mut redownload = false;

        let edge = if open {
            blend(t.border, t.accent, 0.5)
        } else {
            t.border
        };
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let inner = ui.max_rect();
                let head = plugin_head(
                    ui,
                    inner.width(),
                    "ytdlp",
                    "yt-dlp",
                    &pill,
                    &meta,
                    open,
                );
                if head.clicked() {
                    want_open = !open;
                }
                if !open {
                    return;
                }
                // soft rule between the identity row and the body
                let y = ui.cursor().top();
                ui.painter().hline(
                    inner.left()..=inner.right(),
                    y,
                    Stroke::new(1.0, blend(t.card, t.border, 0.6)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 13,
                        right: 13,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(inner.width() - 26.0);

                        sec_head(ui, "Supported sites");
                        platform_grid(ui);
                        ui.add_space(13.0);

                        sec_head(ui, "Capabilities");
                        capabilities(ui, &YTDLP_CAPS);
                        ui.add_space(13.0);

                        sec_head(ui, "yt-dlp location");
                        let managed_sub = managed_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "your application data folder".to_string());
                        let (hit, _) = loc_option(
                            ui,
                            source == "managed",
                            "App's own copy",
                            Some("Recommended"),
                            &managed_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("managed");
                        }
                        ui.add_space(6.0);

                        let path_sub = match info.as_ref().and_then(|i| i.on_path.as_deref()) {
                            Some(p) => p.display().to_string(),
                            None => "no yt-dlp found on PATH".to_string(),
                        };
                        let (hit, _) = loc_option(
                            ui,
                            source == "path",
                            "System PATH",
                            None,
                            &path_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("path");
                        }
                        ui.add_space(6.0);

                        let custom_sub = if custom.trim().is_empty() {
                            if cfg!(windows) { "pick yt-dlp.exe on this computer" } else { "pick an executable yt-dlp file" }.to_string()
                        } else if custom_missing {
                            format!("{custom}  — not there any more")
                        } else {
                            custom.clone()
                        };
                        let (hit, browse) = loc_option(
                            ui,
                            source == "custom",
                            "Custom path",
                            None,
                            &custom_sub,
                            true,
                            Some("Browse…"),
                        );
                        if browse {
                            // Picking the file *is* the choice: select it too.
                            if let Some(p) = pick_executable() {
                                set_custom = Some(p.to_string_lossy().to_string());
                                set_source = Some("custom");
                            }
                        } else if hit {
                            set_source = Some("custom");
                        }
                        ui.add_space(9.0);

                        // What a YouTube download would really run, right now.
                        let (status, warn) = match &info {
                            None => ("Checking what you have…".to_string(), false),
                            Some(i) => {
                                let from = if i.managed {
                                    "App's own copy"
                                } else if i.path.is_some() {
                                    if !custom.trim().is_empty() && i.path.as_deref()==Some(std::path::Path::new(custom.trim())) { "Custom yt-dlp" } else { "System yt-dlp" }
                                } else {
                                    "Nothing installed yet"
                                };
                                let what = match (&i.version, i.path.is_some()) {
                                    (Some(v), true) => format!(" — yt-dlp {v} detected"),
                                    (None, true) => " — it did not answer --version".to_string(),
                                    _ => " — the first YouTube download fetches it".to_string(),
                                };
                                (
                                    format!("Active: {from}{what}"),
                                    i.path.is_none() || i.stale || custom_missing,
                                )
                            }
                        };
                        loc_status(ui, &status, warn);
                        ui.add_space(13.0);

                        // Buttons left, runtime note pushed right — and dropped
                        // onto its own line when the two stop fitting, which is
                        // what the concept's `flex-wrap` does. The note is
                        // measured first because otherwise it ran straight over
                        // the last button in a narrow window.
                        let rt = info.as_ref().and_then(|i| i.js_runtime.as_deref());
                        let mut note_beside = false;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_enabled_ui(!busy, |ui| {
                                // e5d5 renders as a refresh arrow in this build
                                // of Material Icons — which is what re-fetching
                                // the helper means anyway.
                                if btn_icon(ui, "\u{e5d5}", if info.as_ref().and_then(|i|i.path.as_ref()).is_some(){"Update yt-dlp"}else{"Download yt-dlp"}, true).clicked() {
                                    redownload = true;
                                }
                            });
                            if btn_icon(ui, "\u{e2c7}", "Show in folder", false).clicked() {
                                if let Some(dir) = crate::engine::ytdlp_bin_dir() {
                                    let _ = std::fs::create_dir_all(&dir);
                                    crate::app::open_path(&dir, false);
                                }
                            }
                            if busy {
                                ui.add(egui::Spinner::new().size(14.0));
                            }
                            if ui.available_width() >= runtime_note_width(ui, rt) {
                                runtime_note(ui, rt);
                                note_beside = true;
                            }
                        });
                        if !note_beside {
                            ui.add_space(6.0);
                            runtime_note(ui, rt);
                        }

                        // One click installs a portable Deno into the app's own
                        // tools folder (like yt-dlp itself): no web page, no
                        // installer, no PATH changes.
                        let installing = self.deno_rx.is_some();
                        let own = crate::engine::deno_local_path().map(|p| p.is_file()).unwrap_or(false);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let label = if own {
                                "Update JavaScript runtime"
                            } else if rt.is_some() {
                                "Install app's own runtime"
                            } else {
                                "Install JavaScript runtime"
                            };
                            let b = ui.add_enabled_ui(!installing, |ui| btn_icon(ui, "\u{e2c4}", label, rt.is_none() && !own)).inner;
                            if b.on_hover_text("Downloads a portable Deno (about 45 MB) next to yt-dlp — nothing else to set up").clicked() {
                                let (tx, rx) = std::sync::mpsc::channel();
                                std::thread::spawn(move || {
                                    let _ = tx.send(crate::engine::deno_install_blocking().map_err(|e| format!("{e:#}")));
                                });
                                self.deno_rx = Some(rx);
                                self.ytdlp_msg = "Installing the JavaScript runtime (Deno)…".into();
                            }
                            if installing {
                                ui.add(egui::Spinner::new().size(14.0));
                            } else if own {
                                txt(ui, "✓ Installed (app's own copy)", reg(10.0), pl().success);
                            }
                        });
                        if !msg.is_empty() {
                            ui.add_space(7.0);
                            wrapped(ui, msg.as_str(), reg(10.0), t.accent);
                        }
                        if info.as_ref().map(|i| i.stale).unwrap_or(false) {
                            ui.add_space(4.0);
                            wrapped(
                                ui,
                                "This copy is past its 30-day refresh window — press Re-download.",
                                reg(9.0),
                                pl().warning,
                            );
                        }
                    });
            });

        self.plugins_open[0] = want_open;
        if let Some(src) = set_source {
            if set_custom.is_none() {
                self.s.ytdlp_source = src.to_string();
                // Keep the old switch truthful for anything still reading it,
                // then re-ask so the status line matches the new choice. The
                // probe reads ui.json, so the choice is committed first (as
                // the ffmpeg card does); otherwise it described the old one
                // until Save.
                self.s.ytdlp_prefer_path = src == "path";
                crate::model::ui_pref_set_str("ytdlp_source", src);
                self.run_ytdlp_job(|| ytdlp_probe(false));
            }
        }
        if let Some(p) = set_custom {
            self.s.ytdlp_custom_path = p;
            self.s.ytdlp_source = "custom".to_string();
            self.s.ytdlp_prefer_path = false;
            crate::model::ui_pref_set_str("ytdlp_source", "custom");
            crate::model::ui_pref_set_str("ytdlp_custom_path", &self.s.ytdlp_custom_path);
            self.run_ytdlp_job(|| ytdlp_probe(false));
        }
        if redownload {
            self.run_ytdlp_job(|| ytdlp_probe(true));
        }
    }

    /// Start a yt-dlp card job unless one still runs: replacing its receiver
    /// dropped that job's result ("Updated yt-dlp X → Y") and cleared the
    /// busy state while it was still writing the executable.
    fn run_ytdlp_job(
        &mut self,
        job: impl FnOnce() -> std::sync::mpsc::Receiver<(String, crate::engine::YtdlpInfo, crate::engine::FfmpegInfo)>,
    ) {
        if self.ytdlp_rx.is_some() {
            self.ytdlp_msg = "Still finishing the previous step — try again in a moment.".to_string();
            return;
        }
        self.ytdlp_rx = Some(job());
        self.ytdlp_busy = true;
        self.ytdlp_msg.clear();
    }

    /// Same for the ffmpeg card; the tools install (which runs on the yt-dlp
    /// channel) also writes ffmpeg, so it blocks these too.
    fn run_ffmpeg_job(
        &mut self,
        job: impl FnOnce() -> std::sync::mpsc::Receiver<(String, crate::engine::FfmpegInfo)>,
    ) {
        if self.ffmpeg_rx.is_some() || self.ffmpeg_installing {
            self.ffmpeg_msg = "Still finishing the previous step — try again in a moment.".to_string();
            return;
        }
        self.ffmpeg_rx = Some(job());
        self.ffmpeg_busy = true;
        self.ffmpeg_msg.clear();
    }

    /// ffmpeg: the concept's second card, closed by default — with real
    /// detection behind it, since yt-dlp needs it for anything above 360p.
    fn plugin_card_ffmpeg(&mut self, ui: &mut Ui) {
        let t = tok();
        let ff = self.ffmpeg.clone();
        let open = self.plugins_open[1];
        let source = self.s.ffmpeg_source.clone();
        let custom = self.s.ffmpeg_custom_path.clone();
        let busy = self.ffmpeg_busy;
        let msg = self.ffmpeg_msg.clone();
        let managed_dir = crate::engine::ffmpeg_bin_dir();
        let found = ff.as_ref().and_then(|f| f.path.clone());
        let pill = match ff.as_ref().and_then(|f| f.version.as_deref()) {
            Some(v) => format!("v{v}"),
            None => "missing".to_string(),
        };
        let meta = ff
            .as_ref()
            .map(|f| f.meta())
            .unwrap_or_else(|| "looking…".to_string());

        let w = ui.available_width();
        let mut want_open = open;
        let mut set_source: Option<&'static str> = None;
        let mut set_custom: Option<String> = None;
        let mut redownload = false;
        let mut install = false;
        let mut recheck = false;
        let mut custom_pick: Option<std::path::PathBuf> = None;
        let mut clear_custom = false;

        let edge = if open {
            blend(t.border, t.accent, 0.5)
        } else {
            t.border
        };
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let inner = ui.max_rect();
                let head = plugin_head(
                    ui,
                    inner.width(),
                    "ffmpeg",
                    "ffmpeg",
                    &pill,
                    &meta,
                    open,
                );
                if head.clicked() {
                    want_open = !open;
                }
                if !open {
                    return;
                }
                let y = ui.cursor().top();
                ui.painter().hline(
                    inner.left()..=inner.right(),
                    y,
                    Stroke::new(1.0, blend(t.card, t.border, 0.6)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 13,
                        right: 13,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.set_width(inner.width() - 26.0);

                        sec_head(ui, "Capabilities");
                        capabilities(ui, &FFMPEG_CAPS);
                        ui.add_space(13.0);

                        sec_head(ui, "ffmpeg location");
                        let managed_sub = managed_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "your application data folder".to_string());
                        let (hit, _) = loc_option(
                            ui,
                            source == "managed",
                            "App's own copy",
                            Some("On demand"),
                            &managed_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("managed");
                        }
                        ui.add_space(6.0);

                        let path_sub = match ff.as_ref().and_then(|f| f.on_path.as_deref()) {
                            Some(p) => p.display().to_string(),
                            None => "no ffmpeg found on PATH".to_string(),
                        };
                        let (hit, _) = loc_option(
                            ui,
                            source == "path",
                            "System PATH",
                            None,
                            &path_sub,
                            true,
                            None,
                        );
                        if hit {
                            set_source = Some("path");
                        }
                        ui.add_space(6.0);

                        let custom_missing =
                            !custom.trim().is_empty() && !std::path::Path::new(&custom).is_file();
                        let custom_sub = if custom.trim().is_empty() {
                            if cfg!(windows) { "pick an ffmpeg.exe on this computer" } else { "pick an executable ffmpeg file" }.to_string()
                        } else if custom_missing {
                            format!("{custom}  — not there any more")
                        } else {
                            custom.clone()
                        };
                        let (hit, browse) = loc_option(
                            ui,
                            source == "custom",
                            "Custom path",
                            None,
                            &custom_sub,
                            true,
                            Some("Browse…"),
                        );
                        if browse {
                            if let Some(p) = pick_ffmpeg() {
                                set_custom = Some(p.to_string_lossy().to_string());
                                set_source = Some("custom");
                            }
                        } else if hit {
                            set_source = Some("custom");
                        }
                        ui.add_space(9.0);

                        let (status, warn) = match &ff {
                            None => ("Checking what you have…".to_string(), false),
                            Some(f) => {
                                let from = if f.managed {
                                    "App's own copy"
                                } else if f.path.is_some() {
                                    "ffmpeg on PATH"
                                } else {
                                    "Nothing installed yet"
                                };
                                let what = match (&f.version, f.path.is_some()) {
                                    (Some(v), true) => format!(" — ffmpeg {v} detected"),
                                    (None, true) => " — it did not answer -version".to_string(),
                                    _ => " — the app fetches one on the next merge".to_string(),
                                };
                                (
                                    format!("Active: {from}{what}"),
                                    f.path.is_none() || custom_missing,
                                )
                            }
                        };
                        loc_status(ui, &status, warn);
                        if let Some(p) = &found {
                            ui.add_space(5.0);
                            wrapped(
                                ui,
                                p.display().to_string(),
                                egui::FontId::monospace(9.0),
                                t.muted,
                            );
                        }
                        ui.add_space(13.0);

                        ui.add_enabled_ui(!self.ytdlp_busy,|ui| {
                            ui.horizontal_wrapped(|ui| {
                                // Same gaps as the action row below (these
                                // buttons were drawn touching each other).
                                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                                if crate::helpers::install_supported() && button(ui,"Download tools",true).clicked(){install=true;}
                                if button(ui,"Choose FFmpeg…",false).clicked(){custom_pick=pick_tool("FFmpeg");}
                                if button(ui,"Check again",false).clicked(){recheck=true;}
                                if !self.s.ffmpeg_custom_path.is_empty() && button(ui,"Use automatic detection",false).clicked(){clear_custom=true;}
                            });
                        });
                        if self.ytdlp_busy {ui.add(egui::Spinner::new().size(14.0));}
                        if !self.ytdlp_msg.is_empty(){wrapped(ui,&self.ytdlp_msg,reg(10.0),t.accent);}
                        if let Some(info)=&ff { if info.path.is_some() && info.probe_path.is_none(){wrapped(ui,if cfg!(windows){"ffprobe is missing. Install it next to FFmpeg."}else{"ffprobe is missing. Download tools installs the matching pair."},reg(10.0),pl().warning);} }
                        if cfg!(target_os="macos"){hint(ui,"System alternative: brew install ffmpeg. Homebrew locations are detected automatically.");}
                        #[cfg(target_os="linux")]
                        hint(ui,"System alternatives: Ubuntu/Debian: sudo apt install ffmpeg; Fedora: sudo dnf install ffmpeg-free. Download tools needs no administrator access.");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_enabled_ui(!busy, |ui| {
                                if btn_icon(ui, "\u{e5d5}", "Re-download", true).clicked() {
                                    redownload = true;
                                }
                            });
                            if btn_icon(ui, "\u{e2c7}", "Show in folder", false).clicked() {
                                if let Some(dir) = crate::engine::ffmpeg_bin_dir() {
                                    let _ = std::fs::create_dir_all(&dir);
                                    crate::app::open_path(&dir, false);
                                }
                            }
                            if busy {
                                ui.add(egui::Spinner::new().size(14.0));
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    txt(
                                        ui,
                                        "merges video + audio; no re-encoding",
                                        reg(9.0),
                                        t.muted,
                                    );
                                },
                            );
                        });
                        if !msg.is_empty() {
                            ui.add_space(7.0);
                            wrapped(ui, msg.as_str(), reg(10.0), t.accent);
                        }
                    });
            });

        self.plugins_open[1] = want_open;
        if let Some(src) = set_source {
            if set_custom.is_none() {
                self.s.ffmpeg_source = src.to_string();
                crate::model::ui_pref_set_str("ffmpeg_source", src);
                self.run_ffmpeg_job(|| ffmpeg_probe(false));
            }
        }
        if let Some(p) = set_custom {
            self.s.ffmpeg_source = "custom".to_string();
            self.s.ffmpeg_custom_path = p;
            crate::model::ui_pref_set_str("ffmpeg_source", "custom");
            crate::model::ui_pref_set_str("ffmpeg_custom_path", &self.s.ffmpeg_custom_path);
            self.run_ffmpeg_job(|| ffmpeg_probe(false));
        }
        if install && self.ytdlp_rx.is_none() && self.ffmpeg_rx.is_none() {
            self.s.ffmpeg_source = "managed".to_string();
            crate::model::ui_pref_set_str("ffmpeg_source", "managed");
            self.ffmpeg_installing = true;
            self.run_ytdlp_job(ffmpeg_install_probe);
        }
        if recheck {
            self.run_ffmpeg_job(|| ffmpeg_probe(false));
        }
        if let Some(path) = custom_pick {
            let path = path.to_string_lossy().into_owned();
            self.s.ffmpeg_source = "custom".to_string();
            self.s.ffmpeg_custom_path = path.clone();
            crate::model::ui_pref_set_str("ffmpeg_source", "custom");
            crate::model::ui_pref_set_str("ffmpeg_custom_path", &path);
            self.run_ffmpeg_job(|| ffmpeg_probe(false));
        }
        if clear_custom {
            self.s.ffmpeg_source = "path".to_string();
            self.s.ffmpeg_custom_path.clear();
            crate::model::ui_pref_set_str("ffmpeg_source", "path");
            crate::model::ui_pref_set_str("ffmpeg_custom_path", "");
            self.run_ffmpeg_job(|| ffmpeg_probe(false));
        }
        if redownload {
            // Re-download means "use the copy this app keeps up to date", so
            // the choice is committed now (the probe and the engine both read
            // ui.json, not this form's unsaved state).
            self.s.ffmpeg_source = "managed".to_string();
            crate::model::ui_pref_set_str("ffmpeg_source", "managed");
            self.run_ffmpeg_job(|| ffmpeg_probe(true));
        }
    }

    // ---------- Browser Integration (one card per extension, like Plugins) ----------
    fn pane_browser(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Browser Integration");
        hint(
            ui,
            "The extension catches videos and downloads; the app's connection repairs itself on every launch.",
        );
        ui.add_space(12.0);
        if self.host_status.is_none() {
            // Loading the extension should be all anyone does: a browser
            // that is not linked yet is linked here, without a button.
            let probe = || -> Vec<(&'static str, bool)> {
                ["chrome", "edge", "brave", "opera", "firefox"]
                    .iter()
                    .map(|b| (*b, crate::app::native_host_status(b)))
                    .collect()
            };
            let mut status = probe();
            if status.iter().any(|(_, ok)| !ok) {
                for (b, _) in status.iter().filter(|(_, ok)| !ok) {
                    let _ = crate::app::install_native_host(b);
                }
                status = probe();
            }
            self.host_status = Some(status);
        }
        self.browser_card(ui, 0);
        ui.add_space(10.0);
        self.browser_card(ui, 1);
        ui.add_space(10.0);
        // The folder the extension is loaded from: the app keeps it at its
        // own version, wherever the app itself is unpacked or updated.
        if let Some(dir) = crate::extsync::stable_dir() {
            let mut open = true;
            card(ui, "Extension folder", None, &mut open, |ui| {
                wrapped(
                    ui,
                    "Load the extension from this folder (chrome://extensions → Developer mode → Load unpacked). The app copies its own version here on every start, and the extension reloads itself onto it, so updating or moving the app never breaks the browser side.",
                    reg(10.5),
                    t.muted,
                );
                ui.add_space(8.0);
                let shown = dir.display().to_string();
                let ver = crate::extsync::manifest_version(&dir).map(|v| format!("  ·  v{v}")).unwrap_or_default();
                ui.label(egui::RichText::new(format!("{shown}{ver}")).font(egui::FontId::monospace(10.5)).color(t.text));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if button(ui, "Open folder", true).clicked() {
                        crate::app::open_path(&dir, false);
                    }
                    if button(ui, "Copy path", false).clicked() {
                        ui.ctx().copy_text(shown.clone());
                    }
                });
            });
            ui.add_space(10.0);
        }
        let s = &mut self.s;
        card(ui, "Capture", None, &mut self.open_capture, |ui| {
            switch_row(
                ui,
                "cap_auto",
                &mut s.capture_auto,
                "Take over browser downloads",
                "The browser's download is paused, handed to the app, and resumed in the browser if the app cannot take it.",
            );
            ui.add_space(6.0);
            let mut c = field_row(ui, "cap_types", "File types", None, Some("Empty = every download. Example: zip rar 7z exe msi iso mp4 mkv mp3 pdf"));
            let w = c.available_width();
            input_box(&mut c, "cap_types_in", &mut s.capture_types, w, true, false, "every type");
            let mut c = field_row(ui, "cap_skip", "Never from", None, Some("Sites whose downloads stay in the browser, comma separated."));
            let w = c.available_width();
            input_box(&mut c, "cap_skip_in", &mut s.capture_skip_sites, w, true, false, "example.com, intranet.local");
            let mut c = field_row(ui, "cap_min", "Smaller than (KB)", None, Some("Files below this stay in the browser."));
            input_box(&mut c, "cap_min_in", &mut s.capture_min_kb, 90.0, true, false, "100");
            ui.add_space(6.0);
            sec_head(ui, "Hold while clicking a link to keep it in the browser");
            let mut key = match s.capture_skip_key.as_str() {
                "shift" => "Shift",
                "off" => "Off",
                _ => "Alt",
            }
            .to_string();
            if segmented(ui, "cap_key", &["Alt", "Shift", "Off"], &mut key) {
                s.capture_skip_key = key.to_ascii_lowercase();
            }
            ui.add_space(8.0);
            switch_row(
                ui,
                "cap_notify",
                &mut s.notify_done,
                "Browser notification when done",
                "For downloads sent from the browser.",
            );
            switch_row(
                ui,
                "cap_fallback",
                &mut s.browser_fallback,
                "Fall back to downloading in the browser",
                "When the app cannot download a video the extension sent, the extension downloads it itself.",
            );
        });
        if !self.browser_msg.is_empty() {
            ui.add_space(10.0);
            wrapped(ui, self.browser_msg.as_str(), reg(10.0), t.muted);
        }
    }

    /// Card 0: Chrome, Edge, Brave and Opera (one Chromium extension).
    /// Card 1: Firefox.
    fn browser_card(&mut self, ui: &mut Ui, idx: usize) {
        let t = tok();
        let firefox = idx == 1;
        let browsers: &[(&str, &str)] = if firefox {
            &[("firefox", "Firefox")]
        } else {
            &[("chrome", "Chrome"), ("edge", "Edge"), ("brave", "Brave"), ("opera", "Opera")]
        };
        let status = self.host_status.clone().unwrap_or_default();
        let connected = browsers
            .iter()
            .filter(|(k, _)| status.iter().any(|(b, ok)| b == k && *ok))
            .count();
        let open = self.browser_open[idx];
        let (icon, name) = if firefox {
            ("Firefox", "Firefox")
        } else {
            ("Chromium", "Chrome · Edge · Brave · Opera")
        };
        let pill = if connected > 0 { "connected" } else { "not set up" };
        let meta = format!("{connected} of {} linked", browsers.len());
        let w = ui.available_width();
        let mut want_open = open;
        let mut repair: Option<&'static str> = None;
        let mut open_folder = false;
        let mut copy_id = false;
        let edge = if open { blend(t.border, t.accent, 0.5) } else { t.border };
        egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, edge))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let inner = ui.max_rect();
                if plugin_head(ui, inner.width(), icon, name, pill, &meta, open).clicked() {
                    want_open = !open;
                }
                if !open {
                    return;
                }
                let y = ui.cursor().top();
                ui.painter().hline(
                    inner.left()..=inner.right(),
                    y,
                    Stroke::new(1.0, blend(t.card, t.border, 0.6)),
                );
                egui::Frame::new()
                    .inner_margin(egui::Margin { left: 13, right: 13, top: 12, bottom: 16 })
                    .show(ui, |ui| {
                        ui.set_width(inner.width() - 26.0);
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                        sec_head(ui, "Extension");
                        wrapped(
                            ui,
                            if firefox {
                                "about:debugging → This Firefox → Load Temporary Add-on, then pick manifest.json in this folder (or install the .xpi)."
                            } else {
                                "chrome://extensions → turn on Developer mode → Load unpacked, then pick this folder. The same folder works in Edge, Brave and Opera."
                            },
                            reg(10.0),
                            t.text2,
                        );
                        if !firefox {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("ID  {}", crate::app::EXTENSION_ID))
                                        .font(egui::FontId::monospace(9.0))
                                        .color(t.muted),
                                )
                                .selectable(true),
                            );
                        }
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                            if btn_icon(ui, "\u{e2c7}", "Open extension folder", true).clicked() {
                                open_folder = true;
                            }
                            if !firefox && btn_icon(ui, "\u{e14d}", "Copy extension ID", false).clicked() {
                                copy_id = true;
                            }
                        });
                        ui.add_space(12.0);
                        sec_head(ui, "Connection to the app");
                        for (key, label) in browsers {
                            let ok = status.iter().any(|(b, s)| b == key && *s);
                            loc_status(
                                ui,
                                &if ok {
                                    format!("{label} — linked to Download Craft")
                                } else {
                                    format!("{label} — not linked yet")
                                },
                                !ok,
                            );
                        }
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                            for (key, label) in browsers {
                                if btn_icon(ui, "\u{e869}", &format!("Repair {label}"), false).clicked() {
                                    repair = Some(*key);
                                }
                            }
                        });
                        ui.add_space(6.0);
                        hint(ui, "Repair re-registers the app for that browser; restart the browser afterwards.");
                    });
            });
        self.browser_open[idx] = want_open;
        if open_folder {
            match crate::platform::extension_directory(firefox) {
                Ok(dir) => crate::app::open_path(&dir, false),
                Err(e) => self.browser_msg = e,
            }
        }
        if copy_id {
            ui.ctx().copy_text(crate::app::EXTENSION_ID.to_string());
            self.browser_msg = "Extension ID copied.".to_string();
        }
        if let Some(b) = repair {
            self.browser_msg = install_one(b);
            self.host_status = None; // re-check on the next frame
        }
    }

    // ---------- Categories & Rules ----------
    // ---------- Proxy / Socks ----------
    fn pane_proxy(&mut self, ui: &mut Ui) {
        h2(ui, "Proxy / Socks");
        hint(ui, "How downloads reach the internet. yt-dlp and video downloads use the same choice.");
        ui.add_space(10.0);
        let s = &mut self.s;
        let mut open = true;
        card(ui, "From the browser", None, &mut open, |ui| {
            switch_row(
                ui,
                "proxy_from_browser",
                &mut s.proxy_from_browser,
                "Use the browser's proxy for downloads caught from it",
                "A download handed over by the extension goes out the same way the browser reached the site (when \"Use system settings\" is chosen below).",
            );
        });
        let mut open = true;
        card(ui, "Connection", None, &mut open, |ui| {
            let modes: [(&str, &str, &str); 4] = [
                ("none", "No proxy", "Connect directly, even when Windows has a proxy set."),
                ("system", "Use system settings", "Whatever Windows' proxy settings say, including its automatic script."),
                ("pac", "Use automatic configuration script", "A PAC script picks the proxy for each site."),
                ("manual", "Manual proxy / socks configuration", "One HTTP or SOCKS5 proxy for every download."),
            ];
            for (key, title, sub) in modes {
                let (picked, _) = loc_option(ui, s.proxy_mode == key, title, None, sub, false, None);
                if picked {
                    s.proxy_mode = key.to_string();
                }
                ui.add_space(4.0);
            }
            if s.proxy_mode == "pac" {
                ui.add_space(4.0);
                let mut c = field_row(ui, "proxy_pac", "Script address", None, Some(if cfg!(windows) { "The PAC file's URL, e.g. http://intranet/proxy.pac" } else { "PAC scripts run through Windows; on this system such downloads go direct." }));
                let w = c.available_width();
                input_box(&mut c, "proxy_pac_in", &mut s.proxy_pac, w, true, false, "http://host/proxy.pac");
            }
            if s.proxy_mode == "manual" {
                ui.add_space(6.0);
                sec_head(ui, "Proxy type");
                let mut kind = if s.proxy_kind == "socks5" { "SOCKS5".to_string() } else { "HTTP".to_string() };
                if segmented(ui, "proxy_kind", &["HTTP", "SOCKS5"], &mut kind) {
                    s.proxy_kind = if kind == "SOCKS5" { "socks5".into() } else { "http".into() };
                }
                ui.add_space(8.0);
                let mut c = field_row(ui, "proxy_host", "Server address", None, None);
                let w = (c.available_width() - 96.0).max(80.0);
                input_box(&mut c, "proxy_host_in", &mut s.proxy_host, w, true, false, "proxy.example.com");
                c.add_space(6.0);
                input_box(&mut c, "proxy_port_in", &mut s.proxy_port, 90.0, true, true, if s.proxy_kind == "socks5" { "1080" } else { "8080" });
                let mut c = field_row(ui, "proxy_user", "User name", None, Some("Only when the proxy asks for a login."));
                let w = c.available_width();
                input_box(&mut c, "proxy_user_in", &mut s.proxy_user, w, true, false, "optional");
                let mut c = field_row(ui, "proxy_pass", "Password", None, None);
                let w = c.available_width();
                c.add(egui::TextEdit::singleline(&mut s.proxy_pass).password(true).desired_width(w).hint_text("optional"));
                ui.add_space(6.0);
                if s.proxy_kind == "http" {
                    sec_head(ui, "Use this proxy for");
                    ui.horizontal(|ui| {
                        checkbox(ui, &mut s.proxy_http, "http", 14.0, 11.5, false);
                        ui.add_space(14.0);
                        checkbox(ui, &mut s.proxy_https, "https", 14.0, 11.5, false);
                    });
                    ui.add_space(4.0);
                }
                if button(ui, "Get system proxy", false).clicked() {
                    let (proxy, pac) = crate::sysproxy::system_settings();
                    if let Some(list) = proxy {
                        // "host:port" or "http=a:80;https=b:443": take the https one, else the first.
                        let pick = list
                            .split(';')
                            .map(str::trim)
                            .find(|x| x.starts_with("https="))
                            .or_else(|| list.split(';').map(str::trim).next())
                            .unwrap_or("")
                            .rsplit('=')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if s.proxy_kind == "socks5" { fill_manual(s, &format!("socks5://{pick}")); } else { fill_manual(s, &pick); }
                    } else if let Some(pac) = pac {
                        s.proxy_mode = "pac".into();
                        s.proxy_pac = pac;
                    }
                }
            }
            ui.add_space(4.0);
            hint(ui, "FTP downloads always use passive (PASV) mode and connect directly.");
        });
    }

    // ---------- Sounds ----------
    fn pane_sounds(&mut self, ui: &mut Ui) {
        h2(ui, "Sounds");
        hint(ui, "A sound for each event. Leave the file empty for the system's own sound; a .wav file plays instead.");
        ui.add_space(10.0);
        let mut open = true;
        let evs = &mut self.s.sounds_ev;
        card(ui, "Events", None, &mut open, |ui| {
            // One row per event, as IDM lays it out: on/off with the event's
            // name, the sound file, then Browse and Play.
            // Every row shares one grid: a fixed label column, then the file
            // box and the two buttons, sized from the card's width (gaps
            // included) so all rows line up and none runs past the edge.
            const LABEL_W: f32 = 200.0;
            const GAP: f32 = 6.0;
            const BTN: f32 = 28.0;
            let row_w = ui.available_width();
            let w = (row_w - LABEL_W - 2.0 * BTN - 3.0 * GAP - 2.0).max(60.0);
            for (key, on, file) in evs.iter_mut() {
                let label = SOUND_EVENTS.iter().find(|e| e.0 == *key).map(|e| e.1).unwrap_or(key);
                ui.horizontal(|c| {
                    c.spacing_mut().item_spacing = Vec2::new(GAP, 0.0);
                    c.allocate_ui_with_layout(
                        Vec2::new(LABEL_W, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |c| {
                            c.set_min_width(LABEL_W);
                            c.set_max_width(LABEL_W);
                            checkbox(c, on, label, 14.0, 11.5, false);
                        },
                    );
                    input_box(c, &format!("snd_in_{key}"), file, w, true, false, "system sound");
                    if icon_sq(c, "\u{e2c7}", 28.0, 6.0).on_hover_text("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Sound", &["wav"]).pick_file() {
                            *file = p.to_string_lossy().to_string();
                        }
                    }
                    if icon_sq(c, "\u{e037}", 28.0, 6.0).on_hover_text("Play").clicked() {
                        crate::platform::play_sound_file(key, file);
                    }
                });
                ui.add_space(6.0);
            }
        });
    }

    fn pane_rules(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Categories & Rules");
        lbl(ui, "Rename template:");
        ui.add_space(5.0);
        field(ui, &mut self.s.rename_template, None, "{name}");
        ui.add_space(4.0);
        hint(ui, "Tokens: {name} {ext} {date} {host}");
        ui.add_space(2.0);
        hint(
            ui,
            &format!(
                "Preview: {}",
                crate::model::apply_rename_template(
                    &self.s.rename_template,
                    "My Video.mp4",
                    "https://cdn.example/v"
                )
            ),
        );
        ui.add_space(14.0);

        lbl(ui, "Type → folder rules (first match wins, max 8):");
        ui.add_space(8.0);

        let avail = ui.available_width();
        let input_w = (avail - 230.0).max(80.0);
        let mut up: Option<usize> = None;
        let mut down: Option<usize> = None;
        let mut del: Option<usize> = None;
        for i in 0..self.s.folder_rules.len().min(8) {
            let rule = &mut self.s.folder_rules[i];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                field(ui, &mut rule.exts, Some(input_w), "mp4,mkv");
                txt(ui, "→", reg(11.0), t.muted);
                field(ui, &mut rule.folder, Some(88.0), "Video");
                if rule_btn(ui, "\u{e5d8}", false).clicked() {
                    up = Some(i);
                }
                if rule_btn(ui, "\u{e5db}", false).clicked() {
                    down = Some(i);
                }
                if rule_btn(ui, "\u{e5cd}", true).clicked() {
                    del = Some(i);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(i) = del {
            self.s.folder_rules.remove(i);
        }
        if let Some(i) = up {
            if i > 0 {
                self.s.folder_rules.swap(i - 1, i);
            }
        }
        if let Some(i) = down {
            if i + 1 < self.s.folder_rules.len() {
                self.s.folder_rules.swap(i, i + 1);
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if button(ui, "Add rule", false).clicked() && self.s.folder_rules.len() < 8 {
                self.s.folder_rules.push(FolderRule {
                    exts: String::new(),
                    folder: String::new(),
                });
            }
            if button(ui, "Reset defaults", false).clicked() {
                self.s.folder_rules = default_folder_rules();
            }
        });

        // ---- site logins (IDM "Site logins") ----
        ui.add_space(18.0);
        lbl(ui, "Site logins (used automatically for that site and its subdomains):");
        ui.add_space(8.0);
        let avail = ui.available_width();
        let w = ((avail - 60.0) / 3.0).max(70.0);
        let mut del: Option<usize> = None;
        for i in 0..self.s.logins.len() {
            let l = &mut self.s.logins[i];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                field(ui, &mut l.host, Some(w), "example.com");
                field(ui, &mut l.user, Some(w), "user name");
                ui.add(egui::TextEdit::singleline(&mut l.pass).password(true).desired_width(w).hint_text("password"));
                if rule_btn(ui, "\u{e5cd}", true).clicked() {
                    del = Some(i);
                }
            });
            ui.add_space(8.0);
        }
        if let Some(i) = del {
            self.s.logins.remove(i);
        }
        if button(ui, "Add login", false).clicked() {
            self.s.logins.push(crate::model::SiteLogin::default());
        }
        ui.add_space(4.0);
        hint(ui, if cfg!(windows) {
            "Passwords are encrypted for your Windows account (DPAPI)."
        } else {
            "Passwords are stored in a file only your account can read."
        });
    }

    // ---------- Updates ----------
    fn pane_updates(&mut self, ui: &mut Ui) {
        let t = tok();
        h2(ui, "Updates");
        hint(ui, "Download Craft itself, and the helpers it keeps for you.");
        ui.add_space(10.0);

        let mut open_updates = self.open_updates;
        let mut check = self.s.update_check_on;
        card(ui, "Download Craft", Some(&format!("v{}", crate::model::APP_VERSION)), &mut open_updates, |ui| {
            switch_row(
                ui,
                "update_check_on",
                &mut check,
                "Check for updates on launch",
                "Asks GitHub for a newer release; never downloads one by itself.",
            );
        });
        self.open_updates = open_updates;
        self.s.update_check_on = check;

        let mut open_version = self.open_version;
        let mut go_plugins = false;
        card(ui, "Helpers", None, &mut open_version, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            wrapped(
                ui,
                "yt-dlp and ffmpeg are checked against their latest releases whenever Plugins opens; the copies this app keeps are updated in place, your own copies are only reported.",
                reg(9.5),
                t.muted,
            );
            ui.add_space(2.0);
            if btn_icon(ui, "\u{e9b0}", "Open Plugins", false).clicked() {
                go_plugins = true;
            }
        });
        self.open_version = open_version;
        if go_plugins {
            self.pane = PANE_YTDLP;
        }
    }
}
