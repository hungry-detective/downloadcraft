//! Tray behaviour: park the window off-screen instead of hiding it.
//!
//! eframe only runs a frame when the window repaints, and a *hidden* window
//! never receives `WM_PAINT` (winit's own comment: "the window technically has
//! to be visible to receive WM_PAINT messages"). `ViewportCommand::Visible(false)`
//! therefore froze the entire app while it sat in the tray: transfer progress
//! and completions were never reported, the browser inbox was never drained
//! (so captures looked stuck), the liveness heartbeat went stale and the tray
//! menu - polled from the frame - did nothing at all.
//!
//! "In the tray" is therefore a window that stays `WS_VISIBLE` (so it keeps
//! painting and the app keeps working) but is moved far off-screen and marked
//! `WS_EX_TOOLWINDOW` so it has no taskbar button. Un-traying restores the
//! remembered placement and takes the foreground back.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI8, AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;

/// True while the window is parked off-screen ("in the tray").
static PARKED: AtomicBool = AtomicBool::new(false);
/// Native window handle of the main window (set from the eframe frame).
static HWND: AtomicIsize = AtomicIsize::new(0);
/// Set by the tray's Exit item, consumed by the frame loop.
static QUIT: AtomicBool = AtomicBool::new(false);
/// Set once a quit is in flight: nothing may un-park the window on the way out.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Frame counter, bumped by every `update` (see [`start_keepalive`]).
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// Placement to restore on un-tray (position, size, maximized state).
static PLACEMENT: Mutex<Option<win32::WindowPlacement>> = Mutex::new(None);
/// Last title-bar theme requested with [`set_titlebar_dark`] (-1 = never set).
/// Kept so the caption can be re-asserted after Windows resets the frame.
static TITLEBAR_DARK: AtomicI8 = AtomicI8::new(-1);
/// Last caption colour requested with [`set_titlebar_color`], packed as a
/// Win32 `COLORREF` (`0x00BBGGRR`); -1 = never set.
static TITLEBAR_RGB: AtomicI32 = AtomicI32::new(-1);
/// Set by another launch (or the show-event listener) to ask the *frame
/// thread* to raise the window: only the thread that owns the window can win
/// the foreground lock reliably, so the request is parked here until `update`.
static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

const TRAY_TIP: &str = "Download Craft";
/// Auto-reset event used to wake the running manager when the user launches
/// the app again (Start menu, pinned shortcut, taskbar). A second process
/// cannot raise another process' window reliably once it is parked, so it
/// signals this event and the first instance restores itself.
const SHOW_EVENT: &str = "Local\\DownloadCraft-Show";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Become the one running manager, or wake the instance that already is.
/// Returns the held lock file for the lifetime of the process; `None` means
/// this launch should exit after handing over to the running window.
pub fn claim_main_instance() -> Option<std::fs::File> {
    match crate::model::claim_window("main") {
        Some(slot) => {
            install_show_listener();
            Some(slot)
        }
        None => {
            signal_show();
            None
        }
    }
}

/// Ask the frame thread to bring the window forward (safe from any thread).
pub fn request_show() {
    SHOW_REQUESTED.store(true, Ordering::SeqCst);
    nudge_paint();
}

/// True once, when a raise was requested since the last call.
pub fn take_show_request() -> bool {
    SHOW_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Listen for "show yourself" signals from later launches.
fn install_show_listener() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let name = wide(SHOW_EVENT);
        let ev = unsafe { win32::CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if ev == 0 {
            return;
        }
        loop {
            let r = unsafe { win32::WaitForSingleObject(ev, 1000) };
            if r == win32::WAIT_OBJECT_0 {
                if !quitting() {
                    // The frame thread does the actual raise.
                    request_show();
                }
            } else if quitting() {
                break;
            }
        }
    });
}

/// Wake the running instance. Falls back to finding its window when it was
/// started by an older build that has no event.
fn signal_show() {
    let name = wide(SHOW_EVENT);
    unsafe {
        let ev = win32::OpenEventW(win32::EVENT_MODIFY_STATE, 0, name.as_ptr());
        if ev != 0 {
            win32::SetEvent(ev);
            return;
        }
        let title = wide(TRAY_TIP);
        let hwnd = win32::FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            raise_existing(hwnd);
        }
    }
}

/// Bring another process' window back on screen. A parked window sits at
/// `OFFSCREEN`; move it somewhere visible first, then activate it.
unsafe fn raise_existing(hwnd: isize) {
    let mut r = win32::Rect::default();
    if win32::GetWindowRect(hwnd, &mut r) != 0 && r.left <= win32::OFFSCREEN + 64 {
        win32::ShowWindow(hwnd, win32::SW_RESTORE);
        win32::SetWindowPos(
            hwnd,
            0,
            140,
            110,
            0,
            0,
            win32::SWP_NOSIZE | win32::SWP_NOZORDER | win32::SWP_SHOWWINDOW,
        );
    }
    force_foreground(hwnd);
}

/// The main window's native handle (0 before the first frame).
pub fn hwnd() -> isize {
    HWND.load(Ordering::SeqCst)
}

pub fn set_hwnd(hwnd: isize) {
    if hwnd != 0 {
        if HWND.swap(hwnd, Ordering::SeqCst) != hwnd {
            round_corners(hwnd);
        }
    }
}

/// Windows 11 rounded window corners (`DWMWA_WINDOW_CORNER_PREFERENCE` =
/// 33, `DWMWCP_ROUND` = 2). The app paints its own frame, so Windows would
/// otherwise keep square corners; Windows 10 ignores the request.
fn round_corners(hwnd: isize) {
    let pref: i32 = 2;
    unsafe {
        win32::DwmSetWindowAttribute(
            hwnd,
            33,
            &pref as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

pub fn is_parked() -> bool {
    PARKED.load(Ordering::SeqCst)
}

/// True once the tray's Exit has been chosen and the window is on its way out.
pub fn quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// True once, when the tray's Exit item was chosen.
pub fn take_quit() -> bool {
    QUIT.swap(false, Ordering::SeqCst)
}

/// Count one frame and keep the paint pump armed.
///
/// eframe only runs `update` when the window paints, and Windows stops
/// generating `WM_PAINT` on its own for a window the compositor has nothing to
/// draw for: parked far off-screen (and, less often, fully covered by other
/// windows). eframe's own redraw requests go through an internal-paint flag
/// that such a window never turns into a paint. The app then looks alive - the
/// window is visible, the tray icon answers right-clicks, the menu opens - while
/// every frame-driven job is dead: progress reporting, the browser inbox, the
/// liveness heartbeat and the tray menu items, whose "Show manager" and "Exit"
/// simply did nothing. Marking the window dirty (see [`nudge_paint`]) produces a
/// genuine `WM_PAINT`, which is exactly what the frame loop waits for.
pub fn frame_tick() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// Force one real paint of the main window; safe from any thread.
pub fn nudge_paint() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        win32::InvalidateRect(hwnd, std::ptr::null(), 0);
        win32::RedrawWindow(hwnd, std::ptr::null(), 0, win32::RDW_INVALIDATE);
    }
}

/// Watchdog for the frame loop: if no frame has run half a second after the
/// last check, ask for a paint. A healthy app (its own repaint request runs
/// every 100-250 ms) never gets nudged, so this costs nothing in the common
/// case; a stalled one is restarted, which is what keeps tray actions - and
/// everything else the frame loop does - working while parked.
pub fn start_keepalive() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let mut seen = FRAMES.load(Ordering::Relaxed);
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let now = FRAMES.load(Ordering::Relaxed);
            if now == seen {
                nudge_paint();
            }
            seen = now;
            // Windows re-derives the non-client frame from the system theme in
            // a few situations (show after hide, composition changes). Re-send
            // the caption colours every couple of seconds so an explicit light
            // app on a dark desktop never falls back to a black title bar.
            ticks += 1;
            if ticks % 4 == 0 {
                refresh_titlebar();
            }
        }
    });
}

/// Park the main window off-screen: still visible (so the frame loop keeps
/// running), but invisible to the user and without a taskbar button.
pub fn park() {
    if PARKED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let mut placement = win32::WindowPlacement {
            length: std::mem::size_of::<win32::WindowPlacement>() as u32,
            ..Default::default()
        };
        let saved = if win32::GetWindowPlacement(hwnd, &mut placement) != 0 {
            // Coming back *minimized* would freeze the app again.
            if placement.show_cmd == win32::SW_SHOWMINIMIZED {
                placement.show_cmd = win32::SW_SHOWNORMAL as u32;
            }
            Some(placement)
        } else {
            None
        };
        if let Ok(mut slot) = PLACEMENT.lock() {
            *slot = saved;
        }
        // Un-minimize/un-maximize so the move off-screen sticks (a maximized
        // window ignores SetWindowPos), then hide -> retag -> show so the
        // taskbar drops the button.
        win32::ShowWindow(hwnd, win32::SW_RESTORE);
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_APPWINDOW) | win32::WS_EX_TOOLWINDOW,
        );
        win32::SetWindowPos(
            hwnd,
            0,
            win32::OFFSCREEN,
            win32::OFFSCREEN,
            0,
            0,
            win32::SWP_NOSIZE
                | win32::SWP_NOZORDER
                | win32::SWP_NOACTIVATE
                | win32::SWP_SHOWWINDOW,
        );
    }
}

/// Bring the main window back on screen and forward (tray click / Show).
pub fn show() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    if !PARKED.swap(false, Ordering::SeqCst) {
        // Already on screen: just raise it.
        unsafe { force_foreground(hwnd) };
        return;
    }
    unsafe {
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_TOOLWINDOW) | win32::WS_EX_APPWINDOW,
        );
        let placement = PLACEMENT.lock().ok().and_then(|g| *g);
        match placement {
            Some(placement) => {
                win32::SetWindowPlacement(hwnd, &placement);
            }
            None => {
                win32::SetWindowPos(
                    hwnd,
                    0,
                    140,
                    110,
                    0,
                    0,
                    win32::SWP_NOSIZE | win32::SWP_NOZORDER | win32::SWP_NOACTIVATE,
                );
            }
        }
        force_foreground(hwnd);
        // Showing a hidden window can make DWM rebuild the frame with the OS
        // theme; put our caption colours back before the user sees it.
        refresh_titlebar();
    }
}

/// Same as [`show`] but without stealing the foreground (settings changes).
pub fn restore() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 || !PARKED.swap(false, Ordering::SeqCst) {
        return;
    }
    unsafe {
        win32::ShowWindow(hwnd, win32::SW_HIDE);
        let ex = win32::GetWindowLongW(hwnd, win32::GWL_EXSTYLE);
        win32::SetWindowLongW(
            hwnd,
            win32::GWL_EXSTYLE,
            (ex & !win32::WS_EX_TOOLWINDOW) | win32::WS_EX_APPWINDOW,
        );
        let placement = PLACEMENT.lock().ok().and_then(|g| *g);
        match placement {
            Some(placement) => {
                win32::SetWindowPlacement(hwnd, &placement);
            }
            None => {
                win32::SetWindowPos(
                    hwnd,
                    0,
                    140,
                    110,
                    0,
                    0,
                    win32::SWP_NOSIZE | win32::SWP_NOZORDER | win32::SWP_NOACTIVATE,
                );
            }
        }
        win32::ShowWindow(hwnd, win32::SW_SHOWNOACTIVATE);
        refresh_titlebar();
    }
}

/// Ask the frame loop to quit cleanly (tray's Exit item).
///
/// The request is honoured from the menu click itself rather than from the
/// frame loop, so Exit works even when the frame loop has gone to sleep, and
/// a watchdog covers the one case where no paint can happen at all (a locked
/// session): leaving a dead tray icon behind is worse than a forced exit.
pub fn request_quit() {
    crate::model::clear_heartbeat();
    QUITTING.store(true, Ordering::SeqCst);
    QUIT.store(true, Ordering::SeqCst);
    nudge_paint(); // wake the frame loop now, not half a second from now
    arm_quit_watchdog();
}

/// See [`request_quit`]. Five seconds is far longer than a clean shutdown
/// needs; if the app is still here by then it never got a frame.
fn arm_quit_watchdog() {
    static ARMED: AtomicBool = AtomicBool::new(false);
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        std::process::exit(0);
    });
}

/// Act on the tray menu the moment it is clicked instead of waiting for the
/// next frame (muda calls this handler from the menu's own message loop, which
/// keeps running even when the frame loop does not). `Show` therefore brings
/// the window back and `Exit` starts the shutdown regardless of frames; the
/// frame loop keeps its own handling as the fallback.
pub fn install_menu_handler() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    tray_icon::menu::MenuEvent::set_event_handler(Some(|ev: tray_icon::menu::MenuEvent| {
        if ev.id() == "tray-show" {
            show();
            nudge_paint();
        } else if ev.id() == "tray-exit" {
            request_quit();
        } else {
            // The other entries (pause all, a running download…) need the
            // app's state: queued here, done by the next frame.
            if let Ok(mut q) = TRAY_ACTIONS.lock() {
                q.push(ev.id().0.clone());
            }
            nudge_paint();
        }
    }));
}

/// Tray menu entries waiting for the frame loop.
static TRAY_ACTIONS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The tray menu entries clicked since the last frame (their ids).
pub fn take_actions() -> Vec<String> {
    TRAY_ACTIONS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

pub fn tooltip() -> &'static str {
    TRAY_TIP
}

/// Raise the window and take the foreground. Clicking the tray already gave
/// us the user's intent, but Windows' foreground lock still applies, so we
/// attach to both input queues for the switch, tap ALT to become the process
/// that produced the last input, and bounce through topmost as a last resort.
/// Without this the window came up *behind* whatever the user had in front.
unsafe fn force_foreground(hwnd: isize) {
    // A minimized window ignores every raise: restore first.
    if win32::IsIconic(hwnd) != 0 {
        win32::ShowWindow(hwnd, win32::SW_RESTORE);
    }
    win32::ShowWindow(hwnd, win32::SW_SHOW);
    win32::BringWindowToTop(hwnd);

    let foreground = win32::GetForegroundWindow();
    let foreground_thread = if foreground != 0 {
        win32::GetWindowThreadProcessId(foreground, std::ptr::null_mut())
    } else {
        0
    };
    let target_thread = win32::GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
    let me = win32::GetCurrentThreadId();
    // Borrow the current foreground queue (the classic recipe) and the
    // window's own queue, so SetFocus/SetActiveWindow are legal as well.
    let attached_fg = foreground_thread != 0
        && foreground_thread != me
        && win32::AttachThreadInput(me, foreground_thread, 1) != 0;
    let attached_target = target_thread != 0
        && target_thread != me
        && win32::AttachThreadInput(me, target_thread, 1) != 0;

    win32::SetForegroundWindow(hwnd);
    win32::SetActiveWindow(hwnd);
    win32::SetFocus(hwnd);

    if attached_target {
        win32::AttachThreadInput(me, target_thread, 0);
    }
    if attached_fg {
        win32::AttachThreadInput(me, foreground_thread, 0);
    }

    if win32::GetForegroundWindow() != hwnd {
        // Still refused: the lock only yields to the process that produced
        // the last input event. A synthetic ALT tap makes us that process.
        win32::keybd_event(win32::VK_MENU as u8, 0, 0, 0);
        win32::keybd_event(win32::VK_MENU as u8, 0, win32::KEYEVENTF_KEYUP, 0);
        win32::SetForegroundWindow(hwnd);
        // The shell's own activation path; works in some cases where
        // SetForegroundWindow is ignored (taskbar/tray clicks).
        win32::SwitchToThisWindow(hwnd, 1);
    }
    hold_above(hwnd);
}

/// Keep the raised window above other applications for a moment. Windows can
/// grant the foreground for an instant and then hand it back to the app that
/// was in front (Chrome does this constantly), which looked exactly like "my
/// app opens behind everything". Topmost guarantees visibility; the hold ends
/// once the window has *stayed* foreground for a moment, or after six
/// seconds, and the drop never activates anything.
unsafe fn hold_above(hwnd: isize) {
    win32::SetWindowPos(
        hwnd,
        win32::HWND_TOPMOST,
        0,
        0,
        0,
        0,
        win32::SWP_NOMOVE | win32::SWP_NOSIZE | win32::SWP_SHOWWINDOW,
    );
    std::thread::spawn(move || {
        let mut ever_foreground = false;
        let mut stable = 0u32;
        let mut lost = 0u32;
        for _ in 0..80 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            unsafe {
                if win32::GetForegroundWindow() == hwnd {
                    if ever_foreground && lost > 0 {
                        // The user clicked back on us: done.
                        break;
                    }
                    ever_foreground = true;
                    lost = 0;
                    stable += 1;
                    if stable >= 20 {
                        break;
                    }
                } else {
                    stable = 0;
                    if ever_foreground {
                        // Something else took over (Chrome loves doing this
                        // right after a raise). Stay on top briefly so the
                        // window cannot simply vanish behind it again.
                        lost += 1;
                        if lost >= 30 {
                            break;
                        }
                    } else {
                        win32::SwitchToThisWindow(hwnd, 1);
                        win32::SetForegroundWindow(hwnd);
                    }
                }
            }
        }
        unsafe {
            win32::SetWindowPos(
                hwnd,
                win32::HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                win32::SWP_NOMOVE | win32::SWP_NOSIZE | win32::SWP_NOACTIVATE,
            );
        }
    });
}

/// Paint the native title bar in the app's own theme. Windows derives the
/// non-client colour from the *system* dark mode, so a light app on a dark
/// desktop kept a black title bar (and a dark app on a light desktop a white
/// one). `DWMWA_USE_IMMERSIVE_DARK_MODE` overrides that per window.
pub fn set_titlebar_dark(dark: bool) {
    TITLEBAR_DARK.store(i8::from(dark), Ordering::SeqCst);
    apply_titlebar(dark, true);
}

/// Paint the native title bar in an exact RGB colour (`DWMWA_CAPTION_COLOR`,
/// Windows 11 / build 22000+; older builds ignore it and simply keep the
/// themed caption). Passing the app's page background makes the caption and
/// the window body read as one surface instead of two stacked strips.
pub fn set_titlebar_color(rgb: (u8, u8, u8)) {
    // COLORREF is 0x00BBGGRR, not the usual 0xRRGGBB.
    let packed = i32::from(rgb.2) << 16 | i32::from(rgb.1) << 8 | i32::from(rgb.0);
    TITLEBAR_RGB.store(packed, Ordering::SeqCst);
    apply_caption_color();
}

/// Theme a window's native title bar: dark or light, in the page's own
/// background colour (`bg` of each palette), so the title bar and the window
/// read as one surface, with a fine border around the window.
pub fn theme_caption(dark: bool) {
    set_titlebar_dark(dark);
    // The active look's own page colour (Petal: its toolbar's, which the
    // title bar runs into).
    let p = crate::app::pl();
    let c = if crate::skins::active() == "petal" { p.header_bg } else { p.bg };
    set_titlebar_color((c.r(), c.g(), c.b()));
}

/// Re-apply the last caption theme without touching the window geometry.
pub fn refresh_titlebar() {
    let state = TITLEBAR_DARK.load(Ordering::SeqCst);
    if state >= 0 {
        apply_titlebar(state != 0, false);
    }
}

fn apply_titlebar(dark: bool, reframe: bool) {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let value: i32 = i32::from(dark);
        // The documented switch: attribute 20 on Win10 20H1+, 19 on 1809-1909.
        let value_ptr = &value as *const i32 as *const std::ffi::c_void;
        let size = std::mem::size_of::<i32>() as u32;
        if win32::DwmSetWindowAttribute(hwnd, 20, value_ptr, size) != 0 {
            win32::DwmSetWindowAttribute(hwnd, 19, value_ptr, size);
        }
        // Older builds (and some 10/11 revisions) ignore the DWM attribute but
        // honour these two, which is how winit itself themes the frame. Doing
        // both keeps the caption correct across Windows versions.
        let theme_name: Vec<u16> = if dark {
            "DarkMode_Explorer\0".encode_utf16().collect()
        } else {
            vec![0]
        };
        win32::SetWindowTheme(hwnd, theme_name.as_ptr(), std::ptr::null());
        win32::set_dark_colors(hwnd, dark);
        // The caption colour is a separate attribute: re-assert it here so a
        // Windows-side frame reset does not bring back the default grey.
        apply_caption_color_unsafe(hwnd, dark);
        if reframe {
            // Repaint the non-client area so the switch is immediate.
            win32::SetWindowPos(
                hwnd,
                0,
                0,
                0,
                0,
                0,
                win32::SWP_NOMOVE
                    | win32::SWP_NOSIZE
                    | win32::SWP_NOZORDER
                    | win32::SWP_FRAMECHANGED,
            );
        }
    }
}

fn apply_caption_color() {
    let hwnd = HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    let dark = TITLEBAR_DARK.load(Ordering::SeqCst) == 1;
    unsafe { apply_caption_color_unsafe(hwnd, dark) };
}

/// `DWMWA_CAPTION_COLOR` (35) / `DWMWA_TEXT_COLOR` (36). Both are Windows 11
/// additions: on anything older the calls fail and the themed caption from
/// `DWMWA_USE_IMMERSIVE_DARK_MODE` stands, which is the best that build offers.
unsafe fn apply_caption_color_unsafe(hwnd: isize, dark: bool) {
    let packed = TITLEBAR_RGB.load(Ordering::SeqCst);
    if packed < 0 {
        return;
    }
    let size = std::mem::size_of::<u32>() as u32;
    let caption = packed as u32;
    win32::DwmSetWindowAttribute(
        hwnd,
        35,
        &caption as *const u32 as *const std::ffi::c_void,
        size,
    );
    let text: u32 = if dark { 0x00FF_FFFF } else { 0x0000_0000 };
    win32::DwmSetWindowAttribute(hwnd, 36, &text as *const u32 as *const std::ffi::c_void, size);
    // DWMWA_BORDER_COLOR (34): a visible edge around the window.
    let border: u32 = if dark { 0x003A_2C2A } else { 0x00DB_CEC9 };
    win32::DwmSetWindowAttribute(hwnd, 34, &border as *const u32 as *const std::ffi::c_void, size);
}

#[cfg(windows)]
mod win32 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    /// `WINDOWPLACEMENT` (without the trailing `rcDevice` field newer SDKs add;
    /// the API honours `length`).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct WindowPlacement {
        pub length: u32,
        pub flags: u32,
        pub show_cmd: u32,
        pub min_position: Point,
        pub max_position: Point,
        pub normal_position: Rect,
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        pub fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        pub fn GetWindowLongW(hwnd: isize, index: i32) -> i32;
        pub fn SetWindowLongW(hwnd: isize, index: i32, value: i32) -> i32;
        pub fn GetWindowPlacement(hwnd: isize, placement: *mut WindowPlacement) -> i32;
        pub fn SetWindowPlacement(hwnd: isize, placement: *const WindowPlacement) -> i32;
        pub fn SetForegroundWindow(hwnd: isize) -> i32;
        pub fn BringWindowToTop(hwnd: isize) -> i32;
        pub fn GetForegroundWindow() -> isize;
        pub fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        pub fn AttachThreadInput(attach: u32, attach_to: u32, attach_flag: i32) -> i32;
        pub fn InvalidateRect(hwnd: isize, rect: *const Rect, erase: i32) -> i32;
        pub fn RedrawWindow(hwnd: isize, rect: *const Rect, region: isize, flags: u32) -> i32;
        pub fn IsIconic(hwnd: isize) -> i32;
        pub fn SetActiveWindow(hwnd: isize) -> isize;
        pub fn SetFocus(hwnd: isize) -> isize;
        pub fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
        pub fn SwitchToThisWindow(hwnd: isize, alt_tab: i32);
        pub fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        pub fn FindWindowW(class_name: *const u16, window_name: *const u16) -> isize;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        pub fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const std::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    #[link(name = "uxtheme")]
    extern "system" {
        pub fn SetWindowTheme(
            hwnd: isize,
            sub_app_name: *const u16,
            sub_id_list: *const u16,
        ) -> i32;
    }

    /// `WINDOWCOMPOSITIONATTRIBDATA` for the undocumented dark-colours switch
    /// (`SetWindowCompositionAttribute`), which older Win10 builds honour even
    /// when `DWMWA_USE_IMMERSIVE_DARK_MODE` is missing.
    #[repr(C)]
    pub struct WindowCompositionAttribData {
        attrib: u32,
        data: *mut std::ffi::c_void,
        size: usize,
    }

    pub const WCA_USEDARKMODECOLORS: u32 = 26;

    pub unsafe fn set_dark_colors(hwnd: isize, dark: bool) {
        let mut value: i32 = i32::from(dark);
        let mut data = WindowCompositionAttribData {
            attrib: WCA_USEDARKMODECOLORS,
            data: &mut value as *mut _ as *mut std::ffi::c_void,
            size: std::mem::size_of::<i32>(),
        };
        // This undocumented API is exported by user32.dll on supported
        // Windows versions, but is absent from some Windows SDK import libs.
        // Resolve it at runtime so MSVC builds do not require a static import.
        let user32 = GetModuleHandleW("user32.dll\0".encode_utf16().collect::<Vec<_>>().as_ptr());
        if user32 == 0 {
            return;
        }
        let Some(address) = GetProcAddress(user32, b"SetWindowCompositionAttribute\0".as_ptr()) else {
            return;
        };
        let set_window_composition_attribute: unsafe extern "system" fn(
            isize,
            *mut WindowCompositionAttribData,
        ) -> i32 = std::mem::transmute(address);
        set_window_composition_attribute(hwnd, &mut data);
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentThreadId() -> u32;
        pub fn GetModuleHandleW(name: *const u16) -> isize;
        pub fn GetProcAddress(
            module: isize,
            name: *const u8,
        ) -> Option<unsafe extern "system" fn()>;
        pub fn CreateEventW(
            attributes: *const std::ffi::c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> isize;
        pub fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
        pub fn SetEvent(event: isize) -> i32;
        pub fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    }

    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOWNORMAL: i32 = 1;
    pub const SW_SHOWMINIMIZED: u32 = 2;
    pub const SW_SHOWNOACTIVATE: i32 = 4;
    pub const SW_SHOW: i32 = 5;
    pub const SW_RESTORE: i32 = 9;
    pub const GWL_EXSTYLE: i32 = -20;
    pub const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
    pub const WS_EX_APPWINDOW: i32 = 0x0004_0000;
    pub const SWP_NOSIZE: u32 = 0x0001;
    pub const SWP_NOMOVE: u32 = 0x0002;
    pub const SWP_NOZORDER: u32 = 0x0004;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const SWP_FRAMECHANGED: u32 = 0x0020;
    pub const SWP_SHOWWINDOW: u32 = 0x0040;
    /// Top of the z-order / back to normal for the foreground bounce.
    pub const HWND_TOPMOST: isize = -1;
    pub const HWND_NOTOPMOST: isize = -2;
    /// Virtual key for ALT + `keybd_event` release flag.
    pub const VK_MENU: i32 = 0x12;
    pub const KEYEVENTF_KEYUP: u32 = 0x0002;
    /// `RedrawWindow`: the window itself (not `RDW_INTERNALPAINT`, which a
    /// parked window never turns into a paint) must be marked dirty.
    pub const RDW_INVALIDATE: u32 = 0x0001;
    /// Far outside any monitor: the window keeps painting, nobody sees it.
    pub const OFFSCREEN: i32 = -32000;
    /// `OpenEventW` access right and the "signalled" wait result.
    pub const EVENT_MODIFY_STATE: u32 = 0x0002;
    pub const WAIT_OBJECT_0: u32 = 0;
}

/// Windows already uses the native HWND; portable builds retain the egui context.
pub fn set_context(_: &egui::Context) {}
