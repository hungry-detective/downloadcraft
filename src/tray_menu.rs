//! The tray icon's right-click menu, drawn by the app (Windows).
//!
//! A stock Windows menu keeps a wide empty strip on its right for submenu
//! arrows and shortcut keys, which this menu has none of. This one is still a
//! real Windows popup menu (keyboard, focus and closing work as usual), but
//! every row is owner-drawn: as wide as its own icon and text, painted in the
//! app's dark or light colours.

use std::sync::Mutex;
use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_GRAYED, ODS_SELECTED, ODT_MENU};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// One row of the menu.
#[derive(Clone)]
pub struct Entry {
    /// What a click does (the ids the app already acts on: "tray-show"…).
    pub id: String,
    pub label: String,
    pub enabled: bool,
    /// 32×32 RGBA picture (straight alpha), drawn at 16 px × the screen scale.
    pub icon: Option<Vec<u8>>,
    /// A line between groups (id and label unused).
    pub separator: bool,
}

pub const ICON_PX: u32 = 32;

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static DARK: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
static OWNER: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// The rows the next right-click shows.
pub fn set_entries(entries: Vec<Entry>) {
    if let Ok(mut e) = ENTRIES.lock() {
        *e = entries;
    }
}

/// The app's mode for the menu: 1 = follow Windows, 2 = dark, 3 = light.
pub fn set_mode(mode: i32) {
    DARK.store(mode, std::sync::atomic::Ordering::SeqCst);
}

fn is_dark() -> bool {
    match DARK.load(std::sync::atomic::Ordering::SeqCst) {
        2 => true,
        3 => false,
        _ => windows_apps_dark(),
    }
}

/// Windows' own "app mode" (Settings → Personalisation → Colours).
pub(crate) fn windows_apps_dark() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0".encode_utf16().collect();
    let val: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut data: u32 = 1;
    let mut size: u32 = 4;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut size)
    };
    r == 0 && data == 0
}

const fn rgb(c: u32) -> COLORREF {
    // COLORREF is 0x00BBGGRR.
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

struct Colors {
    bg: COLORREF,
    hover: COLORREF,
    text: COLORREF,
    grey: COLORREF,
    line: COLORREF,
}

/// The app's own colours for the menu (0xRRGGBB): background, hovered row,
/// text, greyed text, separator line. Unset, it uses plain dark or light.
static APP_COLORS: Mutex<Option<[u32; 5]>> = Mutex::new(None);

pub fn set_colors(c: [u32; 5]) {
    if let Ok(mut a) = APP_COLORS.lock() {
        *a = Some(c);
    }
}

fn colors() -> Colors {
    if let Some([bg, hover, text, grey, line]) = APP_COLORS.lock().ok().and_then(|a| *a) {
        return Colors { bg: rgb(bg), hover: rgb(hover), text: rgb(text), grey: rgb(grey), line: rgb(line) };
    }
    if is_dark() {
        Colors { bg: rgb(0x2B2B2B), hover: rgb(0x3D3D3D), text: rgb(0xF2F2F2), grey: rgb(0x8A8A8A), line: rgb(0x454545) }
    } else {
        Colors { bg: rgb(0xF9F9F9), hover: rgb(0xE8E8E8), text: rgb(0x1A1A1A), grey: rgb(0x9A9A9A), line: rgb(0xE0E0E0) }
    }
}

/// The menu font (Windows' own, at the window's scale) and the scale.
unsafe fn menu_font(hwnd: HWND) -> (HFONT, f32) {
    let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
    let scale = dpi as f32 / 96.0;
    let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
    ncm.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
    SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, ncm.cbSize, &mut ncm as *mut _ as *mut _, 0);
    let mut lf = ncm.lfMenuFont;
    // The metrics come at the system DPI; scale to this window's.
    let sys = GetDpiForSystemOr96();
    lf.lfHeight = (lf.lfHeight as f32 * dpi as f32 / sys as f32).round() as i32;
    (CreateFontIndirectW(&lf), scale)
}

#[allow(non_snake_case)]
unsafe fn GetDpiForSystemOr96() -> u32 {
    windows_sys::Win32::UI::HiDpi::GetDpiForSystem().max(96)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

// Row metrics at 100 %: left gutter, icon, gap, right gutter, heights.
const PAD_L: f32 = 10.0;
const ICON: f32 = 16.0;
const GAP: f32 = 12.0;
const PAD_R: f32 = 16.0;
const ROW_H: f32 = 24.0;
const SEP_H: f32 = 7.0;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MEASUREITEM => {
            let mi = &mut *(lparam as *mut MEASUREITEMSTRUCT);
            if mi.CtlType != ODT_MENU {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let entry = ENTRIES.lock().ok().and_then(|e| e.get(mi.itemData).cloned());
            let (font, scale) = menu_font(hwnd);
            let s = |v: f32| (v * scale).round() as u32;
            match entry {
                Some(e) if e.separator => {
                    mi.itemWidth = s(40.0);
                    mi.itemHeight = s(SEP_H);
                }
                Some(e) => {
                    let dc = GetDC(hwnd);
                    let old = SelectObject(dc, font as HGDIOBJ);
                    let text = wide(&e.label);
                    let mut size = SIZE { cx: 0, cy: 0 };
                    GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                    SelectObject(dc, old);
                    ReleaseDC(hwnd, dc);
                    // Windows adds the check-mark column to an owner-drawn
                    // item's width: taken back off, so the row is exactly as
                    // wide as what it shows.
                    let check = (GetSystemMetrics(SM_CXMENUCHECK) - 1).max(0) as u32;
                    let w = s(PAD_L + ICON + GAP) + size.cx as u32 + s(PAD_R);
                    mi.itemWidth = w.saturating_sub(check).max(s(80.0));
                    mi.itemHeight = s(ROW_H).max(size.cy as u32 + s(6.0));
                }
                None => {}
            }
            DeleteObject(font as HGDIOBJ);
            1
        }
        WM_DRAWITEM => {
            let di = &*(lparam as *const DRAWITEMSTRUCT);
            if di.CtlType != ODT_MENU {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let Some(e) = ENTRIES.lock().ok().and_then(|e| e.get(di.itemData).cloned()) else { return 1 };
            let c = colors();
            let (font, scale) = menu_font(hwnd);
            let s = |v: f32| (v * scale).round() as i32;
            let dc = di.hDC;
            let r = di.rcItem;
            let bg = CreateSolidBrush(c.bg);
            FillRect(dc, &r, bg);
            DeleteObject(bg as HGDIOBJ);
            if e.separator {
                let y = (r.top + r.bottom) / 2;
                let line = CreateSolidBrush(c.line);
                let lr = RECT { left: r.left + s(8.0), top: y, right: r.right - s(8.0), bottom: y + s(1.0).max(1) };
                FillRect(dc, &lr, line);
                DeleteObject(line as HGDIOBJ);
                DeleteObject(font as HGDIOBJ);
                return 1;
            }
            let disabled = di.itemState & (ODS_GRAYED | ODS_DISABLED) != 0;
            if di.itemState & ODS_SELECTED != 0 && !disabled {
                // A soft rounded highlight, inset from the edges.
                let brush = CreateSolidBrush(c.hover);
                let old_b = SelectObject(dc, brush as HGDIOBJ);
                let old_p = SelectObject(dc, GetStockObject(NULL_PEN));
                RoundRect(dc, r.left + s(4.0), r.top + s(2.0), r.right - s(4.0) + 1, r.bottom - s(2.0) + 1, s(8.0), s(8.0));
                SelectObject(dc, old_p);
                SelectObject(dc, old_b);
                DeleteObject(brush as HGDIOBJ);
            }
            if let Some(px) = &e.icon {
                let size = s(ICON);
                draw_icon(dc, px, r.left + s(PAD_L), (r.top + r.bottom - size) / 2, size, disabled);
            }
            let old_f = SelectObject(dc, font as HGDIOBJ);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, if disabled { c.grey } else { c.text });
            let mut tr = RECT { left: r.left + s(PAD_L + ICON + GAP), top: r.top, right: r.right - s(PAD_R) + s(4.0), bottom: r.bottom };
            let text = wide(&e.label);
            DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut tr, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS | DT_NOPREFIX);
            SelectObject(dc, old_f);
            DeleteObject(font as HGDIOBJ);
            1
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Paint a straight-alpha RGBA picture (ICON_PX square) at `size` px.
unsafe fn draw_icon(dc: HDC, rgba: &[u8], x: i32, y: i32, size: i32, faded: bool) {
    let n = ICON_PX as i32;
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = n;
    bmi.bmiHeader.biHeight = -n; // top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        return;
    }
    let out = std::slice::from_raw_parts_mut(bits as *mut u8, (n * n * 4) as usize);
    for (i, px) in rgba.chunks_exact(4).enumerate().take((n * n) as usize) {
        let a = if faded { px[3] as u32 * 45 / 100 } else { px[3] as u32 };
        // BGRA, premultiplied (AlphaBlend's AC_SRC_ALPHA).
        out[i * 4] = (px[2] as u32 * a / 255) as u8;
        out[i * 4 + 1] = (px[1] as u32 * a / 255) as u8;
        out[i * 4 + 2] = (px[0] as u32 * a / 255) as u8;
        out[i * 4 + 3] = a as u8;
    }
    let mem = CreateCompatibleDC(dc);
    let old = SelectObject(mem, bmp as HGDIOBJ);
    let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    AlphaBlend(dc, x, y, size, size, mem, 0, 0, n, n, bf);
    SelectObject(mem, old);
    DeleteDC(mem);
    DeleteObject(bmp as HGDIOBJ);
}

// ---- The menu's own window: no light frame, rounded corners. ----
//
// Windows draws a popup menu's frame itself (a light line on Windows 10, a
// DWM border on Windows 11). While our menu is open, a hook catches the menu
// window being made and takes over its frame painting.

static HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static OLD_PROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static RGN_SIZE: Mutex<(i32, i32)> = Mutex::new((0, 0));

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: HWND, attr: u32, value: *const core::ffi::c_void, size: u32) -> i32;
}

unsafe fn is_menu_window(hwnd: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == "#32768"
}

unsafe fn round_corners(hwnd: HWND) {
    if crate::platform::windows_build() >= 22000 {
        // Windows 11: DWM rounds it; drop the border line.
        let round: u32 = 2; // DWMWCP_ROUND
        let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        DwmSetWindowAttribute(hwnd, 33, &round as *const u32 as *const _, 4);
        DwmSetWindowAttribute(hwnd, 34, &none as *const u32 as *const _, 4);
        return;
    }
    // Windows 10: cut the corners with a window region.
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut r);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return;
    }
    if let Ok(mut last) = RGN_SIZE.lock() {
        if *last == (w, h) {
            return;
        }
        *last = (w, h);
    }
    let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as i32;
    let d = 16 * dpi / 96;
    let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
    SetWindowRgn(hwnd, rgn, 1);
}

unsafe extern "system" fn menu_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let old: WNDPROC = std::mem::transmute(OLD_PROC.load(std::sync::atomic::Ordering::SeqCst));
    match msg {
        WM_NCPAINT => {
            // Paint the frame in the menu's own background.
            let dc = GetWindowDC(hwnd);
            let mut wr: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut wr);
            let mut cr: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut cr);
            let mut tl = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut tl);
            let (ox, oy) = (tl.x - wr.left, tl.y - wr.top);
            ExcludeClipRect(dc, ox, oy, ox + cr.right, oy + cr.bottom);
            let all = RECT { left: 0, top: 0, right: wr.right - wr.left, bottom: wr.bottom - wr.top };
            let bg = CreateSolidBrush(colors().bg);
            FillRect(dc, &all, bg);
            DeleteObject(bg as HGDIOBJ);
            ReleaseDC(hwnd, dc);
            0
        }
        WM_WINDOWPOSCHANGED | WM_SHOWWINDOW => {
            let r = CallWindowProcW(old, hwnd, msg, wparam, lparam);
            round_corners(hwnd);
            r
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, OLD_PROC.load(std::sync::atomic::Ordering::SeqCst));
            CallWindowProcW(old, hwnd, msg, wparam, lparam)
        }
        _ => CallWindowProcW(old, hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn cbt_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 {
        let hwnd = wparam as HWND;
        if is_menu_window(hwnd) && GetWindowLongPtrW(hwnd, GWLP_WNDPROC) != menu_proc as *const () as usize as isize {
            if let Ok(mut last) = RGN_SIZE.lock() {
                *last = (0, 0);
            }
            let old = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, menu_proc as *const () as usize as isize);
            OLD_PROC.store(old, std::sync::atomic::Ordering::SeqCst);
        }
    }
    CallNextHookEx(HOOK.load(std::sync::atomic::Ordering::SeqCst) as HHOOK, code, wparam, lparam)
}

/// The hidden window that owns the menu and draws its rows.
unsafe fn owner() -> HWND {
    let have = OWNER.load(std::sync::atomic::Ordering::SeqCst);
    if have != 0 {
        return have as HWND;
    }
    let class = wide("DownloadCraftTrayMenu\0");
    let inst = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: inst,
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), inst, std::ptr::null());
    OWNER.store(hwnd as isize, std::sync::atomic::Ordering::SeqCst);
    hwnd
}

/// Show the menu at the pointer; returns the id of the row picked.
pub fn show_at_cursor() -> Option<String> {
    show_at(None)
}

/// Show the menu at `at` (screen pixels), else at the pointer.
pub fn show_at(at: Option<(i32, i32)>) -> Option<String> {
    let entries = ENTRIES.lock().ok()?.clone();
    if entries.is_empty() {
        return None;
    }
    unsafe {
        let hwnd = owner();
        if hwnd.is_null() {
            return None;
        }
        let menu = CreatePopupMenu();
        for (i, e) in entries.iter().enumerate() {
            let mut flags = MF_OWNERDRAW;
            if e.separator || !e.enabled {
                flags |= MF_GRAYED;
            }
            // Command ids start at 1 (0 means "nothing picked").
            AppendMenuW(menu, flags, i + 1, i as *const u16);
        }
        let bg = CreateSolidBrush(colors().bg);
        let info = MENUINFO { cbSize: std::mem::size_of::<MENUINFO>() as u32, fMask: MIM_BACKGROUND, hbrBack: bg, ..std::mem::zeroed() };
        SetMenuInfo(menu, &info);
        let mut pt = POINT { x: 0, y: 0 };
        match at {
            Some((x, y)) => pt = POINT { x, y },
            None => {
                GetCursorPos(&mut pt);
            }
        }
        // The menu closes when it loses focus only if its owner is in front.
        SetForegroundWindow(hwnd);
        let hook = SetWindowsHookExW(WH_CBT, Some(cbt_hook), std::ptr::null_mut(), windows_sys::Win32::System::Threading::GetCurrentThreadId());
        HOOK.store(hook as isize, std::sync::atomic::Ordering::SeqCst);
        let picked = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_NOANIMATION, pt.x, pt.y, hwnd, std::ptr::null());
        if !hook.is_null() {
            UnhookWindowsHookEx(hook);
        }
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        DeleteObject(bg as HGDIOBJ);
        if picked <= 0 {
            return None;
        }
        entries.get(picked as usize - 1).filter(|e| !e.separator && e.enabled).map(|e| e.id.clone())
    }
}
