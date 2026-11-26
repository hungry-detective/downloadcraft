//! Crash reports: what a panic leaves behind, and the window that offers to
//! report it on GitHub the next time the app starts.
//!
//! The report names what a developer needs to find the fault (version,
//! Windows build, which part of the app, renderer, layout, the error and
//! where it happened, the last steps of the newest download) and nothing
//! private: web addresses are cut to their site. The window
//! (`download_craft.exe --crash-report`) is drawn like the app's other
//! dialogs. Reporting opens a GitHub issue filled in with that report, for
//! the user to read and send: nothing is sent by itself.

use crate::app::pl;
use crate::dm::*;
use crate::fonts::{reg, semi};
use crate::shaped::ShapeText;
use std::path::PathBuf;
use std::time::Duration;

const REPO: &str = "https://github.com/hungry-detective/downloadcraft";
const W: f32 = 540.0;
const H_SHORT: f32 = 430.0;

fn reports_dir() -> Option<PathBuf> {
    crate::model::config_dir().map(|d| d.join("crash-reports"))
}

/// Which part of the app this process is, from its command line.
fn role() -> &'static str {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--native-host") || crate::native_host::launched_directly(&args) {
        "browser helper (native host)"
    } else if has("--confirm-download") {
        "Download File Info window"
    } else if has("--progress") {
        "progress window"
    } else if has("--complete") {
        "Download complete window"
    } else if has("--settings") {
        "Settings"
    } else if has("--links") {
        "Download all links"
    } else if has("--server") {
        "server mode"
    } else if has("--mcp") {
        "AI assistant (MCP) server"
    } else if args.len() > 1 && !args[1].starts_with('-') {
        "command line"
    } else {
        "main window"
    }
}

/// `https://site.example/path?token=…` → `https://site.example/…`: the
/// report goes to a public issue, so no paths, queries or names.
pub(crate) fn redact(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("http") {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let end = from.find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')').unwrap_or(from.len());
        let link = &from[..end];
        match url::Url::parse(link) {
            Ok(u) if u.host_str().is_some() => {
                out.push_str(&format!("{}://{}/…", u.scheme(), u.host_str().unwrap_or("")));
            }
            _ => out.push_str(link),
        }
        rest = &from[end..];
    }
    out.push_str(rest);
    // Windows user folders name the person.
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.is_empty() {
            out = out.replace(&home, "%USERPROFILE%");
        }
    }
    out
}

/// The last steps of the most recently written download log, redacted.
fn recent_activity() -> String {
    let Some(dir) = crate::model::config_dir().map(|d| d.join("logs")) else { return String::new() };
    let newest = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t);
    let Some((_, path)) = newest else { return String::new() };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(12)..].iter().map(|l| redact(l)).collect::<Vec<_>>().join("\n")
}

fn system() -> String {
    #[cfg(windows)]
    {
        let build = crate::platform::windows_build();
        let name = if build >= 22000 { "Windows 11" } else { "Windows 10" };
        format!("{name} (build {build}), {}", std::env::consts::ARCH)
    }
    #[cfg(not(windows))]
    {
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

/// The settings that change how the app draws and behaves.
fn look() -> String {
    let theme = crate::model::ui_pref_str("theme", "system");
    let scale = crate::model::ui_pref_str("ui_scale", "");
    let lang = crate::model::ui_pref_str("language", "en");
    #[cfg(windows)]
    let renderer = if crate::platform::software_renderer() { "Direct3D 12 / WARP" } else { "OpenGL" };
    #[cfg(not(windows))]
    let renderer = "OpenGL";
    let frame = if crate::platform::system_title_bar() { "Windows title bar" } else { "app title bar" };
    format!(
        "theme {theme}, language {lang}{}, {renderer}, {frame}",
        if scale.is_empty() { String::new() } else { format!(", interface size {scale}") }
    )
}

/// Write the report for a panic (from the panic hook; must not panic).
pub fn record(info: &std::panic::PanicHookInfo<'_>) {
    let Some(dir) = reports_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into());
    let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
    let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
    // Release builds carry no symbols: only the named frames say anything.
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<String> = bt
        .lines()
        .map(str::trim)
        .filter(|l| !l.contains("<unknown>") && !l.starts_with("at ") && l.contains(':'))
        .take(12)
        .map(str::to_string)
        .collect();
    let frames = if frames.is_empty() { "(none in this build)".to_string() } else { frames.join("\n") };
    let report = compose("crash", &redact(&message), &format!("{at} (thread {thread})"), "Named stack frames", &frames);
    let name = format!("crash-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let _ = std::fs::write(dir.join(&name), &report);
    let _ = std::fs::write(dir.join("pending.txt"), name);
    if let Some(d) = crate::model::config_dir() {
        let _ = std::fs::write(d.join("crash.log"), &report);
    }
}

/// A report's text: the same fields for a crash and a freeze.
fn compose(kind: &str, message: &str, at: &str, extra_head: &str, extra: &str) -> String {
    format!(
        "Download Craft {version}\n\
         Kind: {kind}\n\
         System: {system}\n\
         Part: {role}\n\
         Look: {look}\n\
         Error: {message}\n\
         Where: {at}\n\
         Time: {time}\n\
         \n\
         {extra_head}:\n{extra}\n\
         \n\
         Recent activity (newest download log, sites only):\n{activity}\n",
        version = crate::model::APP_VERSION,
        system = system(),
        role = role(),
        look = look(),
        time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z"),
        activity = {
            let a = recent_activity();
            if a.is_empty() { "(none)".to_string() } else { a }
        },
    )
}

/// What the window is doing, set as it goes, so a freeze's report can say
/// which part of a frame it was stuck in.
static STEP: std::sync::Mutex<&'static str> = std::sync::Mutex::new("starting");

/// How many downloads run (for a freeze's report).
pub static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Name the part of the window's work that runs next (cheap: one lock).
pub fn step(what: &'static str) {
    if let Ok(mut s) = STEP.lock() {
        *s = what;
    }
}

/// A window that has not answered Windows for this long is a freeze
/// (Windows itself shows "Not responding" after 5 s).
pub const FREEZE_AFTER: Duration = Duration::from_secs(15);

/// Freeze reports: a watcher thread asks Windows whether this process's
/// visible windows still answer (`IsHungAppWindow`: no messages handled
/// for 5 s; a window closed to the tray is parked visible, so it is watched
/// too). One that
/// stays silent past [`FREEZE_AFTER`] gets a report like a crash's, naming
/// the part of the frame it is stuck in. If the window comes back, the
/// report window opens at once; if the app has to be ended, it opens at the
/// next start. A sleep of the computer is not a freeze.
#[cfg(windows)]
pub fn watch_for_freezes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible};
    static HUNG: AtomicBool = AtomicBool::new(false);
    static FOUND: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> i32 {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        // Only the app's visible windows: hidden helper windows (the
        // graphics device's, the input method's) never handle messages and
        // always look "not responding". A window parked in the tray stays
        // visible (off-screen), so it is still watched.
        if pid != std::process::id() || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        FOUND.store(true, Ordering::Relaxed);
        if IsHungAppWindow(hwnd) != 0 {
            HUNG.store(true, Ordering::Relaxed);
            return 0;
        }
        1
    }
    let _ = std::thread::Builder::new().name("freeze watchdog".into()).spawn(|| {
        let tick = Duration::from_secs(1);
        let mut hung_since: Option<std::time::Instant> = None;
        let mut reported: Option<PathBuf> = None;
        let mut last = std::time::Instant::now();
        loop {
            std::thread::sleep(tick);
            // The computer slept: the window could not answer meanwhile.
            let gap = last.elapsed();
            last = std::time::Instant::now();
            if gap > tick * 5 {
                hung_since = None;
                continue;
            }
            HUNG.store(false, Ordering::Relaxed);
            FOUND.store(false, Ordering::Relaxed);
            // SAFETY: a plain enumeration with a callback that only reads.
            unsafe { EnumWindows(Some(each), 0) };
            let hung = FOUND.load(Ordering::Relaxed) && HUNG.load(Ordering::Relaxed);
            if hung {
                // Windows says so after 5 s of silence.
                let since = *hung_since.get_or_insert_with(|| std::time::Instant::now() - Duration::from_secs(5));
                if reported.is_none() && since.elapsed() >= FREEZE_AFTER {
                    reported = record_freeze(since.elapsed());
                }
            } else if let Some(since) = hung_since.take() {
                if let Some(path) = reported.take() {
                    // It came back: say for how long, and offer it now.
                    let secs = since.elapsed().as_secs();
                    if let Ok(mut text) = std::fs::read_to_string(&path) {
                        text = text.replacen("\nWhere: ", &format!("\nAnswered again after: {secs} s\nWhere: "), 1);
                        let _ = std::fs::write(&path, text);
                    }
                    offer_if_pending();
                }
            }
        }
    });
}

#[cfg(not(windows))]
pub fn watch_for_freezes() {}

/// Write a freeze's report (pending, like a crash's).
#[cfg_attr(not(windows), allow(dead_code))]
fn record_freeze(for_how_long: Duration) -> Option<PathBuf> {
    let dir = reports_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    let step = STEP.lock().map(|s| *s).unwrap_or("unknown");
    let report = compose(
        "freeze",
        &format!("The window stopped responding for more than {} s", for_how_long.as_secs()),
        &format!("main window, while {step}"),
        "What was running",
        &format!("{} download(s)", RUNNING.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let name = format!("freeze-{}.txt", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(&name);
    std::fs::write(&path, &report).ok()?;
    let _ = std::fs::write(dir.join("pending.txt"), name);
    Some(path)
}

/// At start: a crash is waiting to be reported, so open the report window.
pub fn offer_if_pending() {
    let Some(dir) = reports_dir() else { return };
    if !dir.join("pending.txt").is_file() {
        return;
    }
    let Ok(exe) = crate::platform::launch_path() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--crash-report");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008);
    }
    let _ = cmd.spawn();
}

/// The GitHub issue for a report: a place to say what happened, then the
/// details the app filled in.
pub(crate) fn issue_url(report: &str) -> String {
    let field = |key: &str| {
        report.lines().find_map(|l| l.strip_prefix(&format!("{key}: ")).map(str::trim)).unwrap_or("").to_string()
    };
    let error = field("Error");
    // A short title that ends on a whole word.
    let short: String = if error.chars().count() <= 70 {
        error.clone()
    } else {
        let cut: String = error.chars().take(70).collect();
        let at = cut.rfind(' ').unwrap_or(cut.len());
        format!("{}…", cut[..at].trim_end_matches([',', ';', ':', ' ']))
    };
    let what = if field("Kind") == "freeze" { "Freeze" } else { "Crash" };
    let title = format!("{what} in {}: {}", crate::model::APP_VERSION, if short.is_empty() { "unexpected error".into() } else { short });
    let mut details = report.to_string();
    // GitHub refuses very long new-issue links.
    if details.chars().count() > 5200 {
        details = details.chars().take(5200).collect::<String>() + "\n…";
    }
    let body = format!(
        "**What I was doing when it happened**\n<!-- For example: downloading a YouTube video, pausing a download, opening Settings -->\n\n\n\
         **Can it happen again?**\n<!-- Every time / sometimes / once -->\n\n\n\
         **Details filled in by Download Craft**\n```\n{details}\n```\n"
    );
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let label = if what == "Freeze" { "freeze" } else { "crash" };
    format!("{REPO}/issues/new?title={}&body={}&labels={label}", enc(&title), enc(&body))
}

struct CrashApp {
    name: String,
    report: String,
    error: String,
    part: String,
    /// The window froze (rather than crashed).
    freeze: bool,
    details: bool,
    placed: bool,
    height: f32,
}

fn close_soon(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        std::process::exit(0);
    });
}

/// `--crash-report`: the report window.
pub fn run() -> eframe::Result<()> {
    let Some(dir) = reports_dir() else { return Ok(()) };
    let Some(_slot) = crate::model::claim_window("crash-report") else { return Ok(()) };
    let marker = dir.join("pending.txt");
    let Ok(name) = std::fs::read_to_string(&marker) else { return Ok(()) };
    let _ = std::fs::remove_file(&marker);
    let name = name.trim().to_string();
    let Ok(report) = std::fs::read_to_string(dir.join(&name)) else { return Ok(()) };
    let field = |key: &str| report.lines().find_map(|l| l.strip_prefix(&format!("{key}: ")).map(str::to_string));
    // Reports from before 0.11.6 have no fields: their panic line stands in.
    let error = field("Error")
        .or_else(|| report.lines().skip_while(|l| !l.starts_with("panic:")).nth(1).map(str::to_string))
        .unwrap_or_default();
    let part = field("Part").unwrap_or_else(|| "Download Craft".into());
    let freeze = field("Kind").as_deref() == Some("freeze");
    let app = CrashApp { name, report, error, part, freeze, details: false, placed: false, height: H_SHORT };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Download Craft — problem report")
        .with_app_id(crate::platform::app_id())
        .with_inner_size([W, H_SHORT])
        .with_resizable(false)
        .with_always_on_top()
        .with_decorations(crate::platform::system_title_bar())
        .with_transparent(!crate::platform::system_title_bar());
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        crate::model::APP_NAME,
        crate::platform::native_options(viewport),
        Box::new(|cc| {
            crate::platform::guard_gpu(cc);
            crate::fonts::install(cc, true);
            crate::app::apply_theme(&cc.egui_ctx, &crate::app::stored_theme());
            Ok(Box::new(app))
        }),
    )
}

/// A filled (primary) or outline button; true when clicked.
fn button(ui: &egui::Ui, p: &egui::Painter, r: egui::Rect, id: &str, label: &str, primary: bool) -> bool {
    let t = pl();
    let resp = ui.interact(r, egui::Id::new(("crash-btn", id)), egui::Sense::click());
    let hov = resp.hovered();
    if primary {
        p.rect_filled(r, 8.0, if hov { t.primary.gamma_multiply(0.85) } else { t.primary });
        p.stext(r.center(), egui::Align2::CENTER_CENTER, label, semi(13.0), egui::Color32::WHITE);
    } else {
        if hov {
            p.rect_filled(r, 8.0, t.surface_lt);
        }
        p.rect_stroke(r, 8.0, egui::Stroke::new(1.0_f32, t.card_bd), egui::StrokeKind::Inside);
        p.stext(r.center(), egui::Align2::CENTER_CENTER, label, semi(12.5), t.txt);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

impl eframe::App for CrashApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let h = self.height;
        if !self.placed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if crate::model::center_window(ctx, egui::vec2(W, h)) {
                self.placed = true;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            close_soon(ctx);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| self.paint(ui));
    }
}

impl CrashApp {
    fn paint(&mut self, ui: &mut egui::Ui) {
        let t = pl();
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.rect(full.shrink(0.5), 14.0, t.modal_bg, egui::Stroke::new(1.0_f32, t.card_bd), egui::StrokeKind::Inside);
        let o = full.min;
        let x0 = o.x + 24.0;
        let w = full.width() - 48.0;
        let warn = egui::Color32::from_rgb(0xf5, 0x9e, 0x0b);

        // ---- title bar ----
        let bar = egui::Rect::from_min_size(o, egui::vec2(full.width(), 44.0));
        let drag = ui.interact(bar.with_max_x(bar.max.x - 50.0), egui::Id::new("crash-drag"), egui::Sense::click_and_drag());
        if drag.drag_started() {
            crate::platform::start_drag(&ctx);
        }
        p.stext(egui::pos2(x0 - 4.0, o.y + 22.0), egui::Align2::LEFT_CENTER, "Problem report", semi(14.0), t.txt);
        let close = egui::Rect::from_center_size(egui::pos2(full.max.x - 27.0, o.y + 22.0), egui::vec2(30.0, 30.0));
        let cr = ui.interact(close, egui::Id::new("crash-x"), egui::Sense::click()).on_hover_text("Close");
        if cr.hovered() {
            p.rect_filled(close, 7.0, t.surface_lt);
        }
        dm_icon(&p, close.shrink(9.0), DmIcon::Close, if cr.hovered() { t.txt } else { t.dim }, 2.0);
        if cr.clicked() {
            close_soon(&ctx);
        }
        p.hline(full.x_range(), bar.max.y, egui::Stroke::new(1.0_f32, t.card_bd));

        // ---- headline: a warning mark and what went wrong ----
        let mut y = bar.max.y + 20.0;
        let mark = egui::pos2(x0 + 20.0, y + 20.0);
        p.circle_filled(mark, 20.0, warn.gamma_multiply(0.18));
        p.stext(mark, egui::Align2::CENTER_CENTER, "!", semi(22.0), warn);
        let tx = x0 + 56.0;
        p.stext(egui::pos2(tx, y + 4.0), egui::Align2::LEFT_TOP, if self.freeze { "Download Craft stopped responding" } else { "Download Craft ran into a problem last time" }, semi(15.5), t.txt);
        let what = format!("In the {}", self.part);
        p.stext(egui::pos2(tx, y + 26.0), egui::Align2::LEFT_TOP, &what, reg(12.5), t.dim);
        y += 58.0;

        // ---- the error, in a card that fits it ----
        let err: String = {
            let e = self.error.trim();
            if e.chars().count() > 180 { e.chars().take(178).collect::<String>() + "…" } else { e.to_string() }
        };
        let galley = p.layout(err, reg(12.0), t.txt, w - 24.0);
        let card = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, galley.size().y + 20.0));
        p.rect(card, 8.0, t.surface_lt, egui::Stroke::new(1.0_f32, t.card_bd), egui::StrokeKind::Inside);
        p.galley(egui::pos2(card.min.x + 12.0, card.min.y + 10.0), galley, t.txt);
        y = card.max.y + 16.0;

        // ---- why reporting matters: three steps, drawn ----
        let banner = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, 132.0));
        p.rect(banner, 10.0, t.primary.gamma_multiply(0.10), egui::Stroke::new(1.0_f32, t.primary.gamma_multiply(0.35)), egui::StrokeKind::Inside);
        p.stext(egui::pos2(banner.min.x + 16.0, banner.min.y + 14.0), egui::Align2::LEFT_TOP, "Every report makes Download Craft stronger", semi(14.0), t.txt);
        p.stext(
            egui::pos2(banner.min.x + 16.0, banner.min.y + 34.0),
            egui::Align2::LEFT_TOP,
            "One click helps fix this for you and everyone who uses it.",
            reg(12.0),
            t.dim,
        );
        let steps = [("You send it", 0u8), ("We find the cause", 1), ("Everyone gets the fix", 2)];
        let step_w = (w - 32.0) / 3.0;
        let sy = banner.min.y + 84.0;
        for (i, (label, kind)) in steps.iter().enumerate() {
            let cx = banner.min.x + 16.0 + step_w * (i as f32 + 0.5);
            let c = egui::pos2(cx, sy - 8.0);
            p.circle_filled(c, 17.0, t.primary.gamma_multiply(0.22));
            let ink = egui::Stroke::new(1.8_f32, t.primary);
            match kind {
                // A paper plane.
                0 => {
                    let pts = vec![c + egui::vec2(-8.0, -1.0), c + egui::vec2(8.0, -7.0), c + egui::vec2(3.0, 8.0), c + egui::vec2(-1.0, 2.0)];
                    p.add(egui::Shape::closed_line(pts, ink));
                    p.line_segment([c + egui::vec2(-1.0, 2.0), c + egui::vec2(8.0, -7.0)], ink);
                }
                // A magnifying glass.
                1 => {
                    p.circle_stroke(c + egui::vec2(-2.0, -2.0), 6.0, ink);
                    p.line_segment([c + egui::vec2(2.5, 2.5), c + egui::vec2(8.0, 8.0)], egui::Stroke::new(2.4_f32, t.primary));
                }
                // A tick: fixed for everyone.
                _ => dm_icon(&p, egui::Rect::from_center_size(c, egui::vec2(16.0, 16.0)), DmIcon::Check, t.primary, 2.2),
            }
            p.stext(egui::pos2(cx, sy + 16.0), egui::Align2::CENTER_TOP, label, reg(11.5), t.txt);
            if i < 2 {
                // An arrow to the next step.
                let ax = cx + step_w / 2.0;
                p.line_segment([egui::pos2(ax - 8.0, c.y), egui::pos2(ax + 6.0, c.y)], egui::Stroke::new(1.4_f32, t.dim));
                p.line_segment([egui::pos2(ax + 2.0, c.y - 4.0), egui::pos2(ax + 6.0, c.y)], egui::Stroke::new(1.4_f32, t.dim));
                p.line_segment([egui::pos2(ax + 2.0, c.y + 4.0), egui::pos2(ax + 6.0, c.y)], egui::Stroke::new(1.4_f32, t.dim));
            }
        }
        y = banner.max.y + 14.0;

        p.stext(
            egui::pos2(x0, y),
            egui::Align2::LEFT_TOP,
            "A GitHub issue opens with the details filled in. You read it and send it yourself;",
            reg(11.5),
            t.dim,
        );
        p.stext(
            egui::pos2(x0, y + 17.0),
            egui::Align2::LEFT_TOP,
            "nothing is sent by itself, and web addresses only show their site.",
            reg(11.5),
            t.dim,
        );
        y += 46.0;

        // ---- details (the full report) ----
        if self.details {
            let area = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, 220.0));
            p.rect(area, 8.0, t.surface_lt, egui::Stroke::new(1.0_f32, t.card_bd), egui::StrokeKind::Inside);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(10.0)));
            egui::ScrollArea::vertical().id_salt("crash-details").show(&mut child, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&self.report).font(egui::FontId::monospace(11.0)).color(t.txt)).wrap());
            });
            y = area.max.y + 16.0;
        }

        // ---- buttons: below everything above, never on it ----
        let bh = 38.0;
        let by = y;
        let report = egui::Rect::from_min_size(egui::pos2(full.max.x - 24.0 - 170.0, by), egui::vec2(170.0, bh));
        let later = egui::Rect::from_min_size(egui::pos2(report.min.x - 10.0 - 104.0, by), egui::vec2(104.0, bh));
        let show = egui::Rect::from_min_size(egui::pos2(x0, by), egui::vec2(132.0, bh));
        if button(ui, &p, report, "report", "Report problem", true) {
            crate::platform::open_url(&issue_url(&self.report));
            close_soon(&ctx);
        }
        if button(ui, &p, later, "later", "Not now", false) {
            close_soon(&ctx);
        }
        let label = if self.details { "Hide details" } else { "Show details" };
        if button(ui, &p, show, "details", label, false) {
            self.details = !self.details;
        }
        // The window fits what it shows.
        let want = (by + bh + 22.0 - o.y).ceil();
        if (want - self.height).abs() > 1.0 {
            self.height = want;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(W, want)));
        }
        let _ = &self.name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_keep_only_the_site_of_a_link() {
        let line = "[21:28:28] start: https://www.youtube.com/watch?v=0dgYA8j9AbE&list=RD (8 connections)";
        assert_eq!(redact(line), "[21:28:28] start: https://www.youtube.com/… (8 connections)");
        assert_eq!(redact("no links here"), "no links here");
    }

    /// `DC_CRASH_REPORT=<report file> cargo test print_issue_link -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_issue_link() {
        let Ok(path) = std::env::var("DC_CRASH_REPORT") else { return };
        println!("ISSUE_URL={}", issue_url(&std::fs::read_to_string(path).unwrap()));
    }

    #[test]
    fn the_issue_link_carries_the_error_in_its_title() {
        let report = "Download Craft 0.11.6\nError: end byte index 5 is not a char boundary\nWhere: src\\youtube.rs:231:25\n";
        let url = issue_url(report);
        assert!(url.starts_with("https://github.com/hungry-detective/downloadcraft/issues/new?title=Crash%20in%20"));
        assert!(url.contains("char%20boundary"));
        assert!(url.contains("labels=crash"));
    }
}
