use crate::dm::*;
use crate::engine::{download, EngineOpts, SharedState};
use crate::manual::ManState;
use crate::model::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

#[cfg(windows)]
type PlatformTray = tray_icon::TrayIcon;
#[cfg(not(windows))]
type PlatformTray = ();

// ---------- theme ----------
/// Unified geometry: every card, modal, field, and button across ALL
/// windows (main, Add-download dialog, progress) uses this radius.
pub const RADIUS: f32 = 10.0;

/// Unified title bar height (`main_window_titlebar.html` `.title-bar`).
pub const TB_H: f32 = 46.0;

/// Every hand-painted surface reads its colours from the ACTIVE palette
/// (`pl()`) instead of a hard-coded constant, so a theme switch repaints the
/// whole app — not just the stock egui widgets.
///
/// `DARK_PAL` holds the exact values the concept designs specify, so the dark
/// theme (and "system" while Windows is dark) still matches the concept HTML;
/// `LIGHT_PAL` is the same layout in a light key: same accents and geometry,
/// inverted surfaces/text.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Panel background behind the workspace.
    pub bg: egui::Color32,
    /// Cards, menus, fields, buttons.
    pub surface: egui::Color32,
    /// Raised/hovered surface.
    pub surface_lt: egui::Color32,
    /// Menu bar / window header strip.
    pub header_bg: egui::Color32,
    /// List body background.
    pub central_bg: egui::Color32,
    /// 1px card border.
    pub card_bd: egui::Color32,
    pub primary: egui::Color32,
    pub secondary: egui::Color32,
    /// Body text.
    pub txt: egui::Color32,
    /// Secondary text.
    pub dim: egui::Color32,
    pub success: egui::Color32,
    pub warning: egui::Color32,
    pub error: egui::Color32,
    /// Selected row / selection wash + its border.
    pub sel_bg: egui::Color32,
    pub sel_bd: egui::Color32,
    /// Translucent wash on hovered rows, check rows, buttons.
    pub hover_bg: egui::Color32,
    /// Modal (dialog) card fill + border.
    pub modal_bg: egui::Color32,
    pub modal_bd: egui::Color32,
    /// Backdrop behind an open dialog: heavy over a dark app, light enough
    /// that a light app doesn't turn grey.
    pub scrim: egui::Color32,
    /// Status-bar progress track: hairline + empty fill + pending segment.
    pub bar_bd: egui::Color32,
    pub track: egui::Color32,
    /// Warm "notice" text (grabber diagnostics).
    pub notice: egui::Color32,
    // ---- concept tokens (see `crate::dm`) used by the restyled dialogs.
    // The dark values are the concepts' own hex values (in brackets).
    /// Card fill (`#0f0f22`).
    pub dm_card: egui::Color32,
    /// Card border *and* field fill (`#1e293b`).
    pub dm_line: egui::Color32,
    /// Field box border (`#334155`).
    pub dm_box_bd: egui::Color32,
    /// Primary text (`#e2e8f0`).
    pub dm_txt: egui::Color32,
    /// Secondary text (`#94a3b8`).
    pub dm_dim: egui::Color32,
    /// Row hover (`#1c2436`).
    pub dm_hover: egui::Color32,
    /// Raised wash: icon-button hover, header buttons (`#27334a`).
    pub dm_hover2: egui::Color32,
    /// Page/segmented trough, progress rules (`#0a0a14`).
    pub dm_trough: egui::Color32,
    /// Muted label grey — info rows, part indices (`#64748b`).
    pub dm_lbl: egui::Color32,
    /// Table column headers (`#475569`).
    pub dm_head: egui::Color32,
    /// Warning / failed text (`#ff9678`).
    pub dm_warn: egui::Color32,
    // ---- grayscale tokens of the "Download complete" concept ----
    pub cg_bg: egui::Color32,
    pub cg_bd: egui::Color32,
    pub cg_txt: egui::Color32,
    pub cg_dim: egui::Color32,
    pub cg_ctl: egui::Color32,
    pub cg_box_bg: egui::Color32,
    pub cg_box_bd: egui::Color32,
    pub cg_btn_bd: egui::Color32,
    pub cg_box_txt: egui::Color32,
    pub cg_hover: egui::Color32,
}

pub const DARK_PAL: Palette = Palette {
    bg: egui::Color32::from_rgb(18, 19, 26),
    surface: egui::Color32::from_rgb(26, 27, 36),
    surface_lt: egui::Color32::from_rgb(36, 37, 50),
    header_bg: egui::Color32::from_rgb(13, 14, 20),
    // A step brighter than `bg`, as light mode's white cards are on its
    // grey: the sidebar and the list read as panels on the window.
    central_bg: egui::Color32::from_rgb(25, 26, 35),
    card_bd: egui::Color32::from_rgb(46, 48, 64),
    primary: egui::Color32::from_rgb(124, 92, 252),
    secondary: egui::Color32::from_rgb(184, 93, 255),
    txt: egui::Color32::from_rgb(238, 238, 246),
    // Secondary text at 7:1 on the panels (was 140,142,160 — about 5:1).
    dim: egui::Color32::from_rgb(168, 171, 190),
    success: egui::Color32::from_rgb(76, 217, 100),
    warning: egui::Color32::from_rgb(255, 159, 10),
    error: egui::Color32::from_rgb(255, 69, 58),
    sel_bg: egui::Color32::from_rgba_premultiplied(18, 14, 38, 38),
    sel_bd: egui::Color32::from_rgb(124, 92, 252),
    hover_bg: egui::Color32::from_rgba_premultiplied(15, 16, 19, 26),
    modal_bg: egui::Color32::from_rgb(20, 21, 29),
    modal_bd: egui::Color32::from_rgb(45, 47, 65),
    scrim: egui::Color32::from_rgba_premultiplied(0, 0, 0, 160),
    bar_bd: egui::Color32::from_rgba_premultiplied(20, 20, 21, 25),
    track: egui::Color32::from_rgb(0x3A, 0x3B, 0x44),
    notice: egui::Color32::from_rgb(255, 150, 120),
    dm_card: egui::Color32::from_rgb(0x0f, 0x0f, 0x22),
    dm_line: egui::Color32::from_rgb(0x1e, 0x29, 0x3b),
    dm_box_bd: egui::Color32::from_rgb(0x33, 0x41, 0x55),
    dm_txt: egui::Color32::from_rgb(0xe2, 0xe8, 0xf0),
    dm_dim: egui::Color32::from_rgb(0x94, 0xa3, 0xb8),
    dm_hover: egui::Color32::from_rgb(0x1c, 0x24, 0x36),
    dm_hover2: egui::Color32::from_rgb(0x27, 0x33, 0x4a),
    dm_trough: egui::Color32::from_rgb(0x0a, 0x0a, 0x14),
    dm_lbl: egui::Color32::from_rgb(0x64, 0x74, 0x8b),
    dm_head: egui::Color32::from_rgb(0x47, 0x55, 0x69),
    dm_warn: egui::Color32::from_rgb(0xff, 0x96, 0x78),
    // The "Download complete" dialog in the app's own blue-tinted dark
    // (it used neutral greys that did not match the windows around it).
    cg_bg: egui::Color32::from_rgb(18, 19, 26),
    cg_bd: egui::Color32::from_rgb(35, 36, 48),
    cg_txt: egui::Color32::from_rgb(0xe0, 0xe0, 0xf0),
    cg_dim: egui::Color32::from_rgb(0x99, 0x99, 0x99),
    cg_ctl: egui::Color32::from_rgb(0x88, 0x88, 0x88),
    cg_box_bg: egui::Color32::from_rgb(25, 26, 35),
    cg_box_bd: egui::Color32::from_rgb(46, 48, 64),
    cg_btn_bd: egui::Color32::from_rgb(58, 60, 78),
    cg_box_txt: egui::Color32::from_rgb(0xcc, 0xcc, 0xcc),
    cg_hover: egui::Color32::from_rgb(36, 37, 50),
};

pub const LIGHT_PAL: Palette = Palette {
    bg: egui::Color32::from_rgb(0xEE, 0xF0, 0xF6),
    surface: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_lt: egui::Color32::from_rgb(0xE6, 0xE9, 0xF2),
    header_bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    central_bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    card_bd: egui::Color32::from_rgb(0xC8, 0xCD, 0xDC),
    primary: egui::Color32::from_rgb(124, 92, 252),
    secondary: egui::Color32::from_rgb(154, 62, 226),
    txt: egui::Color32::from_rgb(0x20, 0x24, 0x33),
    // Secondary text at 7:1 on white (was 5F6778, about 5.6:1).
    dim: egui::Color32::from_rgb(0x4A, 0x51, 0x60),
    success: egui::Color32::from_rgb(0x1A, 0xA9, 0x4B),
    warning: egui::Color32::from_rgb(0xB4, 0x6A, 0x05),
    error: egui::Color32::from_rgb(0xD9, 0x2B, 0x1E),
    sel_bg: egui::Color32::from_rgba_premultiplied(18, 13, 36, 36),
    sel_bd: egui::Color32::from_rgb(124, 92, 252),
    hover_bg: egui::Color32::from_rgba_premultiplied(1, 2, 3, 18),
    modal_bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    modal_bd: egui::Color32::from_rgb(0xD5, 0xD9, 0xE6),
    scrim: egui::Color32::from_rgba_premultiplied(0, 0, 0, 90),
    bar_bd: egui::Color32::from_rgba_premultiplied(2, 3, 5, 30),
    track: egui::Color32::from_rgb(0xDD, 0xE1, 0xEC),
    notice: egui::Color32::from_rgb(0xB0, 0x4A, 0x08),
    dm_card: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    dm_line: egui::Color32::from_rgb(0xE2, 0xE8, 0xF0),
    dm_box_bd: egui::Color32::from_rgb(0xC7, 0xD2, 0xE1),
    dm_txt: egui::Color32::from_rgb(0x1E, 0x29, 0x3B),
    dm_dim: egui::Color32::from_rgb(0x64, 0x74, 0x8B),
    dm_hover: egui::Color32::from_rgb(0xEF, 0xF3, 0xFA),
    dm_hover2: egui::Color32::from_rgb(0xE1, 0xE8, 0xF2),
    dm_trough: egui::Color32::from_rgb(0xE8, 0xED, 0xF4),
    dm_lbl: egui::Color32::from_rgb(0x5A, 0x68, 0x7E),
    dm_head: egui::Color32::from_rgb(0x7C, 0x89, 0x9C),
    dm_warn: egui::Color32::from_rgb(0xC0, 0x39, 0x0B),
    cg_bg: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
    cg_bd: egui::Color32::from_rgb(0xD2, 0xD6, 0xDF),
    cg_txt: egui::Color32::from_rgb(0x20, 0x24, 0x33),
    cg_dim: egui::Color32::from_rgb(0x6B, 0x72, 0x80),
    cg_ctl: egui::Color32::from_rgb(0x8A, 0x90, 0x9E),
    cg_box_bg: egui::Color32::from_rgb(0xF5, 0xF6, 0xFA),
    cg_box_bd: egui::Color32::from_rgb(0xCF, 0xD4, 0xDE),
    cg_btn_bd: egui::Color32::from_rgb(0xC3, 0xC8, 0xD4),
    cg_box_txt: egui::Color32::from_rgb(0x2E, 0x33, 0x40),
    cg_hover: egui::Color32::from_rgb(0xE9, 0xEC, 0xF3),
};

thread_local! {
    /// Palette of the thread doing the painting. Defaults to `DARK_PAL`, so a
    /// window that never calls `apply_theme` keeps the concept colours.
    static PAL: std::cell::Cell<Palette> = const { std::cell::Cell::new(DARK_PAL) };
}

pub fn set_palette(p: Palette) {
    PAL.with(|c| c.set(p));
}

/// Active palette. Cheap: `Color32` is a plain POD and the cell read is
/// thread-local, so calling this per painted primitive costs nothing.
#[inline]
pub fn pl() -> Palette {
    PAL.with(|c| c.get())
}

/// Linear blend between two colours (`t` = 0 keeps `a`, 1 gives `b`). The
/// restyled dialogs use it for the card's softer hairlines and raised fills.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Translucent wash of `c` (tinted fills, hovers, chips).
fn wash(c: egui::Color32, a: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn visuals_dark(p: &Palette) -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.window_fill = p.surface;
    v.panel_fill = p.bg;
    // Table cells paint faint_bg: keep the list on app background.
    v.faint_bg_color = p.bg;
    v.extreme_bg_color = p.bg;
    v.override_text_color = Some(p.txt);
    v.weak_text_color = Some(p.dim);
    // Hide idle table resize lines + plain separators for a minimal look
    // (hover/drag lines still show while resizing).
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.bg);
    v.selection.bg_fill = p.sel_bg;
    v.selection.stroke = egui::Stroke::new(1.0_f32, p.sel_bd);
    v.widgets.inactive.bg_fill = p.surface;
    v.widgets.hovered.bg_fill = p.surface_lt;
    v.widgets.active.bg_fill = p.track;
    v.widgets.noninteractive.bg_fill = p.surface;
    v
}

fn visuals_light(p: &Palette) -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.window_fill = p.surface;
    v.panel_fill = p.bg;
    v.faint_bg_color = p.bg;
    v.extreme_bg_color = p.surface;
    v.override_text_color = Some(p.txt);
    v.weak_text_color = Some(p.dim);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.bg);
    v.window_stroke = egui::Stroke::new(1.0_f32, p.card_bd);
    v.selection.bg_fill = p.sel_bg;
    v.selection.stroke = egui::Stroke::new(1.0_f32, p.sel_bd);
    // On a white panel an unfilled button needs its edge drawn.
    v.widgets.inactive.bg_fill = p.surface;
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.card_bd);
    v.widgets.hovered.bg_fill = p.surface_lt;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.card_bd);
    v.widgets.active.bg_fill = p.track;
    v.widgets.noninteractive.bg_fill = p.surface;
    v
}

/// Pure-black preset, kept for configs that already selected it.
pub fn black_visuals() -> egui::Visuals {
    let p = &DARK_PAL;
    let mut v = egui::Visuals::dark();
    let black = egui::Color32::BLACK;
    let panel = egui::Color32::from_rgb(0x0C, 0x0C, 0x0E);
    v.window_fill = panel;
    v.panel_fill = black;
    v.faint_bg_color = black;
    v.extreme_bg_color = panel;
    v.override_text_color = Some(p.txt);
    v.weak_text_color = Some(p.dim);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, black);
    v.selection.bg_fill = p.sel_bg;
    v.selection.stroke = egui::Stroke::new(1.0_f32, p.sel_bd);
    v.widgets.inactive.bg_fill = panel;
    v.widgets.hovered.bg_fill = p.surface;
    v.widgets.active.bg_fill = p.surface_lt;
    v.widgets.noninteractive.bg_fill = panel;
    v
}

pub fn dark_visuals() -> egui::Visuals {
    visuals_dark(&DARK_PAL)
}

pub fn light_visuals() -> egui::Visuals {
    visuals_light(&LIGHT_PAL)
}

pub fn visuals_for(theme: &str) -> egui::Visuals {
    match theme {
        "black" => black_visuals(),
        "light" => light_visuals(),
        _ => dark_visuals(),
    }
}

/// Does `theme` ("system" | "dark" | "light" | "black") resolve to a dark
/// palette right now? "system" follows the OS and falls back to dark when the
/// platform reports no theme.
pub fn theme_is_dark(ctx: &egui::Context, theme: &str) -> bool {
    match theme {
        "light" => false,
        "dark" | "black" => true,
        _ => !matches!(
            ctx.input(|i| i.raw.system_theme),
            Some(egui::Theme::Light)
        ),
    }
}

/// Apply a theme to one context: swap the palette every hand-painted surface
/// reads from, then the stock egui visuals and the shared widget geometry.
/// Every window (main, settings, basket, confirm, progress, complete) calls
/// this once at start-up, so they all agree on the theme.
pub fn apply_theme(ctx: &egui::Context, theme: &str) {
    let dark = theme_is_dark(ctx, theme);
    set_palette(if dark { DARK_PAL } else { LIGHT_PAL });
    // Pin egui's own preference: with the default (`System`) an OS theme flip
    // would swap in egui's stock visuals for the other theme, while every
    // hand-painted surface kept OUR palette. Pinned, `set_visuals` below
    // targets the theme we are actually using, and our own check (which
    // carries the OS answer for "system") is the only thing that re-applies.
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    ctx.set_visuals(if theme == "black" {
        black_visuals()
    } else {
        visuals_for(if dark { "dark" } else { "light" })
    });
    apply_widget_theme(ctx);
}

/// Theme key as saved in `ui.json`. For the windows that live in their own
/// process (settings, basket, confirm, progress, complete) — the main app
/// warns them through the shared config file.
/// When this window opened (Unix seconds, fixed on first use, which is the
/// first poll right after start-up). Browser captures and start requests
/// older than this were made while the app was closed.
fn app_opened_at() -> i64 {
    static AT: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *AT.get_or_init(|| chrono::Local::now().timestamp())
}

pub fn stored_theme() -> String {
    config_dir()
        .map(|d| d.join("ui.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("theme")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "dark".to_string())
}

/// Widget-level theme shared by EVERY window: rounded controls, roomy
/// buttons, consistent spacing. Call after each `set_visuals` (theme
/// switches) so visuals + geometry never drift apart again.
pub fn apply_widget_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(RADIUS as u8);
    }
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(16.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(10.0, egui::FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(11.0, egui::FontFamily::Monospace),
    );
    style.spacing.button_padding = egui::Vec2::new(14.0, 10.0);
    style.spacing.item_spacing = egui::Vec2::new(8.0, 10.0);
    style.spacing.icon_width = 14.0;
    ctx.set_style(style);
}

// ---------- Material Icons codepoints (font bundled under assets/fonts) ----------
pub(crate) const MI_SEARCH: &str = "\u{e8b6}";
pub(crate) const MI_PLAY: &str = "\u{e037}";
pub(crate) const MI_STOP: &str = "\u{e047}";
pub(crate) const MI_IMAGE: &str = "\u{e3f4}";
pub(crate) const MI_MUSIC: &str = "\u{e405}";
pub(crate) const MI_GRID: &str = "\u{e9b0}";
/// Nine dots in a 3×3 grid (sidebar "All"; the concept draws its dotted
/// "everything" tile this way, and it keeps All distinct from Apps).
pub(crate) const MI_APPS_DOTS: &str = "\u{e5c3}";
pub(crate) const MI_DOC: &str = "\u{e873}";
pub(crate) const MI_ARCHIVE: &str = "\u{e149}";
pub(crate) const MI_CLOUD_DL: &str = "\u{e2c0}";
pub(crate) const MI_PAUSE: &str = "\u{e034}";
pub(crate) const MI_COPY: &str = "\u{e14d}";
pub(crate) const MI_PUBLIC: &str = "\u{e80b}";
pub(crate) const MI_DELETE: &str = "\u{e872}";
// ---- icons added for the main-window redesign (main_window.html) ----
/// Plus (toolbar "Add URL").
pub(crate) const MI_ADD: &str = "\u{e145}";
/// Rounded rect with a play mark (sidebar "Videos", table Video rows).
pub(crate) const MI_SLIDESHOW: &str = "\u{e41b}";
/// Three shrinking rules (sidebar "Queued").
pub(crate) const MI_FILTER: &str = "\u{e152}";
/// Circle with an X (sidebar "Failed").
pub(crate) const MI_CANCEL: &str = "\u{e5c9}";
/// Circle with a clock (sidebar "Unfinished").
pub(crate) const MI_SCHEDULE: &str = "\u{e8b5}";
/// Circle with a tick (sidebar "Finished").
pub(crate) const MI_CHECK_CIRCLE: &str = "\u{e86c}";
/// Outlined circle with a play mark (toolbar "Resume").
pub(crate) const MI_PLAY_CIRCLE: &str = "\u{e039}";
/// Filled circle with pause bars (toolbar "Stop All").
pub(crate) const MI_PAUSE_CIRCLE: &str = "\u{e035}";
/// Outlined circle with pause bars (toolbar "Stop").
pub(crate) const MI_STOP_CIRCLE: &str = "\u{e1a2}";
/// Three equal rules (toolbar "Open Queue").
pub(crate) const MI_REORDER: &str = "\u{e8fe}";
/// Slider rules (toolbar "Settings").
pub(crate) const MI_TUNE: &str = "\u{e429}";
/// Page with a folded corner (table "Other"/unknown rows).
pub(crate) const MI_INSERT_FILE: &str = "\u{e24d}";
/// Collapsed / expanded affordances for the sidebar sections.
const MI_OPEN: &str = "\u{e89e}";
const MI_SPEED: &str = "\u{e9e4}";
pub(crate) const MI_ARROW_DOWN: &str = "\u{e5db}";
const MI_CHECK: &str = "\u{e5ca}";
/// Popup-menu label prefixes: a ticked choice, and an unticked one in the
/// same group (indented to line up with the ticked one).
const POPUP_CHECK: &str = "\u{1}";
const POPUP_INDENT: &str = "\u{2}";
const MI_FOLDER_OPEN: &str = "\u{e2c8}";
const MI_INFO: &str = "\u{e88e}";
const MI_REPLAY: &str = "\u{e042}";
const MI_TRANSFORM: &str = "\u{e428}";
const MI_UNARCHIVE: &str = "\u{e169}";
const MI_PLAYLIST_ADD: &str = "\u{e03b}";
const MI_LINK: &str = "\u{e157}";
const MI_REFRESH: &str = "\u{e5d5}";
const MI_VERIFIED: &str = "\u{e8e8}";
const MI_DELETE_FOREVER: &str = "\u{e92b}";
pub(crate) const MI_CHEVRON_RIGHT: &str = "\u{e5cc}";
pub(crate) const MI_EXPAND_MORE: &str = "\u{e5cf}";
/// Arrow onto a tray (sidebar "Downloading").
pub(crate) const MI_GET_APP: &str = "\u{e884}";

/// Material Icons (Apache-2.0, Google) as fallback font for UI glyphs, plus
/// the named Segoe UI families the Add-download dialog asks for.
/// Every window (main, confirm, progress, basket, mini) must call this —
/// otherwise icon glyphs render as tofu boxes.
pub fn setup_fonts(cc: &eframe::CreationContext<'_>) {
    // `false`: the manager window keeps its own typography and only the named
    // families ("segoe"/"popup-semibold") are added for the dialog.
    crate::fonts::install(cc, false);
}

fn file_icon(cat: &str) -> &'static str {
    match cat {
        "Image" => MI_IMAGE,
        "Music" => MI_MUSIC,
        "Video" => MI_SLIDESHOW,
        "Apps" => MI_GRID,
        "Document" => MI_DOC,
        "Compressed" => MI_ARCHIVE,
        _ => MI_INSERT_FILE,
    }
}

/// Per-category tint for the list's name-cell marks, taken from the concept
/// (`main_window.html`): blue video, amber archive, green picture, purple
/// music, red document, cyan app. Light enough to read on both themes.
fn cat_color(cat: &str) -> egui::Color32 {
    match cat {
        "Video" => egui::Color32::from_rgb(0x3b, 0x82, 0xf6),
        "Compressed" => egui::Color32::from_rgb(0xf5, 0x9e, 0x0b),
        "Image" => egui::Color32::from_rgb(0x22, 0xc5, 0x5e),
        "Music" => egui::Color32::from_rgb(0xa8, 0x55, 0xf7),
        "Document" => egui::Color32::from_rgb(0xef, 0x44, 0x44),
        "Apps" => egui::Color32::from_rgb(0x06, 0xb6, 0xd4),
        _ => pl().dm_dim,
    }
}

/// Standard modal dialog frame (dark card, 1px border, rounded).
fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(pl().modal_bg)
        .stroke(egui::Stroke::new(1.0_f32, pl().modal_bd))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(16))
}

/// Centered-modal width that can never overflow the viewport: clamped to
/// 90% of the screen (and to a sane min/max), so input fields and buttons
/// always fit, even on narrow windows.
fn modal_width(ctx: &egui::Context, max_w: f32) -> f32 {
    (ctx.screen_rect().width() * 0.90).clamp(320.0, max_w)
}

/// Advanced ("settings") panel height. The concept draws it collapsed, so this
/// part is app design rendered in the concept's own tokens (see `crate::dm`).
const DM_PANEL_H: f32 = 224.0;


/// Concept icon button: hover wash plus glyph. Input stays on the manual hit
/// table, so this only paints.
fn dm_icon_btn(
    p: &egui::Painter,
    man: &ManState,
    rect: egui::Rect,
    icon: DmIcon,
    size: f32,
    w: f32,
    color: egui::Color32,
) {
    crate::dm::icon_btn(p, rect, man.hover(&rect), icon, size, w, color);
}

/// Icon button on the field surface (same fill/border as an input box).
fn dm_icon_btn_box(
    p: &egui::Painter,
    man: &ManState,
    rect: egui::Rect,
    icon: DmIcon,
    size: f32,
    w: f32,
    color: egui::Color32,
) {
    crate::dm::icon_btn_box(p, rect, man.hover(&rect), icon, size, w, color);
}

/// Outline button (concept "Add" / "Cancel").
fn dm_btn_flat(p: &egui::Painter, man: &ManState, r: egui::Rect, label: &str) {
    crate::dm::btn_flat(p, r, man.hover(&r), label);
}


/// Manual slider painted in the concept's tokens. Geometry comes from
/// `slider_track`, so the manual drag handling stays exact.
fn dm_slider(p: &egui::Painter, man: &ManState, rect: egui::Rect, frac: f32) {
    let (tx0, tw) = slider_track(&rect);
    crate::dm::slider(p, rect, man.hover(&rect), frac, tx0, tw);
}


/// Static field keys for the capped dynamic rows (rules ≤8, windows ≤6).
/// `HitKind::Field` needs `&'static str`, so a fixed pool stands in for
/// per-row dynamic ids.
fn rule_ext_key(i: usize) -> &'static str {
    match i {
        0 => "rule_ext_0", 1 => "rule_ext_1", 2 => "rule_ext_2", 3 => "rule_ext_3",
        4 => "rule_ext_4", 5 => "rule_ext_5", 6 => "rule_ext_6",
        _ => "rule_ext_7",
    }
}
fn rule_dir_key(i: usize) -> &'static str {
    match i {
        0 => "rule_dir_0", 1 => "rule_dir_1", 2 => "rule_dir_2", 3 => "rule_dir_3",
        4 => "rule_dir_4", 5 => "rule_dir_5", 6 => "rule_dir_6",
        _ => "rule_dir_7",
    }
}
fn win_start_key(i: usize) -> &'static str {
    match i {
        0 => "win_s_0", 1 => "win_s_1", 2 => "win_s_2",
        3 => "win_s_3", 4 => "win_s_4",
        _ => "win_s_5",
    }
}
fn win_end_key(i: usize) -> &'static str {
    match i {
        0 => "win_e_0", 1 => "win_e_1", 2 => "win_e_2",
        3 => "win_e_3", 4 => "win_e_4",
        _ => "win_e_5",
    }
}
fn win_lim_key(i: usize) -> &'static str {
    match i {
        0 => "win_l_0", 1 => "win_l_1", 2 => "win_l_2",
        3 => "win_l_3", 4 => "win_l_4",
        _ => "win_l_5",
    }
}

/// Full-width sidebar navigation row, following `main_window.html`:
/// 6px/8px padding, 6px radius, 13px icon, 11px label left, pill count right.
/// Selected rows take a raised fill and the accent colour (no bar); hovered
/// rows take a softer wash. Manual-input version: reserves space, paints,
/// returns the hit rect; the whole row is one click target.
fn side_nav_row(
    ui: &mut egui::Ui,
    man: &ManState,
    selected: bool,
    icon: &str,
    label: &str,
    count: Option<usize>,
    dot: bool,
    key: &str,
) -> egui::Rect {
    const H: f32 = 24.0;
    let w = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    let p = ui.painter();
    // Highlight inset by a pixel top and bottom: the rows sit flush (no item
    // spacing) so the visual rhythm still reads as the concept's 2px gap.
    let hl = rect.shrink2(egui::vec2(0.0, 1.0));
    if selected {
        p.rect_filled(hl, 6.0, pl().surface_lt);
    } else if man.hover(&rect) {
        p.rect_filled(hl, 6.0, pl().hover_bg);
    }
    let fg = if selected { pl().txt } else { pl().txt.gamma_multiply(0.72) };
    if dot {
        p.circle_filled(
            egui::pos2(rect.min.x + 15.0, rect.center().y),
            3.5,
            pl().success,
        );
    } else {
        let _ = icon;
        line_icon(p, egui::pos2(rect.min.x + 15.0, rect.center().y), 15.0, key, fg);
    }
    p.text(
        egui::pos2(rect.min.x + 30.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(11.5),
        fg,
    );
    if let Some(n) = count {
        // A plain number, accent on the open filter (no badge).
        p.text(
            egui::pos2(rect.max.x - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            egui::FontId::proportional(10.5),
            if selected { pl().primary } else { pl().dim },
        );
    }
    rect
}

/// Collapsible sidebar section header (`main_window.html` `.sidebar-section`):
/// a chevron plus 9px upper-case letters, and a clickable row that folds the
/// items below it away.
fn side_section(
    ui: &mut egui::Ui,
    man: &ManState,
    title: &str,
    open: bool,
) -> egui::Rect {
    let w = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    let p = ui.painter();
    let hov = man.hover(&rect);
    let c = if hov { pl().txt } else { pl().dim };
    p.text(
        egui::pos2(rect.min.x + 13.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        if open { MI_EXPAND_MORE } else { MI_CHEVRON_RIGHT }.to_string(),
        crate::fonts::icon(13.0),
        c,
    );
    p.text(
        egui::pos2(rect.min.x + 24.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title.to_uppercase(),
        egui::FontId::proportional(9.5),
        c,
    );
    rect
}

/// Small square icon button (Material glyph, guaranteed to render).
fn icon_btn(ui: &mut egui::Ui, glyph: &str, tip: &str) -> egui::Response {
    ui.add_sized(
        [30.0, 30.0],
        egui::Button::new(egui::RichText::new(glyph).size(16.0).color(pl().dim))
            .fill(egui::Color32::TRANSPARENT)
            .corner_radius(RADIUS),
    )
    .on_hover_text(tip)
}

/// A status in two lines that fit the row: the text, and a thin bar under
/// it showing how far the file has come.
fn status_bar_at(p: &egui::Painter, rect: egui::Rect, text: &str, frac: f32, color: egui::Color32) {
    let x0 = rect.min.x + 10.0;
    let x1 = (rect.max.x - 14.0).max(x0 + 20.0);
    let font = egui::FontId::proportional(11.0);
    // "62% Downloading" in a narrow column: the percentage alone reads
    // better than a word cut in half.
    let full = p.layout_no_wrap(text.to_string(), font.clone(), color).size().x;
    let short = text.split_whitespace().next().unwrap_or(text);
    let text = if full > x1 - x0 && short.ends_with('%') { short } else { text };
    let shown = truncate_to_width(p, text, font.clone(), color, x1 - x0);
    p.text(egui::pos2(x0, rect.center().y - 5.0), egui::Align2::LEFT_CENTER, shown, font, color);
    let bar = egui::Rect::from_min_max(egui::pos2(x0, rect.center().y + 7.0), egui::pos2(x1, rect.center().y + 10.0));
    p.rect_filled(bar, 1.5, pl().surface_lt);
    let fw = (bar.width() * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 3.0 } else { 0.0 });
    if fw > 0.0 {
        p.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(fw, bar.height())), 1.5, color);
    }
}

/// A status as a coloured dot and text (the redesign's list style).
fn status_dot_at(p: &egui::Painter, rect: egui::Rect, text: &str, color: egui::Color32) {
    let font = egui::FontId::proportional(11.0);
    let shown = truncate_to_width(p, text, font.clone(), color, (rect.width() - 30.0).max(12.0));
    let tw = p.layout_no_wrap(shown.clone(), font.clone(), color).size().x;
    // Dot (6) + gap (5) + word, the pair centred under the header.
    let x0 = (rect.center().x - (11.0 + tw) / 2.0).max(rect.min.x + 6.0);
    p.circle_filled(egui::pos2(x0 + 3.0, rect.center().y), 3.0, color);
    p.text(egui::pos2(x0 + 11.0, rect.center().y), egui::Align2::LEFT_CENTER, shown, font, color);
}

/// Small rounded status badge, painted into an explicit rect (no cursor use).
/// `right` anchors it to the cell's right edge, matching the concept's
/// right-aligned Status column.
fn status_pill_at(p: &egui::Painter, rect: egui::Rect, text: &str, color: egui::Color32, right: bool) {
    let font = egui::FontId::proportional(10.5);
    let avail = (rect.width() - 16.0).max(18.0);
    let shown = truncate_to_width(p, text, font.clone(), color, avail);
    let galley = p.layout_no_wrap(shown, font, color);
    let h = (galley.size().y + 6.0).max(16.0);
    let pw = (galley.size().x + 16.0).min(rect.width().max(20.0)).max(18.0);
    let cx = if right {
        rect.max.x - 8.0 - pw / 2.0
    } else {
        rect.center().x
    };
    let pr = egui::Rect::from_min_size(
        egui::pos2(cx - pw / 2.0, rect.center().y - h / 2.0),
        egui::vec2(pw, h),
    );
    let bg = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 36);
    p.rect_filled(pr, pr.height() / 2.0, bg);
    p.galley(
        egui::pos2(
            pr.center().x - galley.size().x / 2.0,
            pr.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// Manual checkbox (paint only; input via manual routing).
fn paint_check(p: &egui::Painter, rect: egui::Rect, checked: bool, hov: bool) {
    p.rect_filled(
        rect,
        3.0,
        if checked {
            pl().primary
        } else {
            egui::Color32::TRANSPARENT
        },
    );
    p.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.2, if hov || checked { pl().txt } else { pl().dim }),
        egui::StrokeKind::Inside,
    );
    if checked {
        let c = rect.center();
        p.line_segment(
            [c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.5, 3.0)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );
        p.line_segment(
            [c + egui::vec2(-1.5, 3.0), c + egui::vec2(4.5, -3.5)],
            egui::Stroke::new(1.8, egui::Color32::WHITE),
        );
    }
}

/// Button identity for manual routing (every clickable dialog control).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ButtonId {
    AddPaste,
    AddFolder,
    AddRefresh,
    AddAdv,
    AddAdd,
    AddDownload,
    AddCancel,
    QStart,
    QStop,
    QSchedReset,
    QClose,
    SFolder,
    SChrome,
    SEdge,
    SFirefox,
    SBrave,
    SOpera,
    SClose,
    DResume,
    DClose,
    DOpen,
    DCopyAddr,
    DCopyPage,
    DMove,
    DVerify,
    AbClose,
    ThemeSystem,
    ThemeDark,
    ThemeLight,
    GrabStart,
    GrabQueue,
    GrabClose,
    GrabPreset,
    /// A file-type chip: the n-th preset.
    GrabPresetPick(u8),
    GrabLevelsDown,
    GrabLevelsUp,
    GrabSave,
    RuleAdd,
    RuleDel(usize),
    RuleUp(usize),
    RuleDown(usize),
    RuleReset,
    WinAdd,
    WinDel(usize),
    UpdCheck,
    UpdDownload,
    UpdRestart,
    RfOpen,
    RfApply,
    RfCancel,
}

/// Why a failure means the link itself is dead (and a fresh one from the
/// page would continue the download), in plain words; `None` for failures a
/// new link would not fix (disk full, a cut connection, a checksum).
fn dead_link_reason(reason: &str) -> Option<String> {
    let low = reason.to_ascii_lowercase();
    if low.contains("no such host") || low.contains("dns error") || low.contains("name or service not known") || low.contains("failed to lookup address") {
        Some("the server's address no longer exists.".into())
    } else if low.contains("410") || low.contains("404") || low.contains("not found") {
        Some("the file is no longer at this address (404).".into())
    } else if low.contains("403") || low.contains("expired") || low.contains("forbidden") || low.contains("refused this link") {
        Some("the server refused it (403): download links like this one expire.".into())
    } else if low.contains("web page instead of the file") {
        Some("the server now answers with a web page instead of the file.".into())
    } else {
        None
    }
}

/// The "Refresh download address" window's state.
struct RefreshDlg {
    id: u64,
    name: String,
    /// The page the file came from (Open page), when the row knows it.
    page: String,
    /// Why the window opened by itself (a resume on a dead link).
    reason: Option<String>,
    new_url: String,
    error: Option<String>,
    opened: std::time::Instant,
}

/// Checkbox identity for manual routing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CheckId {
    CatFold,
    Tray,
    Unlimited,
    SchedOn,
    AddOverwrite,
    GrabDepth,
    GrabStay,
    WinOn,
    UpdOn,
}

/// Slider identity for manual routing (values live in self fields/items).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SliderId {
    AddConns,
    LimitKB,
    Timeout,
    Retries,
    DefConns,
    QMax,
}

/// Active manual slider drag.
struct SliderCap {
    id: SliderId,
    min: f64,
    max: f64,
    log: bool,
    x0: f32,
    tw: f32,
}

/// Manual push button: reserves space, paints (+hover), returns hit rect.
/// Primary = filled accent; otherwise dark card. Text centered.
/// A small caps heading over a group of controls in a dialog.
fn msection(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(10.0).strong().color(pl().dim));
}

/// A text field in a visible box (the bare field was hard to see on the
/// dialog's background).
fn mfield(ui: &mut egui::Ui, id: &str, text: &mut String, hint: &str, width: f32) -> egui::Rect {
    egui::Frame::new()
        .fill(pl().surface)
        .stroke(egui::Stroke::new(1.0_f32, pl().card_bd))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            mtext_bare(ui, id, text, hint, (width - 22.0).max(40.0), egui::FontId::proportional(12.5))
        })
        .response
        .rect
}

/// A pill that is either picked (accent) or not.
fn mchip(ui: &mut egui::Ui, man: &ManState, label: &str, selected: bool) -> egui::Rect {
    let font = egui::FontId::proportional(12.0);
    let w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), pl().txt).size().x + 24.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 28.0), egui::Sense::hover());
    let p = ui.painter();
    let hov = man.hover(&rect);
    let (fill, stroke, col) = if selected {
        (pl().primary.gamma_multiply(0.22), pl().primary, pl().txt)
    } else if hov {
        (pl().surface_lt, pl().card_bd, pl().txt)
    } else {
        (pl().surface, pl().card_bd, pl().txt.gamma_multiply(0.8))
    };
    p.rect_filled(rect, 14.0, fill);
    p.rect_stroke(rect, 14.0, egui::Stroke::new(1.0_f32, stroke), egui::StrokeKind::Inside);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, col);
    rect
}

fn mbutton(
    ui: &mut egui::Ui,
    man: &ManState,
    label: &str,
    w: f32,
    h: f32,
    primary: bool,
) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter();
    if primary {
        p.rect_filled(rect, RADIUS, egui::Color32::from_rgb(108, 62, 220));
    } else {
        p.rect_filled(rect, RADIUS, pl().surface);
        if man.hover(&rect) {
            p.rect_filled(rect, RADIUS, pl().surface_lt);
        }
        p.rect_stroke(
            rect,
            RADIUS,
            egui::Stroke::new(1.0, pl().modal_bd),
            egui::StrokeKind::Inside,
        );
    }
    let g = p.layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(13.0),
        if primary { egui::Color32::WHITE } else { pl().txt },
    );
    p.galley(
        egui::pos2(
            rect.center().x - g.size().x / 2.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        if primary { egui::Color32::WHITE } else { pl().txt },
    );
    rect
}

/// `.btn` of the restyled dialogs: 34 px tall, 8 px radius, optional leading
/// Material icon. Painted only — the caller records the hit rect.
fn about_btn(
    p: &egui::Painter,
    man: &ManState,
    x: f32,
    y: f32,
    label: &str,
    glyph: Option<&str>,
    primary: bool,
) -> egui::Rect {
    let pal = pl();
    let gap = 7.0;
    let f_btn = egui::FontId::proportional(12.0);
    let fg0 = if primary { egui::Color32::WHITE } else { pal.dim };
    let iw = glyph.map_or(0.0, |g| {
        p.layout_no_wrap(g.to_owned(), crate::fonts::icon(13.0), fg0)
            .size()
            .x
            + gap
    });
    let tw = p.layout_no_wrap(label.to_owned(), f_btn.clone(), fg0).size().x;
    let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(15.0 + iw + tw + 15.0, 34.0));
    let hov = man.hover(&rect);
    let fg = if !primary && hov { pal.txt } else { fg0 };
    if primary {
        let fill = if hov {
            mix(pal.primary, egui::Color32::WHITE, 0.18)
        } else {
            pal.primary
        };
        p.rect_filled(rect, 8.0, fill);
    } else {
        p.rect(
            rect,
            8.0,
            if hov {
                pal.dm_hover
            } else {
                mix(pal.modal_bg, pal.surface_lt, 0.45)
            },
            egui::Stroke::new(1.0, mix(pal.modal_bg, pal.modal_bd, 0.6)),
            egui::StrokeKind::Inside,
        );
    }
    let mut cx = rect.center().x - (iw + tw) / 2.0;
    if let Some(g) = glyph {
        let ig = p.layout_no_wrap(g.to_owned(), crate::fonts::icon(13.0), fg);
        p.galley(
            egui::pos2(cx, rect.center().y - ig.size().y / 2.0),
            ig,
            fg,
        );
        cx += iw;
    }
    let tg = p.layout_no_wrap(label.to_owned(), f_btn, fg);
    p.galley(
        egui::pos2(cx, rect.center().y - tg.size().y / 2.0),
        tg,
        fg,
    );
    rect
}

/// Manual checkbox row: box + label, full width. Paint only.
fn mcheck(ui: &mut egui::Ui, man: &ManState, label: &str, checked: bool) -> egui::Rect {
    let w = ui.available_width().max(80.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::hover());
    if man.hover(&rect) {
        ui.painter().rect_filled(rect, 4.0, pl().hover_bg);
    }
    let p = ui.painter();
    let br = egui::Rect::from_min_size(
        egui::pos2(rect.min.x + 2.0, rect.center().y - 7.0),
        egui::vec2(14.0, 14.0),
    );
    paint_check(&p, br, checked, man.hover(&br.expand(4.0)));
    p.text(
        egui::pos2(br.max.x + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.0),
        pl().txt,
    );
    rect
}

/// Manual slider: label + track + knob, painted; drag via slider_cap.
fn mslider(
    ui: &mut egui::Ui,
    man: &ManState,
    label: &str,
    minv: f64,
    maxv: f64,
    val: f64,
    log: bool,
) -> egui::Rect {
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::hover());
    let p = ui.painter();
    let frac = if log {
        ((val.max(minv).ln() - minv.ln()) / (maxv.ln() - minv.ln())).clamp(0.0, 1.0)
    } else {
        ((val - minv) / (maxv - minv)).clamp(0.0, 1.0)
    };
    p.text(
        egui::pos2(rect.min.x, rect.min.y + 7.0),
        egui::Align2::LEFT_CENTER,
        label.to_string(),
        egui::FontId::proportional(12.0),
        pl().dim,
    );
    let (tx0, tw) = slider_track(&rect);
    let ty = rect.min.y + 22.0;
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(tx0, ty - 2.0), egui::vec2(tw, 4.0)),
        2.0,
        pl().surface_lt,
    );
    let kx = tx0 + tw * frac as f32;
    if man.hover(&rect) {
        p.circle_filled(
            egui::pos2(kx, ty),
            9.0,
            egui::Color32::from_rgba_unmultiplied(124, 92, 252, 40),
        );
    }
    p.circle_filled(egui::pos2(kx, ty), 7.0, pl().primary);
    rect
}

/// Track geometry for a manual slider (same math in layout and process).
fn slider_track(rect: &egui::Rect) -> (f32, f32) {
    let tx0 = rect.min.x + 4.0;
    let tw = (rect.width() - 8.0).max(40.0);
    (tx0, tw)
}

/// Standard text field with an explicit id (so manual clicks can focus
/// it) — returns its rect for hit recording. Typing flows through normal
/// keyboard events once focused.
fn mtext(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    password: bool,
) -> egui::Rect {
    let te = egui::TextEdit::singleline(text)
        .id(egui::Id::new(id))
        .hint_text(hint)
        .password(password)
        .desired_width(width);
    ui.add(te).rect
}

/// Frameless single-line field for use *inside* a caller-painted pill.
///
/// The manager's search sits in its own rounded pill, so the field must not
/// draw a second inner box (that read as "a box inside the search box").
fn mtext_bare(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    font: egui::FontId,
) -> egui::Rect {
    let te = egui::TextEdit::singleline(text)
        .id(egui::Id::new(id))
        .hint_text(hint)
        .desired_width(width)
        .font(font)
        .margin(egui::Margin::ZERO)
        .frame(false);
    ui.add(te).rect
}

// ---------- compact file-properties dialog (file_properties.html) ----------
//
// One short page, no tabs: title bar (file icon + name + X), right-aligned
// 70 px labels, a slim live progress line, and the concept's bottom bar.
// Every control is painted here and dispatched through the manual hit
// router, so the dialog inherits the app's theme in both light and dark.

/// Concept width (`file_properties.html` `.dialog { width: 480px }`).
const DP_W: f32 = 480.0;
/// Label column of the redesigned `.row`s (mock `grid-template-columns: 76px`).
const DP_ROW_LABEL: f32 = 76.0;
const DP_ROW_H: f32 = 16.0;

/// One clipped value cell (mock `.field-value`). Always occupies exactly
/// `width`, so every row can line up on one right edge.
fn dp_value(ui: &mut egui::Ui, text: &str, color: egui::Color32, size: f32, width: f32) {
    let w = width.max(24.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, DP_ROW_H), egui::Sense::hover());
    let p = ui.painter();
    let font = crate::fonts::reg(size);
    let shown = truncate_to_width(p, text, font.clone(), color, w);
    p.text(
        egui::pos2(rect.min.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        color,
    );
}

/// 20×20 copy affordance (mock `.copy-btn`).
fn dp_icon(ui: &mut egui::Ui, man: &ManState, glyph: &str) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
    let hovered = man.hover(&rect);
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, 4.0, pl().surface_lt);
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph.to_string(),
        crate::fonts::icon(12.0),
        if hovered { pl().primary } else { pl().dim },
    );
    rect
}

/// Small bordered button (mock `.btn-small`): 20 px tall, 10 px label.
fn dp_small(ui: &mut egui::Ui, man: &ManState, label: &str, w: f32) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    let hovered = man.hover(&rect);
    let p = ui.painter();
    p.rect_filled(rect, 5.0, if hovered { pl().surface_lt } else { pl().surface });
    p.rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, pl().modal_bd),
        egui::StrokeKind::Inside,
    );
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label.to_string(),
        crate::fonts::reg(10.0),
        if hovered { pl().txt } else { pl().dim },
    );
    rect
}

/// Bottom-bar button (mock `.btn`): 74×28, 6 px radius.
fn dp_button(ui: &mut egui::Ui, man: &ManState, label: &str, primary: bool) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(74.0, 28.0), egui::Sense::hover());
    let hovered = man.hover(&rect);
    let p = ui.painter();
    if primary {
        p.rect_filled(rect, 6.0, pl().primary);
    } else {
        p.rect_filled(rect, 6.0, if hovered { pl().surface_lt } else { pl().surface });
        p.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, pl().modal_bd),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label.to_string(),
        crate::fonts::reg(11.0),
        if primary { egui::Color32::WHITE } else { pl().txt },
    );
    rect
}

/// Compact input (mock `.input`, or the mock's borderless `.auth-input` with
/// an underline). Returns the hit rect for the manual focus router.
fn dp_input(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    width: f32,
    password: bool,
    underline: bool,
) -> egui::Rect {
    // `desired_width` is the inner text width; the frame margin sits outside
    // it, so subtract it to make the *outer* rect exactly `width` wide.
    let inset_x = if underline { 0.0 } else { 16.0 };
    let rect = ui
        .scope(|ui| {
            let v = ui.visuals_mut();
            for w in [
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = egui::CornerRadius::same(5);
            }
            v.widgets.inactive.bg_stroke = egui::Stroke::new(
                1.0,
                if underline {
                    egui::Color32::TRANSPARENT
                } else {
                    pl().modal_bd
                },
            );
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, pl().primary);
            v.widgets.active.bg_stroke = egui::Stroke::new(1.0, pl().primary);
            ui.spacing_mut().interact_size.y = if underline { 18.0 } else { 22.0 };
            let mut te = egui::TextEdit::singleline(text)
                .id(egui::Id::new(id))
                .hint_text(hint)
                .password(password)
                .font(crate::fonts::reg(11.0))
                .background_color(if underline { pl().modal_bg } else { pl().surface })
                .margin(egui::Margin::symmetric(
                    if underline { 0 } else { 8 },
                    if underline { 2 } else { 3 },
                ))
                .desired_width((width - inset_x).max(24.0));
            if underline {
                te = te.frame(false);
            }
            ui.add(te).rect
        })
        .inner;
    if underline {
        ui.painter().line_segment(
            [
                egui::pos2(rect.min.x, rect.max.y + 1.0),
                egui::pos2(rect.max.x, rect.max.y + 1.0),
            ],
            egui::Stroke::new(1.0, pl().modal_bd),
        );
    }
    rect
}

/// 76 px left-aligned label column of the redesigned rows (mock `.row-label`).
fn dp_lbl(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(DP_ROW_LABEL, DP_ROW_H), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.min.x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        crate::fonts::reg(10.5),
        pl().dim,
    );
}

/// Hairline divider in an explicit tone (the redesigned card's `--border-soft`).
fn dp_rule_soft(ui: &mut egui::Ui, color: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 1.0),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [r.left_center(), r.right_center()],
        egui::Stroke::new(1.0, color),
    );
}

/// A byte count with thousands separators, like the mock's "76,833,799".
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*ch as char);
    }
    out
}

/// "MP4 Video File", "ZIP Archive", ... for the mock's Type row.
fn dp_type_label(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let kind = match crate::model::category_of(filename) {
        "Image" => "Image File",
        "Music" => "Audio File",
        "Video" => "Video File",
        "Document" => "Document",
        "Compressed" => "Archive",
        "Apps" => "Application",
        _ => "File",
    };
    if ext.is_empty() || ext == filename.to_ascii_uppercase() {
        kind.to_string()
    } else {
        format!("{ext} {kind}")
    }
}

/// A captured referer that is an HTML page (not a media/CDN file) fills the
/// concept's "Web page" row; media URLs stay in "Referer".
fn page_url_of(referer: &str) -> String {
    let r = referer.trim();
    if !(r.starts_with("http://") || r.starts_with("https://")) {
        return String::new();
    }
    let path = r.split(['?', '#']).next().unwrap_or(r).to_ascii_lowercase();
    let file = path.rsplit('/').next().unwrap_or("");
    let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    const MEDIA: &[&str] = &[
        "mp4", "mkv", "webm", "mov", "avi", "m4v", "mp3", "m4a", "aac", "ogg", "wav", "flac",
        "m3u8", "mpd", "jpg", "jpeg", "png", "gif", "webp", "vtt", "srt",
    ];
    if MEDIA.contains(&ext) {
        String::new()
    } else {
        r.to_string()
    }
}

/// Small square icon button (paint only; input via manual routing).
fn micon(ui: &mut egui::Ui, man: &ManState, glyph: &str) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    if man.hover(&rect) {
        ui.painter().rect_filled(rect, 6.0, pl().surface_lt);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph.to_string(),
        crate::fonts::icon(16.0),
        pl().dim,
    );
    rect
}

/// Focus a text field by its explicit id (manual click routing).
fn focus_field(ctx: &egui::Context, id: &str) {
    ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
}

/// Manual mini progress bar (paint only).
fn paint_progress(p: &egui::Painter, center_x: f32, top_y: f32, width: f32, frac: f32) {
    let w = width.max(20.0);
    let bar = egui::Rect::from_min_size(egui::pos2(center_x - w / 2.0, top_y), egui::vec2(w, 6.0));
    p.rect_filled(bar, 3.0, pl().surface);
    let fw = (w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 5.0 } else { 0.0 });
    if fw > 0.0 {
        p.rect_filled(
            egui::Rect::from_min_size(bar.min, egui::vec2(fw, 6.0)),
            3.0,
            pl().primary,
        );
    }
}

/// One-line cell text anchored to the cell's 8px padding edge — left for the
/// Name column, right for Size / Status / Speed / Time / Date, as the concept
/// does. Overflowing text is ellipsised so it never bleeds into the next
/// column.
fn cell_text(
    p: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    size: f32,
    color: egui::Color32,
    right: bool,
) {
    use egui::Align2;
    const PAD: f32 = 8.0;
    let avail = (rect.width() - PAD * 2.0).max(12.0);
    let shown = truncate_to_width(p, text, egui::FontId::proportional(size), color, avail);
    let (pos, align) = if right {
        (egui::pos2(rect.max.x - PAD, rect.center().y), Align2::RIGHT_CENTER)
    } else {
        (egui::pos2(rect.min.x + PAD, rect.center().y), Align2::LEFT_CENTER)
    };
    crate::shaped::draw(p, pos, align, &shown, egui::FontId::proportional(size), color);
}

/// Text centred in a cell (the Size, Speed and Time Left values).
fn cell_text_center(p: &egui::Painter, rect: egui::Rect, text: &str, size: f32, color: egui::Color32) {
    let avail = (rect.width() - 12.0).max(12.0);
    let shown = truncate_to_width(p, text, egui::FontId::proportional(size), color, avail);
    crate::shaped::draw(p, rect.center(), egui::Align2::CENTER_CENTER, &shown, egui::FontId::proportional(size), color);
}

/// A column header as icon + title, the pair centred in the cell; the
/// icon is dropped when the column is too narrow for both.
fn header_icon_text(p: &egui::Painter, rect: egui::Rect, icon: &str, text: &str, color: egui::Color32) {
    const GAP: f32 = 5.0;
    let font = egui::FontId::proportional(9.5);
    let icon_font = crate::fonts::icon(12.5);
    let iw = p.layout_no_wrap(icon.to_string(), icon_font.clone(), color).size().x;
    let avail = (rect.width() - 12.0).max(12.0);
    let tw = p.layout_no_wrap(text.to_string(), font.clone(), color).size().x;
    if iw + GAP + tw > avail {
        let shown = truncate_to_width(p, text, font.clone(), color, avail);
        crate::shaped::draw(p, rect.center(), egui::Align2::CENTER_CENTER, &shown, font, color);
        return;
    }
    let x0 = rect.center().x - (iw + GAP + tw) / 2.0;
    p.text(egui::pos2(x0, rect.center().y), egui::Align2::LEFT_CENTER, icon, icon_font, color);
    crate::shaped::draw(p, egui::pos2(x0 + iw + GAP, rect.center().y), egui::Align2::LEFT_CENTER, text, font, color);
}

/// Truncate `text` with an ellipsis so it fits `max_w` (painter-measured).
///
/// Widths come from `shaped` so the measurement matches what is drawn for the
/// scripts egui cannot shape itself, and the cut lands on grapheme boundaries:
/// slicing a Bengali consonant away from its matra leaves a dangling mark.
pub(crate) fn truncate_to_width(
    p: &egui::Painter,
    text: &str,
    font: egui::FontId,
    _color: egui::Color32,
    max_w: f32,
) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if crate::shaped::width(p, text, &font) <= max_w {
        return text.to_owned();
    }
    let parts: Vec<&str> = text.graphemes(true).collect();
    let mut lo = 0usize;
    let mut hi = parts.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let w = crate::shaped::width(p, &(parts[..mid].concat() + "…"), &font);
        if w <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    parts[..lo].concat() + "…"
}

/// One-colour outline icons for categories and states, drawn as strokes so
/// they follow the theme's text colour in dark and light alike (the icon
/// font only has filled glyphs). `key` is a category or state name.
pub(crate) fn line_icon(p: &egui::Painter, center: egui::Pos2, size: f32, key: &str, color: egui::Color32) {
    let k = size / 16.0;
    let o = center - egui::vec2(8.0 * k, 8.0 * k);
    let pt = |x: f32, y: f32| o + egui::vec2(x * k, y * k);
    let st = egui::Stroke::new(1.35_f32 * k.max(0.9), color);
    let line = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(pts.iter().map(|&(x, y)| pt(x, y)).collect(), st));
    };
    let closed = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::closed_line(pts.iter().map(|&(x, y)| pt(x, y)).collect(), st));
    };
    let rrect = |x0: f32, y0: f32, x1: f32, y1: f32, rad: f32| {
        p.rect_stroke(egui::Rect::from_min_max(pt(x0, y0), pt(x1, y1)), rad * k, st, egui::StrokeKind::Middle);
    };
    let circle = |x: f32, y: f32, rad: f32| {
        p.circle_stroke(pt(x, y), rad * k, st);
    };
    let page = || {
        closed(&[(4.0, 1.5), (10.0, 1.5), (13.0, 4.5), (13.0, 14.5), (4.0, 14.5)]);
        line(&[(10.0, 1.5), (10.0, 4.5), (13.0, 4.5)]);
    };
    match key {
        "Image" => {
            rrect(1.5, 2.5, 14.5, 13.5, 2.0);
            circle(5.5, 6.0, 1.3);
            line(&[(2.0, 12.0), (6.0, 8.5), (9.0, 11.0), (11.0, 9.0), (14.0, 11.5)]);
        }
        "Music" => {
            line(&[(6.0, 12.0), (6.0, 3.5), (13.5, 2.0), (13.5, 10.5)]);
            line(&[(6.0, 6.0), (13.5, 4.5)]);
            circle(4.2, 12.2, 1.9);
            circle(11.7, 10.7, 1.9);
        }
        "Video" => {
            rrect(1.5, 4.0, 10.5, 12.0, 2.0);
            closed(&[(10.5, 7.0), (14.5, 4.8), (14.5, 11.2), (10.5, 9.0)]);
        }
        "Apps" => {
            rrect(2.0, 2.0, 7.0, 7.0, 1.2);
            rrect(9.0, 2.0, 14.0, 7.0, 1.2);
            rrect(2.0, 9.0, 7.0, 14.0, 1.2);
            rrect(9.0, 9.0, 14.0, 14.0, 1.2);
        }
        "Document" => {
            page();
            line(&[(6.0, 8.0), (11.0, 8.0)]);
            line(&[(6.0, 11.0), (11.0, 11.0)]);
        }
        "Compressed" => {
            page();
            line(&[(8.5, 2.0), (8.5, 3.0)]);
            line(&[(8.5, 4.5), (8.5, 5.5)]);
            line(&[(8.5, 7.0), (8.5, 8.0)]);
            rrect(7.3, 9.3, 9.7, 12.3, 0.6);
        }
        "All" => {
            closed(&[(1.5, 3.5), (6.0, 3.5), (7.5, 5.0), (14.5, 5.0), (14.5, 13.0), (1.5, 13.0)]);
        }
        "Downloading" => {
            line(&[(8.0, 2.0), (8.0, 10.5)]);
            line(&[(4.5, 7.0), (8.0, 10.5), (11.5, 7.0)]);
            line(&[(3.0, 13.5), (13.0, 13.5)]);
        }
        "Queued" => {
            line(&[(3.0, 4.0), (13.0, 4.0)]);
            line(&[(3.0, 8.0), (13.0, 8.0)]);
            line(&[(3.0, 12.0), (9.0, 12.0)]);
        }
        "Paused" => {
            circle(8.0, 8.0, 6.2);
            line(&[(6.5, 5.5), (6.5, 10.5)]);
            line(&[(9.5, 5.5), (9.5, 10.5)]);
        }
        "Completed" => {
            circle(8.0, 8.0, 6.2);
            line(&[(5.2, 8.2), (7.2, 10.2), (10.9, 6.2)]);
        }
        "Failed" => {
            circle(8.0, 8.0, 6.2);
            line(&[(5.8, 5.8), (10.2, 10.2)]);
            line(&[(10.2, 5.8), (5.8, 10.2)]);
        }
        "Unfinished" | "Incomplete" => {
            circle(8.0, 8.0, 6.2);
            line(&[(8.0, 4.8), (8.0, 8.0), (10.4, 9.6)]);
        }
        // ---- Settings rail (same strokes, so the two windows match) ----
        "Appearance" => {
            // A palette: rounded blob with a thumb hole and three paint dots.
            closed(&[(8.0, 1.8), (12.6, 3.4), (14.4, 7.6), (12.8, 11.2), (10.2, 11.2), (9.2, 12.6), (9.6, 14.2), (6.6, 14.3), (2.8, 12.0), (1.6, 7.8), (3.4, 3.6)]);
            circle(5.0, 7.0, 1.0);
            circle(8.0, 4.6, 1.0);
            circle(11.2, 6.4, 1.0);
        }
        "Engine" => {
            closed(&[(9.5, 1.5), (3.5, 9.0), (7.8, 9.0), (6.5, 14.5), (12.5, 6.8), (8.2, 6.8)]);
        }
        "Plugins" => {
            // A puzzle piece: one outline with a knob on top and on the right.
            let mut pts: Vec<(f32, f32)> = vec![(2.0, 5.0), (5.6, 5.0)];
            let knob = |pts: &mut Vec<(f32, f32)>, cx: f32, cy: f32, a0: f32, a1: f32| {
                for i in 0..=10 {
                    let a = (a0 + (a1 - a0) * i as f32 / 10.0).to_radians();
                    pts.push((cx + 1.75 * a.cos(), cy + 1.75 * a.sin()));
                }
            };
            knob(&mut pts, 7.35, 4.2, 150.0, 390.0);
            pts.extend([(9.1, 5.0), (12.2, 5.0), (12.2, 7.4)]);
            knob(&mut pts, 13.0, 9.15, 240.0, 480.0);
            pts.extend([(12.2, 10.9), (12.2, 14.0), (2.0, 14.0)]);
            closed(&pts);
        }
        // ---- Plugins page cards ----
        "ytdlp" => {
            // A video with a download arrow under it.
            rrect(1.5, 1.8, 14.5, 10.2, 2.0);
            closed(&[(6.6, 4.0), (10.2, 6.0), (6.6, 8.0)]);
            line(&[(8.0, 11.4), (8.0, 15.0)]);
            line(&[(6.2, 13.3), (8.0, 15.0), (9.8, 13.3)]);
        }
        "ffmpeg" => {
            // A film strip.
            rrect(1.5, 2.5, 14.5, 13.5, 2.0);
            line(&[(4.8, 2.5), (4.8, 13.5)]);
            line(&[(11.2, 2.5), (11.2, 13.5)]);
            line(&[(1.5, 8.0), (4.8, 8.0)]);
            line(&[(11.2, 8.0), (14.5, 8.0)]);
            line(&[(1.5, 5.2), (4.8, 5.2)]);
            line(&[(11.2, 5.2), (14.5, 5.2)]);
            line(&[(1.5, 10.8), (4.8, 10.8)]);
            line(&[(11.2, 10.8), (14.5, 10.8)]);
        }
        "Chromium" => {
            // Chrome / Edge / Brave / Opera: the ring, the hub and three spokes.
            circle(8.0, 8.0, 6.3);
            circle(8.0, 8.0, 2.5);
            for a in [-90.0f32, 30.0, 150.0] {
                let r = a.to_radians();
                let (sx, sy) = (8.0 + 2.5 * r.cos(), 8.0 + 2.5 * r.sin());
                let d = (a + 90.0).to_radians();
                line(&[(sx, sy), (sx + 5.4 * d.cos(), sy + 5.4 * d.sin())]);
            }
        }
        "Firefox" => {
            // The fox curled round the globe: a ring open at the upper left,
            // where its ear rises out of it, and its tail across the middle.
            // A fox's head: two pointed ears, the face narrowing to the
            // snout, two eyes. The logo's own swirl does not survive 16 px.
            closed(&[(2.0, 2.2), (5.6, 5.0), (10.4, 5.0), (14.0, 2.2), (13.6, 8.4), (8.0, 14.2), (2.4, 8.4)]);
            p.circle_filled(pt(5.9, 8.2), 0.95 * k, color);
            p.circle_filled(pt(10.1, 8.2), 0.95 * k, color);
            line(&[(6.8, 11.0), (8.0, 11.9), (9.2, 11.0)]);
        }
        "Browser" => {
            circle(8.0, 8.0, 6.3);
            line(&[(1.8, 8.0), (14.2, 8.0)]);
            line(&[(8.0, 1.7), (5.6, 5.0), (5.2, 8.0), (5.6, 11.0), (8.0, 14.3)]);
            line(&[(8.0, 1.7), (10.4, 5.0), (10.8, 8.0), (10.4, 11.0), (8.0, 14.3)]);
        }
        "Rules" => {
            line(&[(2.0, 4.0), (14.0, 4.0)]);
            line(&[(2.0, 8.0), (14.0, 8.0)]);
            line(&[(2.0, 12.0), (14.0, 12.0)]);
            p.circle_filled(pt(5.0, 4.0), 1.7 * k, color);
            p.circle_filled(pt(10.5, 8.0), 1.7 * k, color);
            p.circle_filled(pt(7.0, 12.0), 1.7 * k, color);
        }
        "Updates" => {
            line(&[(13.2, 6.0), (12.0, 3.8), (9.6, 2.3), (6.6, 2.2), (3.8, 3.8), (2.3, 6.6), (2.4, 9.6)]);
            line(&[(13.2, 2.4), (13.2, 6.0), (9.8, 6.0)]);
            line(&[(2.8, 10.0), (4.0, 12.2), (6.4, 13.7), (9.4, 13.8), (12.2, 12.2), (13.7, 9.4), (13.6, 6.4)]);
            line(&[(2.8, 13.6), (2.8, 10.0), (6.2, 10.0)]);
        }
        "Proxy" => {
            // Two linked boxes: your machine and the relay.
            rrect(1.5, 2.0, 7.0, 6.5, 1.2);
            rrect(9.0, 9.5, 14.5, 14.0, 1.2);
            line(&[(4.25, 6.5), (4.25, 11.75), (9.0, 11.75)]);
            line(&[(7.0, 4.25), (11.75, 4.25), (11.75, 9.5)]);
        }
        "Sounds" => {
            closed(&[(2.0, 6.0), (5.0, 6.0), (9.0, 2.5), (9.0, 13.5), (5.0, 10.0), (2.0, 10.0)]);
            line(&[(11.2, 5.6), (12.0, 7.0), (12.0, 9.0), (11.2, 10.4)]);
            line(&[(13.0, 3.6), (14.3, 6.0), (14.3, 10.0), (13.0, 12.4)]);
        }
        _ => page(),
    }
}

/// Room the thin scroll bar's strip takes beside the rows: margin, bar,
/// margin (see [`modern_scrollbars`]).
pub(crate) const SB_SPACE: f32 = 8.0;

/// Thin scroll bars: no track, a 4 px soft handle that takes the accent
/// while dragged. They sit in their own strip beside / below the rows
/// (floating over the rows let the last row show through the bar).
/// Applies to the scroll areas this `ui` creates next.
pub(crate) fn modern_scrollbars(ui: &mut egui::Ui) {
    let st = ui.style_mut();
    st.spacing.scroll = egui::style::ScrollStyle {
        floating: false,
        bar_width: 4.0,
        floating_width: 4.0,
        floating_allocated_width: 0.0,
        handle_min_length: 28.0,
        bar_inner_margin: 2.0,
        bar_outer_margin: 2.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 1.0,
        active_handle_opacity: 1.0,
        interact_handle_opacity: 1.0,
    };
    let v = &mut st.visuals;
    // egui lets scroll content paint a few pixels past its viewport; above
    // the list that put rows over the column titles while scrolling.
    v.clip_rect_margin = 0.0;
    v.extreme_bg_color = egui::Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = pl().dim.gamma_multiply(0.30);
    v.widgets.hovered.bg_fill = pl().dim.gamma_multiply(0.55);
    v.widgets.active.bg_fill = pl().primary;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = egui::CornerRadius::same(2);
    }
}

/// Name cell painted with ZERO widgets (icon + filename via the painter
/// only). egui labels always interact — even hover-only ones — which steals
/// row hover/clicks from the text; painted text cannot. A tinted category
/// mark and the file name on one line: the sidebar and the icon already say
/// the category, a second line repeating it was noise.
fn name_cell(p: &egui::Painter, rect: egui::Rect, icon: &str, fname: &str, cat: &str) {
    // One-colour outline mark, as in AB Download Manager: it follows the
    // theme instead of a colour per category.
    let _ = icon;
    line_icon(p, egui::pos2(rect.min.x + 17.0, rect.center().y), 16.0, cat, pl().txt.gamma_multiply(0.7));
    let tx = rect.min.x + 32.0;
    let max_tw = (rect.max.x - 8.0 - tx).max(20.0);
    let name = truncate_to_width(p, fname, egui::FontId::proportional(12.0), pl().txt, max_tw);
    // The file name may be in any script, so it is drawn shaped.
    crate::shaped::draw(
        p,
        egui::pos2(tx, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &name,
        egui::FontId::proportional(12.0),
        pl().txt,
    );
}

/// Default / minimum widths per column.
fn col_default(c: SortCol) -> f32 {
    match c {
        SortCol::Name => 200.0,
        SortCol::Size => SIZE_W,
        SortCol::Status => STATUS_W,
        SortCol::Speed => SPEED_W,
        SortCol::Eta => ETA_W,
        SortCol::Date => DATE_W,
    }
}

fn col_min(c: SortCol) -> f32 {
    match c {
        SortCol::Name => 160.0,
        SortCol::Size => 60.0,
        SortCol::Status => 84.0,
        SortCol::Speed => 56.0,
        SortCol::Eta => 56.0,
        SortCol::Date => 80.0,
    }
}

fn col_w_of(map: &std::collections::HashMap<SortCol, f32>, c: SortCol) -> f32 {
    map.get(&c).copied().unwrap_or(col_default(c))
}

/// The width of `col` while its right-hand divider is dragged by `dx`, as in
/// IDM: only that column changes, every column to its right moves along, and
/// the list scrolls sideways once the columns are wider than the window.
fn dragged_width(col: SortCol, start_w: f32, dx: f32) -> f32 {
    (start_w + dx).clamp(col_min(col), 900.0)
}

/// Uniform toolbar metrics: every action button shares one height.
const TOOL_H: f32 = 32.0;
/// Gap between toolbar buttons (`.toolbar { gap: 4px }`).
const TOOL_GAP: f32 = 4.0;

/// Toolbar button: icon above label, content-sized width, uniform height.
/// Manual-input version: reserves space, paints (hover from caller),
/// returns the hit rect for the caller to record. Never reads responses.
///
/// The width follows the concept's `.tool-btn` (`padding: 6px 8px;
/// min-width: 55px`): the label decides it, so the buttons stay packed
/// together at the left of the bar instead of stretching across the card.
///
/// The only fill a button ever carries is the hover wash — no action is
/// pre-highlighted, so nothing reads as permanently selected.
fn tool_button(
    ui: &mut egui::Ui,
    man: &ManState,
    icon: &str,
    label: &str,
    enabled: bool,
    tip: &str,
) -> egui::Rect {
    // Icon beside the label, 32 px tall; an empty label makes an icon-only
    // square (Settings at the far end).
    let font = egui::FontId::proportional(12.0);
    let label_w = if label.is_empty() {
        0.0
    } else {
        ui.painter().layout_no_wrap(label.to_owned(), font.clone(), pl().txt).size().x
    };
    let w = if label.is_empty() { TOOL_H } else { 10.0 + 16.0 + 7.0 + label_w + 12.0 };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, TOOL_H), egui::Sense::hover());
    let hov = enabled && man.hover(&rect);
    let p = ui.painter();
    if hov {
        p.rect_filled(rect, 8.0, pl().surface_lt);
    }
    let c = if !enabled {
        pl().dim.gamma_multiply(0.6)
    } else if hov {
        pl().txt
    } else {
        pl().txt.gamma_multiply(0.75)
    };
    if label.is_empty() {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, icon.to_string(), crate::fonts::icon(17.0), c);
    } else {
        p.text(
            egui::pos2(rect.min.x + 18.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon.to_string(),
            crate::fonts::icon(16.0),
            c,
        );
        p.text(
            egui::pos2(rect.min.x + 33.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label.to_string(),
            font,
            c,
        );
    }
    if enabled && !tip.is_empty() && hov {
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            ui.id().with(("toolbar_tip", label)),
            egui::PopupAnchor::Pointer,
        )
        .gap(12.0)
        .show(|ui| {
            ui.label(tip);
        });
    }
    rect
}

/// Add-URL button: the same metrics and the same hover-only wash as the other
/// toolbar buttons. When a download link is sitting on the clipboard it lights
/// up in the accent colour (a soft breathing tint) so the user knows where to
/// click, instead of a banner pushing the list down.
fn add_url_button(ui: &mut egui::Ui, man: &ManState, hot: bool) -> egui::Rect {
    let label = crate::i18n::t("Add URL").to_string();
    let font = egui::FontId::proportional(12.0);
    let label_w = ui.painter().layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE).size().x;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0 + 16.0 + 7.0 + label_w + 14.0, TOOL_H), egui::Sense::hover());
    let hov = man.hover(&rect);
    let p = ui.painter();
    // A link waiting on the clipboard: a soft ring breathes around the
    // button for the first seconds, then stays as a steady outline.
    if hot {
        let now = ui.input(|i| i.time);
        let since_id = egui::Id::new("add_url_hot_since");
        let since = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(since_id, || now));
        let animate = now - since < 6.0;
        let pulse = if animate { 0.5 + 0.5 * ((now as f32) * 2.6).sin() } else { 0.6 };
        if animate {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(70));
        }
        p.rect_stroke(
            rect.expand(2.5),
            10.0,
            egui::Stroke::new(2.0_f32, pl().primary.gamma_multiply(0.25 + 0.45 * pulse)),
            egui::StrokeKind::Outside,
        );
    } else {
        ui.ctx().data_mut(|d| d.remove::<f64>(egui::Id::new("add_url_hot_since")));
    }
    // Like the other toolbar buttons: no fill of its own, a wash on hover.
    if hov {
        p.rect_filled(rect, 8.0, pl().surface_lt);
    }
    let c = if hov { pl().txt } else { pl().txt.gamma_multiply(0.75) };
    p.text(
        egui::pos2(rect.min.x + 20.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        MI_ADD.to_string(),
        crate::fonts::icon(17.0),
        c,
    );
    p.text(
        egui::pos2(rect.min.x + 35.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        c,
    );
    if hov {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.id().with("toolbar_tip_add_url"), egui::PopupAnchor::Pointer)
            .gap(12.0)
            .show(|ui| {
                ui.label(if hot { "A download link is on the clipboard: click to add it" } else { "Add a download by URL" });
            });
    }
    rect
}

fn status_bar_w(ui: &mut egui::Ui, w: f32, frac: f32, c0: egui::Color32, c1: egui::Color32) {
    let w = w.max(20.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 5.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.5, pl().surface);
    p.rect_stroke(
        rect,
        2.5,
        egui::Stroke::new(1.0_f32, pl().bar_bd),
        egui::StrokeKind::Inside,
    );
    if frac > 0.0 {
        let w = (rect.width() * frac.clamp(0.0, 1.0)).max(5.0);
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height()));
        let strips = 12;
        for i in 0..strips {
            let t0 = i as f32 / strips as f32;
            let t1 = (i + 1) as f32 / strips as f32;
            let seg = egui::Rect::from_min_max(
                egui::pos2(fill.min.x + fill.width() * t0, fill.min.y),
                egui::pos2(fill.min.x + fill.width() * t1, fill.max.y),
            );
            let c = egui::Color32::from_rgb(
                (c0.r() as f32 + (c1.r() as f32 - c0.r() as f32) * t0) as u8,
                (c0.g() as f32 + (c1.g() as f32 - c0.g() as f32) * t0) as u8,
                (c0.b() as f32 + (c1.b() as f32 - c0.b() as f32) * t0) as u8,
            );
            p.rect_filled(seg, 0.0, c);
        }
        p.circle_filled(egui::pos2(fill.min.x + 2.5, fill.center().y), 2.5, c0);
        p.circle_filled(egui::pos2(fill.max.x - 2.5, fill.center().y), 2.5, c1);
    }
}

struct ActiveTask {
    shared: Arc<SharedState>,
    handle: tokio::task::JoinHandle<()>,
    last_sample: Instant,
    last_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SortCol {
    Name,
    Size,
    Status,
    Speed,
    Eta,
    Date,
}

impl SortCol {
    fn key(&self) -> &'static str {
        match self {
            SortCol::Name => "name",
            SortCol::Size => "size",
            SortCol::Status => "status",
            SortCol::Speed => "speed",
            SortCol::Eta => "eta",
            SortCol::Date => "date",
        }
    }
    fn from_key(s: &str) -> Option<SortCol> {
        match s {
            "name" => Some(SortCol::Name),
            "size" => Some(SortCol::Size),
            "status" => Some(SortCol::Status),
            "speed" => Some(SortCol::Speed),
            "eta" => Some(SortCol::Eta),
            "date" => Some(SortCol::Date),
            _ => None,
        }
    }
    /// The header icon of every column (Name's sits at the left).
    fn icon(&self) -> Option<&'static str> {
        match self {
            SortCol::Size => Some("\u{e623}"),   // sd_storage
            SortCol::Status => Some("\u{e917}"), // donut_large
            SortCol::Speed => Some("\u{e9e4}"),  // speed
            SortCol::Eta => Some("\u{e8b5}"),    // schedule
            SortCol::Date => Some("\u{e935}"),   // calendar_today
            SortCol::Name => Some("\u{e24d}"),   // insert_drive_file
        }
    }
    fn title(&self) -> &'static str {
        match self {
            SortCol::Name => "Name",
            SortCol::Size => "Size",
            SortCol::Status => "Status",
            SortCol::Speed => "Speed",
            SortCol::Eta => "Time Left",
            SortCol::Date => "Date",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolId {
    AddUrl,
    StartQueue,
    StopQueue,
    StopAll,
    Queues,
    Settings,
    Grabber,
    /// Toolbar "Resume": resume the checked rows (all paused when none).
    ResumeSel,
    /// Toolbar "Stop": pause the checked rows (all active when none).
    StopSel,
    /// Toolbar "Delete": remove the checked rows (dimmed while none is checked).
    Delete,
}

/// Manual hit-test targets. Layout records (kind, rect) every frame;
/// `process_hits` matches pointer state against them in reverse
/// (topmost first) with click consumption. Never touches egui hit-test.
#[derive(Clone)]
enum HitKind {
    Tool(ToolId),
    MenuBtn(u8), // 0 File, 1 Tasks, 2 Tools, 3 View, 4 Help
    MenuItem(u8), // action code (1..14, 20..22, 30..33)
    SideRow(String),
    /// Sidebar section header fold/unfold: 0 = Categories, 1 = Status.
    SideToggle(u8),
    RowCheck(u64),
    Row(u64),
    HdrCol(SortCol),
    HdrCheck,
    /// The divider between a column and its right neighbour.
    ResizeCol(SortCol),
    CtxItem(RowAct, u64),
    /// A right-click menu row that opens a submenu (0 = Convert, 1 = Move
    /// to queue): hovering shows it, a click keeps the menu open.
    CtxSub(u8),
    Btn(ButtonId),
    Check(CheckId),
    Slider(SliderId, f64, f64, bool), // id, min, max, log
    Field(&'static str),
    ComboAnchor,
    /// Queue v2 reorder: (item id, -1 up / +1 down).
    QMove(u64, i32),
}

/// Active manual divider drag: table column resize (window edges are the
/// OS's job now that the app uses native decorations).
#[derive(Clone, Copy)]
enum ResizeCap {
    /// Dragging the divider on the right edge of `col`.
    Col { col: SortCol, start_x: f32, start_w: f32 },
}

/// One line of the right-click menu.
enum CtxEntry {
    Item { label: String, icon: &'static str, hint: &'static str, act: RowAct, danger: bool },
    Sep,
    /// Opens `items` (label, short tag, action) to the side.
    Sub { label: String, icon: &'static str, key: u8, items: Vec<(String, &'static str, RowAct)> },
}

/// A menu row: icon, label, and a shortcut hint or a submenu chevron.
fn ctx_row(p: &egui::Painter, r: egui::Rect, icon: &str, label: &str, hint: &str, col: egui::Color32, chevron: bool) {
    p.text(egui::pos2(r.min.x + 17.0, r.center().y), egui::Align2::CENTER_CENTER, icon, crate::fonts::icon(15.0), col);
    let right = if chevron {
        p.text(egui::pos2(r.max.x - 12.0, r.center().y), egui::Align2::CENTER_CENTER, MI_CHEVRON_RIGHT, crate::fonts::icon(15.0), pl().dim);
        r.max.x - 24.0
    } else if !hint.is_empty() {
        let g = p.layout_no_wrap(hint.to_string(), egui::FontId::proportional(10.5), pl().dim);
        let w = g.size().x;
        p.galley(egui::pos2(r.max.x - 10.0 - w, r.center().y - g.size().y / 2.0), g, pl().dim);
        r.max.x - 18.0 - w
    } else {
        r.max.x - 8.0
    };
    let tx = r.min.x + 32.0;
    let shown = truncate_to_width(p, label, egui::FontId::proportional(12.0), col, (right - tx).max(20.0));
    crate::shaped::draw(p, egui::pos2(tx, r.center().y), egui::Align2::LEFT_CENTER, &shown, egui::FontId::proportional(12.0), col);
}

/// What the one-click update reports while it runs.
enum SelfUpdMsg {
    Status(String),
    Installed(usize),
    Failed(String),
}

#[derive(Clone, Copy)]
enum RowAct {
    Resume,
    Redownload,
    Pause,
    Remove,
    RemoveFile,
    OpenFolder,
    OpenFile,
    CopyUrl,
    RefreshAddress,
    /// Move to a queue: 0 = Main, n = the n-th named queue.
    MoveToQueue(u8),
    /// List a ZIP's files and fetch only the picked ones.
    ZipPick,
    /// Unpack a finished ZIP, RAR or 7z.
    Extract,
    /// The download's own progress window (segments, speed, log).
    ShowProgress,
    /// Convert a finished file with ffmpeg: 0 = MP3, 1 = MP4, 2 = smaller MP4.
    Convert(u8),
    Details,
    VerifyChecksum,
}

struct ZipPick {
    name: String,
    url: String,
    out_dir: std::path::PathBuf,
    opts: EngineOpts,
    entries: Vec<(downloadcraft_engine::zip::ZipEntry, bool)>,
    status: String,
    busy: bool,
}

enum ZipMsg {
    Listed(Result<Vec<downloadcraft_engine::zip::ZipEntry>, String>),
    Status(String),
    Done(String),
}

/// A download the Add dialog runs in the background while it is open, in a
/// private temporary folder; Download continues it as the real file.
struct AddPreStart {
    url: String,
    shared: Arc<SharedState>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    dest: std::path::PathBuf,
}

pub struct CraftApp {
    rt: Arc<tokio::runtime::Runtime>,
    downloads: Vec<DownloadItem>,
    tasks: HashMap<u64, ActiveTask>,
    next_id: u64,
    log: Vec<String>,
    filter: String,
    search: String,
    sort_col: SortCol,
    sort_asc: bool,
    show_add: bool,
    show_queues: bool,
    show_about: bool,
    add_url: String,
    /// The name the dialog filled in by itself (from the link or the
    /// server). While the name field still holds it, a new link renames it;
    /// once the user types their own, it stays.
    add_file_auto: String,
    /// The download the Add dialog started while it is open (IDM's way).
    add_pre: Option<AddPreStart>,
    add_dir: String,
    add_file: String,
    add_conns: usize,
    connections_default: usize,
    speed_limit_kbps: u64,
    scheduler_enabled: bool,
    scheduler_time: String,
    scheduler_started: bool,
    clipboard_rx: std::sync::mpsc::Receiver<String>,
    /// A download link found on the clipboard that is not in the list yet.
    /// It is not shown as a banner (that pushed the list down); instead the
    /// toolbar's Add URL button lights up and clicking it pastes this.
    clipboard_url: Option<String>,
    browser_msg: String,
    pending_browser_install: Option<String>,
    /// mtime of ui.json at our last load/save — a newer stamp means the
    /// standalone Settings window wrote, so we reload (no restart needed).
    last_ui_mtime: std::cell::Cell<Option<std::time::SystemTime>>,
    last_inbox_check: Instant,
    last_prog_write: Instant,
    /// Every queue id ever seen this session (seeded from disk at load).
    /// Lets `save_queue` tell foreign additions (merge) apart from our own
    /// deletions (drop) when another process changed the file meanwhile.
    known_ids: std::collections::HashSet<u64>,
    col_order: Vec<SortCol>,
    col_w: std::collections::HashMap<SortCol, f32>,
    /// Column widths as drawn last frame (a drag starts from these) and the
    /// list's sideways scroll, which the header follows.
    col_shown: Vec<(SortCol, f32)>,
    list_hoff: f32,
    // ---- power features ----
    max_concurrent: usize,
    /// Named queues besides Main, and the one shown in the Queues window.
    queues: Vec<crate::model::QueueDef>,
    queue_sel: usize,
    queue_new_name: String,
    /// Answers of queue-sync size checks: (row id, size on the server).
    sync_rx: Option<std::sync::mpsc::Receiver<(u64, Option<u64>)>>,
    sync_tx: Option<std::sync::mpsc::Sender<(u64, Option<u64>)>>,
    /// A playlist being listed by yt-dlp: (playlist URL, answer).
    playlist_rx: Option<(String, std::sync::mpsc::Receiver<Result<(String, Vec<(String, String)>), String>>)>,
    /// The "ZIP: pick files" window and its worker's messages.
    zip_pick: Option<ZipPick>,
    /// Which submenu of the right-click menu is open (see HitKind::CtxSub).
    ctx_sub: Option<u8>,
    zip_rx: Option<std::sync::mpsc::Receiver<ZipMsg>>,
    /// ffmpeg conversions report here (one line each when they end).
    conv_tx: std::sync::mpsc::Sender<String>,
    conv_rx: std::sync::mpsc::Receiver<String>,
    /// "Today / this month" line for the sidebar footer, refreshed now and then.
    stats_line: String,
    stats_at: Instant,
    /// Download quota (IDM "download limits"): at most `quota_mb` per
    /// `quota_hours`; rows paused by it, to resume when the window frees.
    quota_mb: u64,
    quota_hours: u64,
    quota_log: std::collections::VecDeque<(Instant, u64)>,
    quota_last: u64,
    quota_paused: Vec<u64>,
    /// Cap on simultaneous connections per host across every download
    /// (0 = no extra cap). Loaded from ui.json so Settings can change it.
    per_host_conns: usize,
    /// Pop the standalone "Download complete" dialog when a download finishes.
    /// Its own "Don't show this dialog again" checkbox clears this.
    show_complete: bool,
    /// Auto-open the IDM-style progress window when a download starts.
    progress_windows: bool,
    /// Sounds for finished / failed downloads (Settings -> Storage).
    sounds: bool,
    queue_running: bool,
    had_active: bool,
    timeout_secs: u64,
    max_retries: u32,
    user_agent: String,
    proxy_url: String,
    antivirus_cmd: String,
    finish_action: String,
    /// Extract finished archives (Settings → On completion).
    extract_auto: bool,
    /// Which kinds, as "zip rar 7z".
    extract_types: String,
    /// Into a new folder named after the archive (else next to it).
    extract_into_folder: bool,
    /// Delete the archive once everything in it is out.
    extract_delete: bool,
    category_folders: bool,
    close_to_tray: bool,
    /// The window kept the OS caption (`platform::system_title_bar`, read
    /// once at launch): the OS owns drag, resize and the window buttons.
    os_caption: bool,
    /// Interface size (Settings → Appearance), as an egui zoom factor.
    ui_scale: f32,
    /// Start with Windows (on by default); the Run entry follows this copy.
    start_with_windows: bool,
    /// Launched at sign-in: go to the tray as soon as the window exists.
    start_hidden: bool,
    startup_frames: u8,
    /// Startup integrations are reconciled after the native window is
    /// created, so filesystem work cannot delay the desktop dock/taskbar icon.
    startup_maintenance_rx: std::sync::mpsc::Receiver<Vec<String>>,
    theme: String,
    theme_applied: String,
    /// The main window is moved into the middle of the work area once, on the
    /// first frame where egui knows the screen geometry.
    placement_done: bool,
    /// Last press on the title bar (when, where): a second one inside 420 ms
    /// and 24 px is the OS's missing "double-click the caption" gesture.
    tb_press: Option<(std::time::Instant, egui::Pos2)>,
    tray: Option<PlatformTray>,
    details_id: Option<u64>,
    /// The "Refresh download address" window, for one row.
    refresh_dlg: Option<RefreshDlg>,
    probing: bool,
    probe_size: Option<u64>,
    probe_name: Option<String>,
    probe_done_for: String,
    probe_rx: Option<std::sync::mpsc::Receiver<(Option<u64>, Option<String>)>>,
    add_adv: bool,
    add_checksum: String,
    add_overwrite: bool,
    verify_jobs: Vec<(u64, std::sync::mpsc::Receiver<anyhow::Result<String>>)>,
    // ---- P3: site grabber ----
    show_grabber: bool,
    grab_url: String,
    grab_preset: usize,
    grab_exts: String,
    grab_depth: bool,
    /// How many links deep the grabber follows (1-5) and whether it stays
    /// in the start page's folder.
    grab_levels: usize,
    grab_stay: bool,
    /// Saved grabber setups and the name typed for the current one.
    grab_projects: Vec<crate::model::GrabProject>,
    grab_project_name: String,
    grab_busy: bool,
    grab_results: Vec<crate::grabber::GrabLink>,
    grab_notice: String,
    grab_rx: Option<std::sync::mpsc::Receiver<Result<Vec<crate::grabber::GrabLink>, String>>>,
    // ---- P3: folder rules + rename templates ----
    folder_rules: Vec<crate::model::FolderRule>,
    rename_template: String,
    // ---- P3: speed windows + update checker ----
    speed_windows: Vec<crate::model::SpeedWindow>,
    speed_windows_on: bool,
    speed_limit_live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    update_check_on: bool,
    /// Column titles without their icons (Settings → Appearance).
    hide_col_icons: bool,
    /// This press already started a drag out of the list.
    drag_out_done: bool,
    update_status: String,
    update_busy: bool,
    update_rx: Option<std::sync::mpsc::Receiver<(String, Option<crate::selfupdate::Asset>, bool)>>,
    /// Portable-update asset URL from the last check (P5), if any.
    update_asset: Option<crate::selfupdate::Asset>,
    /// One-click update in progress: its messages, and "installed, restart
    /// when the downloads are done".
    selfupd_rx: Option<std::sync::mpsc::Receiver<SelfUpdMsg>>,
    update_restart_ready: bool,
    /// The running self-update was started by the app itself (a newer
    /// release found at start-up), not by "Update now".
    update_auto: bool,
    /// First-run helper fetch (yt-dlp, ffmpeg, Deno): its log lines.
    helpers_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    helpers_started: bool,
    /// The last update state written for the About window.
    update_state_sent: String,
    /// A newer release exists: only then does the title bar show its update
    /// chip. (An asset URL can be missing while an update still exists.)
    update_found: bool,
    update_auto_done: bool,
    // ---- P5: per-download speed history (EWMA input + Details graph) ----
    // Last ≤60 half-second samples per download id, pruned with tasks.
    speed_hist: std::collections::HashMap<u64, std::collections::VecDeque<f64>>,
    /// Stage label per active task (yt-dlp's "Getting info"/"Merging"): what a
    /// transfer is doing while it is not moving bytes yet. Pruned with tasks.
    phases: std::collections::HashMap<u64, String>,
    // TEMPORARY input diagnostics (removed after the hover investigation):
    // ring buffer of recent pointer/selection/action states, flushed to
    // input.log so pointer delivery can be verified without watching.
    diag_lines: Vec<String>,
    diag_last: Instant,
    diag_acts: u64,
    diag_std_hover: u64,
    diag_std_click: u64,
    diag_hits_n: usize,
    // ---- manual pointer routing (bypasses egui hit-testing) ----
    man: ManState,
    hits: Vec<(HitKind, egui::Rect)>,
    man_hover_dbg: String,
    ctx_menu: Option<(u64, egui::Pos2)>,
    drop_menu: Option<u8>,
    finish_open: bool,
    resize_cap: Option<ResizeCap>,
    /// The press that is down (or just released) began on a column divider:
    /// its release must not also count as a click that sorts the column.
    col_drag_used: bool,
    slider_cap: Option<SliderCap>,
    // Add-dialog advanced fields (kept on self so manual buttons in
    // process_hits can commit without dialog locals).
    add_referer: String,
    add_cookie: String,
    /// Captured request headers of the browser capture being added (not
    /// editable; consumed by the next commit).
    add_headers: std::collections::BTreeMap<String, String>,
    /// The browser's side of the capture being confirmed (see BrowserContext).
    add_browser: Option<crate::model::BrowserContext>,
    /// Mirrors typed into the Add box ("url | mirror | mirror").
    add_mirrors: Vec<String>,
    /// Details dialog "Start at" text while it is being typed, per row id:
    /// only a complete HH:MM is saved to the row, so reloading the field
    /// from the row every frame threw away each keystroke of "02:30".
    det_start_draft: (u64, String),
    add_user: String,
    add_pass: String,
    /// Preferred quality from the extension menu (used by the size probe so
    /// the Add dialog can show the chosen stream's size, and honored by the
    /// engine at download time).
    add_quality: String,
    captured_media: HashMap<String, (u64, bool)>,
    // Details-dialog checksum staging (validated live; see Details tab).
    det_sum: String,
    det_for: Option<u64>,
    // Autofocus request for the Add dialog URL field.
    add_focus_req: bool,
    // Recent-folders menu (the path field's caret): open flag + caret rect, so
    // the outside-click test and the render agree.
    add_recent_open: bool,
    add_caret: Option<egui::Rect>,
    /// Sidebar sections (`main_window.html`): Categories and Status fold away.
    cats_open: bool,
    status_open: bool,
}

impl CraftApp {
    pub fn new(rt: Arc<tokio::runtime::Runtime>) -> Self {
        let _ = app_opened_at(); // pin "opened" to start-up
        let dl_dir = dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || clipboard_watch(tx));
        let (conv_tx_init, conv_rx_init) = std::sync::mpsc::channel();
        let (maintenance_tx, startup_maintenance_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            rt,
            downloads: Vec::new(),
            tasks: HashMap::new(),
            next_id: 1,
            log: vec!["Download Craft ready.".to_string()],
            filter: "All".to_string(),
            search: String::new(),
            sort_col: SortCol::Date,
            sort_asc: false,
            show_add: false,
            show_queues: false,
            show_about: false,
            add_url: String::new(),
            add_file_auto: String::new(),
            add_pre: None,
            add_dir: dl_dir,
            add_file: String::new(),
            add_conns: 8,
            // 12 parallel ranges per file out of the box: the modern
            // equivalent of IDM's 8 against today's fatter pipes, while the
            // fragile-CDN detector still collapses those hosts to one stream.
            connections_default: 12,
            speed_limit_kbps: 0,
            scheduler_enabled: false,
            scheduler_time: "02:00".to_string(),
            scheduler_started: false,
            clipboard_rx: rx,
            clipboard_url: None,
            browser_msg: String::new(),
            pending_browser_install: None,
            last_ui_mtime: std::cell::Cell::new(None),
            last_inbox_check: Instant::now(),
            last_prog_write: Instant::now(),
            known_ids: std::collections::HashSet::new(),
            col_order: vec![
                SortCol::Name,
                SortCol::Size,
                SortCol::Status,
                SortCol::Speed,
                SortCol::Eta,
                SortCol::Date,
            ],
            col_w: Default::default(),
            col_shown: Vec::new(),
            list_hoff: 0.0,
            max_concurrent: 3,
        per_host_conns: 0,
            show_complete: true,
            // Starting a download opens its IDM-style progress window by
            // default. Users can turn this off in Settings.
            progress_windows: true,
            sounds: false,
            stats_line: String::new(),
            queues: Vec::new(),
            queue_sel: 0,
            queue_new_name: String::new(),
            sync_rx: None,
            sync_tx: None,
            playlist_rx: None,
            zip_pick: None,
            ctx_sub: None,
            zip_rx: None,
            conv_tx: conv_tx_init,
            conv_rx: conv_rx_init,
            stats_at: Instant::now().checked_sub(std::time::Duration::from_secs(60)).unwrap_or_else(Instant::now),
            quota_mb: 0,
            quota_hours: 1,
            quota_log: Default::default(),
            quota_last: 0,
            quota_paused: Vec::new(),
            queue_running: false,
            had_active: false,
            timeout_secs: 60,
            max_retries: 5,
            user_agent: crate::platform::default_user_agent().to_string(),
            proxy_url: String::new(),
            antivirus_cmd: String::new(),
            finish_action: "nothing".to_string(),
            extract_auto: false,
            extract_types: "zip rar 7z".to_string(),
            extract_into_folder: true,
            extract_delete: false,
            category_folders: true,
            // Closing the window tucks the app into the tray (IDM-style); the
            // tray's Exit is how you quit. The Tasks menu switches it off.
            close_to_tray: cfg!(windows),
            os_caption: crate::platform::system_title_bar(),
            ui_scale: 1.0,
            start_with_windows: cfg!(windows),
            start_hidden: crate::autostart::is_startup_launch(&std::env::args().collect::<Vec<_>>()),
            startup_frames: 0,
            startup_maintenance_rx,
            theme: "dark".to_string(),
            theme_applied: String::new(),
            placement_done: false,
            tb_press: None,
            tray: None,
            details_id: None,
            refresh_dlg: None,
            probing: false,
            probe_size: None,
            probe_name: None,
            probe_done_for: String::new(),
            probe_rx: None,
            add_adv: false,
            add_checksum: String::new(),
            add_overwrite: false,
            verify_jobs: Vec::new(),
            show_grabber: false,
            grab_url: String::new(),
            grab_preset: 0,
            grab_exts: crate::grabber::preset_exts("Mixed"),
            grab_depth: false,
            grab_levels: 1,
            grab_stay: true,
            grab_projects: crate::model::grab_projects(),
            grab_project_name: String::new(),
            grab_busy: false,
            grab_results: Vec::new(),
            grab_notice: String::new(),
            grab_rx: None,
            folder_rules: crate::model::default_folder_rules(),
            rename_template: "{name}".to_string(),
            speed_windows: vec![crate::model::SpeedWindow {
                start: "02:00".to_string(),
                end: "06:00".to_string(),
                limit_kbps: 0,
            }],
            speed_windows_on: false,
            speed_limit_live: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
            update_check_on: true,
            hide_col_icons: false,
            drag_out_done: false,
            update_status: String::new(),
            update_busy: false,
            update_rx: None,
            update_asset: None,
            selfupd_rx: None,
            update_restart_ready: false,
            update_auto: false,
            helpers_rx: None,
            helpers_started: false,
            update_state_sent: String::new(),
            update_found: false,
            update_auto_done: false,
            speed_hist: std::collections::HashMap::new(),
            phases: std::collections::HashMap::new(),
            diag_lines: Vec::new(),
            diag_last: Instant::now(),
            diag_acts: 0,
            diag_std_hover: 0,
            diag_std_click: 0,
            diag_hits_n: 0,
            man: ManState::default(),
            hits: Vec::new(),
            man_hover_dbg: "-".to_string(),
            ctx_menu: None,
            drop_menu: None,
            finish_open: false,
            resize_cap: None,
            col_drag_used: false,
            slider_cap: None,
            add_referer: String::new(),
            add_cookie: String::new(),
            add_headers: std::collections::BTreeMap::new(),
            add_browser: None,
            add_mirrors: Vec::new(),
            det_start_draft: (u64::MAX, String::new()),
            add_user: String::new(),
            add_pass: String::new(),
            add_quality: String::new(),
            captured_media: HashMap::new(),
            det_sum: String::new(),
            det_for: None,
            add_focus_req: false,
            add_recent_open: false,
            add_caret: None,
            cats_open: true,
            status_open: true,
        };
        app.load_queue();
        app.load_ui_state();
        app.last_ui_mtime.set(
            Self::ui_state_path()
                .and_then(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok()),
        );
        // These startup integrations can touch several browser config
        // directories and invoke OS tools. Keep them off the window-creation
        // path so the desktop can show the app immediately.
        let start_with_windows = app.start_with_windows;
        std::thread::spawn(move || {
            let mut notes = Vec::new();
            for b in crate::platform::HOST_BROWSERS {
                match ensure_native_host(b) {
                    Ok(true) => notes.push(format!("Browser host ready for {b}.")),
                    Err(e) => notes.push(format!("Browser host ({b}) unavailable: {e}")),
                    Ok(false) => {}
                }
            }
            // Keep the extension copy current after the window is visible.
            notes.extend(crate::extsync::sync());
            crate::autostart::sync(start_with_windows);
            crate::selfupdate::cleanup();
            let _ = maintenance_tx.send(notes);
        });
        app.poll_inbox();
        app.merge_queue_from_disk();
        app.check_wake();
        crate::model::touch_heartbeat();
        app
    }

    fn push_log(&mut self, s: impl Into<String>) {
        self.log.push(format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            s.into()
        ));
        if self.log.len() > 300 {
            self.log.drain(..self.log.len() - 300);
        }
    }

    fn queue_path() -> Option<std::path::PathBuf> {
        crate::model::config_dir().map(|p| p.join("queue.json"))
    }

    /// Publish live per-download numbers for standalone progress windows
    /// (`progress.json`, several times a second while anything is active;
    /// removed when idle).
    fn write_progress_snapshot(&mut self) {
        let Some(path) = crate::model::config_dir().map(|p| p.join("progress.json")) else {
            return;
        };
        if self.tasks.is_empty() {
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            return;
        }
        if self.last_prog_write.elapsed().as_millis() < 100 {
            return;
        }
        self.last_prog_write = Instant::now();
        let mut items = serde_json::Map::new();
        for (id, task) in self.tasks.iter() {
            let row = self.downloads.iter().find(|d| d.id == *id);
            let speed = row.map(|d| d.speed_bps).unwrap_or(0.0);
            let totals = task.shared.part_total.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let dones = task.shared.part_done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let parts: Vec<[u64; 3]> = totals
                .iter()
                .zip(dones.iter())
                .map(|((s, e), d)| [*s, *e, *d])
                .collect();
            // Standalone progress windows read this file, so it must never
            // claim more was downloaded than the size: a retried range can
            // push the raw engine counter past its own total, and one leg of a
            // merged transfer can leave the helper reporting no size at all
            // (then the row's size is still the right answer).
            let mut total = if task.shared.has_total.load(Ordering::SeqCst) {
                Some(task.shared.total.load(Ordering::SeqCst))
            } else {
                None
            };
            // A fragmented stream has no size until it is done; the row may
            // still hold an estimate from before, which must not come back.
            let fragmented = task.shared.frag_count.load(Ordering::SeqCst) > 0;
            if total.unwrap_or(0) == 0 && !fragmented {
                total = row.and_then(|d| d.total_bytes);
            }
            let mut downloaded = task.shared.downloaded.load(Ordering::SeqCst);
            if let Some(t) = total {
                downloaded = downloaded.min(t);
            }
            let resolution = {
                let live = task.shared.resolution_now();
                if live.is_empty() {
                    row.map(|d| d.resolution.clone()).unwrap_or_default()
                } else {
                    live
                }
            };
            items.insert(
                id.to_string(),
                serde_json::json!({
                    "downloaded": downloaded,
                    "total": total,
                    "speed": speed,
                    "resumable": task.shared.resumable.load(Ordering::SeqCst),
                    "phase": self.phases.get(id).cloned().unwrap_or_default(),
                    "resolution": resolution,
                    "parts": parts,
                    "fragments": [
                        task.shared.frag_index.load(Ordering::SeqCst),
                        task.shared.frag_count.load(Ordering::SeqCst),
                    ],
                }),
            );
        }
        if let Ok(json) = serde_json::to_string_pretty(&serde_json::json!({
            "items": serde_json::Value::Object(items)
        })) {
            Self::atomic_write(&path, json.as_bytes());
        }
    }

    /// Run pause/resume/cancel orders left by standalone progress windows.
    fn check_cmd(&mut self) {
        let path = match crate::model::config_dir() {
            Some(p) => p.join("cmd.json"),
            None => return,
        };
        let _guard = crate::model::fs_lock("cmd");
        if !_guard.owned() { return; }
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let _ = std::fs::remove_file(&path); // claim first: at-most-once
        let cmds: Vec<serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
        for c in cmds {
            let id = c.get("id").and_then(|x| x.as_u64()).unwrap_or(u64::MAX);
            let act = c.get("action").and_then(|x| x.as_str()).unwrap_or("");
            match act {
                "pause" => self.pause(id),
                "resume" => self.resume(id),
                "cancel" => self.cancel_task(id),
                "pause_all" => self.stop_all(),
                "self_update" => {
                    self.update_auto = false;
                    self.start_self_update();
                }
                "check_update" => self.start_update_check(),
                // Restart by itself once the running downloads are done.
                "restart_when_idle" => self.update_auto = true,
                "restart_update" => {
                    if self.update_restart_ready {
                        self.restart_for_update();
                    }
                }
                "resume_all" => self.start_queue(),
                "limit" => {
                    if let Some(k) = c.get("kbps").and_then(|x| x.as_u64()) {
                        self.speed_limit_kbps = k;
                        self.refresh_speed_limit();
                        self.save_ui_state();
                        self.push_log(format!("Speed limit set to {k} KB/s from a progress window."));
                    }
                }
                "finish" => {
                    if let Some(v) = c.get("value").and_then(|x| x.as_str()) {
                        if ["nothing", "shutdown", "sleep", "hibernate"].contains(&v) {
                            self.finish_action = v.to_string();
                            self.save_ui_state();
                            self.push_log(format!("Finish action set to {v} from a progress window."));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Pop a standalone IDM-style progress window for one download. It is
    /// this program started again; when that fails (the executable was moved
    /// or replaced while running) the reason goes to the log instead of the
    /// window silently never appearing.
    fn spawn_progress(id: u64) -> std::io::Result<()> {
        let started = crate::platform::launch_path().and_then(|exe| {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("--progress").arg(id.to_string());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x00000008);
            }
            cmd.spawn()
        });
        started.map(|_| ())
    }

    /// Pop the standalone "Download complete" dialog for one finished download.
    fn spawn_complete(id: u64) {
        let Ok(exe) = crate::platform::launch_path() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--complete").arg(id.to_string());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008);
        }
        let _ = cmd.spawn();
    }
    fn ui_state_path() -> Option<std::path::PathBuf> {
        crate::model::config_dir().map(|p| p.join("ui.json"))
    }

    /// Launch the standalone Settings window (separate OS process).
    /// Site Grabber → Save project: the current page, types and depth
    /// under a name (the page's host when none is given).
    fn grab_save_project(&mut self) {
        let name = if self.grab_project_name.trim().is_empty() {
            url::Url::parse(self.grab_url.trim()).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "Project".into())
        } else {
            self.grab_project_name.trim().to_string()
        };
        let p = crate::model::GrabProject {
            name: name.clone(),
            url: self.grab_url.trim().to_string(),
            exts: self.grab_exts.clone(),
            follow: self.grab_depth,
            levels: self.grab_levels,
            stay: self.grab_stay,
        };
        self.grab_projects.retain(|x| x.name != name);
        self.grab_projects.push(p);
        self.grab_notice = if crate::model::save_grab_projects(&self.grab_projects) {
            format!("Saved project \"{name}\".")
        } else {
            "Could not save the project.".into()
        };
    }

    fn open_settings_window() {
        let Ok(exe) = crate::platform::launch_path() else {
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--settings");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x00000008);
        }
        let _ = cmd.spawn();
    }

    /// Reload ui.json when the standalone Settings window rewrote it.
    /// Skipped while the Add dialog is open (never clobber typing).
    fn watch_ui_state(&mut self) {
        let Some(path) = Self::ui_state_path() else {
            return;
        };
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return;
        };
        let changed = self.last_ui_mtime.get().map(|t| mtime > t).unwrap_or(true);
        if changed && !self.show_add {
            self.last_ui_mtime.set(Some(mtime));
            let was = self.start_with_windows;
            self.load_ui_state();
            self.refresh_speed_limit();
            if self.start_with_windows != was {
                crate::autostart::sync(self.start_with_windows);
            }
            self.push_log("Settings reloaded from disk.");
        }
    }

    fn save_ui_state(&self) {
        if let Some(path) = Self::ui_state_path() {
            let _guard=crate::model::fs_lock("ui-settings");
            if !_guard.owned() { return; }
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let order: Vec<&str> = self.col_order.iter().map(|c| c.key()).collect();
            let widths: std::collections::HashMap<&str, f32> = self
                .col_w
                .iter()
                .map(|(c, w)| (c.key(), *w))
                .collect();
            let mine = serde_json::json!({
                "columns": order,
                "widths": widths,
                // 2 = Name keeps a width of its own (columns push each
                // other, as in IDM); before that a saved Name width was
                // ignored and must not be applied now.
                "widths_v": 2,
                "max_concurrent": self.max_concurrent,
                "per_host_conns": self.per_host_conns,
                "show_complete": self.show_complete,
                "progress_windows": self.progress_windows,
                "progress_windows_default_v": 1,
                "timeout_secs": self.timeout_secs,
                "max_retries": self.max_retries,
                "user_agent": self.user_agent,
                "proxy_url": self.proxy_url,
                "antivirus_cmd": self.antivirus_cmd,
                "finish_action": self.finish_action,
                "extract_auto": self.extract_auto,
                "extract_types": self.extract_types,
                "extract_into_folder": self.extract_into_folder,
                "extract_delete": self.extract_delete,
                "queues": self.queues,
                "category_folders": self.category_folders,
                "close_to_tray": self.close_to_tray,
                "start_with_windows": self.start_with_windows,
                "theme": self.theme,
                "connections_default": self.connections_default,
                "speed_limit_kbps": self.speed_limit_kbps,
                "save_dir": self.add_dir,
                "folder_rules": self.folder_rules,
                "rename_template": self.rename_template,
                "speed_windows": self.speed_windows,
                "speed_windows_on": self.speed_windows_on,
                "update_check_on": self.update_check_on,
            });
            // Overlay our keys, never replace the whole file: the Settings
            // window keeps its own keys here too (helper paths and their
            // "app's own copy / PATH / custom" source choices) and a full
            // rewrite would silently drop them.
            let mut v: serde_json::Value = std::fs::read_to_string(&path)
                .ok()
                .and_then(|d| serde_json::from_str(&d).ok())
                .filter(serde_json::Value::is_object)
                .unwrap_or_else(|| serde_json::json!({}));
            if let (Some(dst), Some(src)) = (v.as_object_mut(), mine.as_object()) {
                for (k, val) in src {
                    dst.insert(k.clone(), val.clone());
                }
            }
            if let Ok(json) = serde_json::to_string_pretty(&v) {
                Self::atomic_write(&path, json.as_bytes());
                if let Ok(mtime) =
                    std::fs::metadata(&path).and_then(|m| m.modified())
                {
                    self.last_ui_mtime.set(Some(mtime));
                }
            }
        }
    }

    fn load_ui_state(&mut self) {
        if let Some(path) = Self::ui_state_path() {
            if let Ok(data) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    self.ui_scale = v
                        .get("ui_scale")
                        .and_then(|x| x.as_u64())
                        .map_or(1.0, |pct| (pct.clamp(80, 175) as f32) / 100.0);
                    if let Some(arr) = v.get("columns").and_then(|c| c.as_array()) {
                        let mut order = Vec::new();
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                if let Some(c) = SortCol::from_key(s) {
                                    if !order.contains(&c) {
                                        order.push(c);
                                    }
                                }
                            }
                        }
                        for c in [
                            SortCol::Name,
                            SortCol::Size,
                            SortCol::Status,
                            SortCol::Speed,
                            SortCol::Eta,
                            SortCol::Date,
                        ] {
                            if !order.contains(&c) {
                                order.push(c);
                            }
                        }
                        self.col_order = order;
                    }
                    if let Some(w) = v.get("widths").and_then(|x| x.as_object()) {
                        for (k, val) in w {
                            if let (Some(c), Some(f)) =
                                (SortCol::from_key(k), val.as_f64())
                            {
                                self.col_w.insert(c, (f as f32).clamp(col_min(c), 900.0));
                            }
                        }
                        if v.get("widths_v").and_then(|x| x.as_u64()) != Some(2) {
                            self.col_w.remove(&SortCol::Name);
                        }
                    }
                    let g = |k: &str| v.get(k);
                    if let Some(n) = g("max_concurrent").and_then(|x| x.as_u64()) {
                        self.max_concurrent = (n as usize).clamp(1, 16);
                    }
                    if let Some(n) = g("per_host_conns").and_then(|x| x.as_u64()) {
                        self.per_host_conns = (n as usize).clamp(0, 64);
                    }
                    if let Some(b) = g("sounds").and_then(|x| x.as_bool()) {
                        self.sounds = b;
                    }
                    if let Some(n) = g("quota_mb").and_then(|x| x.as_u64()) {
                        self.quota_mb = n;
                    }
                    if let Some(n) = g("quota_hours").and_then(|x| x.as_u64()) {
                        self.quota_hours = n.clamp(1, 24 * 30);
                    }
                    if let Some(b) = g("show_complete").and_then(|x| x.as_bool()) {
                        self.show_complete = b;
                    }
                    if g("progress_windows_default_v").and_then(|x| x.as_u64()).unwrap_or(0) < 1 {
                        // 0.9.14 shipped with progress windows disabled by
                        // default. Re-enable them once on upgrade, then keep
                        // honoring the user's choice saved by the new UI.
                        self.progress_windows = true;
                    } else if let Some(b) = g("progress_windows").and_then(|x| x.as_bool()) {
                        self.progress_windows = b;
                    }
                    if let Some(n) = g("timeout_secs").and_then(|x| x.as_u64()) {
                        self.timeout_secs = n.clamp(5, 600);
                    }
                    if let Some(n) = g("max_retries").and_then(|x| x.as_u64()) {
                        self.max_retries = (n as u32).clamp(1, 20);
                    }
                    if let Some(s) = g("user_agent").and_then(|x| x.as_str()) {
                        if !s.is_empty() {
                            self.user_agent = s.to_string();
                        }
                    }
                    if let Some(s) = g("proxy_url").and_then(|x| x.as_str()) {
                        self.proxy_url = s.to_string();
                    }
                    if let Some(s) = g("antivirus_cmd").and_then(|x| x.as_str()) {
                        self.antivirus_cmd = s.to_string();
                    }
                    if let Some(s) = g("finish_action").and_then(|x| x.as_str()) {
                        self.finish_action = s.to_string();
                    }
                    if let Some(q) = g("queues").and_then(|x| serde_json::from_value::<Vec<crate::model::QueueDef>>(x.clone()).ok()) {
                        // Keep what only lives in memory (running, fired times).
                        let old = std::mem::take(&mut self.queues);
                        self.queues = q
                            .into_iter()
                            .map(|mut n| {
                                if let Some(o) = old.iter().find(|o| o.name == n.name) {
                                    n.running = o.running;
                                    n.fired_start = o.fired_start.clone();
                                    n.fired_stop = o.fired_stop.clone();
                                    n.last_sync = o.last_sync;
                                }
                                n
                            })
                            .collect();
                    }
                    if let Some(b) = g("category_folders").and_then(|x| x.as_bool()) {
                        self.category_folders = b;
                    }
                    if let Some(b) = g("extract_auto").and_then(|x| x.as_bool()) {
                        self.extract_auto = b;
                    }
                    if let Some(s) = g("extract_types").and_then(|x| x.as_str()) {
                        self.extract_types = s.to_string();
                    }
                    if let Some(b) = g("extract_into_folder").and_then(|x| x.as_bool()) {
                        self.extract_into_folder = b;
                    }
                    if let Some(b) = g("extract_delete").and_then(|x| x.as_bool()) {
                        self.extract_delete = b;
                    }
                    if let Some(b) = g("close_to_tray").and_then(|x| x.as_bool()) {
                        self.close_to_tray = cfg!(windows) && b;
                    }
                    if let Some(b) = g("start_with_windows").and_then(|x| x.as_bool()) {
                        self.start_with_windows = cfg!(windows) && b;
                    }
                    if let Some(s) = g("theme").and_then(|x| x.as_str()) {
                        self.theme = s.to_string();
                    }
                    if let Some(n) = g("connections_default").and_then(|x| x.as_u64()) {
                        self.connections_default = (n as usize).clamp(1, 32);
                    }
                    if let Some(n) = g("speed_limit_kbps").and_then(|x| x.as_u64()) {
                        self.speed_limit_kbps = n;
                    }
                    if let Some(s) = g("save_dir").and_then(|x| x.as_str()) {
                        if !s.is_empty() {
                            self.add_dir = s.to_string();
                        }
                    }
                    if let Some(arr) = g("folder_rules").and_then(|x| x.as_array()) {
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
                            self.folder_rules = rules;
                        }
                    }
                    if let Some(s) = g("rename_template").and_then(|x| x.as_str()) {
                        if !s.trim().is_empty() {
                            self.rename_template = s.to_string();
                        }
                    }
                    if let Some(arr) = g("speed_windows").and_then(|x| x.as_array()) {
                        let wins: Vec<crate::model::SpeedWindow> = arr
                            .iter()
                            .filter_map(|r| {
                                Some(crate::model::SpeedWindow {
                                    start: r.get("start")?.as_str()?.to_string(),
                                    end: r.get("end")?.as_str()?.to_string(),
                                    limit_kbps: r.get("limit_kbps")?.as_u64()?,
                                })
                            })
                            .take(6)
                            .collect();
                        if !wins.is_empty() {
                            self.speed_windows = wins;
                        }
                    }
                    if let Some(b) = g("speed_windows_on").and_then(|x| x.as_bool()) {
                        self.speed_windows_on = b;
                    }
                    if let Some(b) = g("update_check_on").and_then(|x| x.as_bool()) {
                        self.update_check_on = b;
                    }
                    // Settings → Appearance → Hide column icons.
                    self.hide_col_icons = g("hide_col_icons").and_then(|x| x.as_bool()).unwrap_or(false);
                }
            }
        }
    }

    /// Leftover chunk files for a destination (single + segmented parts).
    fn part_files(dest: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = vec![dest.with_extension("dcpart")];
        for i in 0..16 {
            out.push(dest.with_extension(format!("dcpart{i}")));
        }
        out.extend(crate::engine::partial_paths(dest));
        out
    }

    fn redownload(&mut self, id: u64) {
        self.cancel_task(id);
        if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            let dest = d.dest_path();
            // Replace semantics: drop the previous file so the engine does
            // not auto-rename to `name (1).ext`.
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            for p in Self::part_files(&dest) {
                let _ = std::fs::remove_file(p);
            }
        }
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            d.downloaded_bytes = 0;
            d.total_bytes = None;
            d.speed_bps = 0.0;
            d.status = Status::Queued;
        }
        self.save_queue();
        self.start(id);
    }

    /// Re-queue every failed item and pump the queue. Real action behind
    /// the Tasks-menu / toolbar entry (no placeholder controls).
    fn retry_all_failed(&mut self) {
        let mut n = 0;
        for d in self.downloads.iter_mut() {
            if matches!(d.status, Status::Failed(_)) {
                d.status = Status::Queued;
                n += 1;
            }
        }
        if n > 0 {
            self.queue_running = true;
            self.fill_next();
            self.push_log(format!("Re-queued {n} failed item(s)."));
            self.save_queue();
        } else {
            self.push_log("Nothing failed — nothing to retry.");
        }
    }

    /// Menu/dropdown action codes shared by the menubar and popups.
    /// 1 add, 2 folder, 4/5/6/7 tasks, 8 queues, 9 settings, 10 about,
    /// 11/12 import/export, 13 retry-failed, 14 exit, 19/20/22 themes.
    fn do_menu_act(&mut self, ctx: &egui::Context, act: u8) {
        self.diag_acts += 1;
        match act {
            1 => {
                self.add_url.clear();
                self.add_file.clear();
                self.add_checksum.clear();
                self.add_quality.clear();
                self.clear_add_credentials();
                self.show_add = true;
            }
            2 => {
                open_path(std::path::Path::new(&self.add_dir), false);
            }
            4 => {
                for id in self.selected_ids() {
                    self.resume(id);
                }
            }
            5 => {
                for id in self.selected_ids() {
                    self.pause(id);
                }
            }
            6 => {
                for id in self.selected_ids() {
                    self.remove(id);
                }
            }
            7 => {
                let ids: Vec<u64> = self
                    .downloads
                    .iter()
                    .filter(|d| matches!(d.status, Status::Completed))
                    .map(|d| d.id)
                    .collect();
                for id in ids {
                    self.remove(id);
                }
            }
            8 => self.show_queues = true,
            9 => Self::open_settings_window(),
            10 => crate::about::spawn(),
            25 => crate::setup::spawn(false),
            11 => self.import_list(),
            12 => self.export_list(),
            13 => self.retry_all_failed(),
            23 => self.start_queue(),
            24 => self.stop_queue(),
            14 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            19 | 20 | 21 | 22 => {
                self.theme = match act {
                    19 => "system",
                    20 => "dark",
                    21 => "black",
                    _ => "light",
                }
                .to_string();
                self.save_ui_state();
            }
            _ => {}
        }
    }

    /// Dropdown items per menubar menu. `(label, act)`; `("", 0)` renders
    /// a separator row.
    fn menu_items(menu: u8) -> Vec<(String, u8)> {
        match menu {
            0 => vec![
                ("Add URL".into(), 1),
                ("Open download folder".into(), 2),
                (String::new(), 0),
                ("Import URL list (.txt)…".into(), 11),
                ("Export list (.txt)…".into(), 12),
                (String::new(), 0),
                ("Exit".into(), 14),
            ],
            1 => vec![
                ("Start selected".into(), 4),
                ("Pause selected".into(), 5),
                ("Retry all failed".into(), 13),
                (String::new(), 0),
                ("Start queue".into(), 23),
                ("Stop queue".into(), 24),
                ("Remove selected".into(), 6),
                ("Clear finished".into(), 7),
            ],
            2 => vec![("Open Queues".into(), 8), ("Settings".into(), 9)],
            3 => vec![
                ("System".into(), 19),
                ("Dark".into(), 20),
                ("Light".into(), 22),
            ],
            _ => vec![("Get ready…".into(), 25), ("About".into(), 10)],
        }
    }

    /// Row action dispatcher (driven by process_hits).
    fn do_row_act(&mut self, act: RowAct, id: u64) {
        self.diag_acts += 1;
        // Rows the user checked: bulk actions act on all of them when the
        // clicked row is part of the selection, so "remove several files"
        // works from the right-click menu too.
        let sel = self.selected_ids();
        let bulk = sel.len() > 1 && sel.contains(&id);
        match act {
            RowAct::Resume => {
                if bulk {
                    for i in sel {
                        self.resume(i);
                    }
                } else {
                    self.resume(id)
                }
            }
            RowAct::Pause => {
                if bulk {
                    for i in sel {
                        self.pause(i);
                    }
                } else {
                    self.pause(id)
                }
            }
            RowAct::Details => {
                self.details_id = Some(id);
            }
            RowAct::Redownload => self.redownload(id),
            RowAct::Remove => {
                if bulk {
                    for i in sel {
                        self.remove(i);
                    }
                } else {
                    self.remove(id)
                }
            }
            RowAct::RemoveFile => {
                if bulk {
                    for i in sel {
                        self.remove_file(i);
                    }
                } else {
                    self.remove_file(id)
                }
            }
            RowAct::OpenFolder => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    open_path(&d.dest_path(), true);
                }
            }
            RowAct::OpenFile => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    open_path(&d.dest_path(), false);
                }
            }
            RowAct::CopyUrl => {
                if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(d.url.clone());
                    }
                    self.push_log("URL copied to clipboard.");
                }
            }
            RowAct::VerifyChecksum => self.verify_checksum(id),
            RowAct::MoveToQueue(i) => {
                let name = if i == 0 { String::new() } else { self.queues.get(i as usize - 1).map(|q| q.name.clone()).unwrap_or_default() };
                let ids = if bulk { sel } else { vec![id] };
                for d in self.downloads.iter_mut().filter(|d| ids.contains(&d.id)) {
                    d.queue = name.clone();
                }
                self.save_queue();
                self.push_log(format!("Moved {} row(s) to queue {}.", ids.len(), if name.is_empty() { "Main" } else { &name }));
            }
            RowAct::ZipPick => self.open_zip_pick(id),
            RowAct::ShowProgress => { let _ = Self::spawn_progress(id); }
            RowAct::Extract => {
                if let Some(path) = self.downloads.iter().find(|d| d.id == id).map(|d| d.dest_path()) {
                    self.extract_archive(&path, true);
                }
            }
            RowAct::Convert(kind) => self.convert(id, kind),
            RowAct::RefreshAddress => self.open_refresh(id, None),
        }
    }

    /// Opens the "Refresh download address" window for row `id` and starts
    /// waiting (10 minutes) for the next link the browser sends, as IDM does.
    /// `reason` says why when a resume failed on a dead link.
    fn open_refresh(&mut self, id: u64, reason: Option<String>) {
        let Some(d) = self.downloads.iter().find(|d| d.id == id) else { return };
        crate::model::set_refresh_wait(Some(crate::model::RefreshWait {
            id,
            until: chrono::Local::now().timestamp() + 10 * 60,
        }));
        self.refresh_dlg = Some(RefreshDlg {
            id,
            name: d.filename.clone(),
            page: d.referer.clone(),
            reason,
            new_url: String::new(),
            error: None,
            opened: std::time::Instant::now(),
        });
    }

    /// The link typed or pasted into the refresh window takes the row's
    /// place; the bytes already on disk are kept and the row resumes.
    fn apply_typed_link(&mut self) {
        let Some(dlg) = self.refresh_dlg.as_mut() else { return };
        let url = dlg.new_url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ftp://")) {
            dlg.error = Some("Paste a link that starts with http:// or https://".into());
            return;
        }
        let id = dlg.id;
        crate::model::set_refresh_wait(None);
        self.refresh_dlg = None;
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        let dest = d.dest_path();
        d.url = url.clone();
        if matches!(d.status, Status::Failed(_)) {
            d.status = Status::Paused;
        }
        let name = d.filename.clone();
        crate::model::rebind_resume(&dest, &url);
        self.save_queue();
        self.push_log(format!("Refresh address: new link for {name} - continuing."));
        self.resume(id);
    }

    /// The "Refresh download address" window: a card in the app's style with
    /// the refresh icon, what happened, the two ways to get a new link
    /// (click it again in the browser, or paste it) and Cancel.
    fn draw_refresh_dialog(&mut self, ctx: &egui::Context) {
        let Some(dlg) = self.refresh_dlg.as_ref() else { return };
        // The row went away, or the wait ran out.
        let left = (10 * 60) as i64 - dlg.opened.elapsed().as_secs() as i64;
        if left <= 0 || !self.downloads.iter().any(|d| d.id == dlg.id) {
            crate::model::set_refresh_wait(None);
            self.refresh_dlg = None;
            return;
        }
        let (name, page, reason, error) = (dlg.name.clone(), dlg.page.clone(), dlg.reason.clone(), dlg.error.clone());
        let mut new_url = dlg.new_url.clone();
        let w = modal_width(ctx, 460.0);
        egui::Window::new("refresh_modal")
            .id(egui::Id::new("refresh_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size(egui::vec2(w, 0.0))
            .frame(
                egui::Frame::new()
                    .fill(pl().modal_bg)
                    .stroke(egui::Stroke::new(1.0_f32, pl().modal_bd))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 0.0;
                // ---- title bar: refresh icon · title · close ----
                let (bar, _) = ui.allocate_exact_size(egui::vec2(w, 44.0), egui::Sense::hover());
                let p = ui.painter().clone();
                p.line_segment(
                    [egui::pos2(bar.min.x, bar.max.y - 0.5), egui::pos2(bar.max.x, bar.max.y - 0.5)],
                    egui::Stroke::new(1.0, pl().modal_bd),
                );
                let tile = egui::Rect::from_center_size(egui::pos2(bar.min.x + 26.0, bar.center().y), egui::vec2(28.0, 28.0));
                p.rect_filled(tile, 8.0, mix(pl().modal_bg, pl().primary, 0.18));
                line_icon(&p, tile.center(), 16.0, "Updates", pl().primary);
                p.text(
                    egui::pos2(bar.min.x + 50.0, bar.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Refresh download address",
                    crate::fonts::semi(12.5),
                    pl().txt,
                );
                let close_r = egui::Rect::from_center_size(egui::pos2(bar.max.x - 22.0, bar.center().y), egui::vec2(25.0, 25.0));
                let close_hover = self.man.hover(&close_r);
                if close_hover {
                    p.rect_filled(close_r, 6.0, pl().surface_lt);
                }
                p.text(close_r.center(), egui::Align2::CENTER_CENTER, "\u{e5cd}", crate::fonts::icon(14.0), if close_hover { pl().txt } else { pl().dim });
                self.hits.push((HitKind::Btn(ButtonId::RfCancel), close_r));

                egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 16, top: 12, bottom: 14 }).show(ui, |ui| {
                    let row_w = w - 32.0;
                    ui.set_width(row_w);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // The file, with its type icon.
                    let (file_r, _) = ui.allocate_exact_size(egui::vec2(row_w, 40.0), egui::Sense::hover());
                    let p = ui.painter().clone();
                    p.rect(file_r, 8.0, pl().surface, egui::Stroke::new(1.0, mix(pl().modal_bg, pl().modal_bd, 0.45)), egui::StrokeKind::Inside);
                    let cat = crate::model::category_of(&name);
                    p.text(egui::pos2(file_r.min.x + 20.0, file_r.center().y), egui::Align2::CENTER_CENTER, file_icon(cat), crate::fonts::icon(16.0), cat_color(cat));
                    let shown = truncate_to_width(&p, &name, crate::fonts::semi(11.5), pl().txt, row_w - 48.0);
                    crate::shaped::draw(&p, egui::pos2(file_r.min.x + 38.0, file_r.center().y), egui::Align2::LEFT_CENTER, &shown, crate::fonts::semi(11.5), pl().txt);
                    ui.add_space(10.0);
                    if let Some(r) = &reason {
                        ui.add(egui::Label::new(egui::RichText::new(format!("The link stopped working: {r}")).font(crate::fonts::reg(11.0)).color(pl().error)).wrap());
                        ui.add_space(6.0);
                    }
                    ui.add(egui::Label::new(
                        egui::RichText::new("Open the page you downloaded it from and click its download link again: the new link continues this download from where it stopped. Or paste a new link below.")
                            .font(crate::fonts::reg(11.0))
                            .color(pl().dim),
                    ).wrap());
                    ui.add_space(10.0);
                    let r = dp_input(ui, "rf_url", &mut new_url, "Paste the new download link", row_w, false, false);
                    self.hits.push((HitKind::Field("rf_url"), r));
                    if let Some(e) = &error {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(e).font(crate::fonts::reg(10.5)).color(pl().error));
                    }
                    ui.add_space(8.0);
                    // Waiting for the browser, with the time left.
                    let (wait_r, _) = ui.allocate_exact_size(egui::vec2(row_w, 18.0), egui::Sense::hover());
                    let p = ui.painter().clone();
                    let t = ui.ctx().input(|i| i.time) as f32;
                    p.circle_filled(egui::pos2(wait_r.min.x + 5.0, wait_r.center().y), 3.5, pl().success.gamma_multiply(0.55 + 0.45 * (t * 3.0).sin().abs()));
                    p.text(
                        egui::pos2(wait_r.min.x + 16.0, wait_r.center().y),
                        egui::Align2::LEFT_CENTER,
                        format!("Waiting for the new link from your browser… {}:{:02} left", left / 60, left % 60),
                        crate::fonts::reg(10.5),
                        pl().dim,
                    );
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    ui.add_space(12.0);
                    ui.allocate_ui_with_layout(egui::vec2(row_w, 28.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let r = dp_button(ui, &self.man, "Use link", true);
                        self.hits.push((HitKind::Btn(ButtonId::RfApply), r));
                        let r = dp_button(ui, &self.man, "Cancel", false);
                        self.hits.push((HitKind::Btn(ButtonId::RfCancel), r));
                        if url::Url::parse(&page).is_ok() {
                            let r = dp_button(ui, &self.man, "Open page", false);
                            self.hits.push((HitKind::Btn(ButtonId::RfOpen), r));
                        }
                    });
                });
            });
        if let Some(d) = self.refresh_dlg.as_mut() {
            d.new_url = new_url;
        }
    }

    /// A capture that arrived while a row waits for a fresh link: it becomes
    /// that row's link (cookies and browser context too), the bytes already
    /// downloaded are kept, and the row resumes.
    fn apply_refreshed_link(&mut self, id: u64, item: crate::native_host::InboxItem) {
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        if let (Some(have), Some(got)) = (d.total_bytes, item.size) {
            if have > 0 && got > 0 && have != got {
                self.push_log(format!("Refresh address: the new link is a different size ({got} vs {have} bytes) - ignored, still waiting."));
                return;
            }
        }
        crate::model::set_refresh_wait(None);
        if self.refresh_dlg.as_ref().is_some_and(|r| r.id == id) {
            self.refresh_dlg = None;
        }
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        let dest = d.dest_path();
        d.url = item.url.clone();
        if let Some(c) = item.cookie.filter(|c| !c.is_empty()) {
            d.cookie = c;
        }
        if let Some(r) = item.referer.or(item.page_url).filter(|r| !r.is_empty()) {
            d.referer = r;
        }
        d.extra_headers = item.headers.unwrap_or_default();
        d.browser = item.browser;
        if matches!(d.status, Status::Failed(_)) {
            d.status = Status::Paused;
        }
        let name = d.filename.clone();
        crate::model::rebind_resume(&dest, &item.url);
        self.save_queue();
        self.push_log(format!("Refresh address: new link for {name} - continuing."));
        self.resume(id);
    }

    /// Options for a side request (ZIP listing) made for an existing row.
    fn side_opts(&self, d: &DownloadItem) -> EngineOpts {
        EngineOpts {
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", crate::model::APP_VERSION)
            } else {
                self.user_agent.clone()
            },
            referer: d.referer.clone(),
            cookie: d.cookie.clone(),
            captured_headers: crate::engine::CapturedHeaders::for_url(&d.url, &d.extra_headers),
            username: d.username.clone(),
            password: d.password.clone(),
            proxy_url: self.proxy_url.clone(),
            timeout_secs: d.eff_timeout(self.timeout_secs),
            browser: d.browser.clone(),
            ..Default::default()
        }
    }

    /// IDM's "download part of a ZIP": read the archive's table of contents
    /// with a few Range requests and open a window to pick files from it.
    fn open_zip_pick(&mut self, id: u64) {
        let Some(d) = self.downloads.iter().find(|d| d.id == id) else { return };
        let opts = self.side_opts(d);
        let url = d.url.clone();
        let stem = std::path::Path::new(&d.filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "zip".into());
        let out_dir = std::path::PathBuf::from(&d.save_dir).join(stem);
        self.zip_pick = Some(ZipPick {
            name: d.filename.clone(),
            url: url.clone(),
            out_dir,
            opts: opts.clone(),
            entries: Vec::new(),
            status: "Reading the ZIP's table of contents…".into(),
            busy: true,
        });
        let (tx, rx) = std::sync::mpsc::channel();
        self.zip_rx = Some(rx);
        self.rt.spawn(async move {
            let r = async {
                let client = opts.build_client().map_err(|e| format!("{e:#}"))?;
                let request: downloadcraft_engine::RequestFn = Arc::new(move |u: &str| opts.get(&client, u));
                downloadcraft_engine::zip::list_remote(&request, &url).await.map_err(|e| e.to_string())
            }
            .await;
            let _ = tx.send(ZipMsg::Listed(r));
        });
    }

    /// Fetch the ticked entries one after another into the ZIP's folder.
    fn zip_fetch_selected(&mut self) {
        let Some(p) = self.zip_pick.as_mut() else { return };
        let picked: Vec<downloadcraft_engine::zip::ZipEntry> =
            p.entries.iter().filter(|(e, on)| *on && e.can_extract()).map(|(e, _)| e.clone()).collect();
        if picked.is_empty() {
            p.status = "Tick at least one file.".into();
            return;
        }
        p.busy = true;
        let (url, out_dir, opts) = (p.url.clone(), p.out_dir.clone(), p.opts.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        self.zip_rx = Some(rx);
        self.rt.spawn(async move {
            let client = match opts.build_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(ZipMsg::Done(format!("ZIP: {e:#}")));
                    return;
                }
            };
            let request: downloadcraft_engine::RequestFn = Arc::new(move |u: &str| opts.get(&client, u));
            let (mut ok, n) = (0, picked.len());
            for (i, e) in picked.iter().enumerate() {
                let _ = tx.send(ZipMsg::Status(format!("{}/{n}: {}", i + 1, e.name)));
                // Keep the archive's folders, never let a name climb out.
                let rel: std::path::PathBuf = e
                    .name
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                    .map(crate::engine::sanitize_filename)
                    .collect();
                match downloadcraft_engine::zip::extract_remote(&request, &url, e, &out_dir.join(rel)).await {
                    Ok(_) => ok += 1,
                    Err(err) => {
                        let _ = tx.send(ZipMsg::Status(format!("{}: {err}", e.name)));
                    }
                }
            }
            let _ = tx.send(ZipMsg::Done(format!("ZIP: {ok} of {n} file(s) saved to {}", out_dir.display())));
        });
    }

    fn poll_zip(&mut self) {
        let Some(rx) = self.zip_rx.as_ref() else { return };
        let msgs: Vec<ZipMsg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                ZipMsg::Listed(r) => {
                    let Some(p) = self.zip_pick.as_mut() else { continue };
                    p.busy = false;
                    match r {
                        Ok(entries) => {
                            let files = entries.iter().filter(|e| !e.is_dir).count();
                            p.status = format!("{files} file(s). Tick the ones you want.");
                            p.entries = entries.into_iter().filter(|e| !e.is_dir).map(|e| (e, false)).collect();
                        }
                        Err(e) => p.status = format!("Could not read the ZIP: {e}"),
                    }
                    self.zip_rx = None;
                }
                ZipMsg::Status(s) => {
                    if let Some(p) = self.zip_pick.as_mut() {
                        p.status = s;
                    }
                }
                ZipMsg::Done(s) => {
                    if let Some(p) = self.zip_pick.as_mut() {
                        p.busy = false;
                        p.status = s.clone();
                    }
                    self.push_log(s);
                    self.zip_rx = None;
                }
            }
        }
    }

    /// Convert a finished file with ffmpeg in the background; the result
    /// lands next to the original.
    fn convert(&mut self, id: u64, kind: u8) {
        let Some(d) = self.downloads.iter().find(|d| d.id == id) else { return };
        let src = d.dest_path();
        if !src.is_file() {
            self.push_log("Convert: the file is not on disk (finish the download first).");
            return;
        }
        let Some(ffmpeg) = crate::engine::ffmpeg_path() else {
            self.push_log("Convert needs ffmpeg - get it in Settings -> Plugins.");
            return;
        };
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (suffix, args): (&str, &[&str]) = match kind {
            0 => (".mp3", &["-vn", "-c:a", "libmp3lame", "-q:a", "2"]),
            1 => (".mp4", &["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"]),
            _ => (" (small).mp4", &["-vf", "scale=-2:'min(720,ih)'", "-c:v", "libx264", "-preset", "veryfast", "-crf", "28", "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"]),
        };
        let mut out = src.with_file_name(format!("{stem}{suffix}"));
        if out == src {
            out = src.with_file_name(format!("{stem} (converted){suffix}"));
        }
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let tx = self.conv_tx.clone();
        let name = d.filename.clone();
        self.push_log(format!("Converting {name}…"));
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new(ffmpeg);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x08000000);
            }
            cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(&src).args(&args).arg(&out);
            let line = match cmd.output() {
                Ok(o) if o.status.success() => format!("Converted: {}", out.display()),
                Ok(o) => {
                    let _ = std::fs::remove_file(&out);
                    let err = String::from_utf8_lossy(&o.stderr);
                    format!("Convert {name} failed: {}", err.lines().last().unwrap_or("ffmpeg error"))
                }
                Err(e) => format!("Convert {name} failed: {e}"),
            };
            let _ = tx.send(line);
        });
    }

    /// On-demand integrity check for a finished file with a stored SHA-256.
    /// Hashing runs on a worker thread; the result is collected in `update()`
    /// so the GUI never blocks.
    fn verify_checksum(&mut self, id: u64) {
        let (dest, expected) = match self.downloads.iter().find(|d| d.id == id) {
            Some(d) => (d.dest_path(), d.checksum_sha256.clone()),
            None => return,
        };
        if expected.trim().is_empty() {
            self.push_log(format!(
                "Verify #{id}: no SHA-256 stored for this item (set one in Details → Settings)."
            ));
            return;
        }
        if !matches!(
            self.downloads.iter().find(|d| d.id == id).map(|d| &d.status),
            Some(Status::Completed)
        ) {
            self.push_log(format!("Verify #{id}: only finished files can be verified."));
            return;
        }
        if self.verify_jobs.iter().any(|(j, _)| *j == id) {
            self.push_log(format!("Verify #{id}: already running."));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::model::sha256_of_file(&dest));
        });
        self.verify_jobs.push((id, rx));
        self.push_log(format!("Verify #{id}: hashing…"));
    }

    /// Collect finished checksum jobs (called every frame).
    fn poll_verify_jobs(&mut self) {
        let mut done: Vec<(u64, anyhow::Result<String>)> = Vec::new();
        self.verify_jobs.retain(|(id, rx)| match rx.try_recv() {
            Ok(r) => {
                done.push((*id, r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        for (id, r) in done {
            let expected = self
                .downloads
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.checksum_sha256.clone())
                .unwrap_or_default();
            match r {
                Ok(actual)
                    if actual.eq_ignore_ascii_case(
                        crate::model::normalize_sha256(&expected).as_str(),
                    ) =>
                {
                    self.push_log(format!("Verify #{id}: OK (SHA-256 matches)."));
                    notify("Download Craft — checksum OK", &format!("#{id} verified"));
                }
                Ok(actual) => {
                    let short = if actual.len() > 16 { &actual[..16] } else { &actual[..] };
                    self.push_log(format!("Verify #{id}: MISMATCH (file starts {short}…)."));
                    notify("Download Craft — checksum mismatch", &format!("#{id} does not match"));
                }
                Err(e) => self.push_log(format!("Verify #{id}: failed ({e:#}).")),
            }
        }
    }

    fn remove_file(&mut self, id: u64) {
        if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            let dest = d.dest_path();
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            for p in Self::part_files(&dest) {
                let _ = std::fs::remove_file(p);
            }
        }
        self.remove(id);
    }

    /// Final save folder for a file (optionally into a category subfolder).
    fn save_dir_for(&self, filename: &str) -> String {
        if self.category_folders {
            // Ordered user rules first (P3.2), category fallback second.
            if let Some(rule) = self.folder_rules.iter().find(|r| r.matches(filename)) {
                if !rule.folder.trim().is_empty() {
                    let mut p = std::path::PathBuf::from(&self.add_dir);
                    p.push(rule.folder.trim());
                    return p.to_string_lossy().to_string();
                }
            }
            let cat = category_of(filename);
            let mut p = std::path::PathBuf::from(&self.add_dir);
            p.push(cat);
            p.to_string_lossy().to_string()
        } else {
            self.add_dir.clone()
        }
    }

    /// Probe a URL for size + server filename (used by the Add dialog).
    fn start_probe(&mut self, url: String) {
        if !is_download_url(&url) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.probe_rx = Some(rx);
        self.probing = true;
        self.probe_size = None;
        self.probe_name = None;
        let ua = if self.user_agent.trim().is_empty() {
            format!("download_craft/{}", crate::model::APP_VERSION)
        } else {
            self.user_agent.clone()
        };
        let opts = EngineOpts {
            user_agent: ua,
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            quality_pref: self.add_quality.trim().to_string(),
            ..Default::default()
        };
        self.rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => {
                    let (mut total, _, ctype, mut disp) =
                        crate::engine::probe(&client, &opts, &url).await;
                    // A playlist's length is its text, never the video size.
                    if crate::engine::is_stream_manifest(&url, ctype.as_deref(), "") {
                        total = None;
                    }
                    // Page URLs (YouTube watch / generic video pages) answer
                    // HEAD with HTML and no length → resolve the media stream
                    // first so the dialog shows the real size + title (IDM-style).
                    let is_page = ctype
                        .as_deref()
                        .map(|c| c.contains("text/html"))
                        .unwrap_or(false)
                        || url.contains("youtube.com")
                        || url.contains("youtu.be");
                    if total.is_none() && is_page {
                        let shared =
                            std::sync::Arc::new(crate::engine::SharedState::new());
                        if let Some(hit) = crate::engine::resolve_media(&client, &opts, &url, &shared).await
                        {
                            let (t2, _, _, _) =
                                crate::engine::probe(&client, &opts, &hit.url).await;
                            if total.is_none() {
                                total = t2;
                            }
                            // Pair audio adds to the finished size when the
                            // menu choice merges (IDM shows the merged total).
                            if let Some(pair) = hit.pair_url.filter(|p| p.starts_with("http")) {
                                let (ta, _, _, _) =
                                    crate::engine::probe(&client, &opts, &pair).await;
                                if let (Some(a), Some(b)) = (total, ta) {
                                    total = Some(a.saturating_add(b));
                                } else {
                                    total = total.or(ta);
                                }
                            }
                            if !hit.title.trim().is_empty()
                                && disp
                                    .as_deref()
                                    .map_or(true, crate::engine::weak_capture_name)
                            {
                                // A resolved title beats a capture-supplied
                                // one only when that one is meaningless
                                // ("50k", "videoplayback"): a real server
                                // filename still wins.
                                disp = Some(crate::engine::title_filename(&hit.title, None));
                            }
                        }
                    }
                    (total, disp)
                }
                Err(_) => (None, None),
            };
            let _ = tx.send(out);
        });
    }
    /// Start a site-grabber crawl on the Tokio runtime (P3.1).
    fn start_grab(&mut self) {
        let url = self.grab_url.trim().to_string();
        if !is_download_url(&url) {
            self.grab_notice = "Need an http(s) page URL.".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.grab_rx = Some(rx);
        self.grab_busy = true;
        self.grab_results.clear();
        self.grab_notice.clear();
        let exts = crate::grabber::parse_ext_list(&self.grab_exts);
        let depth = if self.grab_depth { self.grab_levels.clamp(1, 5) } else { 0 };
        let stay = self.grab_stay;
        let ua = if self.user_agent.trim().is_empty() {
            format!("download_craft/{}", crate::model::APP_VERSION)
        } else {
            self.user_agent.clone()
        };
        // Settings -> Site logins also open pages behind a login.
        let (username, password) = crate::model::login_for(&url).unwrap_or_default();
        let opts = EngineOpts {
            user_agent: ua,
            timeout_secs: self.timeout_secs,
            proxy_url: self.proxy_url.clone(),
            username,
            password,
            ..Default::default()
        };
        self.rt.spawn(async move {
            let out = match opts.build_client() {
                Ok(client) => crate::grabber::grab_site(&client, &opts, &url, &exts, depth, stay, crate::grabber::MAX_PAGES_DEFAULT * (depth.max(1) * 2))
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(out);
        });
    }

    /// Queue all grabber results through the same dedupe + dest-clash
    /// refresh path as every other capture (P4): re-grabs refresh stale
    /// links instead of colliding, and the queue starts when running.
    fn queue_grab_results(&mut self) {
        if self.grab_results.is_empty() {
            self.grab_notice = "Grab a page first.".to_string();
            return;
        }
        let mut n = 0;
        let results = std::mem::take(&mut self.grab_results);
        for l in results {
            let fname = crate::model::apply_rename_template(&self.rename_template, &l.name, &l.url);
            if self.queue_url(l.url, Some(fname), false, None) {
                n += 1;
            }
        }
        if self.queue_running {
            self.fill_next();
        }
        self.push_log(format!("Grabber queued {n} link(s)."));
        self.grab_notice.clear();
    }

    /// Recompute the live scheduler cap from speed windows (P3.4).
    /// u64::MAX = no window active (per-download caps apply as usual).
    fn refresh_speed_limit(&self) {
        let window = if self.speed_windows_on {
            let now = chrono::Local::now();
            let now_min = (now.format("%H").to_string().parse::<u16>().unwrap_or(0)) * 60
                + now.format("%M").to_string().parse::<u16>().unwrap_or(0);
            self.speed_windows
                .iter()
                .find(|w| w.active_at(now_min))
                .map(|w| w.limit_kbps * 1024)
                .filter(|v| *v > 0)
        } else {
            None
        };
        // The global limit travels live too: tasks start without a frozen
        // copy of it, so changing it (progress window, slider, Unlimited)
        // reaches downloads already running instead of only the next ones.
        let global = Some(self.speed_limit_kbps * 1024).filter(|v| *v > 0);
        let v = match (global, window) {
            (Some(g), Some(w)) => g.min(w),
            (Some(g), None) => g,
            (None, Some(w)) => w,
            (None, None) => u64::MAX, // no cap at all
        };
        self.speed_limit_live.store(v, Ordering::SeqCst);
    }

    /// One-line status for the Speed windows section.
    fn speed_window_status(&self) -> String {
        if !self.speed_windows_on {
            return "Windows off — global limit applies.".to_string();
        }
        let now = chrono::Local::now();
        let now_min = now.format("%H").to_string().parse::<u16>().unwrap_or(0) * 60
            + now.format("%M").to_string().parse::<u16>().unwrap_or(0);
        match self.speed_windows.iter().find(|w| w.active_at(now_min)) {
            Some(w) if w.limit_kbps == 0 => {
                format!("Active now: {}–{} full speed.", w.start, w.end)
            }
            Some(w) => format!("Active now: {}–{} at {} KB/s.", w.start, w.end, w.limit_kbps),
            None => "No window active right now.".to_string(),
        }
    }

    /// The OS caption is off (`main.rs`), so these edge strips do what the
    /// native frame used to: a resize cursor on hover and a resize drag on
    /// press. A maximized window has no edges to grab.
    ///
    /// Corners come first, so a press in a corner is never taken by one of
    /// the straight edges, and only the first matching zone is acted on.
    fn handle_window_edges(&mut self, ctx: &egui::Context) {
        use egui::viewport::ResizeDirection as Dir;
        if self.os_caption || ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let Some(q) = self.man.pos() else {
            return;
        };
        let r = ctx.screen_rect();
        let z = 5.0;
        let c = 12.0;
        let over = |a: egui::Pos2, b: egui::Pos2| egui::Rect::from_min_max(a, b);
        let zones: [(egui::Rect, Dir, egui::CursorIcon); 8] = [
            (over(r.min, egui::pos2(r.min.x + c, r.min.y + c)), Dir::NorthWest, egui::CursorIcon::ResizeNorthWest),
            (over(egui::pos2(r.max.x - c, r.min.y), egui::pos2(r.max.x, r.min.y + c)), Dir::NorthEast, egui::CursorIcon::ResizeNorthEast),
            (over(egui::pos2(r.min.x, r.max.y - c), egui::pos2(r.min.x + c, r.max.y)), Dir::SouthWest, egui::CursorIcon::ResizeSouthWest),
            (over(egui::pos2(r.max.x - c, r.max.y - c), r.max), Dir::SouthEast, egui::CursorIcon::ResizeSouthEast),
            (over(egui::pos2(r.min.x + c, r.min.y), egui::pos2(r.max.x - c, r.min.y + z)), Dir::North, egui::CursorIcon::ResizeNorth),
            (over(egui::pos2(r.min.x + c, r.max.y - z), egui::pos2(r.max.x - c, r.max.y)), Dir::South, egui::CursorIcon::ResizeSouth),
            (over(egui::pos2(r.min.x, r.min.y + c), egui::pos2(r.min.x + z, r.max.y - c)), Dir::West, egui::CursorIcon::ResizeWest),
            (over(egui::pos2(r.max.x - z, r.min.y + c), egui::pos2(r.max.x, r.max.y - c)), Dir::East, egui::CursorIcon::ResizeEast),
        ];
        for (rect, dir, icon) in zones {
            if rect.contains(q) {
                ctx.set_cursor_icon(icon);
                if self.man.press_started_in(&rect) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                }
                return;
            }
        }
    }

    /// Send the window to the tray (the title bar's close button, or the OS
    /// close while the tray is on). Parking, not hiding: a hidden window
    /// stops painting, which freezes transfers and the browser inbox.
    /// True from the second frame on: the native window and its handle
    /// exist by then.
    fn frame_count_for_startup(&mut self) -> bool {
        self.startup_frames = self.startup_frames.saturating_add(1);
        self.startup_frames >= 2
    }

    fn park_to_tray(&mut self) {
        crate::tray::park();
        // Say where the window went the first time, or a vanish reads as a
        // crash (the tray icon is easy to miss). Once per run.
        static HID_HINT: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !HID_HINT.swap(true, std::sync::atomic::Ordering::SeqCst) {
            notify(
                "Download Craft is still running",
                "In the tray — left-click its icon to reopen, right-click for Exit.",
            );
        }
    }

    /// What the About window draws: the update state, written to
    /// `update_state.json` whenever it changes (the window is its own
    /// process; `cmd.json` carries its buttons back).
    fn publish_update_state(&mut self) {
        let phase = if self.update_restart_ready {
            "ready"
        } else if self.selfupd_rx.is_some() {
            "installing"
        } else if self.update_busy {
            "checking"
        } else if self.update_found && self.update_asset.is_some() {
            "available"
        } else if self.update_status.starts_with("Update failed") {
            "failed"
        } else if self.update_status.starts_with("Up to date") {
            "uptodate"
        } else {
            "idle"
        };
        let state = serde_json::json!({
            "phase": phase,
            "status": self.update_status,
            "size": self.update_asset.as_ref().map(|a| a.size).unwrap_or(0),
            "running": self.tasks.len(),
            "portable": cfg!(windows) && crate::selfupdate::portable_dir().is_some(),
        });
        let text = state.to_string();
        if text != self.update_state_sent {
            if let Some(p) = crate::model::config_dir().map(|d| d.join("update_state.json")) {
                if crate::model::atomic_write(&p, text.as_bytes()).is_ok() {
                    self.update_state_sent = text;
                }
            }
        }
    }

    /// One-shot opt-in update check against GitHub releases (P3.4, P5 asset).
    fn start_update_check(&mut self) {
        if self.update_busy {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_rx = Some(rx);
        self.update_busy = true;
        self.update_status.clear();
        self.update_asset = None;
        self.update_found = false;
        self.rt.spawn(async move {
            let _ = tx.send(update_check_async().await);
        });
    }

    /// One-click update of the portable build: download and check the zip,
    /// replace the app's files (all or nothing), restart into the new
    /// version. Anything that cannot be done that way (no portable folder,
    /// a failure on the way) falls back to downloading the zip.
    fn start_self_update(&mut self) {
        let Some(asset) = self.update_asset.clone() else {
            self.push_log("No update known — check for updates first.");
            return;
        };
        let target = if cfg!(windows) { crate::selfupdate::portable_dir() } else { None };
        let Some(target) = target else {
            self.update_status = "This copy cannot update itself (its folder is not writable, or it is a developer build): the update zip is downloading instead.".into();
            self.download_update_asset();
            return;
        };
        if self.selfupd_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.selfupd_rx = Some(rx);
        self.update_busy = true;
        self.update_status = format!("Downloading {}…", asset.name);
        let ua = format!("download_craft/{}", crate::model::APP_VERSION);
        self.rt.spawn(async move {
            let res: anyhow::Result<usize> = async {
                let client = reqwest::Client::builder().user_agent(ua).build()?;
                let txp = tx.clone();
                let mut last = -1i32;
                let zip = crate::selfupdate::download(&client, &asset, move |f| {
                    let pct = (f * 100.0) as i32;
                    if pct != last && pct % 5 == 0 {
                        last = pct;
                        let _ = txp.send(SelfUpdMsg::Status(format!("Downloading update… {pct}%")));
                    }
                })
                .await?;
                let _ = tx.send(SelfUpdMsg::Status("Checked. Installing…".into()));
                tokio::task::spawn_blocking(move || {
                    let dir = crate::selfupdate::unpack(&zip)?;
                    crate::selfupdate::install(&dir, &target)
                })
                .await?
            }
            .await;
            let _ = tx.send(match res {
                Ok(n) => SelfUpdMsg::Installed(n),
                Err(e) => SelfUpdMsg::Failed(format!("{e:#}")),
            });
        });
    }

    fn poll_self_update(&mut self) {
        let Some(rx) = self.selfupd_rx.as_ref() else { return };
        let msgs: Vec<SelfUpdMsg> = rx.try_iter().collect();
        for m in msgs {
            match m {
                SelfUpdMsg::Status(s) => self.update_status = s,
                SelfUpdMsg::Installed(n) => {
                    self.selfupd_rx = None;
                    self.update_busy = false;
                    self.update_found = false;
                    self.push_log(format!("Update installed ({n} files)."));
                    self.update_restart_ready = true;
                    if self.tasks.is_empty() {
                        self.update_auto = false;
                        notify("Download Craft updated", "Restarting into the new version…");
                        self.restart_for_update();
                    } else {
                        self.update_status = format!(
                            "Update installed. Restart to use it ({} download(s) still running resume after the restart).",
                            self.tasks.len()
                        );
                    }
                }
                SelfUpdMsg::Failed(e) => {
                    self.selfupd_rx = None;
                    self.update_busy = false;
                    self.push_log(format!("Update failed: {e}"));
                    if std::mem::take(&mut self.update_auto) {
                        // Automatic attempt: nothing changed; the Update button
                        // stays for a retry (no zip dropped in Downloads).
                        self.update_status = format!("Update failed ({e}). Nothing was changed — click Update to try again.");
                    } else {
                        self.update_status = format!("Update failed ({e}). Nothing was changed; the update zip is downloading instead.");
                        self.download_update_asset();
                    }
                }
            }
        }
    }

    /// Start the new version and close this one cleanly (the list and the
    /// settings are saved on the way out).
    fn restart_for_update(&mut self) {
        self.save_queue();
        self.save_ui_state();
        match crate::selfupdate::relaunch() {
            Ok(()) => {
                self.update_status = "Restarting into the new version…".into();
                #[cfg(windows)]
                crate::tray::request_quit();
                #[cfg(not(windows))]
                std::process::exit(0);
            }
            Err(e) => {
                self.update_status = format!("Update installed, but the restart failed ({e:#}): close and reopen Download Craft.");
            }
        }
    }

    /// Queue the portable update asset as a normal download (P5).
    /// Never self-replaces: the .zip lands in Downloads and the log +
    /// notification tell the user where to unzip it.
    fn download_update_asset(&mut self) {
        let Some(url) = self.update_asset.as_ref().map(|a| a.url.clone()) else {
            self.push_log("No update asset known — check for updates first.");
            return;
        };
        let fname = url
            .split(['?', '#'])
            .next()
            .unwrap_or(&url)
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "download_craft_update.zip".to_string());
        if self.queue_url(url, Some(fname.clone()), true, None) {
            self.push_log(format!("Downloading update: {fname}"));
            notify("Download Craft — update", &format!("Downloading {fname}"));
        } else {
            self.push_log("Update already queued.");
        }
    }

    /// Queue one URL programmatically (used by inbox + import).
    /// Returns true when the URL was newly queued or an existing same-file
    /// entry was refreshed (P4 grab-all counts on this).
    fn queue_url(&mut self, url: String, filename: Option<String>, autostart: bool, size: Option<u64>) -> bool {
        if !is_download_url(&url) {
            return false;
        }
        let media = self.captured_media.get(&url).copied().unwrap_or_default();
        // Same URL already tracked. A live row is left alone; a dead row
        // (failed, or finished with its file gone) is refreshed so a
        // re-capture is never silently dropped.
        if let Some(pos) = self.downloads.iter().position(|d| d.url == url) {
            let dead = match &self.downloads[pos].status {
                Status::Failed(_) => true,
                Status::Completed => !self.downloads[pos].dest_path().exists(),
                _ => false,
            };
            if !dead {
                return false;
            }
            let id = self.downloads[pos].id;
            let fname = self.downloads[pos].filename.clone();
            if let Some(s) = size.filter(|s| *s > 0) {
                self.downloads[pos].total_bytes = Some(s);
            }
            self.downloads[pos].duration_secs = media.0;
            self.downloads[pos].expects_audio = media.1;
            self.downloads[pos].downloaded_bytes = 0;
            self.downloads[pos].speed_bps = 0.0;
            self.downloads[pos].status = Status::Queued;
            let dp = self.downloads[pos].dest_path();
            crate::model::drop_resume_state(&dp);
            self.push_log(format!("Re-queued: {fname}"));
            if autostart {
                self.start(id);
            }
            self.save_queue();
            return true;
        }
        let fname = filename
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| filename_from_url(&url));
        let mut fname = crate::model::apply_rename_template(&self.rename_template, &fname, &url);
        let dir = self.save_dir_for(&fname);
        let mut dest = std::path::Path::new(&dir).join(&fname);
        // A row that is running or paused keeps its download: a different
        // link that wants the same file name gets a numbered name instead.
        // Taking it over swapped the URL and deleted partial files mid-transfer.
        if let Some(pos) = crate::model::find_dest_clash(&self.downloads, &dest) {
            let busy = self.tasks.contains_key(&self.downloads[pos].id)
                || matches!(self.downloads[pos].status, Status::Downloading | Status::Paused);
            if busy && self.downloads[pos].url != url {
                let free = crate::model::conflict_free_dest(&dest);
                let taken = |p: &std::path::Path| self.downloads.iter().any(|d| d.dest_path() == p);
                let mut n = 1;
                let mut cand = free;
                while taken(&cand) && n < 1000 {
                    let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    cand = std::path::Path::new(&dir).join(format!("{stem} ({n}){ext}"));
                    n += 1;
                }
                if let Some(name) = cand.file_name() {
                    fname = name.to_string_lossy().to_string();
                }
                dest = cand;
            }
        }
        // Same file, fresh URL (re-capture): refresh in place instead of a
        // colliding duplicate (two tasks on one dest fail loudly and can
        // corrupt resume state across different URLs).
        if let Some(pos) = crate::model::find_dest_clash(&self.downloads, &dest) {
            let id = self.downloads[pos].id;
            if self.downloads[pos].url != url {
                self.downloads[pos].url = url;
                self.downloads[pos].downloaded_bytes = 0;
                crate::model::drop_resume_state(&dest);
            }
            self.downloads[pos].duration_secs = media.0;
            self.downloads[pos].expects_audio = media.1;
            self.downloads[pos].total_bytes = size.filter(|s| *s > 0);
            self.downloads[pos].speed_bps = 0.0;
            self.downloads[pos].status = Status::Queued;
            self.push_log(format!("Refreshed link: {fname}"));
            if autostart {
                self.start(id);
            }
            self.save_queue();
            return true;
        }
        let mut item = DownloadItem::new(
            0,
            url,
            fname,
            dir,
            self.connections_default,
        );
        // Browser-measured size survives queueing (see confirm::commit).
        item.total_bytes = size.filter(|s| *s > 0);
        item.duration_secs = media.0;
        item.expects_audio = media.1;
        if !self.persist_new_item(&mut item) { return false; }
        let id=item.id;
        self.downloads.retain(|d|d.id!=id);
        self.downloads.push(item);
        if autostart {
            self.start(id);
        }
        self.save_queue();
        true
    }

    fn import_list(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("URL lists and IDM exports", &["txt", "list", "urls", "ef2"])
            .set_title("Import URL list")
            .pick_file();
        let Some(path) = file else { return };
        let Ok(data) = std::fs::read_to_string(&path) else {
            self.push_log("Import failed: could not read that file.");
            return;
        };
        let mut n = 0;
        for l in crate::model::parse_url_list(&data) {
            if self.downloads.iter().any(|d| d.url == l.url) {
                continue;
            }
            let name = (!l.filename.is_empty()).then(|| l.filename.clone());
            if self.queue_url(l.url.clone(), name, false, None) {
                if let Some(d) = self.downloads.iter_mut().find(|d| d.url == l.url) {
                    d.referer = l.referer.clone();
                    d.cookie = l.cookie.clone();
                }
                n += 1;
            }
        }
        if n > 0 {
            self.queue_running = true;
            self.fill_next();
        }
        self.push_log(format!("Imported {n} URL(s) from list."));    }

    fn export_list(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("Text", &["txt"])
            .set_file_name("download_craft_urls.txt")
            .set_title("Export URL list")
            .save_file();
        let Some(path) = file else { return };
        let body: String = self
            .downloads
            .iter()
            .map(|d| d.url.clone())
            .collect::<Vec<_>>()
            .join("\n");
        match std::fs::write(&path, body) {
            Ok(()) => self.push_log(format!("Exported to {}.", path.display())),
            Err(e) => self.push_log(format!("Export failed: {e}")),
        }
    }

    /// Atomically write `data` to `path` (temp + rename).
    fn atomic_write(path: &std::path::Path, data: &[u8]) {
        let _ = crate::model::atomic_write(path, data);
    }

    fn save_queue(&mut self) {
        let _guard = crate::model::fs_lock("queue");
        if !_guard.owned() { return; }
        let Some(path) = Self::queue_path() else {
            return;
        };
        let mut items = self.downloads.clone();
        for it in &mut items {
            it.speed_bps = 0.0;
            if matches!(it.status, Status::Completed) {
                it.cookie.clear();
                it.extra_headers.clear();
            }
        }
        // Union with foreign additions made since our last sync (confirm /
        // basket / CLI). Our own deletions stay deleted: ids we have ever
        // seen but no longer hold are dropped, unknown ids are merged in.
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(disk) = serde_json::from_str::<Vec<DownloadItem>>(data.trim_start_matches('\u{feff}')) {
                for it in disk {
                    // Same row = same id, or same URL at the same quality: one
                    // page queued at 1080p and 720p is two downloads.
                    if items.iter().any(|d| {
                        d.id == it.id
                            || (d.url == it.url
                                && crate::model::same_quality(&d.quality_pref, &it.quality_pref))
                    }) {
                        continue;
                    }
                    if self.known_ids.contains(&it.id) {
                        continue; // deleted here; do not resurrect
                    }
                    let mut it = it;
                    it.speed_bps = 0.0;
                    it.selected = false;
                    items.push(it);
                }
            }
        }
        for it in &items {
            self.known_ids.insert(it.id);
        }
        if let Some(m) = items.iter().map(|d| d.id).max() {
            self.next_id = self.next_id.max(m + 1);
        }
        if let Ok(json) = serde_json::to_string_pretty(&items) {
            Self::atomic_write(&path, json.as_bytes());
        }
    }

    /// Assign an id AND persist the row under one lock. Reserving only the id
    /// allowed a dialog to allocate the same id before our next queue save.
    fn persist_new_item(&mut self, item: &mut DownloadItem) -> bool {
        let mut queued=item.clone();
        match crate::model::append_to_queue(&mut queued) {
            crate::model::AppendResult::Added(id) => {
                item.id=id;
                self.next_id=self.next_id.max(id+1);
                self.known_ids.insert(id);
                true
            }
            crate::model::AppendResult::AlreadyQueued(id) => {
                self.push_log(format!("Already queued as #{id}."));false
            }
            crate::model::AppendResult::Error(e) => { self.push_log(format!("Could not queue download: {e}"));false }
        }
    }

    /// Parse queue items tolerantly: one bad entry (schema drift, partial
    /// write) must not nuke the whole queue. Returns (good, had_bad).
    fn parse_queue_items(data: &str) -> (Vec<DownloadItem>, bool) {
        // A BOM (Notepad/PowerShell saves) made the whole queue unreadable:
        // it loaded empty and the next save wrote "[]" over it.
        let data = data.trim_start_matches('\u{feff}');
        if data.trim().is_empty() {
            return (Vec::new(), false);
        }
        if let Ok(items) = serde_json::from_str::<Vec<DownloadItem>>(data) {
            return (items, false);
        }
        let mut good = Vec::new();
        let mut bad = false;
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(data) {
            for v in arr {
                match serde_json::from_value::<DownloadItem>(v) {
                    Ok(it) => good.push(it),
                    Err(_) => bad = true,
                }
            }
        } else {
            bad = true;
        }
        (good, bad)
    }

    fn quarantine_queue_file(data: &str) {
        if let Some(dir) = crate::model::config_dir() {
            let name = format!("queue.corrupt-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            let _ = std::fs::write(dir.join(name), data);
        }
    }

    fn load_queue(&mut self) {
        if let Some(path) = Self::queue_path() {
            if let Ok(data) = std::fs::read_to_string(path) {
                let (items, had_bad) = Self::parse_queue_items(&data);
                if had_bad {
                    Self::quarantine_queue_file(&data);
                    self.push_log("Queue file had unreadable entries — kept the good ones, quarantined the rest.");
                }
                for mut it in items {
                        if it.status.is_active() {
                            it.status = Status::Queued;
                        }
                        // drop phantom sizes from failed zero-byte transfers
                        if it.downloaded_bytes == 0 {
                            it.total_bytes = None;
                        }
                        // past zero-byte "completions" with no file were phantom successes
                        if matches!(it.status, Status::Completed)
                            && it.downloaded_bytes == 0
                            && !it.dest_path().exists()
                        {
                            it.status =
                                Status::Failed("empty result — resume to retry".into());
                        }
                        it.speed_bps = 0.0;
                        it.selected = false;
                        self.next_id = self.next_id.max(it.id + 1);
                        self.known_ids.insert(it.id);
                        self.downloads.push(it);
                    }
                    self.push_log(format!("Loaded {} item(s).", self.downloads.len()));
                }
            }
        }

    fn start(&mut self, id: u64) {
        let idx = match self.downloads.iter().position(|d| d.id == id) {
            Some(i) => i,
            None => return,
        };
        if self.tasks.contains_key(&id) {
            return;
        }
        // IDM-style: a download that begins gets its own progress window, no
        // matter which path started it (Add dialog, capture popup, Redownload,
        // Resume, Start Queue, retry-all). The window claims a per-download
        // slot, so a start while one is already open never stacks duplicates,
        // while a window the user closed comes straight back on Retry. Users
        // who prefer only the main list turn this off in Settings; the row's
        // Show download window still opens one by hand.
        if self.progress_windows {
            if let Err(e) = Self::spawn_progress(id) {
                self.push_log(format!("Could not open the progress window for #{id}: {e}"));
            }
        }
        // The live limit is the task's only cap: make it current first.
        self.refresh_speed_limit();
        let item = self.downloads[idx].clone();
        let shared = Arc::new(SharedState::new());
        if let Some(t) = item.total_bytes {
            shared.total.store(t, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
        shared
            .downloaded
            .store(item.downloaded_bytes, Ordering::SeqCst);
        let url = item.url.clone();
        let dest = item.dest_path();
        let conns = item.connections;
        let opts = EngineOpts {
            // The global limit is applied live through `global_limit` (see
            // refresh_speed_limit), not frozen into the task at start.
            speed_limit_bps: 0,
            timeout_secs: item.eff_timeout(self.timeout_secs),
            max_retries: item.eff_retries(self.max_retries),
            user_agent: if self.user_agent.trim().is_empty() {
                format!("download_craft/{}", crate::model::APP_VERSION)
            } else {
                self.user_agent.clone()
            },
            referer: item.referer.clone(),
            cookie: item.cookie.clone(),
            captured_headers: crate::engine::CapturedHeaders::for_url(&item.url, &item.extra_headers),
            username: item.username.clone(),
            password: item.password.clone(),
            proxy_url: self.proxy_url.clone(),
            allow_rename: true,
            expected_sha256: item.checksum_sha256.clone(),
            expected_duration_secs: item.duration_secs,
            require_audio: item.expects_audio || !item.pair_url.is_empty(),
            overwrite_existing: item.overwrite_existing,
            quality_pref: item.quality_pref.clone(),
            global_limit: Some(self.speed_limit_live.clone()),
            per_host_conns: self.per_host_conns,
            browser: item.browser.clone(),
            ytdlp_visitor_data: String::new(),
            log_key: item.id.to_string(),
            mirrors: item.mirrors.clone(),
            http2: false,
            any_ip: false,
            impersonate: false,
            digest: Default::default(),
        };
        let shared2 = shared.clone();
        // Adaptive pairs (video-only + audio) merge via ffmpeg on completion.
        let pair = if item.pair_url.starts_with("http") {
            Some(item.pair_url.clone())
        } else {
            None
        };
        let handle = self.rt.spawn(async move {
            let r = match pair {
                Some(a) => {
                    crate::engine::download_adaptive(url, Some(a), dest, conns, opts, shared2.clone()).await
                }
                None => download(url, dest, conns, opts, shared2.clone()).await,
            };
            if let Err(e) = r {
                shared2.set_error(format!("{e:#}"));
                eprintln!("download failed: {e:#}");
            } else {
                shared2.succeeded.store(true, Ordering::SeqCst);
            }
        });
        self.tasks.insert(
            id,
            ActiveTask {
                shared,
                handle,
                last_sample: Instant::now(),
                last_bytes: self.downloads[idx].downloaded_bytes,
            },
        );
        self.downloads[idx].status = Status::Downloading;
        self.downloads[idx].speed_bps = 0.0;
        self.had_active = true;
        self.push_log(format!("Started: {}", item.filename));
        self.save_queue();
    }

    /// Start queued items while under the concurrent limit.
    /// Failed items are NOT picked up here: retrying them in a hot loop
    /// spams failures forever (a failed YouTube bot-check would log
    /// endlessly). The user retries explicitly via Resume/Retry or
    /// Tasks → Retry all failed.
    /// The queue a row runs in: its named queue when that still exists, else
    /// Main ("").
    fn queue_of(&self, d: &DownloadItem) -> String {
        if !d.queue.is_empty() && self.queues.iter().any(|q| q.name == d.queue) {
            d.queue.clone()
        } else {
            String::new()
        }
    }

    fn fill_next(&mut self) {
        let mut runs: Vec<(String, usize)> = Vec::new();
        if self.queue_running {
            runs.push((String::new(), self.max_concurrent.max(1)));
        }
        for q in &self.queues {
            if q.running {
                runs.push((q.name.clone(), q.max.max(1)));
            }
        }
        for (name, cap) in runs {
            self.fill_queue(&name, cap);
        }
    }

    fn fill_queue(&mut self, queue: &str, cap: usize) {
        loop {
            let active = self
                .tasks
                .keys()
                .filter(|id| self.downloads.iter().any(|d| d.id == **id && self.queue_of(d) == queue))
                .count();
            if active >= cap {
                return;
            }
            let next = self
                .downloads
                .iter()
                .filter(|d| matches!(d.status, Status::Queued))
                .filter(|d| self.queue_of(d) == queue)
                // A Queued row whose task still runs (a refresh re-queued it)
                // is not startable: `start` returns without changing it, and
                // picking it again looped forever and froze the window.
                .filter(|d| !self.tasks.contains_key(&d.id))
                // Queue v2: a row scheduled for later waits even when a slot
                // is free, and priority decides who goes first.
                .filter(|d| crate::model::start_at_due(&d.start_at, chrono::Local::now()))
                .max_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        // Same priority: oldest first (stable, predictable).
                        .then_with(|| b.created_at.cmp(&a.created_at))
                })
                .map(|d| d.id);
            match next {
                Some(id) => self.start(id),
                None => return,
            }
        }
    }

    /// Queued rows in the order the pump will start them.
    fn queued_order(&self) -> Vec<u64> {
        let mut rows: Vec<&DownloadItem> = self
            .downloads
            .iter()
            .filter(|d| matches!(d.status, Status::Queued))
            .collect();
        rows.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        rows.into_iter().map(|d| d.id).collect()
    }

    /// Move one queued row up (-1) or down (+1) in the pump's order by nudging
    /// its priority past the neighbour's. Ties keep creation order, so an
    /// untouched queue still runs oldest-first.
    fn move_in_queue(&mut self, id: u64, dir: i32) {
        let order = self.queued_order();
        let Some(pos) = order.iter().position(|x| *x == id) else {
            return;
        };
        let target = pos as i32 + dir;
        if target < 0 || target as usize >= order.len() {
            return;
        }
        let other = order[target as usize];
        let other_prio = self
            .downloads
            .iter()
            .find(|d| d.id == other)
            .map(|d| d.priority)
            .unwrap_or(0);
        let new_prio = if dir < 0 { other_prio + 1 } else { other_prio - 1 };
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            d.priority = new_prio;
        }
        self.save_queue();
        self.push_log(format!(
            "Moved #{} {} the queue.",
            id,
            if dir < 0 { "up in" } else { "down in" }
        ));
    }

    fn pause(&mut self, id: u64) {
        if self.tasks.get(&id).map(|t|t.shared.external_helper.load(Ordering::SeqCst)).unwrap_or(false) {
            // yt-dlp has no portable suspend command. Stop its process now;
            // Resume starts a fresh helper, retaining its .part/.ytdl files.
            self.cancel_task(id);
            self.push_log(format!("Paused #{id}; Resume continues the media download from available partial files."));
            return;
        }
        if let Some(t) = self.tasks.get(&id) {
            t.shared.paused.store(true, Ordering::SeqCst);
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                d.status = Status::Paused;
            }
            self.push_log(format!("Paused #{id}"));
            self.save_queue();
        } else if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            // Taskless (queued) items pause directly so the button never
            // looks dead; the pump skips Paused.
            if matches!(d.status, Status::Queued) {
                d.status = Status::Paused;
                self.push_log(format!("Paused #{id}"));
                self.save_queue();
            }
        }
    }

    fn resume(&mut self, id: u64) {
        if let Some(t) = self.tasks.get(&id) {
            t.shared.paused.store(false, Ordering::SeqCst);
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                d.status = Status::Downloading;
            }
            // The window may have been closed while the transfer sat paused:
            // reopening is cheap and a live one is left alone by its slot.
            // Only when per-download windows are on, though: with them off,
            // Resume must not pop a window the user never asked for.
            if self.progress_windows {
                let _ = Self::spawn_progress(id);
            }
            self.push_log(format!("Resumed #{id}"));
            self.save_queue();
        } else if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
            if matches!(d.status, Status::Paused | Status::Queued | Status::Failed(_)) {
                if let Some(dd) = self.downloads.iter_mut().find(|d| d.id == id) {
                    if matches!(dd.status, Status::Failed(_)) {
                        dd.status = Status::Queued;
                    }
                }
                self.start(id);
            }
        }
    }

    fn cancel_task(&mut self, id: u64) {
        // A live recording has no "later": Stop means finish. Whatever was
        // written is already a playable file, so the row becomes Completed
        // instead of Paused (which would imply a resumable partial).
        let live = self
            .tasks
            .get(&id)
            .map(|t| t.shared.live.load(Ordering::SeqCst))
            .unwrap_or(false);
        let mut stopped: Option<(String, String)> = None;
        if let Some(t) = self.tasks.remove(&id) {
            t.shared.cancelled.store(true, Ordering::SeqCst);
            t.handle.abort();
        }
        // The stage label belongs to the running helper, not the row.
        self.phases.remove(&id);
        // Reflect the stop immediately (the finish handler would only get
        // here on its next poll): a cancelled transfer rests Paused with
        // its partial state kept for resume.
        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
            if !matches!(d.status, Status::Completed) {
                d.status = if live && d.downloaded_bytes > 0 {
                    Status::Completed
                } else {
                    Status::Paused
                };
            }
            if live {
                stopped = Some((
                    d.filename.clone(),
                    crate::model::format_bytes(d.downloaded_bytes),
                ));
            }
        }
        if let Some((name, size)) = stopped {
            self.push_log(format!("Live capture stopped: {name} — {size} saved"));
        }
        self.save_queue();
    }

    fn remove(&mut self, id: u64) {
        self.cancel_task(id);
        self.downloads.retain(|d| d.id != id);
        self.push_log(format!("Removed #{id}"));
        self.save_queue();
    }

    /// Files dropped on the window: `.url` shortcuts and text lists. One link
    /// opens the Add dialog with it; several open the pick-and-queue window.
    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let mut urls: Vec<String> = Vec::new();
        for f in dropped {
            if let Some(p) = f.path {
                for u in crate::basket::urls_from_dropped(&p) {
                    if !urls.contains(&u) {
                        urls.push(u);
                    }
                }
            }
        }
        match urls.len() {
            0 => self.push_log("Nothing to download in the dropped file (drop .url shortcuts or text lists of links)."),
            1 => {
                self.add_file = filename_from_url(&urls[0]);
                self.add_url = urls.remove(0);
                self.add_focus_req = true;
                self.show_add = true;
            }
            n => {
                let links: Vec<crate::native_host::PageLink> =
                    urls.into_iter().map(|url| crate::native_host::PageLink { url, text: String::new() }).collect();
                if let Err(e) = crate::native_host::open_links_window("", "Dropped links", "", &links) {
                    self.push_log(format!("Dropped links: {e}"));
                } else {
                    self.push_log(format!("{n} dropped links - pick them in the window that opened."));
                }
            }
        }
    }

    /// Named queues: start/stop at their times, and sync finished files.
    fn tick_queues(&mut self) {
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let hhmm = now.format("%H:%M").to_string();
        let mut start: Vec<usize> = Vec::new();
        let mut stop: Vec<usize> = Vec::new();
        for (i, q) in self.queues.iter_mut().enumerate() {
            if !q.start_at.is_empty() && q.start_at == hhmm && q.fired_start != today {
                q.fired_start = today.clone();
                start.push(i);
            }
            if !q.stop_at.is_empty() && q.stop_at == hhmm && q.fired_stop != today {
                q.fired_stop = today.clone();
                stop.push(i);
            }
        }
        for i in start {
            self.queues[i].running = true;
            self.push_log(format!("Queue {} started on schedule.", self.queues[i].name));
        }
        for i in stop {
            self.stop_named_queue(i);
            self.push_log(format!("Queue {} stopped on schedule.", self.queues[i].name));
        }
        if !self.queues.iter().any(|q| q.running) && self.sync_rx.is_none() && !self.queues.iter().any(|q| q.sync_hours > 0) {
            return;
        }
        self.fill_next();
        // Queue sync: ask the server for each finished file's size.
        let due: Vec<String> = self
            .queues
            .iter_mut()
            .filter(|q| q.sync_hours > 0 && q.last_sync.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(q.sync_hours * 3600)))
            .map(|q| {
                q.last_sync = Some(Instant::now());
                q.name.clone()
            })
            .collect();
        if !due.is_empty() {
            if self.sync_tx.is_none() {
                let (tx, rx) = std::sync::mpsc::channel();
                self.sync_tx = Some(tx);
                self.sync_rx = Some(rx);
            }
            let tx = self.sync_tx.clone().unwrap();
            let rows: Vec<(u64, String, String, String)> = self
                .downloads
                .iter()
                .filter(|d| matches!(d.status, Status::Completed) && due.contains(&self.queue_of(d)))
                .map(|d| (d.id, d.url.clone(), d.referer.clone(), d.cookie.clone()))
                .collect();
            let ua = self.user_agent.clone();
            self.rt.spawn(async move {
                for (id, url, referer, cookie) in rows {
                    let opts = EngineOpts { user_agent: ua.clone(), referer, cookie, timeout_secs: 20, ..Default::default() };
                    let size = match opts.build_client() {
                        Ok(c) => crate::engine::probe(&c, &opts, &url).await.0,
                        Err(_) => None,
                    };
                    let _ = tx.send((id, size));
                }
            });
        }
        let answers: Vec<(u64, Option<u64>)> = self.sync_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for (id, size) in answers {
            let Some(d) = self.downloads.iter().find(|d| d.id == id) else { continue };
            if let (Some(now_size), Some(had)) = (size, d.total_bytes) {
                if now_size > 0 && now_size != had {
                    let name = d.filename.clone();
                    self.push_log(format!("Queue sync: {name} changed on the server ({had} -> {now_size} bytes) - downloading it again."));
                    self.redownload(id);
                }
            }
        }
    }

    fn stop_named_queue(&mut self, i: usize) {
        let Some(q) = self.queues.get_mut(i) else { return };
        q.running = false;
        let name = q.name.clone();
        let ids: Vec<u64> = self.downloads.iter().filter(|d| d.queue == name && self.tasks.contains_key(&d.id)).map(|d| d.id).collect();
        for id in ids {
            self.pause(id);
        }
    }

    /// Download quota: once `quota_mb` arrived within `quota_hours`, running
    /// downloads pause; they resume by themselves when the window frees up.
    fn enforce_quota(&mut self) {
        let now_total: u64 = self.downloads.iter().map(|d| d.downloaded_bytes).sum();
        // First look: what is already on disk is not "this window's" traffic.
        if self.quota_last == 0 {
            self.quota_last = now_total.max(1);
            return;
        }
        let delta = now_total.saturating_sub(self.quota_last);
        self.quota_last = now_total;
        if self.quota_mb == 0 {
            if !self.quota_paused.is_empty() {
                for id in std::mem::take(&mut self.quota_paused) {
                    self.resume(id);
                }
            }
            return;
        }
        let window = std::time::Duration::from_secs(self.quota_hours.max(1) * 3600);
        if delta > 0 {
            self.quota_log.push_back((Instant::now(), delta));
        }
        while self.quota_log.front().is_some_and(|(t, _)| t.elapsed() > window) {
            self.quota_log.pop_front();
        }
        let used: u64 = self.quota_log.iter().map(|(_, b)| *b).sum();
        let cap = self.quota_mb.saturating_mul(1024 * 1024);
        if used >= cap && self.quota_paused.is_empty() {
            let running: Vec<u64> = self.downloads.iter().filter(|d| matches!(d.status, Status::Downloading)).map(|d| d.id).collect();
            for id in &running {
                self.pause(*id);
            }
            if !running.is_empty() {
                self.push_log(format!("Download quota reached ({} MB per {} h): paused until it frees up.", self.quota_mb, self.quota_hours));
            }
            self.quota_paused = running;
        } else if used < cap * 9 / 10 && !self.quota_paused.is_empty() {
            for id in std::mem::take(&mut self.quota_paused) {
                self.resume(id);
            }
            self.push_log("Download quota freed up: resuming.");
        }
    }

    /// IDM-style list keys (only while no text field has the keyboard and no
    /// dialog is open): Ctrl+A checks every visible row, Delete removes the
    /// checked rows (files stay), Space pauses or resumes them, Enter opens a
    /// finished file or the details of anything else.
    fn list_shortcuts(&mut self, ctx: &egui::Context) {
        if self.show_add || ctx.wants_keyboard_input() {
            return;
        }
        let (all, del, space, enter) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if all {
            let ids = self.visible_ids();
            for d in self.downloads.iter_mut() {
                if ids.contains(&d.id) {
                    d.selected = true;
                }
            }
        }
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        if del {
            let n = ids.len();
            for id in ids {
                self.remove(id);
            }
            self.push_log(format!("Removed {n} selected download(s)."));
        } else if space {
            for id in ids {
                let act = match self.downloads.iter().find(|d| d.id == id).map(|d| &d.status) {
                    Some(Status::Downloading) => RowAct::Pause,
                    Some(Status::Completed) | None => continue,
                    _ => RowAct::Resume,
                };
                self.do_row_act(act, id);
            }
        } else if enter && ids.len() == 1 {
            let done = self.downloads.iter().any(|d| d.id == ids[0] && matches!(d.status, Status::Completed));
            self.do_row_act(if done { RowAct::OpenFile } else { RowAct::Details }, ids[0]);
        }
    }

    /// Checked rows the user can see. A row checked under another filter or
    /// search stays checked but is not acted on: toolbar Delete removed rows
    /// the current view was hiding.
    fn selected_ids(&self) -> Vec<u64> {
        let visible: std::collections::HashSet<u64> = self.visible_ids().into_iter().collect();
        self.downloads
            .iter()
            .filter(|d| d.selected && visible.contains(&d.id))
            .map(|d| d.id)
            .collect()
    }

    fn start_queue(&mut self) {
        self.queue_running = true;
        // Resume paused ones directly, but never above the concurrent limit:
        // paused items with live tasks count, the rest stay queued for
        // fill_next (previously ALL paused resumed at once, ignoring the cap).
        let cap = self.max_concurrent.max(1);
        let ids: Vec<u64> = self
            .downloads
            .iter()
            .filter(|d| matches!(d.status, Status::Paused))
            .map(|d| d.id)
            .collect();
        // A paused row whose task is alive already holds its slot: resuming
        // it costs nothing. Checking the cap first left every download that
        // Stop All paused stuck (their tasks fill the slots).
        let (live, idle): (Vec<u64>, Vec<u64>) = ids.into_iter().partition(|id| self.tasks.contains_key(id));
        for id in live {
            self.resume(id);
        }
        for id in idle {
            if self.tasks.len() >= cap {
                break;
            }
            self.resume(id);
        }
        self.fill_next();
        crate::platform::play_event("queue_start");
        self.push_log("Queue started.");
    }

    /// Stop starting new downloads; active transfers run to completion.
    fn stop_queue(&mut self) {
        self.queue_running = false;
        self.push_log("Queue stopped (active downloads finish).");
    }

    /// Pause everything right now and stop the queue pump.
    fn stop_all(&mut self) {
        self.queue_running = false;
        let ids: Vec<u64> = self.tasks.keys().copied().collect();
        for id in ids {
            self.pause(id);
        }
        self.push_log("All downloads paused.");
    }

    fn poll_tasks(&mut self) {
        let mut finished = Vec::new();
        let mut retry_notes = Vec::new();
        for (id, task) in self.tasks.iter_mut() {
            let dl = task.shared.downloaded.load(Ordering::SeqCst);
            let resumable = task.shared.resumable.load(Ordering::SeqCst);
            // What the helper is doing while it is not moving bytes (yt-dlp's
            // "Getting info"/"Merging"). Empty for the built-in engine, which
            // is always transferring once it starts.
            let phase = task.shared.phase_now();
            if phase.is_empty() {
                self.phases.remove(id);
            } else {
                self.phases.insert(*id, phase);
            }
            if let Ok(g) = task.shared.resolved_name.lock() {
                if let Some(name) = g.clone() {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == *id) {
                        if d.status != Status::Completed {
                            d.filename = name;
                        }
                    }
                }
            }
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == *id) {
                // Displayed progress is strictly monotonic: engine counters
                // reset on retry/fallback legs (segment drop → resume from a
                // lower offset), but the bar must never jump backwards
                // (e.g. 35% -> 20% -> 31%). A changed total means a new leg —
                // adopt both fresh values; otherwise keep the max.
                let new_total = if task.shared.has_total.load(Ordering::SeqCst) {
                    Some(task.shared.total.load(Ordering::SeqCst))
                } else {
                    None
                };
                if new_total != d.total_bytes {
                    // Fresh link / quality fallback: the old bytes belong to
                    // deleted work files, so restart the bar instead of
                    // showing phantom progress — and say so in the log.
                    if d.downloaded_bytes > 0 && dl < d.downloaded_bytes {
                        retry_notes.push(format!(
                            "Retrying {} with a fresh link — restarting progress.",
                            d.filename
                        ));
                    }
                    d.total_bytes = new_total;
                    d.downloaded_bytes = dl;
                } else {
                    d.downloaded_bytes = d.downloaded_bytes.max(dl);
                }
                // Clamp to [0, total] so a stale counter can never exceed 100%.
                if let Some(t) = d.total_bytes {
                    if d.downloaded_bytes > t {
                        d.downloaded_bytes = t;
                    }
                }
                d.resumable = resumable;
                // The resolution that is actually arriving, straight from the
                // helper's chosen format / the merged file. Blank once the
                // helper has not named it yet, so the row keeps the last known.
                let res = task.shared.resolution_now();
                if !res.is_empty() && d.resolution != res {
                    d.resolution = res;
                }
                let el = task.last_sample.elapsed().as_secs_f64();
                if el >= 0.25 {
                    // Speed from the MONOTONIC displayed value (never the raw
                    // engine counter): a leg reset would otherwise show a
                    // negative spike followed by an inflated catch-up burst.
                    // P5: EWMA smoothing (α=0.55) so the readout and ETA stop
                    // flickering on bursty CDNs while still reacting quickly
                    // (a quarter-second cadence reads as live); raw samples
                    // feed the graph.
                    let delta = d.downloaded_bytes.saturating_sub(task.last_bytes) as f64;
                    let raw = if el > 0.0 { delta / el } else { 0.0 };
                    // A helper that measures the rate itself (yt-dlp) is more
                    // accurate than differencing a counter that spans several
                    // connections and streams - but only use it while bytes are
                    // still arriving, so a stalled transfer cannot show a rate
                    // frozen at its last value.
                    let reported = task.shared.speed_bps.load(Ordering::SeqCst) as f64;
                    // A paused transfer has no speed, whatever its last
                    // in-flight bytes did.
                    d.speed_bps = if task.shared.paused.load(Ordering::SeqCst) {
                        0.0
                    } else if delta > 0.0 && reported > 0.0 {
                        reported
                    } else if d.speed_bps <= 0.0 {
                        raw
                    } else {
                        0.55 * raw + 0.45 * d.speed_bps
                    };
                    let hist = self.speed_hist.entry(*id).or_default();
                    hist.push_back(raw);
                    while hist.len() > 120 {
                        hist.pop_front();
                    }
                    task.last_sample = Instant::now();
                    task.last_bytes = d.downloaded_bytes;
                }
            }
            if task.handle.is_finished() {
                finished.push(*id);
            }
        }
        for note in retry_notes {
            self.push_log(note);
        }
        let mut repair_restart: Vec<u64> = Vec::new();
        // Rows that failed on a dead link: the refresh window opens for one.
        let mut dead_links: Vec<(u64, String)> = Vec::new();
        for id in finished {
            // The helper's stage label dies with the helper.
            self.phases.remove(&id);
            if let Some(task) = self.tasks.remove(&id) {
                let cancelled = task.shared.cancelled.load(Ordering::SeqCst);
                let dl = task.shared.downloaded.load(Ordering::SeqCst);
                let was_resumable = task.shared.resumable.load(Ordering::SeqCst);
                let has_total = task.shared.has_total.load(Ordering::SeqCst);
                let total = task.shared.total.load(Ordering::SeqCst);
                let mut msg: Option<String> = None;
                let mut finished_info: Option<(String, std::path::PathBuf)> = None;
                // Set when the checksum repair below re-queues this id.
                let mut repaired_here = false;
                // A non-fatal engine remark (e.g. a fallback that likely cost
                // the audio track) belongs in this row's log line.
                let engine_note = task
                    .shared
                    .note
                    .lock()
                    .ok()
                    .and_then(|mut g| g.take());
                if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                    d.downloaded_bytes = dl;
                    d.resumable = was_resumable;
                    // Last chance to record what really landed (the final
                    // ffprobe on a merged file happens just before the engine
                    // returns, after the last live poll).
                    let res = task.shared.resolution_now();
                    if !res.is_empty() {
                        d.resolution = res;
                    }
                    if has_total {
                        d.total_bytes = Some(total);
                    }
                    d.speed_bps = 0.0;
                    // Bytes the engine *fetched* are not proof the file landed:
                    // a re-fetched/overlapping segment can push the counter to
                    // the total while the assembled file is still short, and a
                    // failed/renamed write leaves nothing behind. "Finished"
                    // must mean the bytes are actually on disk.
                    // Merged (video+audio) deliveries are exempt: muxing
                    // rewrites the container, so sizes are not comparable.
                    let landed = std::fs::metadata(d.dest_path())
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let adaptive = d.pair_url.starts_with("http")
                        || task.shared.merged.load(Ordering::SeqCst);
                    // A merge the engine finished (both legs at their servers'
                    // own lengths, file muxed) is done even when the expected
                    // total was a little off: a Facebook reel failed with
                    // "got 3.22 MB of 3.23 MB" while every byte had arrived.
                    let merged_ok = adaptive
                        && landed > 0
                        && task.shared.succeeded.load(Ordering::SeqCst);
                    let complete = match d.total_bytes {
                        Some(t) if adaptive => (t > 0 && dl >= t) || merged_ok,
                        Some(t) => t > 0 && dl >= t && landed >= t,
                        // unknown size: only a clean end WITH bytes counts
                        None => !cancelled && dl > 0,
                    };
                    // Snapshot what follow-up actions need (borrow ends at block end).
                    // A pre-existing file only counts when the size was never
                    // known AND this run actually wrote bytes (dl > 0): a
                    // shortfall against a KNOWN total must stay Failed, or a
                    // truncated retry over an old finished file masks as success.
                    // (Covers empty files too.)
                    let file_done = !cancelled
                        && !complete
                        && d.total_bytes.is_none()
                        && dl > 0
                        && d.dest_path().exists();
                    if cancelled {
                        d.status = Status::Paused;
                    } else if task_can_complete(&task.shared, complete || file_done) {
                        d.status = Status::Completed;
                        d.cookie.clear();
                        d.extra_headers.clear();
                        // The file on disk is the truth. A retried or merged
                        // transfer can end with a byte counter that claims more
                        // than the file holds, and a probe's size can be an
                        // estimate — showing both is what produced "Downloaded
                        // 847.65 MB" next to "Size 722.80 MB" on one row.
                        if landed > 0 {
                            d.downloaded_bytes = landed;
                            d.total_bytes = Some(landed);
                        }
                        msg = Some(format!("Finished: {}", d.filename));
                        finished_info = Some((d.filename.clone(), d.dest_path()));
                    } else {
                        let reason = task
                            .shared
                            .error
                            .lock()
                            .map(|mut g| g.take())
                            .unwrap_or_default()
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| {
                                // Include landed vs expected bytes so a cut
                                // connection (e.g. throttled FB CDN) tells the
                                // user exactly where resume continues from.
                                match d.total_bytes {
                                    Some(t) if t > 0 && dl >= t => format!(
                                        "transfer reported {} but only {} is on disk — resume to retry",
                                        crate::model::format_bytes(dl),
                                        crate::model::format_bytes(landed)
                                    ),
                                    Some(t) if t > 0 => format!(
                                        "connection ended early (got {} of {}) — resume to retry",
                                        crate::model::format_bytes(dl),
                                        crate::model::format_bytes(t)
                                    ),
                                    _ => "connection ended early — resume to retry".to_string(),
                                }
                            });
                        // P5 checksum auto-repair: one automatic from-scratch
                        // redownload per checksum. A mismatch means the bytes
                        // on disk are wrong, so resume state is dropped and
                        // progress restarts; a second mismatch fails loudly
                        // instead of looping forever.
                        let mut repaired = false;
                        if reason.contains("checksum mismatch")
                            && !d.checksum_sha256.trim().is_empty()
                            && !d.checksum_repaired
                        {
                            d.checksum_repaired = true;
                            let dp = d.dest_path();
                            crate::model::drop_resume_state(&dp);
                            d.downloaded_bytes = 0;
                            d.total_bytes = None;
                            d.speed_bps = 0.0;
                            d.status = Status::Queued;
                            msg = Some(format!(
                                "Checksum mismatch on {} — re-downloading once to repair.",
                                d.filename
                            ));
                            repaired = true;
                        }
                        if !repaired {
                            d.status = Status::Failed(reason.clone());
                            msg = Some(format!("Failed #{id}: {reason}"));
                            if let Some(why) = dead_link_reason(&reason) {
                                // A YouTube row reads a fresh link itself.
                                if !crate::engine::is_youtube_page(&d.url) && !crate::engine::is_youtube_page(&d.referer) {
                                    dead_links.push((id, why));
                                }
                            }
                        }
                        // Never keep a phantom size: a failed transfer with no
                        // bytes (e.g. a walled video page) must not display
                        // the probe's bogus total.
                        if dl == 0 {
                            d.total_bytes = None;
                        }
                        if !repaired {
                            crate::platform::play_event("failed");
                            notify(
                                "Download Craft — download failed",
                                &format!("#{id}: {reason}"),
                            );
                        } else {
                            repaired_here = true;
                        }
                    }
                }
                if let Some(n) = engine_note {
                    msg = Some(match msg.take() {
                        Some(m) => format!("{m} — {n}"),
                        None => format!("#{id}: {n}"),
                    });
                }
                if repaired_here {
                    repair_restart.push(id);
                }
                if let Some((name, path)) = finished_info {
                    self.on_download_finished(&name, &path);
                    // IDM pops its "Download complete" dialog for a finished
                    // transfer (the dialog's own checkbox switches it off).
                    if self.show_complete {
                        Self::spawn_complete(id);
                    }
                }
                if let Some(m) = msg {
                    self.push_log(m);
                }
                self.save_queue();
            }
        }
        // A dead link opens "Refresh download address" (IDM does the same),
        // one at a time.
        if self.refresh_dlg.is_none() {
            if let Some((id, why)) = dead_links.into_iter().next() {
                self.open_refresh(id, Some(why));
            }
        }
        // Checksum repairs restart immediately (the job was already running;
        // this is an internal retry, not a queue jump).
        for id in repair_restart {
            if !self.tasks.contains_key(&id) {
                self.start(id);
            }
        }
        // P5: drop speed history for rows that no longer exist.
        if self.speed_hist.len() > self.downloads.len() + 4 {
            let live: std::collections::HashSet<u64> =
                self.downloads.iter().map(|d| d.id).collect();
            self.speed_hist.retain(|id, _| live.contains(id));
            self.phases.retain(|id, _| live.contains(id));
        }
        self.fill_next();
        self.maybe_finish_action();
    }

    /// Called with an immutable borrow-safe snapshot after a download completes.
    fn on_download_finished(&mut self, name: &str, path: &std::path::Path) {
        notify("Download Craft — download finished", name);
        crate::model::record_finished_bytes(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));
        self.stats_at = Instant::now().checked_sub(std::time::Duration::from_secs(60)).unwrap_or_else(Instant::now);
        crate::platform::play_event("complete");
        if !self.antivirus_cmd.trim().is_empty() {
            match crate::platform::antivirus_command(&self.antivirus_cmd,path) {
                Ok(mut cmd) => {
                    self.push_log(format!("Antivirus scan: {}",path.display()));
                    if let Err(e)=cmd.spawn() {self.push_log(format!("Antivirus launch failed: {e}"));}
                }
                Err(e) => self.push_log(e),
            }
        }
        self.extract_archive(path, false);
    }

    /// Unpack a finished archive on a worker thread. `manual` is the row
    /// menu's Extract; otherwise it is the automatic run after a download,
    /// which follows Settings → On completion. A multi-part RAR waits until
    /// every part of it in the list has finished.
    fn extract_archive(&mut self, path: &std::path::Path, manual: bool) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(vol) = crate::archive::volume_of(&name) else {
            if manual {
                self.push_log(format!("{name} is not a ZIP, RAR or 7z file."));
            }
            return;
        };
        if !manual {
            let wanted = self.extract_types.split_whitespace().any(|k| k.eq_ignore_ascii_case(vol.kind.label()));
            if !self.extract_auto || !wanted {
                return;
            }
            if let Some(set) = &vol.set {
                let folder = path.parent();
                let waiting = self.downloads.iter().any(|d| {
                    crate::archive::in_set(&d.filename, set)
                        && !matches!(d.status, Status::Completed)
                        && d.dest_path().parent() == folder
                });
                if waiting {
                    return;
                }
            }
        }
        let first = path.with_file_name(&vol.first);
        if !first.is_file() {
            self.push_log(format!("Extract {name}: {} is not here yet.", vol.first));
            return;
        }
        let into_folder = self.extract_into_folder;
        let dest = crate::archive::destination(&first, into_folder);
        let delete = self.extract_delete && !manual;
        let tx = self.conv_tx.clone();
        self.push_log(format!("Extracting {}…", vol.first));
        std::thread::spawn(move || {
            let shown = vol.first.clone();
            let line = match crate::archive::extract(&first, &dest) {
                Ok(out) => {
                    let mut line = format!("Extracted {} file(s) from {shown} to {}", out.files, out.dest.display());
                    if out.skipped > 0 {
                        line += &format!(" — {} already there, kept", out.skipped);
                    }
                    if out.refused > 0 {
                        line += &format!(" — {} unsafe name(s) refused", out.refused);
                    }
                    if delete && out.complete() {
                        let parent = first.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                        let mut gone = vec![first.clone()];
                        if let (Some(set), Ok(rd)) = (&vol.set, std::fs::read_dir(&parent)) {
                            gone.extend(rd.flatten().map(|e| e.path()).filter(|p| {
                                p.file_name().is_some_and(|n| crate::archive::in_set(&n.to_string_lossy(), set))
                            }));
                        }
                        gone.sort();
                        gone.dedup();
                        for f in gone {
                            let _ = std::fs::remove_file(f);
                        }
                        line += " — archive deleted";
                    }
                    notify("Download Craft — extracted", &shown);
                    line
                }
                Err(e) => {
                    if into_folder {
                        // Only an empty folder this run created goes away.
                        let _ = std::fs::remove_dir(&dest);
                    }
                    notify("Download Craft — could not extract", &format!("{shown}: {e}"));
                    format!("Could not extract {shown}: {e}")
                }
            };
            let _ = tx.send(line);
        });
    }

    /// Runs the configured end-of-queue action once everything drained.
    /// Only actively-workable items block it (Queued/Downloading). Paused
    /// or failed leftovers no longer veto silently — they get one log line
    /// so the user knows why the action still fired.
    fn maybe_finish_action(&mut self) {
        if !self.had_active || !self.queue_running {
            return;
        }
        if !self.tasks.is_empty() {
            return;
        }
        let pending = self.downloads.iter().any(|d| {
            matches!(d.status, Status::Queued | Status::Downloading)
        });
        if pending {
            return;
        }
        let leftovers = self
            .downloads
            .iter()
            .filter(|d| matches!(d.status, Status::Paused | Status::Failed(_)))
            .count();
        self.queue_running = false;
        self.had_active = false;
        if leftovers > 0 {
            self.push_log(format!(
                "Queue drained with {leftovers} paused/failed item(s) left — finish action still runs."
            ));
        }
        #[cfg(windows)]
        match self.finish_action.as_str() {
            "shutdown" => {
                self.push_log("Queue done — shutting down.");
                let mut c = std::process::Command::new("shutdown");
                c.args(["/s", "/t", "15", "/c", "Download Craft: downloads finished"]);
                quiet(&mut c);
                let _ = c.spawn();
            }
            "sleep" => {
                self.push_log("Queue done — sleeping.");
                let _ = std::process::Command::new("rundll32.exe")
                    .args(["powrprof.dll,SetSuspendState", "0,1,0"])
                    .spawn();
            }
            "hibernate" => {
                self.push_log("Queue done — hibernating.");
                let mut c = std::process::Command::new("shutdown");
                c.args(["/h"]);
                quiet(&mut c);
                let _ = c.spawn();
            }
            _ => {
                self.push_log("Queue finished.");
            }
        }
        #[cfg(not(windows))]
        if self.finish_action != "nothing" && !self.finish_action.is_empty() {
            match crate::platform::power(&self.finish_action) {
                Ok(())=>self.push_log(format!("Queue done — {} requested.",self.finish_action)),
                Err(e)=>self.push_log(format!("Queue finished; power action was not completed: {e}")),
            }
        }
        notify("Download Craft — queue finished", "All downloads completed.");
        crate::platform::play_event("queue_done");
        self.save_queue();
    }

    fn check_scheduler(&mut self) {
        if !self.scheduler_enabled || self.scheduler_started {
            return;
        }
        let now = chrono::Local::now().format("%H:%M").to_string();
        if now == self.scheduler_time.trim() {
            self.scheduler_started = true;
            self.start_queue();
            self.push_log("Scheduler: queue started.");
        }
    }

    /// Import URLs queued by the browser extension's native host.
    /// Unlike auto-starting, the first new item opens the Add-download
    /// dialog prefilled (IDM-style), so the user confirms folder/quality.
    /// Returns true when the window should be brought to the front.
    /// Leftover items stay in the inbox for later polls.
    fn poll_inbox(&mut self) -> bool {
        const MAX_INBOX: usize = 200;
        const CONFIRM_GRACE_SECS: i64 = 10;
        let opened_at = app_opened_at();
        let path = match crate::native_host::inbox_path() {
            Some(p) => p,
            None => return false,
        };
        let _guard = crate::model::fs_lock("inbox");
        if !_guard.owned() { return false; }
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return false,
        };
        // Never destroy the inbox on parse failure (disk-full truncation,
        // foreign bytes) — leave it for the next poll/owner.
        let items: Vec<crate::native_host::InboxItem> = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if items.is_empty() {
            let _ = std::fs::remove_file(&path);
            return false;
        }
        // Standalone Download File Info dialogs (spawned per capture, IDM
        // style) claim their inbox entry on open. Entries younger than
        // CONFIRM_GRACE_SECS are left alone so the dialog wins the race;
        // only entries it never claimed (crashed spawn, old version) fall
        // through to this in-canvas prompt.
        let now_ts = chrono::Local::now().timestamp();
        let age_secs = |rfc: &str| -> i64 {
            chrono::DateTime::parse_from_rfc3339(rfc)
                .map(|dt| now_ts - dt.timestamp())
                .unwrap_or(i64::MAX)
        };
        // A row waiting for a fresh link takes the newest capture at once.
        if let Some(w) = crate::model::refresh_wait() {
            let mut items = items;
            items.sort_by(|a, b| b.received_at.cmp(&a.received_at));
            let fresh = items.remove(0);
            if items.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Ok(json) = serde_json::to_string_pretty(&items) {
                Self::atomic_write(&path, json.as_bytes());
            }
            drop(_guard);
            self.apply_refreshed_link(w.id, fresh);
            return false;
        }
        // Captures sent while the app was closed are dropped, not prompted
        // one after another (or started) the moment it opens: the capture
        // dialog already told the user the app was not running.
        let before = items.len();
        let items: Vec<crate::native_host::InboxItem> = items
            .into_iter()
            .filter(|i| {
                chrono::DateTime::parse_from_rfc3339(&i.received_at)
                    .map(|dt| dt.timestamp() >= opened_at)
                    .unwrap_or(false)
            })
            .collect();
        if items.len() < before {
            self.push_log(format!(
                "Ignored {} browser capture(s) sent while Download Craft was closed.",
                before - items.len()
            ));
        }
        let mut keep: Vec<crate::native_host::InboxItem> = Vec::new();
        let mut next: Option<crate::native_host::InboxItem> = None;
        // Already queued = same URL at the same quality (another quality of
        // one page is a new download).
        let queued = |i: &crate::native_host::InboxItem, downloads: &[DownloadItem]| {
            let q = i.quality.as_deref().unwrap_or("");
            downloads.iter().any(|d| d.url == i.url && crate::model::same_quality(&d.quality_pref, q))
        };
        for i in items.into_iter() {
            if next.is_none()
                && is_download_url(&i.url)
                && !queued(&i, &self.downloads)
                && age_secs(&i.received_at) >= CONFIRM_GRACE_SECS
            {
                next = Some(i);
            } else if is_download_url(&i.url)
                && (!queued(&i, &self.downloads) || age_secs(&i.received_at) < CONFIRM_GRACE_SECS)
            {
                // Young (dialog likely opening) or a later old entry: keep.
                // A young entry for an already-queued row is kept too: it
                // carries the re-capture's fresh cookie/headers, which its
                // dialog claims to refresh a failed row.
                keep.push(i);
            }
            // Invalid URLs and old already-queued duplicates are dropped.
        }
        // Cap the inbox (newest first by received_at) so a stuck producer
        // can't grow it without bound.
        if keep.len() > MAX_INBOX {
            keep.sort_by(|a, b| b.received_at.cmp(&a.received_at));
            keep.truncate(MAX_INBOX);
        }
        let write_keep = |keep: &Vec<crate::native_host::InboxItem>| {
            if keep.is_empty() {
                let _ = std::fs::remove_file(&path);
            } else if let Ok(json) = serde_json::to_string_pretty(&keep) {
                Self::atomic_write(&path, json.as_bytes());
            }
        };
        let Some(item) = next else {
            // Nothing actionable now: keep young entries, drop the file only
            // when fully drained.
            write_keep(&keep);
            return false;
        };
        if self.show_add {
            // The user is typing in another Add dialog: keep this capture for
            // a later poll. Dropping it here lost the download outright.
            keep.insert(0, item);
            write_keep(&keep);
            return false;
        }
        write_keep(&keep);
        let url = item.url;
        let referer = item
            .referer
            .or_else(|| item.page_url)
            .unwrap_or_default();
        let cookie = item.cookie.unwrap_or_default();
        self.add_headers = item.headers.unwrap_or_default();
        self.add_browser = item.browser;
        if self.captured_media.len() >= 200 { self.captured_media.clear(); }
        self.captured_media.insert(url.clone(), (item.dur.unwrap_or(0), item.audio_expected));
        let size = item.size;
        let quality = item.quality;
        self.add_url = url.clone();
        self.add_file = item
            .filename
            .filter(|f| !f.trim().is_empty())
            .map(|f| crate::model::apply_rename_template(&self.rename_template, &f, &url))
            .unwrap_or_else(|| filename_from_url(&url));
        self.add_referer = referer;
        self.add_cookie = cookie;
        self.add_checksum.clear();
        self.add_quality = quality.filter(|s| !s.trim().is_empty()).unwrap_or_default();
        // Browser-measured size rides along: show it without a server-side
        // re-probe (which can 403 off-session on expiring CDN links).
        match size.filter(|s| *s > 0) {
            Some(s) => {
                self.probe_size = Some(s);
                self.probe_done_for = url.clone();
            }
            None => self.probe_done_for.clear(), // force a fresh size/name probe
        }
        self.show_add = true;
        self.push_log("Browser extension sent a download — confirm in Add download.");
        true
    }

    /// Merge queue entries written by other processes (standalone Download
    /// File Info dialogs, CLI) into this window's in-memory list.
    fn merge_queue_from_disk(&mut self) {
        let path = match Self::queue_path() {
            Some(p) => p,
            None => return,
        };
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let (items, _) = Self::parse_queue_items(&data);
        for mut it in items {
            // A row we hold that another process re-queued (a dialog refreshed
            // a failed/finished row with a fresh capture, the CLI re-queued
            // it): take the disk version. Skipping it kept the stale status,
            // so its start order did nothing and our next save undid it.
            if let Some(pos) = self.downloads.iter().position(|d| d.id == it.id) {
                let local = &self.downloads[pos];
                // Only rows another process may refresh: failed, finished with
                // the file gone, or paused without a task. A row this window
                // just completed (file present) is never reverted by a disk
                // copy its own save has not overwritten yet.
                let refreshable = match &local.status {
                    Status::Failed(_) => true,
                    Status::Completed => !local.dest_path().exists(),
                    Status::Paused => true,
                    _ => false,
                };
                let refreshed_elsewhere = matches!(it.status, Status::Queued)
                    && refreshable
                    && !self.tasks.contains_key(&it.id);
                if refreshed_elsewhere {
                    let selected = local.selected;
                    it.speed_bps = 0.0;
                    it.selected = selected;
                    self.downloads[pos] = it;
                }
                continue;
            }
            if self.downloads.iter().any(|d| {
                d.url == it.url && crate::model::same_quality(&d.quality_pref, &it.quality_pref)
            }) {
                continue;
            }
            // Foreign entry for a file we already track (fresh capture):
            // refresh the local entry instead of a colliding duplicate — but
            // never a row with a live task (its download would be hijacked).
            if let Some(pos) =
                crate::model::find_dest_clash(&self.downloads, &it.dest_path())
                    .filter(|p| !self.tasks.contains_key(&self.downloads[*p].id))
            {
                let local = &mut self.downloads[pos];
                if local.url != it.url {
                    local.url = std::mem::take(&mut it.url);
                    local.downloaded_bytes = 0;
                    let dp = local.dest_path();
                    crate::model::drop_resume_state(&dp);
                }
                local.total_bytes = it.total_bytes;
                local.speed_bps = 0.0;
                local.status = Status::Queued;
                continue;
            }
            if it.status.is_active() {
                it.status = Status::Queued;
            }
            it.speed_bps = 0.0;
            it.selected = false;
            self.next_id = self.next_id.max(it.id + 1);
            self.known_ids.insert(it.id);
            self.downloads.push(it);
        }
    }

    /// Consume `%APPDATA%/download_craft/wake.json` (`{"start":[ids]}`):
    /// start IDs queued by a standalone dialog's "Start Download".
    fn check_wake(&mut self) {
        let path = match crate::model::config_dir() {
            Some(p) => p.join("wake.json"),
            None => return,
        };
        let _guard = crate::model::fs_lock("wake");
        if !_guard.owned() { return; }
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return,
        };
        // A start request left from before this window opened (a dialog whose
        // app launch never came up) is dropped, not acted on now.
        let stale = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).timestamp() < app_opened_at())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&path); // claim first: at-most-once
        if stale {
            return;
        }
        let ids: Vec<u64> = serde_json::from_str::<serde_json::Value>(&data)
            .ok()
            .and_then(|v| v.get("start").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        self.merge_queue_from_disk();
        for id in ids {
            let status = self.downloads.iter().find(|d| d.id == id).map(|d| d.status.clone());
            match status {
                Some(Status::Queued) if !self.tasks.contains_key(&id) => {
                    self.queue_running = true;
                    self.start(id);
                }
                // A dialog that found the row Paused asks to start it: resume,
                // or its "starting it" notice promised something that never
                // happened.
                // A failed row clicked again in the browser (a YouTube link
                // that expired, a dropped connection) is retried the same way:
                // it continues from its bytes, a YouTube page re-read fresh.
                Some(Status::Paused) | Some(Status::Failed(_)) => {
                    self.queue_running = true;
                    self.resume(id);
                }
                // Already running: bring its progress window up instead of
                // nothing visibly happening — but only when per-download
                // windows are enabled. With them off the main list is the
                // only surface, and a capture must not pop a window the user
                // switched off in Settings.
                Some(Status::Downloading) if self.progress_windows => {
                    if let Err(e) = Self::spawn_progress(id) {
                        self.push_log(format!("Could not open the progress window for #{id}: {e}"));
                    }
                }
                _ => {}
            }
        }
    }

    fn matches_filter(d: &DownloadItem, filter: &str) -> bool {
        match filter {
            "All" => true,
            // Legacy names (kept for saved state compat) + FluxDown-style names.
            "Finished" | "Completed" => matches!(d.status, Status::Completed),
            "Unfinished" | "Incomplete" => !matches!(d.status, Status::Completed),
            "Failed" => matches!(d.status, Status::Failed(_)),
            "Paused" => matches!(d.status, Status::Paused),
            "Downloading" => matches!(d.status, Status::Downloading),
            "Queued" => matches!(d.status, Status::Queued),
            cat => d.category() == cat,
        }
    }

    fn visible_ids(&self) -> Vec<u64> {
        let q = self.search.trim().to_lowercase();
        let mut ids: Vec<u64> = self
            .downloads
            .iter()
            .filter(|d| Self::matches_filter(d, &self.filter))
            .filter(|d| q.is_empty() || d.filename.to_lowercase().contains(&q))
            .map(|d| d.id)
            .collect();
        let asc = self.sort_asc;
        let col = self.sort_col;
        ids.sort_by(|a, b| {
            let da = self.downloads.iter().find(|d| d.id == *a).unwrap();
            let db = self.downloads.iter().find(|d| d.id == *b).unwrap();
            let ord = match col {
                SortCol::Name => da.filename.to_lowercase().cmp(&db.filename.to_lowercase()),
                SortCol::Size => da.total_bytes.cmp(&db.total_bytes),
                SortCol::Status => format!("{:?}", da.status).cmp(&format!("{:?}", db.status)),
                SortCol::Speed => da.speed_bps.partial_cmp(&db.speed_bps).unwrap_or(std::cmp::Ordering::Equal),
                SortCol::Eta => da.downloaded_bytes.cmp(&db.downloaded_bytes),
                SortCol::Date => da.created_at.cmp(&db.created_at),
            };
            if asc { ord } else { ord.reverse() }
        });
        ids
    }
}

/// Start a console program without a console window. This app is a
/// windows-subsystem binary, so every console child it starts (reg, shutdown,
/// cmd …) would otherwise flash a black window — six of them on every launch,
/// one per browser probed by `ensure_native_host`.
#[cfg(windows)]
fn quiet(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn quiet(_cmd: &mut std::process::Command) {}

/// Open a file (or reveal it in Explorer when `reveal`).
#[cfg(windows)]
pub(crate) fn open_path(path: &std::path::Path, reveal: bool) {
    if reveal {
        if path.exists() {
            select_in_file_manager(path);
        } else if let Some(parent) = path.parent() {
            // The file itself is gone: the folder it was meant for is still
            // the useful answer.
            let _ = std::process::Command::new("explorer").arg(parent).spawn();
        }
    } else if path.exists() {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", &path.to_string_lossy()]);
        quiet(&mut c);
        let _ = c.spawn();
    }
}

#[cfg(not(windows))]
pub(crate) fn open_path(path: &std::path::Path, reveal: bool) { crate::platform::open(path,reveal); }

/// Show `path` selected in the file manager.
#[cfg(windows)]
fn select_in_file_manager(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    // `/select,"<path>"` has to reach Explorer *verbatim*. Built as one
    // `Command::arg`, Rust escapes the quotes (`\"`), which Explorer does not
    // parse: it drops the switch and opens its default folder instead - a file
    // downloaded into `Downloads\Video` made "Open folder" land in
    // `OneDrive\Documents`. `raw_arg` appends it unescaped, the documented
    // form, and the path is quoted because it may contain spaces.
    let _ = std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn();
}

#[cfg(not(windows))]
fn select_in_file_manager(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
}

/// Write the native-messaging manifest + launcher and register it (HKCU, no admin).
/// Zero-config: the unpacked extension carries a pinned key, so its ID is
/// always this value — nothing to paste.
pub(crate) const EXTENSION_ID: &str = "pgdhmfgefjnfijmlheboncdmfhcnoikk";
/// IDs the Edge / Opera stores give the same extension once it is listed
/// there (each store assigns its own); the host accepts them too.
pub(crate) const STORE_EXTENSION_IDS: &[&str] = &[];

#[cfg(windows)]
pub(crate) fn install_native_host(browser: &str) -> Result<String, String> {
    let base = crate::model::config_dir()
        .map(|p| p.join("native-host"))
        .ok_or_else(|| "no config dir".to_string())?;
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;

    let exe = crate::platform::launch_path().map_err(|e| e.to_string())?;
    let bat = base.join("download_craft_host.bat");
    // NOTE: `@echo off` is mandatory — cmd echoes every command to stdout
    // by default, which corrupts the length-prefixed native-messaging
    // protocol and makes Chrome report the host as unreachable.
    std::fs::write(
        &bat,
        format!("@echo off\r\n\"{}\" --native-host %*\r\n", exe.display()),
    )
    .map_err(|e| e.to_string())?;
    // cmd reads a .bat in the OEM code page, so a path with "č" or "ř"
    // (often the user name) came out garbled and the host never started
    // (issue #5). Such a path is given to the browser as the program itself;
    // `native_host::launched_directly` recognises that launch.
    let host = if exe.to_string_lossy().is_ascii() { bat.clone() } else { exe.clone() };

    // Native host name (must match the browser extension's HOST constant).
    const HOST_NAME: &str = "com.download_craft";
    let manifest_path = base.join(format!("{browser}.json"));
    let manifest = match browser {
        "firefox" => serde_json::json!({
            "name": HOST_NAME,
            "description": "Download Craft native host",
            "path": host.to_string_lossy(),
            "type": "stdio",
            "allowed_extensions": ["send-to-download-craft@example"]
        }),
        _ => serde_json::json!({
            "name": HOST_NAME,
            "description": "Download Craft native host",
            "path": host.to_string_lossy(),
            "type": "stdio",
            "allowed_origins": std::iter::once(EXTENSION_ID)
                .chain(STORE_EXTENSION_IDS.iter().copied())
                .map(|id| format!("chrome-extension://{id}/"))
                .collect::<Vec<_>>()
        }),
    };
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let key = native_host_key(browser);
    let mut rc = std::process::Command::new("reg");
    rc.args(["add", key, "/ve", "/d", &manifest_path.to_string_lossy(), "/f"]);
    quiet(&mut rc);
    let out = rc.output().map_err(|e| format!("reg failed: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    // Best-effort cleanup of the legacy host registration.
    let legacy_key = legacy_host_key(browser);
    let mut rc = std::process::Command::new("reg");
    rc.args(["delete", legacy_key, "/f"]);
    quiet(&mut rc);
    let _ = rc.output();
    Ok(manifest_path.to_string_lossy().to_string())
}

#[cfg(not(windows))]
pub(crate) fn install_native_host(browser: &str) -> Result<String,String> { crate::platform::install_native_host(browser) }

/// Whether `browser` has this app registered as its native host: the
/// registration exists and its manifest file is still on disk.
#[cfg(windows)]
pub(crate) fn native_host_status(browser: &str) -> bool {
    registry_default(native_host_key(browser)).is_some_and(|p| std::path::Path::new(&p).is_file())
}

/// The default value of an `HKCU\…` key, read as Unicode. `reg query`
/// prints in the OEM code page, which garbled non-ASCII manifest paths.
#[cfg(windows)]
fn registry_default(key: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let sub = key.strip_prefix(r"HKCU\")?;
    let wide: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `wide` is NUL-terminated and `buf`/`len` describe a writable
    // buffer; a null value name reads the key's default value.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return None;
    }
    let chars = (len as usize / 2).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars]).trim_end_matches('\0').to_string())
}

#[cfg(not(windows))]
pub(crate) fn native_host_status(browser: &str) -> bool {
    crate::platform::native_host_directory(browser)
        .map(|d| d.join("com.download_craft.json").is_file())
        .unwrap_or(false)
}

/// Registry key for a Chromium-family / Firefox native host (HKCU, no admin).
/// Opera / Brave / Vivaldi share the Chromium layout with vendor keys.
fn native_host_key(browser: &str) -> &'static str {
    match browser {
        "chrome" => r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.download_craft",
        "edge" => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.download_craft",
        "brave" => r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.download_craft",
        "opera" => r"HKCU\Software\Opera Software\NativeMessagingHosts\com.download_craft",
        "vivaldi" => r"HKCU\Software\Vivaldi\NativeMessagingHosts\com.download_craft",
        _ => r"HKCU\Software\Mozilla\NativeMessagingHosts\com.download_craft",
    }
}

fn legacy_host_key(browser: &str) -> &'static str {
    match browser {
        "chrome" => r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.idm_rust",
        "edge" => r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts\com.idm_rust",
        "brave" => r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.idm_rust",
        "opera" => r"HKCU\Software\Opera Software\NativeMessagingHosts\com.idm_rust",
        "vivaldi" => r"HKCU\Software\Vivaldi\NativeMessagingHosts\com.idm_rust",
        _ => r"HKCU\Software\Mozilla\NativeMessagingHosts\com.idm_rust",
    }
}
/// IDM-style zero-config: verify the native host is registered to THIS
/// binary on every launch, reinstalling silently when missing or stale
/// (first run, portable exe moved or renamed, etc.).
/// Returns Ok(true) when something was (re)installed.
/// Best-effort: failures are reported to the caller, never fatal.
#[cfg(windows)]
fn ensure_native_host(browser: &str) -> Result<bool, String> {
    let base = crate::model::config_dir()
        .map(|p| p.join("native-host"))
        .ok_or_else(|| "no config dir".to_string())?;
    let exe = crate::platform::launch_path().map_err(|e| e.to_string())?;
    let exe_s = exe.to_string_lossy().into_owned();
    let manifest_path = base.join(format!("{browser}.json"));
    let manifest_s = manifest_path.to_string_lossy().into_owned();
    // Fresh = manifest exists and names this exact binary, in the form
    // `install_native_host` picks: directly, or through its launcher.
    let direct = !exe_s.is_ascii();
    let fresh = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("path")
                .and_then(|p| p.as_str())
                .map(|s| s.to_string())
        })
        .map(|host| {
            if direct {
                host == exe_s
            } else {
                std::fs::read_to_string(&host)
                    .map(|c| c.contains(exe_s.as_str()))
                    .unwrap_or(false)
            }
        })
        .unwrap_or(false);
    let registered = registry_default(native_host_key(browser)).is_some_and(|v| v == manifest_s);
    if fresh && registered {
        return Ok(false);
    }
    install_native_host(browser)?;
    Ok(true)
}

#[cfg(not(windows))]
fn ensure_native_host(browser:&str)->Result<bool,String>{
    crate::platform::install_native_host(browser).map(|_|true)
}

/// Opt-in update check (P3.4, extended P5): compare the running version
/// against the latest GitHub release tag, and locate the portable Windows
/// asset when one is newer. Network failures become a status string —
/// never a popup, never an auto-download. The third field says "a newer
/// release exists", which is what puts the chip in the title bar.
async fn update_check_async() -> (String, Option<crate::selfupdate::Asset>, bool) {
    let current = env!("CARGO_PKG_VERSION");
    let client = match reqwest::Client::builder()
        .user_agent(format!("download_craft/{current}"))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return (format!("Update check failed: {e:#}"), None, false),
    };
    // DOWNLOAD_CRAFT_UPDATE_API points the check at another release JSON
    // (used to test updates end to end without publishing one).
    let api = std::env::var("DOWNLOAD_CRAFT_UPDATE_API")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "https://api.github.com/repos/hungry-detective/downloadcraft/releases/latest".to_string());
    let resp = client
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return (format!("Update check failed (network): {e:#}"), None, false),
    };
    if !resp.status().is_success() {
        return (format!("Update check failed: HTTP {}", resp.status()), None, false);
    }
    let v: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return (format!("Update check failed (parse): {e:#}"), None, false),
    };
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("");
    if tag.is_empty() {
        return ("Update check failed: no releases found.".to_string(), None, false);
    }
    // What the About window shows about the newest release.
    crate::about::save_release_info(&v, crate::model::is_newer_version(current, tag));
    if crate::model::is_newer_version(current, tag) {
        let url = v
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or("https://github.com/hungry-detective/downloadcraft/releases");
        (
            format!("Update available: {tag} (you have v{current}) — {url}"),
            crate::selfupdate::pick_asset(&v),
            true,
        )
    } else {
        (format!("Up to date (v{current})."), None, false)
    }
}

/// Best-effort desktop toast (ignored when unsupported).
fn notify(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show();
}

/// Build the system tray icon (uses the bundled app icon).
#[cfg(windows)]
fn build_tray() -> Option<tray_icon::TrayIcon> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), w, h).ok()?;
    let menu = tray_icon::menu::Menu::new();
    let show = tray_icon::menu::MenuItem::with_id("tray-show", "Show Download Craft", true, None);
    let exit = tray_icon::menu::MenuItem::with_id("tray-exit", "Exit", true, None);
    menu.append_items(&[&show, &exit]).ok()?;
    // Menu clicks are acted on as they arrive (see `tray::install_menu_handler`),
    // so Show and Exit work even if the frame loop has gone to sleep.
    crate::tray::install_menu_handler();
    tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // Left click must bring the app up, not open the menu (right click
        // still opens it).
        .with_menu_on_left_click(false)
        .with_tooltip(crate::tray::tooltip())
        .with_icon(icon)
        .build()
        .ok()
}

// Portable first version uses the regular taskbar/Dock window. This avoids
// depending on an AppIndicator service or a second GTK event loop.
#[cfg(not(windows))]
fn build_tray()->Option<PlatformTray>{None}

fn clipboard_watch(tx: std::sync::mpsc::Sender<String>) {
    let mut last = String::new();
    let mut cb = arboard::Clipboard::new().ok();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let text = cb
            .as_mut()
            .and_then(|c| c.get_text().ok())
            .unwrap_or_default();
        let t = text.trim().to_string();
        if t != last && is_download_url(&t) {
            last = t.clone();
            let _ = tx.send(t);
        }
    }
}

// Column widths.
const CHECK_W: f32 = 32.0;
const SIZE_W: f32 = 96.0;
const STATUS_W: f32 = 104.0;
const SPEED_W: f32 = 74.0;
const ETA_W: f32 = 74.0;
const DATE_W: f32 = 104.0;

impl eframe::App for CraftApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(notes) = self.startup_maintenance_rx.try_recv() {
            for note in notes {
                self.push_log(note);
            }
        }
        // Script fonts load the first time a name needs them (see fonts).
        for d in &self.downloads {
            crate::fonts::ensure_text(ctx, &d.filename);
        }
        crate::fonts::ensure_text(ctx, &self.add_file);
        if let Some(text) = crate::i18n::take_font_text() {
            crate::fonts::ensure_text(ctx, &text);
        }
        crate::tray::set_context(ctx);
        self.watch_ui_state();
        // The tray helpers work on the native window (parking = visible but
        // far off-screen, because a hidden window stops painting and eframe
        // then stops running frames altogether).
        {
            use raw_window_handle::HasWindowHandle;
            if let Ok(handle) = frame.window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(w) = handle.as_raw() {
                    crate::tray::set_hwnd(w.hwnd.get());
                }
            }
        }
        // Started at sign-in: straight to the tray (minimized when the tray
        // is off, so it is never running with no way back to it).
        if self.start_hidden && self.frame_count_for_startup() {
            self.start_hidden = false;
            if self.close_to_tray {
                if self.tray.is_none() {
                    self.tray = build_tray();
                }
                crate::tray::park();
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        // Every frame is counted, and a watchdog thread restarts the loop if the
        // frames stop: Windows hands out no `WM_PAINT` for a window it has
        // nothing to draw for, which used to leave the parked app asleep and its
        // tray menu dead (see `tray::start_keepalive`).
        crate::tray::frame_tick();
        crate::tray::start_keepalive();

        // Open in the middle of the work area (Windows would cascade the
        // window). Positions the *outer* rectangle, so the title bar does not
        // push the visible card off centre, and never fights a maximized
        // window. A move the user makes afterwards is left alone.
        if !self.placement_done {
            let info = ctx.input(|i| i.viewport().clone());
            if info.maximized == Some(true) {
                self.placement_done = true;
            } else {
                let size = info
                    .outer_rect
                    .or(info.inner_rect)
                    .map(|r| r.size())
                    .unwrap_or(egui::vec2(1050.0, 600.0));
                if crate::model::center_window(ctx, size) {
                    self.placement_done = true;
                }
            }
        }
        // Launched from the shell, a fresh window can still come up behind the
        // app that was in front. Ask for the foreground once, after the window
        // has its final size and position.
        if self.placement_done && !crate::tray::is_parked() {
            static STARTUP_RAISED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !STARTUP_RAISED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                crate::tray::show();
            }
        }
        // Manual pointer routing: snapshot input, then match layout rects
        // recorded below (process_hits runs after all layout, topmost first).
        self.man.begin(ctx);
        // Interface size: text, icons and spacing grow together, so nothing
        // laid out by hand drifts from its hit area.
        if (ctx.zoom_factor() - self.ui_scale).abs() > 0.001 {
            ctx.set_zoom_factor(self.ui_scale);
        }
        // Borderless window: the edges resize it (see `handle_window_edges`).
        self.handle_window_edges(ctx);
        self.hits.clear();
        while let Ok(url) = self.clipboard_rx.try_recv() {
            if !self.downloads.iter().any(|d| d.url == url)
                && self.clipboard_url.as_deref() != Some(url.as_str())
            {
                self.clipboard_url = Some(url);
            }
        }

        self.poll_tasks();
        self.poll_verify_jobs();
        self.write_progress_snapshot();
        self.check_scheduler();
        if self.last_inbox_check.elapsed().as_secs() >= 2 {
            self.last_inbox_check = Instant::now();
            crate::model::touch_heartbeat();
            // Notes the browser extension sent (e.g. a site changed layout).
            for note in crate::native_host::take_extension_notes() {
                self.push_log(format!("Browser extension: {note}"));
            }
            self.watch_ui_state();
            self.merge_queue_from_disk();
            self.check_wake();
            self.check_cmd();
            self.publish_update_state();
            self.refresh_speed_limit();
            // Opt-in update check, once per session (silent unless newer).
            if self.update_check_on && !self.update_auto_done {
                self.update_auto_done = true;
                self.start_update_check();
            }
            self.poll_self_update();
            // Missing helpers are fetched in the background right after the
            // first start, so a new user's first video download just works.
            if !self.helpers_started {
                self.helpers_started = true;
                // Something missing (a helper, the browser connection, the
                // extension): the "Get ready" window shows what, and fixes
                // what it can at once. Otherwise helpers are only refreshed.
                if crate::model::ui_pref_bool("setup_at_start", true) && crate::setup::something_missing() {
                    // The window does the downloading: two processes must
                    // not fetch the same helper into the same file.
                    crate::setup::spawn(true);
                } else {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.helpers_rx = Some(rx);
                    self.rt.spawn(async move {
                        let _ = tx.send(crate::engine::prepare_helpers().await);
                    });
                }
            }
            if let Some(lines) = self.helpers_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.helpers_rx = None;
                for l in lines {
                    self.push_log(l);
                }
            }
            // Installed while downloads were running: restart as soon as the
            // last one stops (an interrupted one resumes after the restart).
            if self.update_restart_ready
                && ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("update_restart_now"))).unwrap_or(false)
            {
                self.update_auto = false;
                self.restart_for_update();
            }
            if self.update_restart_ready && self.update_auto && self.tasks.is_empty() {
                self.update_auto = false;
                notify("Download Craft updated", "Restarting into the new version…");
                self.restart_for_update();
            }
            if let Some(rx) = self.update_rx.as_ref() {
                let mut got = None;
                while let Ok(m) = rx.try_recv() {
                    got = Some(m);
                }
                if let Some((m, asset, found)) = got {
                    self.update_busy = false;
                    self.update_status = m.clone();
                    self.update_asset = asset;
                    self.update_found = found;
                    if m.starts_with("Update available") {
                        self.push_log(m);
                    }
                    // A portable copy updates itself: download, check, replace
                    // every file in its folder, then restart into the new
                    // version once no download is running.
                    let portable = cfg!(windows) && crate::selfupdate::portable_dir().is_some();
                    let auto = crate::model::ui_pref_bool("update_auto", true);
                    if found && portable && auto && self.update_asset.is_some() && self.selfupd_rx.is_none() {
                        self.push_log("Installing the update in the background…");
                        self.update_auto = true;
                        self.start_self_update();
                    }
                }
            }
            if self.poll_inbox() {
                // Extension sent something: pop the app up with the prefilled
                // Add dialog (out of the tray if that's where it lives).
                crate::tray::show();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // Repaint often while a transfer is live so the speed column and the
        // progress windows move in real time; idle, a few times a second is
        // plenty (pointer input repaints immediately on its own). Parked in
        // the tray nobody watches the charts, so a slower tick keeps the
        // app fully alive (inbox, completions, heartbeat) without burning
        // frames on an invisible window.
        // Footprint: every repaint redraws the whole window, so idle it only
        // wakes once a second (inbox, schedule, completions) and a live list
        // four times a second - enough for the speed column. Input repaints
        // at once anyway.
        let tick_ms = if crate::tray::is_parked() || self.tasks.is_empty() {
            1000
        } else {
            250
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(tick_ms));

        // theme (re-applied when the user switches it, and while "system"
        // follows the OS: the resolved key carries the OS's answer)
        let resolved = format!("{}:{}", self.theme, theme_is_dark(ctx, &self.theme));
        if self.theme_applied != resolved {
            apply_theme(ctx, &self.theme);
            // The native title bar follows the app theme (Windows otherwise
            // keeps it dark for as long as the OS itself is dark), in a shade
            // of its own: the page colour made it look like there was none.
            crate::tray::theme_caption(theme_is_dark(ctx, &self.theme));
            self.theme_applied = resolved;
        }

        // system tray
        if self.close_to_tray && self.tray.is_none() {
            self.tray = build_tray();
        }
        if !self.close_to_tray {
            self.tray = None;
            // Turning the tray off must not leave the window parked off-screen
            // (but a quit in flight is on its way out: don't flash it back).
            if !crate::tray::quitting() {
                crate::tray::restore();
            }
        }
        // The menu itself is handled in `tray::install_menu_handler`, which acts
        // on the click without waiting for a frame; this loop is the fallback
        // for the case where that handler could not be installed.
        #[cfg(windows)]
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id() == "tray-show" {
                crate::tray::show();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if ev.id() == "tray-exit" {
                crate::tray::request_quit();
            }
        }
        #[cfg(windows)]
        while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if matches!(
                ev,
                tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } | tray_icon::TrayIconEvent::DoubleClick {
                    button: tray_icon::MouseButton::Left,
                    ..
                }
            ) {
                crate::tray::show();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // Another launch (Start menu, pinned shortcut) asked us to come
        // forward. Only this thread can win the foreground lock, so the
        // request sat in a flag until now instead of raising from the listener.
        if crate::tray::take_show_request() && !crate::tray::quitting() {
            crate::tray::show();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // Tray's Exit asks for a clean shutdown; the menu handler starts it and
        // nudges a paint, so this frame runs even if the loop had gone to sleep.
        if crate::tray::take_quit() {
            self.close_to_tray = false;
            // Drop the icon *now*: the window can close before another frame
            // runs, and an icon left behind lingers in the tray as a ghost.
            self.tray = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // close button => minimize to tray instead of quitting (only if tray lives)
        if self.close_to_tray
            && self.tray.is_some()
            && ctx.input(|i| i.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            // Parking (not hiding) keeps the app running: hiding the window
            // stops WM_PAINT, which stops eframe frames, which froze transfers
            // and the browser inbox while "in the tray".
            self.park_to_tray();
        } else if self.close_to_tray
            && self.tray.is_some()
            && ctx.input(|i| i.viewport().minimized) == Some(true)
            && !crate::tray::is_parked()
        {
            // A minimized window doesn't paint either, and that freezes the
            // same work; minimizing therefore also goes to the tray.
            crate::tray::park();
        }

        // The window is borderless: the title bar below is the app's own
        // (`main.rs` turns the OS caption off). Dragging and Aero snap go
        // through the OS move loop, the resize strips through
        // `handle_window_edges`.

        // ---- per-frame counts, shared by the sidebar and the status bar ----
        let n_all = self.downloads.len();
        let mut n_done = 0usize;
        let mut n_active = 0usize;
        let mut n_paused = 0usize;
        let mut n_failed = 0usize;
        let mut n_queued = 0usize;
        let mut cat_n: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for d in &self.downloads {
            *cat_n.entry(d.category()).or_default() += 1;
            match &d.status {
                Status::Completed => n_done += 1,
                Status::Downloading => n_active += 1,
                Status::Paused => n_paused += 1,
                Status::Failed(_) => n_failed += 1,
                Status::Queued => n_queued += 1,
            }
        }
        let n_incomplete = n_all - n_done;

        // ================= title bar =================
        // `main_window_titlebar.html`: one 46 px row carrying the brand, the
        // File/Tasks/Tools/Theme/Help menus, the list search, the update chip
        // and the window buttons. The taskbar (Add URL, Resume, Stop …) stays
        // in the workspace card below — only the menu row and the search moved
        // up here. The OS caption is off (`main.rs` drops the decorations), so
        // this row is also what drags the window; `handle_window_edges` owns
        // the resize strips. Dropdowns still render at the end of update
        // (topmost).
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        egui::TopBottomPanel::top("titlebar")
            .exact_height(TB_H)
            .frame(egui::Frame::new().fill(pl().bg).inner_margin(egui::Margin {
                left: 12,
                right: 8,
                top: 0,
                bottom: 0,
            }))
            .show(ctx, |ui| {
                let (bar, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), TB_H),
                    egui::Sense::hover(),
                );
                let p = ui.painter().clone();
                let cy = bar.center().y;
                // Interactive children: a press on one of these must never
                // turn into a window drag.
                let mut live: Vec<egui::Rect> = Vec::new();

                // ---- brand: the dialog mark + the product name ----
                let mark = egui::Rect::from_center_size(
                    egui::pos2(bar.left() + 9.0, cy),
                    egui::Vec2::splat(18.0),
                );
                crate::dm::dm_mark(&p, mark);
                let name = p.layout_no_wrap(
                    crate::model::APP_NAME.to_owned(),
                    crate::fonts::semi(13.0),
                    pl().txt,
                );
                p.galley(
                    egui::pos2(mark.right() + 6.0, cy - name.size().y / 2.0),
                    name.clone(),
                    pl().txt,
                );
                let mut x = mark.right() + 6.0 + name.size().x + 11.0;
                p.vline(x, (cy - 9.0)..=(cy + 9.0), egui::Stroke::new(1.0, pl().card_bd));
                x += 9.0;

                // ---- menus (`.menu-item`): the same dropdowns, now up here ----
                for (m, label) in ["File", "Tasks", "Tools", "Theme", "Help"].iter().enumerate() {
                    let g = p.layout_no_wrap(label.to_string(), crate::fonts::reg(11.0), pl().dim);
                    let r = egui::Rect::from_min_size(
                        egui::pos2(x, cy - 12.0),
                        egui::vec2(g.size().x + 12.0, 24.0),
                    );
                    let hov = self.man.hover(&r);
                    let open = self.drop_menu == Some(m as u8);
                    if hov || open {
                        p.rect_filled(r, 5.0, pl().surface_lt);
                    }
                    p.galley(
                        egui::pos2(r.center().x - g.size().x / 2.0, cy - g.size().y / 2.0),
                        g,
                        if hov || open { pl().txt } else { pl().dim },
                    );
                    self.hits.push((HitKind::MenuBtn(m as u8), r));
                    live.push(r);
                    x = r.right() + 1.0;
                }

                // ---- right edge, built inwards from the window buttons:
                // controls · update chip · search (`.win-controls` order) ----
                let ctrl_l = if self.os_caption {
                    bar.right()
                } else {
                    bar.right() - (28.0 * 3.0 + 2.0)
                };
                let buttons: &[(DmIcon, u8)] = if self.os_caption {
                    &[]
                } else {
                    &[
                        (DmIcon::Minimize, 0u8),
                        (
                            if maximized { DmIcon::Restore } else { DmIcon::Maximize },
                            1,
                        ),
                        (DmIcon::Close, 2),
                    ]
                };
                for (i, (icon, kind)) in buttons.iter().enumerate() {
                    let r = egui::Rect::from_min_size(
                        egui::pos2(ctrl_l + i as f32 * 29.0, cy - 13.0),
                        egui::vec2(28.0, 26.0),
                    );
                    let hov = self.man.hover(&r);
                    let close = *kind == 2;
                    if hov {
                        p.rect_filled(r, 4.0, if close { pl().error } else { pl().surface_lt });
                    }
                    dm_icon(
                        &p,
                        egui::Rect::from_center_size(r.center(), egui::Vec2::splat(12.0)),
                        *icon,
                        if hov {
                            if close { egui::Color32::WHITE } else { pl().txt }
                        } else {
                            pl().dim
                        },
                        2.0,
                    );
                    if self.man.clicked(&r) {
                        match kind {
                            0 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                            1 => {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized))
                            }
                            _ => {
                                if self.close_to_tray && self.tray.is_some() {
                                    self.park_to_tray();
                                } else {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            }
                        }
                    }
                    live.push(r);
                }
                let mut right = ctrl_l - 8.0;

                // ---- update chip: only when a newer release was found on
                // launch (`.update-btn`). A circle at rest; the word "Update"
                // grows out of it on hover. The glyph is the concept's arrow
                // *without* its baseline (the line under it read as a second,
                // unrelated icon).
                // Every stage of an update shows here: found ("Update", opens
                // the About & Updates window), downloading and installing in
                // the background ("Updating 45%"), and installed while
                // downloads were running ("Restart": swaps in the new version
                // now; the running downloads resume after it).
                let installing = self.selfupd_rx.is_some();
                let stage: Option<(String, &str, egui::Color32, bool)> = if self.update_restart_ready {
                    Some(("Restart to update".to_owned(), MI_REFRESH, pl().success, true))
                } else if installing {
                    let pct: String = self.update_status.chars().filter(|c| c.is_ascii_digit()).collect();
                    let label = if self.update_status.contains('%') && !pct.is_empty() {
                        format!("Updating {pct}%")
                    } else {
                        "Updating…".to_owned()
                    };
                    Some((label, MI_ARROW_DOWN, pl().primary, false))
                } else if self.update_found && !self.update_busy {
                    Some(("Update".to_owned(), MI_ARROW_DOWN, pl().primary, true))
                } else {
                    None
                };
                if let Some((word, glyph, accent, clickable)) = stage {
                    // The first stage rests as a circle; the later ones keep
                    // their word showing, since they report something.
                    let always = self.update_restart_ready || installing;
                    let rest = egui::Rect::from_min_size(
                        egui::pos2(right - 28.0, cy - 14.0),
                        egui::vec2(28.0, 28.0),
                    );
                    let hov = self.man.hover(&rest);
                    let t = if always { 1.0 } else { ctx.animate_bool_with_time(egui::Id::new("update_chip"), hov, 0.22) };
                    if installing {
                        ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    }
                    // Blink for the first seconds after an update is found,
                    // so it is noticed; then it rests.
                    let now = ui.input(|i| i.time);
                    let since = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(egui::Id::new("update_chip_since"), || now));
                    let blink = if now - since < 6.0 && !hov {
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                        0.5 + 0.5 * ((now - since) as f32 * 5.0).sin()
                    } else {
                        0.0
                    };
                    let label = p.layout_no_wrap(
                        word.clone(),
                        crate::fonts::semi(10.5),
                        accent,
                    );
                    let w = 28.0 + t * (label.size().x + 10.0);
                    let chip = egui::Rect::from_min_size(
                        egui::pos2(right - w, cy - 14.0),
                        egui::vec2(w, 28.0),
                    );
                    let chip_hov = self.man.hover(&chip);
                    p.rect_filled(chip, 14.0, wash(pl().txt, if chip_hov && clickable { 20 } else { 12 }));
                    if always {
                        p.rect_stroke(chip, 14.0, egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
                    }
                    if blink > 0.0 {
                        p.rect_filled(chip, 14.0, accent.gamma_multiply(0.35 * blink));
                        p.rect_stroke(chip, 14.0, egui::Stroke::new(1.5_f32, accent.gamma_multiply(blink)), egui::StrokeKind::Inside);
                    }
                    // The whole arrow (head included) from the icon font: the
                    // hand-drawn strokes lost their head at this size.
                    let ib = egui::Rect::from_min_size(
                        egui::pos2(chip.left() + 7.0, cy - 7.0),
                        egui::Vec2::splat(14.0),
                    );
                    p.text(
                        egui::pos2(chip.left() + 14.0, cy),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        crate::fonts::icon(16.0),
                        accent,
                    );
                    if t > 0.01 {
                        p.galley(
                            egui::pos2(ib.right() + 5.0, cy - label.size().y / 2.0),
                            p.layout_no_wrap(
                                word.clone(),
                                crate::fonts::semi(10.5),
                                accent.gamma_multiply(t),
                            ),
                            accent,
                        );
                    }
                    if clickable && self.man.clicked(&chip) {
                        if self.update_restart_ready {
                            // Handled after the frame: restarting closes it.
                            ctx.data_mut(|d| d.insert_temp(egui::Id::new("update_restart_now"), true));
                        } else {
                            // The About & Updates window: details, Update now.
                            crate::about::spawn();
                        }
                    }
                    live.push(chip);
                    right = chip.left() - 8.0;
                }

                // ---- search: a round magnifier at rest; clicking it opens
                // the box with the cursor in it. It stays open while it has
                // the cursor or holds a search, and closes when you click
                // elsewhere with it empty. ----
                // As wide as the free stretch of the bar allows (issue #8),
                // between the old 176 px and 320 px.
                let search_full = (right - x - 24.0).clamp(176.0, 320.0);
                let search_id = egui::Id::new("search");
                let focused = ui.memory(|m| m.has_focus(search_id));
                let open_w = ctx.data(|d| d.get_temp::<f32>(egui::Id::new("search_w"))).unwrap_or(28.0);
                let hot = egui::Rect::from_min_size(egui::pos2(right - open_w, cy - 14.0), egui::vec2(open_w, 28.0));
                let hov = self.man.hover(&hot);
                // Clicked and waiting for the cursor to land in the box.
                let req_id = egui::Id::new("search_req");
                let mut requested = ctx.data(|d| d.get_temp::<bool>(req_id)).unwrap_or(false);
                if !focused && self.search.is_empty() && self.man.clicked(&hot) {
                    requested = true;
                }
                if requested && focused {
                    requested = false;
                }
                ctx.data_mut(|d| d.insert_temp(req_id, requested));
                let open = requested || focused || !self.search.is_empty();
                let t = ctx.animate_bool_with_time(egui::Id::new("search_open"), open, 0.18);
                let w = 28.0 + t * (search_full - 28.0);
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("search_w"), w));
                if requested && !focused && t > 0.6 {
                    ui.memory_mut(|m| m.request_focus(search_id));
                }
                let pill = egui::Rect::from_min_size(
                    egui::pos2(right - w, cy - 14.0),
                    egui::vec2(w, 28.0),
                );
                if focused {
                    // 3 px accent glow, where the concept uses box-shadow.
                    p.rect_stroke(
                        pill.expand(1.5),
                        16.0,
                        egui::Stroke::new(3.0, wash(pl().primary, 34)),
                        egui::StrokeKind::Middle,
                    );
                }
                p.rect(
                    pill,
                    14.0,
                    wash(pl().txt, if focused { 13 } else if hov { 16 } else { 10 }),
                    egui::Stroke::new(
                        1.0,
                        if focused { pl().primary } else { egui::Color32::TRANSPARENT },
                    ),
                    egui::StrokeKind::Inside,
                );
                p.text(
                    egui::pos2(pill.left() + 14.0 + 3.5 * t, cy),
                    egui::Align2::CENTER_CENTER,
                    MI_SEARCH.to_string(),
                    crate::fonts::icon(if t > 0.5 { 11.0 } else { 14.0 }),
                    if hov { pl().txt } else { pl().dim },
                );
                if t > 0.35 {
                    let inner = egui::Rect::from_min_max(
                        egui::pos2(pill.left() + 30.0, pill.top() + 4.0),
                        egui::pos2(pill.right() - 10.0, pill.bottom() - 4.0),
                    );
                    let mut fui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(inner)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    fui.set_clip_rect(inner);
                    mtext_bare(
                        &mut fui,
                        "search",
                        &mut self.search,
                        "Search in the List",
                        inner.width(),
                        crate::fonts::reg(10.5),
                    );
                }
                self.hits.push((HitKind::Field("search"), pill));
                live.push(pill);

                // ---- the strip itself moves the window, through the OS: a
                // custom drag hook would lose Aero snap. A second press
                // inside the double-click window toggles maximize instead
                // (there is no OS caption left to do that for us). ----
                let over_live = self
                    .man
                    .pos()
                    .map_or(false, |q| live.iter().any(|r| r.contains(q)));
                if !over_live && !self.os_caption {
                    if let Some(q) = self.man.pos() {
                        if self.man.press_started_in(&bar) {
                            let now = std::time::Instant::now();
                            let again = matches!(
                                self.tb_press,
                                Some((t, p))
                                    if now.duration_since(t).as_millis() <= 420
                                        && p.distance(q) <= 24.0
                            );
                            if again {
                                self.tb_press = None;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                            } else {
                                self.tb_press = Some((now, q));
                                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            }
                        }
                    }
                }
            });
        // Menu/dropdown actions run from process_hits (see do_menu_act).


        // ================= bottom status bar =================
        // The concept's `.status-bar`: session totals on the left, state on the
        // right — a full-width strip under both cards. Declared before the
        // side panel so it spans the whole window instead of only the
        // workspace's column.
        egui::TopBottomPanel::bottom("statusbar")
            .frame(egui::Frame::new().fill(pl().bg).inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 3,
                bottom: 3,
            }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let active = self.tasks.len();
                    let total = self.downloads.len();
                    let total_speed: f64 = self.downloads.iter().map(|d| d.speed_bps).sum();
                    let mut left = format!("{active} downloading · {total} in list");
                    if total_speed > 0.0 {
                        left.push_str(&format!("   ↓ {}", format_speed(total_speed)));
                    }
                    ui.label(egui::RichText::new(left).size(10.5).color(pl().dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let q = if self.queue_running { "running" } else { "idle" };
                        ui.label(egui::RichText::new(format!("Queue: {q}")).size(10.5).color(pl().dim));
                    });
                });
            });


        // ================= left sidebar (collapsible tree) =================
        // The concept draws the sidebar as its own bordered card inset from the
        // window edge (`.sidebar`): scrolling nav rows on top, a ruled footer
        // pinned to the bottom.
        egui::SidePanel::left("cats")
            .resizable(false)
            .exact_width(198.0)
            .frame(
                egui::Frame::new()
                    .fill(pl().bg)
                    .inner_margin(egui::Margin {
                        left: 12,
                        right: 6,
                        top: 12,
                        bottom: 12,
                    }),
            )
            .show(ctx, |ui| {
                // The card is built from explicit heights below, so the
                // style's 10px item spacing must not creep in between the
                // scroll area and the pinned footer — that made the sidebar
                // card 12px taller than the download-list card.
                ui.spacing_mut().item_spacing.y = 0.0;
                let card_h = ui.available_height();
                let foot_h = 24.0;
                let pad = 8.0;
                let list_h = (card_h - pad * 2.0 - foot_h).max(80.0);
                egui::Frame::new()
                    .fill(pl().central_bg)
                    .stroke(egui::Stroke::new(1.0_f32, pl().card_bd))
                    .corner_radius(RADIUS)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let cur = self.filter.clone();
                        let cats: [(&str, &str, &str); 6] = [
                            ("Image", MI_IMAGE, crate::i18n::t("Pictures")),
                            ("Video", MI_SLIDESHOW, crate::i18n::t("Videos")),
                            ("Music", MI_MUSIC, crate::i18n::t("Music")),
                            ("Document", MI_DOC, crate::i18n::t("Documents")),
                            ("Compressed", MI_ARCHIVE, crate::i18n::t("Compressed")),
                            ("Apps", MI_GRID, crate::i18n::t("Apps")),
                        ];
                        let statuses: [(&str, &str, &str, usize, bool); 5] = [
                            ("Downloading", MI_GET_APP, crate::i18n::t("Downloading"), n_active, n_active > 0),
                            ("Queued", MI_FILTER, crate::i18n::t("Queued"), n_queued, false),
                            ("Paused", MI_PAUSE, crate::i18n::t("Paused"), n_paused, false),
                            ("Completed", MI_CHECK_CIRCLE, crate::i18n::t("Finished"), n_done, false),
                            ("Failed", MI_CANCEL, crate::i18n::t("Failed"), n_failed, false),
                        ];
                        egui::ScrollArea::vertical()
                            .id_salt("side_nav")
                            .max_height(list_h)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                // Flush rows: the 1px-in highlight inset gives
                                // the concept's 2px gap without paying for it in
                                // layout, so the whole tree fits in the card.
                                ui.spacing_mut().item_spacing.y = 0.0;
                                // Manual rows: paint (+record hit); process_hits
                                // applies selection. `cur` snapshot avoids
                                // borrow issues.
                                let nav = |ui: &mut egui::Ui,
                                           hits: &mut Vec<(HitKind, egui::Rect)>,
                                           man: &ManState,
                                           cur: &str,
                                           key: &str,
                                           icon: &str,
                                           label: &str,
                                           n: Option<usize>,
                                           dot: bool| {
                                    let r = side_nav_row(
                                        ui,
                                        man,
                                        cur == key,
                                        icon,
                                        label,
                                        n,
                                        dot,
                                        key,
                                    );
                                    hits.push((HitKind::SideRow(key.to_string()), r));
                                };
                                nav(
                                    ui,
                                    &mut self.hits,
                                    &self.man,
                                    &cur,
                                    "All",
                                    MI_APPS_DOTS,
                                    "All",
                                    Some(n_all),
                                    false,
                                );
                                ui.add_space(4.0);
                                let hdr =
                                    side_section(ui, &self.man, "Categories", self.cats_open);
                                self.hits.push((HitKind::SideToggle(0), hdr));
                                if self.cats_open {
                                    for (key, icon, label) in cats {
                                        let n = cat_n.get(key).copied().unwrap_or(0);
                                        nav(
                                            ui,
                                            &mut self.hits,
                                            &self.man,
                                            &cur,
                                            key,
                                            icon,
                                            label,
                                            Some(n),
                                            false,
                                        );
                                    }
                                }
                                ui.add_space(4.0);
                                let hdr =
                                    side_section(ui, &self.man, "Status", self.status_open);
                                self.hits.push((HitKind::SideToggle(1), hdr));
                                if self.status_open {
                                    for (key, icon, label, n, dot) in statuses {
                                        nav(
                                            ui,
                                            &mut self.hits,
                                            &self.man,
                                            &cur,
                                            key,
                                            icon,
                                            label,
                                            Some(n),
                                            dot,
                                        );
                                    }
                                    // Legacy "Unfinished" view kept reachable
                                    // (same as Incomplete).
                                    let r = side_nav_row(
                                        ui,
                                        &self.man,
                                        cur == "Unfinished" || cur == "Incomplete",
                                        MI_SCHEDULE,
                                        "Unfinished",
                                        Some(n_incomplete),
                                        false,
                                        "Unfinished",
                                    );
                                    self.hits
                                        .push((HitKind::SideRow("Unfinished".to_string()), r));
                                }
                            });
                        // ---- pinned footer (`.sidebar-footer`) ----
                        let (fr, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), foot_h),
                            egui::Sense::hover(),
                        );
                        ui.painter().hline(
                            fr.x_range(),
                            fr.top() + 0.5,
                            egui::Stroke::new(1.0_f32, pl().card_bd),
                        );
                        ui.painter().text(
                            egui::pos2(fr.min.x, fr.center().y + 3.0),
                            egui::Align2::LEFT_CENTER,
                            if self.stats_line.is_empty() {
                                format!("{n_active} active · {n_all} total")
                            } else {
                                format!("{n_active} active · {n_all} total · {}", self.stats_line)
                            },
                            egui::FontId::proportional(10.0),
                            pl().dim,
                        );
                    });
            });

        // ================= main area (toolbar + list, beside the sidebar) =================
        // The workspace is its own bordered card (`.content-area`), inset by the
        // same 12px gutter as the sidebar card.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pl().bg).inner_margin(egui::Margin {
                left: 6,
                right: 12,
                top: 12,
                bottom: 12,
            }))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let (outer, _) = ui.allocate_exact_size(avail, egui::Sense::hover());
                // One card for the toolbar and the list, a hairline between
                // them (the toolbar's height is known once it has laid out).
                ui.painter().rect(
                    outer,
                    RADIUS,
                    pl().central_bg,
                    egui::Stroke::new(1.0_f32, pl().card_bd),
                    egui::StrokeKind::Inside,
                );
                // Everything below lives inside the cards' 1px border.
                let mut card = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(outer.shrink(1.0))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                let ui = &mut card;
                // The card's vertical rhythm is entirely explicit (the toolbar,
                // the header band and the rows each place themselves), so the
                // style's 10px item spacing is switched off here — it used to
                // add a phantom gap around the column header.
                ui.spacing_mut().item_spacing.y = 0.0;
                // ---- toolbar card: Add URL, the actions, Settings ----
                // Manual routing: paint (+record hit); process_hits acts.
                let n_checked = self.selected_ids().len();
                let mut tools: Vec<(ToolId, &str, &str, bool, &str)> = Vec::new();
                tools.push((ToolId::ResumeSel, MI_PLAY_CIRCLE, crate::i18n::t("Resume"), true, crate::i18n::t("Resume the selected downloads (all paused when none is checked)")));
                tools.push((ToolId::StopSel, MI_PAUSE_CIRCLE, crate::i18n::t("Stop"), true, crate::i18n::t("Pause the selected downloads (all active when none is checked)")));
                tools.push((ToolId::StopAll, MI_STOP_CIRCLE, crate::i18n::t("Stop All"), true, crate::i18n::t("Pause every active download now")));
                tools.push((ToolId::Delete, MI_DELETE, crate::i18n::t("Delete"), n_checked > 0, crate::i18n::t("Remove the checked rows from the list")));
                tools.push((ToolId::Queues, MI_REORDER, crate::i18n::t("Queues"), true, crate::i18n::t("Queues: start, stop, schedule (also in Tasks)")));
                tools.push((ToolId::Grabber, MI_PUBLIC, crate::i18n::t("Grabber"), true, crate::i18n::t("Collect file links from a web page")));
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    ui.add_space(7.0);
                    ui.spacing_mut().item_spacing = egui::vec2(TOOL_GAP, 2.0);
                    // A narrow window: the buttons drop their labels, then
                    // the last ones go (Queues and Grabber are also in the
                    // Tasks / Tools menus), so nothing runs into Settings.
                    let room = ui.available_width() - (TOOL_H + 8.0 + TOOL_GAP) - 6.0;
                    let font = egui::FontId::proportional(12.0);
                    let label_w = |l: &str| ui.painter().layout_no_wrap(l.to_owned(), font.clone(), pl().txt).size().x;
                    let add_w = 12.0 + 16.0 + 7.0 + label_w(crate::i18n::t("Add URL")) + 14.0;
                    let sep_w = 13.0 + TOOL_GAP;
                    let full: f32 = tools.iter().map(|t| 10.0 + 16.0 + 7.0 + label_w(t.2) + 12.0 + TOOL_GAP).sum();
                    let icons_only = add_w + TOOL_GAP + sep_w + full > room;
                    let mut fit = tools.len();
                    if icons_only {
                        while fit > 0 && add_w + TOOL_GAP + sep_w + fit as f32 * (TOOL_H + TOOL_GAP) > room {
                            fit -= 1;
                        }
                    }
                    let r = add_url_button(ui, &self.man, self.clipboard_url.is_some());
                    self.hits.push((HitKind::Tool(ToolId::AddUrl), r));
                    // A hairline between the primary action and the rest.
                    let (sep, _) = ui.allocate_exact_size(egui::vec2(13.0, TOOL_H), egui::Sense::hover());
                    ui.painter().vline(sep.center().x, (sep.min.y + 6.0)..=(sep.max.y - 6.0), egui::Stroke::new(1.0_f32, pl().card_bd));
                    for (id, icon, label, enabled, tip) in tools.into_iter().take(fit) {
                        // Icon only: the label moves into the tooltip.
                        let (label, tip) = if icons_only { ("", label) } else { (label, tip) };
                        let r = tool_button(ui, &self.man, icon, label, enabled, tip);
                        self.hits.push((HitKind::Tool(id), r));
                    }
                    // Settings: an icon at the far end.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        let r = tool_button(ui, &self.man, MI_TUNE, "", true, crate::i18n::t("Open Settings in its own window"));
                        self.hits.push((HitKind::Tool(ToolId::Settings), r));
                    });
                });
                ui.add_space(7.0);
                // The toolbar ends here: a hairline, then the list.
                let split = ui.cursor().min.y;
                ui.painter().hline(
                    (outer.min.x + 1.0)..=(outer.max.x - 1.0),
                    split + 0.5,
                    egui::Stroke::new(1.0_f32, pl().card_bd),
                );
                ui.add_space(1.0);
                // A link found on the clipboard is surfaced on the toolbar's
                // Add URL button (see `add_url_button`), not as a banner here:
                // the banner pushed the whole list down by a row.
                // Bulk actions for the checked rows live in the toolbar itself
                // (Delete sits next to Stop All): a bar that appeared only while
                // something was checked pushed the whole list down by a row's
                // worth of height, so it is gone.
                let ids = self.visible_ids();
                if ids.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            ui.add_space(40.0);
                            ui.label(
                                egui::RichText::new(MI_CLOUD_DL)
                                    .font(crate::fonts::icon(48.0))
                                    .color(pl().surface_lt),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("No downloads yet").size(14.0).color(pl().dim),
                            );
                            ui.label(
                                egui::RichText::new("Click Add URL to start downloading.")
                                    .size(12.0)
                                    .color(pl().dim),
                            );
                        });
                    });
                } else {
                    // ---- fully manual table: explicit geometry for header
                    // and rows + manual pointer routing (no egui hit-test) ----
                    // Column metrics follow `main_window.html`: contiguous
                    // columns (no gutters), Name left-aligned, every numeric
                    // column right-aligned, and one rule under the header.
                    const ROW_H: f32 = 34.0;
                    const HDR_H: f32 = 30.0;
                    let cols = self.col_order.clone();
                    // The always-visible vertical bar keeps its own strip.
                    let avail_w = ui.available_width() - SB_SPACE;
                    // Every column keeps its own width (IDM): widening one moves
                    // the ones to its right, and the list scrolls sideways when
                    // they no longer fit. Name fills the free room until the
                    // user gives it a width of its own.
                    let fixed: Vec<(SortCol, f32)> = cols
                        .iter()
                        .filter(|c| **c != SortCol::Name)
                        .map(|c| (*c, col_w_of(&self.col_w, *c)))
                        .collect();
                    let fixed_sum: f32 = fixed.iter().map(|(_, w)| *w).sum();
                    // Name also takes any room left over, so a wide window has
                    // no empty band at the right of the list.
                    let fill = avail_w - CHECK_W - fixed_sum;
                    let name_w = match self.col_w.get(&SortCol::Name) {
                        Some(w) => w.max(fill),
                        None => fill.max(col_min(SortCol::Name)),
                    };
                    let col_ws: Vec<f32> = cols
                        .iter()
                        .map(|c| {
                            if *c == SortCol::Name {
                                name_w
                            } else {
                                fixed
                                    .iter()
                                    .find(|(fc, _)| fc == c)
                                    .map(|(_, w)| *w)
                                    .unwrap_or_else(|| col_w_of(&self.col_w, *c))
                            }
                        })
                        .collect();
                    let total_w: f32 = CHECK_W + col_ws.iter().sum::<f32>();
                    self.col_shown = cols.iter().copied().zip(col_ws.iter().copied()).collect();
                    // Select-all state, snapshotted before the header.
                    let all_sel = !ids.is_empty()
                        && ids.iter().all(|id| {
                            self.downloads
                                .iter()
                                .find(|d| d.id == *id)
                                .map(|d| d.selected)
                                .unwrap_or(false)
                        });
                    // ---- header row: sticky above the rows, and moved
                    // sideways with them when the list scrolls horizontally ----
                    let hdr_top = ui.cursor().min;
                    let hdr_clip = egui::Rect::from_min_size(
                        hdr_top,
                        egui::vec2(ui.available_width(), HDR_H),
                    )
                    .intersect(ui.clip_rect());
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), HDR_H), egui::Sense::hover());
                    let mut hdr_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(egui::Rect::from_min_size(
                                egui::pos2(hdr_top.x - self.list_hoff, hdr_top.y),
                                egui::vec2(total_w + 40.0, HDR_H),
                            ))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    hdr_ui.set_clip_rect(hdr_clip);
                    hdr_ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let (cr, _) = ui.allocate_exact_size(
                            egui::vec2(CHECK_W, HDR_H),
                            egui::Sense::hover(),
                        );
                        // The row layout can push cells below the band's top;
                        // every cell is put on the band so its title sits in
                        // the middle.
                        let cr = egui::Rect::from_min_size(egui::pos2(cr.min.x, hdr_top.y), egui::vec2(cr.width(), HDR_H));
                        paint_check(
                            ui.painter(),
                            egui::Rect::from_center_size(cr.center(), egui::vec2(13.0, 13.0)),
                            all_sel,
                            self.man.hover(&cr),
                        );
                        self.hits.push((HitKind::HdrCheck, cr.intersect(hdr_clip)));
                        for (ci, col) in cols.iter().enumerate() {
                            let w = col_ws[ci];
                            let (r, _) = ui.allocate_exact_size(
                                egui::vec2(w, HDR_H),
                                egui::Sense::hover(),
                            );
                            let r = egui::Rect::from_min_size(egui::pos2(r.min.x, hdr_top.y), egui::vec2(w, HDR_H));
                            let sorted = self.sort_col == *col;
                            let arrow = if sorted {
                                if self.sort_asc { " ▲" } else { " ▼" }
                            } else {
                                ""
                            };
                            let txt = format!("{}{}", col.title().to_uppercase(), arrow);
                            let color = if self.man.hover(&r) || sorted {
                                pl().txt
                            } else {
                                pl().dim
                            };
                            // Size, Status, Speed, Time Left and Date: an icon and the
                            // title, centred over values that are centred too. Name
                            // reads from the left.
                            let icon = if self.hide_col_icons { None } else { col.icon() };
                            match icon {
                                // Name: its icon over the rows' file marks, its
                                // title over the file names (read from the left).
                                Some(icon) if matches!(*col, SortCol::Name) => {
                                    let p = ui.painter();
                                    p.text(egui::pos2(r.min.x + 17.0, r.center().y), egui::Align2::CENTER_CENTER, icon, crate::fonts::icon(12.5), color);
                                    let shown = truncate_to_width(p, &txt, egui::FontId::proportional(9.5), color, (r.width() - 40.0).max(12.0));
                                    crate::shaped::draw(p, egui::pos2(r.min.x + 32.0, r.center().y), egui::Align2::LEFT_CENTER, &shown, egui::FontId::proportional(9.5), color);
                                }
                                Some(icon) => header_icon_text(ui.painter(), r, icon, &txt, color),
                                None if matches!(*col, SortCol::Name) => cell_text(ui.painter(), r, &txt, 9.5, color, false),
                                // Icons hidden: the title alone, still centred
                                // over its centred values.
                                None => {
                                    let p = ui.painter();
                                    let f = egui::FontId::proportional(9.5);
                                    let shown = truncate_to_width(p, &txt, f.clone(), color, (r.width() - 12.0).max(12.0));
                                    crate::shaped::draw(p, r.center(), egui::Align2::CENTER_CENTER, &shown, f, color);
                                }
                            }
                            self.hits.push((HitKind::HdrCol(*col), r.intersect(hdr_clip)));
                            {
                                // Divider grab strip on the column's right edge
                                // (`.resizer`): a faint line always, so it can
                                // be found, and an accent bar while hovered or
                                // dragged.
                                let zr = egui::Rect::from_min_size(
                                    egui::pos2(r.max.x - 4.0, r.min.y),
                                    egui::vec2(8.0, HDR_H),
                                );
                                let dragging = matches!(self.resize_cap,
                                    Some(ResizeCap::Col { col: c, .. }) if c == *col);
                                if self.man.hover(&zr) || dragging {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(r.max.x - 1.5, r.min.y + 5.0),
                                            egui::vec2(3.0, HDR_H - 10.0),
                                        ),
                                        1.5,
                                        pl().primary,
                                    );
                                } else {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_size(
                                            egui::pos2(r.max.x - 0.5, r.min.y + 8.0),
                                            egui::vec2(1.0, HDR_H - 16.0),
                                        ),
                                        0.0,
                                        pl().card_bd,
                                    );
                                }
                                self.hits.push((HitKind::ResizeCol(*col), zr.intersect(hdr_clip)));
                            }
                        }
                    });
                    let (hr, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 1.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(hr, 0.0, pl().card_bd);
                    // ---- body rows: manual geometry inside scroll content.
                    // Row screen rects come straight from layout, so manual
                    // hit-testing stays exact under scrolling ----
                    // Both bars always shown, below and beside the rows.
                    modern_scrollbars(ui);
                    let body_top = ui.cursor().min.y;
                    let body_h = (ui.available_height() - 4.0).max(80.0);
                    ui.scope(|ui| {
                            egui::ScrollArea::both()
                                .id_salt("dl_v")
                                .max_height(body_h)
                                .auto_shrink([false, false])
                                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                                .show(ui, |ui| {
                                    // Rows are flush: the list's rhythm comes
                                    // from ROW_H plus the faint row rule, not
                                    // from the style's 10px item spacing.
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    // A row that is scrolled out of sight still
                                    // has a rect (egui only clips the paint),
                                    // so its hit would reach over the toolbar
                                    // and swallow the buttons there. Every hit
                                    // is therefore clipped to the viewport.
                                    // Rows never paint above the line under the
                                    // column titles.
                                    let c = ui.clip_rect();
                                    ui.set_clip_rect(egui::Rect::from_min_max(
                                        egui::pos2(c.min.x, c.min.y.max(body_top)),
                                        c.max,
                                    ));
                                    let view = ui.clip_rect();
                                    // How far the rows are scrolled sideways:
                                    // the header follows it.
                                    let hoff = (view.min.x - ui.max_rect().min.x).max(0.0);
                                    if (hoff - self.list_hoff).abs() > 0.5 {
                                        self.list_hoff = hoff;
                                        ui.ctx().request_repaint();
                                    }
                                    for id in &ids {
                                        let id = *id;
                                        let (sel, fname, cat, size_s, prog, spd, eta, date, st, preparing) = {
                                            let d = self
                                                .downloads
                                                .iter()
                                                .find(|d| d.id == id)
                                                .unwrap();
                                            // Nothing has arrived yet: the row is in a
                                            // phase that moves no bytes (YouTube's player
                                            // challenge, a merge), where a motionless 0%
                                            // reads as a hang. Say what is happening
                                            // instead. A known size does not rule this
                                            // out — the browser probe often measured it
                                            // before the transfer even started.
                                            let preparing = match self.phases.get(&id) {
                                                // A live capture has no percentage to
                                                // show, so the pill names the state.
                                                Some(p) if p == "Recording live" => {
                                                    "LIVE".to_string()
                                                }
                                                Some(p)
                                                    if d.downloaded_bytes == 0
                                                        && !crate::engine::phase_is_downloading(p) =>
                                                {
                                                    p.clone()
                                                }
                                                _ => String::new(),
                                            };
                                            (
                                                d.selected,
                                                d.filename.clone(),
                                                d.category().to_string(),
                                                d.total_bytes
                                                    .map(format_bytes)
                                                    .unwrap_or_else(|| "--".into()),
                                                d.progress(),
                                                d.speed_bps,
                                                format_eta(d),
                                                relative_time(&d.created_at),
                                                d.status.clone(),
                                                preparing,
                                            )
                                        };
                                        // Row spacer = hit + paint rect.
                                        let (rrect, _) = ui.allocate_exact_size(
                                            egui::vec2(total_w, ROW_H),
                                            egui::Sense::hover(),
                                        );
                                        let hl = rrect.shrink2(egui::vec2(2.0, 1.0));
                                        if sel {
                                            // The tint alone marks the selection (no outline).
                                            ui.painter().rect_filled(hl, 6.0, pl().sel_bg);
                                        } else if self.man.hover(&rrect) {
                                            ui.painter().rect_filled(hl, 6.0, pl().hover_bg);
                                        }
                                        // `.downloads td { border-bottom: 1px
                                        // solid rgba(30,41,59,.5) }` — with the
                                        // rows flush, this faint rule is what
                                        // keeps them readable.
                                        ui.painter().hline(
                                            rrect.x_range(),
                                            rrect.max.y,
                                            egui::Stroke::new(
                                                1.0_f32,
                                                pl().card_bd.gamma_multiply(0.55),
                                            ),
                                        );
                                        self.hits.push((HitKind::Row(id), rrect.intersect(view)));
                                        // Manual select checkbox.
                                        let cb = egui::Rect::from_center_size(
                                            egui::pos2(
                                                rrect.min.x + CHECK_W / 2.0,
                                                rrect.center().y,
                                            ),
                                            egui::vec2(13.0, 13.0),
                                        );
                                        paint_check(
                                            ui.painter(),
                                            cb,
                                            sel,
                                            self.man.hover(&cb.expand(4.0)),
                                        );
                                        self.hits.push((
                                            HitKind::RowCheck(id),
                                            cb.expand(6.0).intersect(view),
                                        ));
                                // Cells painted at explicit sub-rects: pure
                                // paint, no widgets, no cursor use — the row
                                // spacer above already advanced the cursor.
                                let p = ui.painter();
                                let mut x = rrect.min.x + CHECK_W;
                                for (ci, col) in cols.iter().enumerate() {
                                    let w = col_ws[ci];
                                    let crect = egui::Rect::from_min_size(
                                        egui::pos2(x, rrect.min.y),
                                        egui::vec2(w, ROW_H),
                                    );
                                    match col {
                                        SortCol::Name => {
                                            name_cell(
                                                &p,
                                                crect,
                                                file_icon(&cat),
                                                &fname,
                                                &cat,
                                            );
                                        }
                                        SortCol::Size => {
                                            cell_text_center(&p, crect, &size_s, 11.0, pl().dim);
                                        }
                                        SortCol::Status => match &st {
                                            Status::Downloading => {
                                                // While a phase that moves no bytes is
                                                // running, the text names it instead of
                                                // sitting on a motionless 0%.
                                                let text = if preparing.is_empty() {
                                                    format!("{}% {}", (prog * 100.0) as u32, crate::i18n::t("Downloading"))
                                                } else {
                                                    preparing.clone()
                                                };
                                                status_bar_at(&p, crect, &text, prog, pl().primary);
                                            }
                                            Status::Queued => {
                                                cell_text_center(&p, crect, "Added", 10.5, pl().dim);
                                            }
                                            Status::Paused => {
                                                status_bar_at(
                                                    &p,
                                                    crect,
                                                    &format!("{}% {}", (prog * 100.0) as u32, crate::i18n::t("Paused")),
                                                    prog,
                                                    egui::Color32::from_rgb(0xf5, 0x9e, 0x0b),
                                                );
                                            }
                                            Status::Completed => {
                                                status_dot_at(&p, crect, crate::i18n::t("Finished"), egui::Color32::from_rgb(0x34, 0xd3, 0x99));
                                            }
                                            Status::Failed(_) => {
                                                status_dot_at(&p, crect, crate::i18n::t("Failed"), egui::Color32::from_rgb(0xf8, 0x71, 0x71));
                                            }
                                        },
                                        SortCol::Speed => {
                                            let spd_s = if spd > 0.0 {
                                                format_speed(spd)
                                            } else {
                                                "-".to_string()
                                            };
                                            cell_text_center(&p, crect, &spd_s, 10.0, pl().dim);
                                        }
                                        SortCol::Eta => {
                                            let eta_s: &str =
                                                if eta == "-" { "-" } else { &eta };
                                            cell_text_center(&p, crect, eta_s, 10.0, pl().dim);
                                        }
                                        SortCol::Date => {
                                            cell_text_center(&p, crect, &date, 10.0, pl().dim);
                                        }
                                    }
                                    x += w;
                                }
                            }
                        });
                    });
                }
            });

        // ================= browser install result =================
        // (row/menu actions dispatch from process_hits at the end of update)
        if let Some(which) = self.pending_browser_install.take() {
            match install_native_host(&which) {
                Ok(manifest) => {
                    self.push_log(format!("Native host installed for {which}."));
                    self.browser_msg = format!("Installed {which} host ✓ ({manifest})");
                }
                Err(e) => self.browser_msg = format!("Install failed: {e}"),
            }
        }

        // ================= modal backdrop (dim only — never intercepts clicks) =================
        if self.show_add
            || self.show_queues
            || self.show_grabber
            || self.show_about
            || self.details_id.is_some()
        {
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("modal_dim"),
            ))
            .rect_filled(
                ctx.screen_rect(),
                0.0,
                pl().scrim,
            );
        }

        self.list_shortcuts(ctx);
        self.take_dropped_files(ctx);
        if self.stats_at.elapsed() >= std::time::Duration::from_secs(30) {
            self.stats_at = Instant::now();
            let s = crate::model::stats();
            self.stats_line = if s.day_bytes > 0 { format!("today {}", format_bytes(s.day_bytes)) } else { String::new() };
        }
        self.enforce_quota();
        self.tick_queues();
        while let Ok(line) = self.conv_rx.try_recv() {
            self.push_log(line);
        }
        self.poll_zip();
        let listed = self.playlist_rx.as_ref().and_then(|(u, rx)| rx.try_recv().ok().map(|r| (u.clone(), r)));
        if let Some((url, res)) = listed {
            self.playlist_rx = None;
            match res {
                Ok((title, items)) if !items.is_empty() => {
                    let n = items.len();
                    let links: Vec<crate::native_host::PageLink> =
                        items.into_iter().map(|(url, text)| crate::native_host::PageLink { url, text }).collect();
                    match crate::native_host::open_links_window(&url, &title, "", &links) {
                        Ok(()) => self.push_log(format!("{title}: {n} videos - pick them in the window that opened.")),
                        Err(e) => self.push_log(format!("Playlist: {e}")),
                    }
                }
                Ok(_) => self.push_log("The playlist is empty (or private)."),
                Err(e) => self.push_log(format!("Playlist: {e}")),
            }
        }
        // ================= dialogs =================
        if !self.show_add && self.add_pre.is_some() {
            // Cancelled (or closed): its background download goes with it.
            self.stop_add_prestart(None);
            self.add_file_auto.clear();
        }
        if self.show_add {
            if self.add_focus_req {
                // Focus lands without any click (hit-test independent).
                self.add_focus_req = false;
                focus_field(ctx, "add_url");
            }
            // Esc backs out of the menu first, then the dialog (a modal this
            // tall can otherwise be hard to dismiss).
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                if self.add_recent_open {
                    self.add_recent_open = false;
                } else {
                    self.show_add = false;
                    self.add_focus_req = false;
                }
            }
            // auto-probe once per URL
            if !self.add_url.trim().is_empty()
                && self.add_url.trim() != self.probe_done_for
                && !self.probing
            {
                self.probe_done_for = self.add_url.trim().to_string();
                // A new link: the old one's background download goes, and a
                // name the dialog chose itself follows the link.
                self.stop_add_prestart(None);
                if self.add_file.trim().is_empty() || self.add_file == self.add_file_auto {
                    self.add_file = filename_from_url(self.add_url.trim());
                    self.add_file_auto = self.add_file.clone();
                }
                self.start_probe(self.add_url.trim().to_string());
            }
            // collect probe result
            let mut got_probe = None;
            if let Some(rx) = self.probe_rx.as_ref() {
                while let Ok(r) = rx.try_recv() {
                    got_probe = Some(r);
                }
            }
            if let Some((sz, nm)) = got_probe {
                self.probing = false;
                self.probe_size = sz;
                if let Some(n) = nm {
                    self.probe_name = Some(n.clone());
                    if self.add_file.trim().is_empty() || self.add_file == self.add_file_auto {
                        self.add_file = crate::model::apply_rename_template(
                            &self.rename_template,
                            &n,
                            &self.add_url,
                        );
                        self.add_file_auto = self.add_file.clone();
                    }
                }
                self.maybe_add_prestart();
            }
            let psize = self.probe_size;
            let pbusy = self.probing;
            let mut open = self.show_add;
            // In-canvas centered modal (a separate OS window proved unstable
            // here, so the dialog lives in the main canvas like before).
            // The concept is a fixed-size dialog; only the advanced panel makes
            // it taller (the concept draws that collapsed).
            let adv_h = if self.add_adv { DM_PANEL_H + 14.0 } else { 0.0 };
            let card_h = (DM_H + adv_h).min((ctx.screen_rect().height() * 0.94).max(300.0));
            egui::Window::new("Add download")
                .id(egui::Id::new("add_download_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .title_bar(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .fixed_size(egui::vec2(DM_W, card_h))
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    // Everything is painted from the card origin: the concept is
                    // a pixel layout, not a flow layout.
                    let card = ui.max_rect();
                    let o = card.min;
                    let p = ui.painter().clone();
                    let at = |x: f32, y: f32, w: f32, h: f32| {
                        egui::Rect::from_min_size(egui::pos2(o.x + x, o.y + y), egui::vec2(w, h))
                    };
                    // ---- card ----
                    p.rect(
                        card,
                        8.0,
                        pl().dm_card,
                        egui::Stroke::new(1.0, pl().dm_line),
                        egui::StrokeKind::Inside,
                    );
                    // ---- header: mark + title + window controls ----
                    dm_mark(&p, at(15.0, 11.0, 18.0, 18.0));
                    p.text(
                        egui::pos2(o.x + 41.0, o.y + 20.0),
                        egui::Align2::LEFT_CENTER,
                        "Add download",
                        crate::fonts::semi(13.0),
                        pl().dm_txt,
                    );
                    p.line_segment(
                        [
                            egui::pos2(o.x + 1.0, o.y + 39.5),
                            egui::pos2(o.x + DM_W - 1.0, o.y + 39.5),
                        ],
                        egui::Stroke::new(1.0, pl().dm_line),
                    );
                    // The dialog lives in the main canvas, so minimise/restore
                    // drive the real OS window and the X closes the dialog.
                    for (cx, icon, ctl) in [
                        (485.0f32, DmIcon::Minimize, 0u8),
                        (513.0, DmIcon::Maximize, 1),
                        (541.0, DmIcon::Close, 2),
                    ] {
                        let r = at(cx - 8.0, 12.0, 16.0, 16.0);
                        dm_icon_btn(&p, &self.man, r, icon, 12.0, 2.0, pl().dm_dim);
                        if self.man.clicked(&r) {
                            match ctl {
                                0 => ui
                                    .ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                                1 => {
                                    let max = ui
                                        .ctx()
                                        .input(|i| i.viewport().maximized.unwrap_or(false));
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                                }
                                _ => {
                                    self.show_add = false;
                                    self.add_focus_req = false;
                                    self.add_recent_open = false;
                                }
                            }
                        }
                    }
                    // ---- row 1: URL + clipboard ----
                    let url_box = at(DM_X, DM_ROW1, DM_W_IN, DM_BOX_H);
                    let url_btn = egui::Rect::from_min_size(
                        egui::pos2(url_box.max.x - 33.0, url_box.min.y + 5.0),
                        egui::Vec2::splat(28.0),
                    );
                    dm_field(
                        ui,
                        "add_url",
                        url_box,
                        url_btn.min.x - 6.0,
                        &mut self.add_url,
                        "https://…",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_url"), url_box));
                    dm_icon_btn(&p, &self.man, url_btn, DmIcon::Clipboard, 14.0, 1.5, pl().dm_dim);
                    self.hits.push((HitKind::Btn(ButtonId::AddPaste), url_btn));
                    // ---- row 2: save folder + carets | size chip ----
                    let path_box = at(DM_X, DM_ROW2, DM_PATH_W, DM_BOX_H);
                    let caret_btn = egui::Rect::from_min_size(
                        egui::pos2(path_box.max.x - 33.0, path_box.min.y + 5.0),
                        egui::Vec2::splat(28.0),
                    );
                    let folder_btn = egui::Rect::from_min_size(
                        egui::pos2(caret_btn.min.x - 28.0, caret_btn.min.y),
                        egui::Vec2::splat(28.0),
                    );
                    dm_field(
                        ui,
                        "add_dir",
                        path_box,
                        folder_btn.min.x - 6.0,
                        &mut self.add_dir,
                        "Save folder",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_dir"), path_box));
                    dm_icon_btn(&p, &self.man, folder_btn, DmIcon::Folder, 14.0, 1.5, pl().dm_dim);
                    self.hits.push((HitKind::Btn(ButtonId::AddFolder), folder_btn));
                    dm_icon_btn(&p, &self.man, caret_btn, DmIcon::Chevron, 10.0, 2.0, pl().dm_dim);
                    self.add_caret = Some(caret_btn);
                    if self.man.clicked(&caret_btn) {
                        self.add_recent_open = !self.add_recent_open;
                    }
                    // size chip: grid mark, size, and a tick once the probe is in
                    let chip = at(DM_X + DM_PATH_W + 8.0, DM_ROW2, DM_SIZE_W, DM_BOX_H);
                    dm_box(&p, chip);
                    dm_icon(
                        &p,
                        egui::Rect::from_center_size(
                            egui::pos2(chip.min.x + 21.0, chip.center().y),
                            egui::Vec2::splat(16.0),
                        ),
                        DmIcon::Grid,
                        DM_ACC,
                        0.0,
                    );
                    let known = !pbusy && psize.filter(|s| *s > 0).is_some();
                    let sz_txt: String = if pbusy {
                        "…".to_string()
                    } else {
                        psize
                            .filter(|s| *s > 0)
                            .map(format_bytes)
                            .unwrap_or_else(|| "—".to_string())
                    };
                    let g = p.layout_no_wrap(sz_txt, crate::fonts::reg(11.0), pl().dm_txt);
                    let tw = g.size().x;
                    let tx = chip.min.x + 37.0;
                    p.galley(
                        egui::pos2(tx, chip.center().y - g.size().y * 0.5),
                        g,
                        pl().dm_txt,
                    );
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
                        "add_file",
                        file_box,
                        file_box.max.x - 13.0,
                        &mut self.add_file,
                        "Filename",
                        false,
                    );
                    self.hits.push((HitKind::Field("add_file"), file_box));
                    dm_icon_btn_box(&p, &self.man, refr_btn, DmIcon::Refresh, 14.0, 1.5, pl().dm_dim);
                    self.hits.push((HitKind::Btn(ButtonId::AddRefresh), refr_btn));
                    dm_icon_btn_box(&p, &self.man, gear_btn, DmIcon::Settings, 15.0, 1.5, pl().dm_dim);
                    self.hits.push((HitKind::Btn(ButtonId::AddAdv), gear_btn));
                    // ---- advanced panel (gear): same tokens, placeholders
                    // instead of labels, per the concept's field style ----
                    if self.add_adv {
                        let panel = at(DM_X, DM_ACT, DM_W_IN, DM_PANEL_H);
                        dm_box(&p, panel);
                        let cx = panel.min.x + 13.0;
                        let cw = panel.width() - 26.0;
                        let mut cy = panel.min.y + 13.0;
                        let row = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 30.0));
                        p.text(
                            egui::pos2(row.min.x, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Connections",
                            crate::fonts::reg(11.0),
                            pl().dm_dim,
                        );
                        let srect = egui::Rect::from_min_size(
                            egui::pos2(row.min.x + 92.0, row.min.y + 5.0),
                            egui::vec2(row.width() - 144.0, 20.0),
                        );
                        let frac =
                            (((self.add_conns as f64) - 1.0) / 15.0).clamp(0.0, 1.0) as f32;
                        dm_slider(&p, &self.man, srect, frac);
                        self.hits.push((
                            HitKind::Slider(SliderId::AddConns, 1.0, 32.0, false),
                            srect,
                        ));
                        p.text(
                            egui::pos2(row.max.x, row.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("{}", self.add_conns),
                            crate::fonts::reg(11.0),
                            pl().dm_txt,
                        );
                        cy += 38.0;
                        let half = (cw - 8.0) / 2.0;
                        let r_ref = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(half, DM_BOX_H));
                        let r_cok = egui::Rect::from_min_size(
                            egui::pos2(cx + half + 8.0, cy),
                            egui::vec2(half, DM_BOX_H),
                        );
                        dm_field(ui, "add_ref", r_ref, r_ref.max.x - 13.0, &mut self.add_referer, "Referer (optional)", false);
                        self.hits.push((HitKind::Field("add_ref"), r_ref));
                        dm_field(ui, "add_cookie", r_cok, r_cok.max.x - 13.0, &mut self.add_cookie, "Cookie (optional)", false);
                        self.hits.push((HitKind::Field("add_cookie"), r_cok));
                        cy += DM_BOX_H + 8.0;
                        let r_usr = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(half, DM_BOX_H));
                        let r_pwd = egui::Rect::from_min_size(
                            egui::pos2(cx + half + 8.0, cy),
                            egui::vec2(half, DM_BOX_H),
                        );
                        dm_field(ui, "add_user", r_usr, r_usr.max.x - 13.0, &mut self.add_user, "User (optional)", false);
                        self.hits.push((HitKind::Field("add_user"), r_usr));
                        dm_field(ui, "add_pass", r_pwd, r_pwd.max.x - 13.0, &mut self.add_pass, "Password", true);
                        self.hits.push((HitKind::Field("add_pass"), r_pwd));
                        cy += DM_BOX_H + 8.0;
                        let r_sum = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, DM_BOX_H));
                        dm_field(ui, "add_sum", r_sum, r_sum.max.x - 13.0, &mut self.add_checksum, "SHA-256 checksum (optional, 64 hex)", false);
                        self.hits.push((HitKind::Field("add_sum"), r_sum));
                        cy += DM_BOX_H + 8.0;
                        let ow = egui::Rect::from_min_size(egui::pos2(cx, cy), egui::vec2(cw, 22.0));
                        let cbx = egui::Rect::from_min_size(
                            egui::pos2(ow.min.x, ow.center().y - 8.0),
                            egui::Vec2::splat(16.0),
                        );
                        if self.add_overwrite {
                            p.rect_filled(cbx, 4.0, DM_ACC);
                            dm_icon(
                                &p,
                                egui::Rect::from_center_size(cbx.center(), egui::Vec2::splat(12.0)),
                                DmIcon::Check,
                                egui::Color32::WHITE,
                                2.5,
                            );
                        } else {
                            p.rect(
                                cbx,
                                4.0,
                                pl().dm_card,
                                egui::Stroke::new(1.0, pl().dm_box_bd),
                                egui::StrokeKind::Inside,
                            );
                        }
                        p.text(
                            egui::pos2(cbx.max.x + 8.0, ow.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Overwrite file if it already exists",
                            crate::fonts::reg(11.0),
                            pl().dm_dim,
                        );
                        self.hits.push((HitKind::Check(CheckId::AddOverwrite), ow));
                    }
                    // ---- actions (manual hit table, as before) ----
                    let act_y = DM_ACT + adv_h;
                    let add_btn = at(DM_X, act_y, 100.0, 36.0);
                    dm_btn_flat(&p, &self.man, add_btn, "Add");
                    self.hits.push((HitKind::Btn(ButtonId::AddAdd), add_btn));
                    let dl_btn = egui::Rect::from_min_size(
                        egui::pos2(add_btn.max.x + 8.0, add_btn.min.y),
                        egui::vec2(140.0, 36.0),
                    );
                    dm_gradient(&p, dl_btn, DM_RAD, DM_ACC, DM_ACC2);
                    if self.man.hover(&dl_btn) {
                        p.rect_filled(dl_btn, DM_RAD, egui::Color32::from_white_alpha(22));
                    }
                    p.text(
                        dl_btn.center(),
                        egui::Align2::CENTER_CENTER,
                        "Download",
                        crate::fonts::semi(13.0),
                        egui::Color32::WHITE,
                    );
                    self.hits.push((HitKind::Btn(ButtonId::AddDownload), dl_btn));
                    let cancel_btn = egui::Rect::from_min_size(
                        egui::pos2(o.x + DM_W - 17.0 - 100.0, add_btn.min.y),
                        egui::vec2(100.0, 36.0),
                    );
                    dm_btn_flat(&p, &self.man, cancel_btn, "Cancel");
                    self.hits.push((HitKind::Btn(ButtonId::AddCancel), cancel_btn));
                });
            // The egui X button can't be relied on: keep the flag unless
            // our own Cancel/Add/Download closed it.
            self.show_add = open && self.show_add;
            if self.add_dir.trim().is_empty() {
                self.add_dir = dirs::download_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
            }
            // ---- recent-folders menu (the path field's caret) ----
            // A click outside the caret and outside the menu closes it. The
            // caret rect is this frame's; the menu rect is derived, never
            // stored, so the two can't drift apart.
            if self.add_recent_open && ctx.input(|i| i.pointer.any_click()) {
                let pos = ctx.input(|i| i.pointer.interact_pos());
                let rows = self.add_recent_dirs().len();
                let menu = self.add_caret.map(|c| dm_menu_rect(c, rows));
                let inside = |r: Option<egui::Rect>| match (r, pos) {
                    (Some(r), Some(q)) => r.contains(q),
                    _ => false,
                };
                if !inside(self.add_caret) && !inside(menu) {
                    self.add_recent_open = false;
                }
            }
            if self.add_recent_open {
                match self.add_caret {
                    None => self.add_recent_open = false,
                    Some(caret) => {
                        let anchors = self.add_recent_dirs();
                        let mut pick: Option<String> = None;
                        egui::Area::new(egui::Id::new("add_recent_menu"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(dm_menu_rect(caret, anchors.len()).min)
                            .show(ctx, |ui| {
                                egui::Frame::new()
                                    .fill(pl().dm_card)
                                    .stroke(egui::Stroke::new(1.0, pl().dm_line))
                                    .corner_radius(DM_RAD)
                                    .inner_margin(egui::Margin::same(6))
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        for d in &anchors {
                                            let resp = ui
                                                .add_sized(
                                                    egui::vec2(308.0, 24.0),
                                                    egui::Button::new(
                                                        egui::RichText::new(elide_left(d, 46))
                                                            .size(11.0)
                                                            .color(pl().dm_txt),
                                                    )
                                                    .fill(egui::Color32::TRANSPARENT)
                                                    .stroke(egui::Stroke::NONE),
                                                )
                                                .on_hover_text(d);
                                            if resp.clicked() {
                                                pick = Some(d.clone());
                                            }
                                        }
                                    });
                            });
                        if let Some(d) = pick {
                            self.add_dir = d;
                            self.add_recent_open = false;
                        }
                    }
                }
            }
        }
        if !self.show_add {
            self.add_recent_open = false;
            self.add_caret = None;
        }

        // ================= site grabber (P3.1) =================
        if self.show_grabber {
            // Collect finished crawl without holding borrows across UI code.
            let mut got_grab: Option<Result<Vec<crate::grabber::GrabLink>, String>> = None;
            if let Some(rx) = self.grab_rx.as_ref() {
                while let Ok(r) = rx.try_recv() {
                    got_grab = Some(r);
                }
            }
            if let Some(r) = got_grab {
                self.grab_busy = false;
                match r {
                    Ok(links) => {
                        self.grab_results = links;
                        self.grab_notice.clear();
                    }
                    Err(e) => self.grab_notice = e,
                }
            }
            if self.grab_busy {
                ctx.request_repaint();
            }
            let mut open = self.show_grabber;
            let grab_w = modal_width(ctx, 560.0);
            egui::Window::new("Site Grabber")
                .id(egui::Id::new("grab_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .default_width(grab_w)
                .min_width(grab_w.min(320.0))
                .max_width(grab_w)
                .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                .frame(modal_frame())
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let full = ui.available_width();
                    ui.label(egui::RichText::new("Find every file a web page links to, then queue the ones you want.").size(11.5).color(pl().dim));
                    ui.add_space(6.0);
                    // Page
                    msection(ui, "PAGE");
                    let r = mfield(ui, "grab_url", &mut self.grab_url, "https://example.com/gallery", full);
                    self.hits.push((HitKind::Field("grab_url"), r));
                    ui.add_space(6.0);
                    // File types: one chip per preset, the list it fills in
                    // stays editable below.
                    msection(ui, "FILE TYPES");
                    const PRESETS: &[&str] = &["Mixed", "Video", "Audio", "Images", "Documents", "Archives"];
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for (i, name) in PRESETS.iter().enumerate() {
                            let r = mchip(ui, &self.man, name, self.grab_preset == i);
                            self.hits.push((HitKind::Btn(ButtonId::GrabPresetPick(i as u8)), r));
                        }
                    });
                    let r = mfield(ui, "grab_exts", &mut self.grab_exts, "mp4, zip, pdf…", full);
                    self.hits.push((HitKind::Field("grab_exts"), r));
                    ui.add_space(6.0);
                    // How far to look
                    msection(ui, "LINKED PAGES");
                    let r = mcheck(ui, &self.man, "Also look in pages it links to (same site)", self.grab_depth);
                    self.hits.push((HitKind::Check(CheckId::GrabDepth), r));
                    if self.grab_depth {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add_space(26.0);
                            ui.label(egui::RichText::new("Levels deep").size(12.0).color(pl().txt));
                            let r = mbutton(ui, &self.man, "−", 28.0, 26.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::GrabLevelsDown), r));
                            ui.label(egui::RichText::new(self.grab_levels.to_string()).size(12.5).strong().color(pl().txt));
                            let r = mbutton(ui, &self.man, "+", 28.0, 26.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::GrabLevelsUp), r));
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(26.0);
                            let r = mcheck(ui, &self.man, "Stay inside the start page's folder", self.grab_stay);
                            self.hits.push((HitKind::Check(CheckId::GrabStay), r));
                        });
                    }
                    ui.add_space(6.0);
                    // Saved projects
                    msection(ui, "PROJECTS");
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let mut load: Option<crate::model::GrabProject> = None;
                        let mut delete: Option<usize> = None;
                        egui::ComboBox::from_id_salt("grab_projects")
                            .width(150.0)
                            .selected_text(if self.grab_projects.is_empty() { "No saved projects" } else { "Open a project…" })
                            .show_ui(ui, |ui| {
                                for (i, p) in self.grab_projects.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        if ui.selectable_label(false, &p.name).on_hover_text(&p.url).clicked() {
                                            load = Some(p.clone());
                                        }
                                        if ui.small_button("✕").on_hover_text("Delete this project").clicked() {
                                            delete = Some(i);
                                        }
                                    });
                                }
                            });
                        let name_w = (ui.available_width() - 120.0).max(100.0);
                        let r = mfield(ui, "grab_project_name", &mut self.grab_project_name, "Project name", name_w);
                        self.hits.push((HitKind::Field("grab_project_name"), r));
                        let r = mbutton(ui, &self.man, "Save project", 110.0, 32.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::GrabSave), r));
                        if let Some(p) = load {
                            self.grab_url = p.url;
                            self.grab_exts = p.exts;
                            self.grab_depth = p.follow;
                            self.grab_levels = p.levels.clamp(1, 5);
                            self.grab_stay = p.stay;
                            self.grab_project_name = p.name.clone();
                            self.grab_notice = format!("Loaded \"{}\" - press Find links to run it again.", p.name);
                        }
                        if let Some(i) = delete {
                            self.grab_projects.remove(i);
                            crate::model::save_grab_projects(&self.grab_projects);
                        }
                    });
                    if !self.grab_notice.is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&self.grab_notice).size(11.5).color(pl().notice));
                    }
                    // Results: a card with the count and the names.
                    if self.grab_busy || !self.grab_results.is_empty() {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(pl().surface)
                            .stroke(egui::Stroke::new(1.0_f32, pl().card_bd))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if self.grab_busy {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(egui::RichText::new("Looking for links…").size(12.0).color(pl().txt));
                                    });
                                } else {
                                    let n = self.grab_results.len();
                                    ui.label(egui::RichText::new(format!("{n} file link{} found", if n == 1 { "" } else { "s" })).size(12.0).strong().color(pl().txt));
                                    ui.add_space(2.0);
                                    egui::ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 3.0;
                                        for l in self.grab_results.iter().take(60) {
                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(MI_LINK).font(crate::fonts::icon(13.0)).color(pl().dim));
                                                ui.label(egui::RichText::new(&l.name).size(11.5).color(pl().txt)).on_hover_text(&l.url);
                                            });
                                        }
                                        if n > 60 {
                                            ui.label(egui::RichText::new(format!("…and {} more", n - 60)).size(11.0).color(pl().dim));
                                        }
                                    });
                                }
                            });
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let r = mbutton(ui, &self.man, "Find links", 110.0, 34.0, self.grab_results.is_empty());
                        self.hits.push((HitKind::Btn(ButtonId::GrabStart), r));
                        if !self.grab_results.is_empty() {
                            let r = mbutton(ui, &self.man, &format!("Queue all ({})", self.grab_results.len()), 140.0, 34.0, true);
                            self.hits.push((HitKind::Btn(ButtonId::GrabQueue), r));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let r = mbutton(ui, &self.man, "Close", 90.0, 34.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::GrabClose), r));
                        });
                    });
                });
            self.show_grabber = open && self.show_grabber;
        }

        if self.zip_pick.is_some() {
            let mut open = true;
            let mut fetch = false;
            let p = self.zip_pick.as_mut().unwrap();
            egui::Window::new(format!("ZIP contents - {}", p.name))
                .open(&mut open)
                .collapsible(false)
                .default_width(560.0)
                .show(ctx, |ui| {
                    ui.label(&p.status);
                    if !p.entries.is_empty() {
                        ui.horizontal(|ui| {
                            if ui.button("All").clicked() {
                                p.entries.iter_mut().for_each(|(e, on)| *on = e.can_extract());
                            }
                            if ui.button("None").clicked() {
                                p.entries.iter_mut().for_each(|(_, on)| *on = false);
                            }
                            let (n, bytes) = p.entries.iter().filter(|(_, on)| *on).fold((0, 0u64), |a, (e, _)| (a.0 + 1, a.1 + e.compressed_size));
                            ui.label(format!("{n} picked, {} to fetch", format_bytes(bytes)));
                        });
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            for (e, on) in p.entries.iter_mut() {
                                ui.add_enabled_ui(e.can_extract(), |ui| {
                                    ui.checkbox(on, format!("{}   ({})", e.name, format_bytes(e.size)));
                                });
                            }
                        });
                        ui.label(format!("Saved to: {}", p.out_dir.display()));
                        ui.add_enabled_ui(!p.busy, |ui| {
                            if ui.button("Download picked files").clicked() {
                                fetch = true;
                            }
                        });
                    }
                });
            if fetch {
                self.zip_fetch_selected();
            }
            if !open {
                self.zip_pick = None;
            }
        }

        if self.show_queues {
            let mut open = true;
            let n_items = self.downloads.len();
            let n_active = self.tasks.len();
            let sched_fired = self.scheduler_started;
            let log: Vec<String> = self.log.iter().rev().take(60).cloned().collect();
            let q_w = modal_width(ctx, 540.0);
            egui::Window::new("Queues")
                .id(egui::Id::new("queues_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .default_width(q_w)
                .min_width(q_w.min(320.0))
                .max_width(q_w)
                .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                .frame(modal_frame())
                .show(ctx, |ui| {
                // The Next-up list grows the body past a short window;
                // scroll the body and keep the Close button on screen.
                let body_max =
                    (ctx.screen_rect().height() * 0.86 - 120.0).clamp(180.0, 760.0);
                egui::ScrollArea::vertical()
                    .max_height(body_max)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                    // ---- named queues (IDM: several queues) ----
                    ui.horizontal_wrapped(|ui| {
                        if ui.selectable_label(self.queue_sel == 0, "Main").clicked() {
                            self.queue_sel = 0;
                        }
                        for i in 0..self.queues.len() {
                            let label = format!("{}{}", self.queues[i].name, if self.queues[i].running { " ▶" } else { "" });
                            if ui.selectable_label(self.queue_sel == i + 1, label).clicked() {
                                self.queue_sel = i + 1;
                            }
                        }
                        ui.add(egui::TextEdit::singleline(&mut self.queue_new_name).hint_text("new queue name").desired_width(120.0));
                        let name = self.queue_new_name.trim().to_string();
                        if ui.button("Add queue").clicked() && !name.is_empty() && name != "Main" && !self.queues.iter().any(|q| q.name == name) {
                            self.queues.push(crate::model::QueueDef { name, max: 2, ..Default::default() });
                            self.queue_sel = self.queues.len();
                            self.queue_new_name.clear();
                            self.save_ui_state();
                        }
                    });
                    ui.separator();
                    if self.queue_sel > 0 && self.queue_sel <= self.queues.len() {
                        let i = self.queue_sel - 1;
                        let mut changed = false;
                        let mut delete = false;
                        let mut toggle: Option<bool> = None;
                        {
                            let q = &mut self.queues[i];
                            let members = self.downloads.iter().filter(|d| d.queue == q.name).count();
                            ui.label(format!("Queue {}: {members} item(s){}", q.name, if q.running { ", running" } else { "" }));
                            ui.horizontal(|ui| {
                                if ui.button(if q.running { "Stop this queue" } else { "Start this queue" }).clicked() {
                                    toggle = Some(!q.running);
                                }
                                if ui.button("Delete queue").clicked() {
                                    delete = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Downloads at the same time:");
                                changed |= ui.add(egui::DragValue::new(&mut q.max).range(1..=16)).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Start at (HH:MM):");
                                changed |= ui.add(egui::TextEdit::singleline(&mut q.start_at).desired_width(60.0)).changed();
                                ui.label("Stop at:");
                                changed |= ui.add(egui::TextEdit::singleline(&mut q.stop_at).desired_width(60.0)).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Check finished files for newer versions every");
                                changed |= ui.add(egui::DragValue::new(&mut q.sync_hours).range(0..=720)).changed();
                                ui.label("hours (0 = never)");
                            });
                            ui.small("Move rows here with right-click → Move to queue.");
                        }
                        if let Some(run) = toggle {
                            if run {
                                self.queues[i].running = true;
                                self.fill_next();
                            } else {
                                self.stop_named_queue(i);
                            }
                        }
                        if delete {
                            let name = self.queues[i].name.clone();
                            self.stop_named_queue(i);
                            for d in self.downloads.iter_mut().filter(|d| d.queue == name) {
                                d.queue.clear();
                            }
                            self.queues.remove(i);
                            self.queue_sel = 0;
                            changed = true;
                            self.save_queue();
                        }
                        if changed {
                            self.save_ui_state();
                        }
                    } else {
                    ui.label(format!("{n_items} item(s) in queue, {n_active} active."));
                    ui.horizontal(|ui| {
                        let r = mbutton(ui, &self.man, "Start Queue", 130.0, 32.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::QStart), r));
                        let r = mbutton(ui, &self.man, "Stop Queue", 130.0, 32.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::QStop), r));
                    });
                    ui.label(
                        egui::RichText::new(format!(
                            "Max simultaneous downloads: {}",
                            self.max_concurrent
                        ))
                        .color(pl().dim),
                    );
                    let r = mslider(
                        ui,
                        &self.man,
                        "",
                        1.0,
                        16.0,
                        self.slider_get(SliderId::QMax),
                        false,
                    );
                    self.hits.push((
                        HitKind::Slider(SliderId::QMax, 1.0, 16.0, false),
                        r,
                    ));
                    // Queue v2: live ordering. The list is exactly the pump's
                    // start order (priority, then oldest first).
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Next up — the first row starts first")
                            .color(pl().dim),
                    );
                    let order = self.queued_order();
                    if order.is_empty() {
                        ui.small("Nothing waiting — every row is running or finished.");
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(132.0)
                            .show(ui, |ui| {
                                for (i, id) in order.iter().take(24).enumerate() {
                                    let row = self.downloads.iter().find(|d| d.id == *id);
                                    let mut name = row
                                        .map(|d| d.filename.clone())
                                        .unwrap_or_else(|| format!("#{id}"));
                                    if name.chars().count() > 42 {
                                        name = name.chars().take(41).collect::<String>() + "…";
                                    }
                                    let at = row.map(|d| d.start_at.clone()).unwrap_or_default();
                                    ui.horizontal(|ui| {
                                        let when = if at.is_empty() {
                                            String::new()
                                        } else {
                                            format!("  (at {at})")
                                        };
                                        ui.label(
                                            egui::RichText::new(format!("{}. {name}{when}", i + 1))
                                                .size(11.0),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                let r = mbutton(ui, &self.man, "Down", 40.0, 22.0, false);
                                                self.hits.push((HitKind::QMove(*id, 1), r));
                                                let r = mbutton(ui, &self.man, "Up", 32.0, 22.0, false);
                                                self.hits.push((HitKind::QMove(*id, -1), r));
                                            },
                                        );
                                    });
                                }
                            });
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("When queue finishes:");
                        let cur = match self.finish_action.as_str() {
                            "shutdown" => "Shut down",
                            "sleep" => "Sleep",
                            "hibernate" => "Hibernate",
                            _ => "Do nothing",
                        };
                        let g = ui.painter().layout_no_wrap(
                            cur.to_string(),
                            egui::FontId::proportional(12.5),
                            pl().txt,
                        );
                        let (r, _) = ui.allocate_exact_size(
                            egui::vec2((g.size().x + 40.0).max(140.0), 28.0),
                            egui::Sense::hover(),
                        );
                        if self.man.hover(&r) {
                            ui.painter().rect_filled(r, 6.0, pl().surface_lt);
                        }
                        ui.painter().rect_stroke(
                            r,
                            6.0,
                            egui::Stroke::new(1.0, pl().modal_bd),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().galley(
                            egui::pos2(r.min.x + 10.0, r.center().y - g.size().y / 2.0),
                            g,
                            pl().txt,
                        );
                        // dropdown arrow
                        ui.painter().text(
                            egui::pos2(r.max.x - 14.0, r.center().y),
                            egui::Align2::CENTER_CENTER,
                            "▾".to_string(),
                            egui::FontId::proportional(12.0),
                            pl().dim,
                        );
                        self.hits.push((HitKind::ComboAnchor, r));
                    });
                    ui.separator();
                    let r = mcheck(
                        ui,
                        &self.man,
                        "Start queue automatically at (HH:MM):",
                        self.scheduler_enabled,
                    );
                    self.hits.push((HitKind::Check(CheckId::SchedOn), r));
                    let r = mtext(ui, "sched_time", &mut self.scheduler_time, "HH:MM", 90.0, false);
                    self.hits.push((HitKind::Field("sched_time"), r));
                    if !sched_fired {
                        ui.small("Waiting for scheduled time…");
                    } else {
                        ui.small("Scheduler already fired this session.");
                        let r = mbutton(ui, &self.man, "Reset Scheduler", 140.0, 30.0, false);
                        self.hits.push((HitKind::Btn(ButtonId::QSchedReset), r));
                    }
                    }
                    ui.separator();
                    ui.collapsing("Log", |ui| {
                        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                            for line in &log {
                                ui.monospace(line);
                            }
                        });
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let r = mbutton(ui, &self.man, "Close", 110.0, 36.0, false);
                            self.hits.push((HitKind::Btn(ButtonId::QClose), r));
                        },
                    );
                });
            });
            let was_open = self.show_queues;
            self.show_queues = open;
            if was_open && !open {
                self.save_ui_state();
            }
        }

        self.draw_refresh_dialog(ctx);
        if let Some(did) = self.details_id {
            let snap = self.downloads.iter().find(|d| d.id == did).cloned();
            match snap {
                None => self.details_id = None,
                Some(item) => {
                    let active = self.tasks.contains_key(&did);
                    let paused = matches!(item.status, Status::Paused);
                    let finished = matches!(item.status, Status::Completed);
                    let pct = (item.progress() * 100.0).clamp(0.0, 100.0);
                    // Editing buffers: `item` is this frame's snapshot, so the
                    // inputs edit locals that are written back after the UI.
                    if self.det_for != Some(did) {
                        self.det_for = Some(did);
                        self.det_sum = item.checksum_sha256.clone();
                    }
                    let mut desc = item.description.clone();
                    let mut user = item.username.clone();
                    let mut pass = item.password.clone();
                    if self.det_start_draft.0 != item.id {
                        self.det_start_draft = (item.id, item.start_at.clone());
                    }
                    let mut start_at = self.det_start_draft.1.clone();
                    let mut sum = self.det_sum.clone();
                    let mut referer = item.referer.clone();

                    let det_w = modal_width(ctx, DP_W);
                    let mut open = true;

                    egui::Window::new("details_modal")
                        .id(egui::Id::new("details_modal"))
                        .order(egui::Order::Foreground)
                        .open(&mut open)
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .pivot(egui::Align2::CENTER_CENTER)
                        .title_bar(false)
                        .collapsible(false)
                        .resizable(false)
                        .default_width(det_w)
                        .min_width(det_w.min(320.0))
                        .max_width(det_w)
                        .max_height((ctx.screen_rect().height() * 0.86).clamp(300.0, 900.0))
                        .frame(
                            egui::Frame::new()
                                .fill(pl().modal_bg)
                                .stroke(egui::Stroke::new(1.0_f32, pl().modal_bd))
                                .corner_radius(12.0)
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| {
                            ui.set_width(det_w);
                            // The dialog is a card: no window item spacing
                            // between the header, the content and the bar.
                            ui.spacing_mut().item_spacing.y = 0.0;
                            // ---- title bar: file badge · name · close ----
                            let (bar, _) = ui.allocate_exact_size(
                                egui::vec2(det_w, 40.0),
                                egui::Sense::hover(),
                            );
                            let p = ui.painter().clone();
                            p.line_segment(
                                [
                                    egui::pos2(bar.min.x, bar.max.y - 0.5),
                                    egui::pos2(bar.max.x, bar.max.y - 0.5),
                                ],
                                egui::Stroke::new(1.0, pl().modal_bd),
                            );
                            p.text(
                                egui::pos2(bar.min.x + 22.0, bar.center().y),
                                egui::Align2::CENTER_CENTER,
                                file_icon(crate::model::category_of(&item.filename)).to_string(),
                                crate::fonts::icon(16.0),
                                pl().warning,
                            );
                            let close_r = egui::Rect::from_center_size(
                                egui::pos2(bar.max.x - 21.5, bar.center().y),
                                egui::vec2(25.0, 25.0),
                            );
                            let close_hover = self.man.hover(&close_r);
                            if close_hover {
                                p.rect_filled(close_r, 6.0, pl().surface_lt);
                            }
                            p.text(
                                close_r.center(),
                                egui::Align2::CENTER_CENTER,
                                "\u{e5cd}".to_string(),
                                crate::fonts::icon(14.0),
                                if close_hover { pl().txt } else { pl().dim },
                            );
                            self.hits.push((HitKind::Btn(ButtonId::DClose), close_r));
                            // Filename between icon and X, elided on overflow.
                            // Drawn through the shaper so a Bangla caption name
                            // keeps its conjuncts and matra order.
                            let title_l = bar.min.x + 40.0;
                            let title_w = (close_r.min.x - 8.0 - title_l).max(40.0);
                            let shown = truncate_to_width(
                                &p,
                                &item.filename,
                                crate::fonts::semi(12.0),
                                pl().txt,
                                title_w,
                            );
                            crate::shaped::draw(
                                &p,
                                egui::pos2(title_l, bar.center().y),
                                egui::Align2::LEFT_CENTER,
                                &shown,
                                crate::fonts::semi(12.0),
                                pl().txt,
                            );

                            // ---- content (mock `.dlg-body`) ----
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: 13,
                                    right: 13,
                                    top: 9,
                                    bottom: 10,
                                })
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    let row_w = det_w - 26.0;
                                    ui.set_width(row_w);
                                    let soft = mix(pl().modal_bg, pl().modal_bd, 0.45);

                                    let (status_txt, status_col) = match &item.status {
                                        Status::Queued => ("Queued".to_string(), pl().dim),
                                        Status::Downloading => (
                                            if item.downloaded_bytes == 0 {
                                                "Connecting…".to_string()
                                            } else {
                                                "Downloading".to_string()
                                            },
                                            pl().primary,
                                        ),
                                        Status::Paused => ("Paused".to_string(), pl().warning),
                                        Status::Completed => ("Complete".to_string(), pl().success),
                                        Status::Failed(_) => ("Failed".to_string(), pl().error),
                                    };

                                    // ---- meta strip: Type | Status | Size ----
                                    let (strip, _) = ui.allocate_exact_size(
                                        egui::vec2(row_w, 48.0),
                                        egui::Sense::hover(),
                                    );
                                    {
                                        let p = ui.painter();
                                        p.rect(
                                            strip,
                                            8.0,
                                            pl().surface,
                                            egui::Stroke::new(1.0, soft),
                                            egui::StrokeKind::Inside,
                                        );
                                        let fr = [1.5_f32, 0.95, 1.0];
                                        let sum_fr: f32 = fr.iter().sum();
                                        let mut cx = strip.min.x;
                                        for i in 0..3 {
                                            let cw = row_w * fr[i] / sum_fr;
                                            if i > 0 {
                                                p.line_segment(
                                                    [
                                                        egui::pos2(cx, strip.min.y + 6.0),
                                                        egui::pos2(cx, strip.max.y - 6.0),
                                                    ],
                                                    egui::Stroke::new(1.0, soft),
                                                );
                                            }
                                            let tx = cx + 11.0;
                                            let tw = (cw - 22.0).max(10.0);
                                            p.text(
                                                egui::pos2(tx, strip.min.y + 5.0),
                                                egui::Align2::LEFT_TOP,
                                                ["TYPE", "STATUS", "SIZE"][i],
                                                crate::fonts::reg(8.0),
                                                pl().dim,
                                            );
                                            let vy = strip.min.y + 17.0;
                                            match i {
                                                0 => {
                                                    let tv = truncate_to_width(
                                                        p,
                                                        &dp_type_label(&item.filename),
                                                        crate::fonts::reg(11.0),
                                                        pl().txt,
                                                        tw,
                                                    );
                                                    p.text(
                                                        egui::pos2(tx, vy),
                                                        egui::Align2::LEFT_TOP,
                                                        tv,
                                                        crate::fonts::reg(11.0),
                                                        pl().txt,
                                                    );
                                                }
                                                1 => {
                                                    p.circle_filled(
                                                        egui::pos2(tx + 2.5, vy + 5.5),
                                                        2.5,
                                                        status_col,
                                                    );
                                                    let sv = truncate_to_width(
                                                        p,
                                                        &status_txt,
                                                        crate::fonts::reg(11.0),
                                                        status_col,
                                                        (tw - 10.0).max(10.0),
                                                    );
                                                    p.text(
                                                        egui::pos2(tx + 10.0, vy),
                                                        egui::Align2::LEFT_TOP,
                                                        sv,
                                                        crate::fonts::reg(11.0),
                                                        status_col,
                                                    );
                                                }
                                                _ => {
                                                    let raw = match item.total_bytes {
                                                        Some(t) if t > 0 => t,
                                                        _ => item.downloaded_bytes,
                                                    };
                                                    p.text(
                                                        egui::pos2(tx, vy),
                                                        egui::Align2::LEFT_TOP,
                                                        format_bytes(raw),
                                                        crate::fonts::reg(11.0),
                                                        pl().txt,
                                                    );
                                                    p.text(
                                                        egui::pos2(tx, vy + 14.0),
                                                        egui::Align2::LEFT_TOP,
                                                        format!("{} bytes", thousands(raw)),
                                                        crate::fonts::reg(8.5),
                                                        pl().dim,
                                                    );
                                                }
                                            }
                                            cx += cw;
                                        }
                                    }

                                    // Live progress for anything not finished:
                                    // one slim bar plus a summary line under the
                                    // meta strip (the concept has no room for a
                                    // second dialog).
                                    if !finished {
                                        ui.add_space(8.0);
                                        let (br, _) = ui.allocate_exact_size(
                                            egui::vec2(row_w, 4.0),
                                            egui::Sense::hover(),
                                        );
                                        let pb = ui.painter();
                                        pb.rect_filled(br, 2.0, pl().track);
                                        let fill = br.width() * item.progress() as f32;
                                        if fill > 0.5 {
                                            pb.rect_filled(
                                                egui::Rect::from_min_size(
                                                    br.min,
                                                    egui::vec2(fill, br.height()),
                                                ),
                                                2.0,
                                                pl().primary,
                                            );
                                        }
                                        let eta = format_eta(&item);
                                        let mut bits = vec![format!("{pct}%")];
                                        if item.speed_bps > 1.0 {
                                            bits.push(format_speed(item.speed_bps));
                                        }
                                        if eta != "-" {
                                            bits.push(format!("{eta} left"));
                                        }
                                        ui.add_space(3.0);
                                        dp_value(ui, &bits.join("  ·  "), pl().dim, 10.0, row_w);
                                        ui.add_space(3.0);
                                    } else {
                                        ui.add_space(8.0);
                                    }

                                    // ---- Save to + Move ----
                                    let dest = item.dest_path();
                                    let exists = dest.exists();
                                    let shown = if exists {
                                        dest.to_string_lossy().to_string()
                                    } else if finished {
                                        "The file has been moved.".to_string()
                                    } else {
                                        item.save_dir.clone()
                                    };
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        dp_lbl(ui, "Save to");
                                        if finished {
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    let r = dp_small(ui, &self.man, "Move", 56.0);
                                                    self.hits.push((HitKind::Btn(ButtonId::DMove), r));
                                                    let w = ui.available_width();
                                                    dp_value(ui, &shown, pl().dim, 9.5, w);
                                                },
                                            );
                                        } else {
                                            let w = ui.available_width();
                                            dp_value(ui, &shown, pl().dim, 9.5, w);
                                        }
                                    });
                                    ui.add_space(4.0);

                                    // ---- Address (media URL) + copy ----
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        dp_lbl(ui, "Address");
                                        let text_w = (ui.available_width() - 30.0).max(40.0);
                                        dp_value(ui, &item.url, pl().primary, 9.5, text_w);
                                        let r = dp_icon(ui, &self.man, MI_COPY);
                                        self.hits.push((HitKind::Btn(ButtonId::DCopyAddr), r));
                                    });
                                    ui.add_space(5.0);
                                    dp_rule_soft(ui, soft);
                                    ui.add_space(5.0);

                                    // ---- Description (live note) ----
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        dp_lbl(ui, "Description");
                                        let w = ui.available_width();
                                        let r = dp_input(ui, "det_desc", &mut desc, "Add a note", w, false, false);
                                        self.hits.push((HitKind::Field("det_desc"), r));
                                    });
                                    ui.add_space(4.0);

                                    // ---- Start at + clock (Queue v2: start
                                    // this row no earlier than HH:MM; blank =
                                    // as soon as a slot is free) ----
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        dp_lbl(ui, "Start at");
                                        let w = (ui.available_width() - 30.0).max(40.0);
                                        let r = dp_input(
                                            ui,
                                            "det_start",
                                            &mut start_at,
                                            "HH:MM (blank = now)",
                                            w,
                                            false,
                                            false,
                                        );
                                        self.hits.push((HitKind::Field("det_start"), r));
                                        dp_icon(ui, &self.man, MI_SCHEDULE);
                                    });
                                    if !start_at.trim().is_empty()
                                        && crate::model::parse_hhmm(&start_at).is_none()
                                    {
                                        ui.add_space(2.0);
                                        dp_value(
                                            ui,
                                            "Use HH:MM (24-hour), e.g. 23:30.",
                                            pl().error,
                                            10.0,
                                            row_w,
                                        );
                                    }
                                    ui.add_space(4.0);

                                    // ---- Web page + Referer ----
                                    let web = page_url_of(&item.referer);
                                    if !web.is_empty() {
                                        dp_value(
                                            ui,
                                            "The web page from which this file was obtained:",
                                            pl().dim,
                                            9.5,
                                            row_w,
                                        );
                                        ui.add_space(2.0);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            let text_w = (ui.available_width() - 26.0).max(40.0);
                                            dp_value(ui, &web, pl().primary, 10.5, text_w);
                                            let r = dp_icon(ui, &self.man, MI_COPY);
                                            self.hits.push((HitKind::Btn(ButtonId::DCopyPage), r));
                                        });
                                        ui.add_space(7.0);
                                    }
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 10.0;
                                        dp_lbl(ui, "Referer");
                                        let w = ui.available_width();
                                        let r = dp_input(
                                            ui,
                                            "det_ref",
                                            &mut referer,
                                            "https://… (empty = none)",
                                            w,
                                            false,
                                            false,
                                        );
                                        self.hits.push((HitKind::Field("det_ref"), r));
                                    });
                                    ui.add_space(4.0);

                                    // ---- Integrity + Verify ----
                                    if finished || !item.checksum_sha256.trim().is_empty() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 10.0;
                                            dp_lbl(ui, "Integrity");
                                            if finished {
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        let vr = dp_small(ui, &self.man, "Verify", 62.0);
                                                        self.hits.push((
                                                            HitKind::Btn(ButtonId::DVerify),
                                                            vr,
                                                        ));
                                                        let w = ui.available_width();
                                                        let r = dp_input(
                                                            ui,
                                                            "det_sum",
                                                            &mut sum,
                                                            "SHA-256 (optional)",
                                                            w,
                                                            false,
                                                            false,
                                                        );
                                                        self.hits.push((HitKind::Field("det_sum"), r));
                                                    },
                                                );
                                            } else {
                                                let w = ui.available_width();
                                                let r = dp_input(
                                                    ui,
                                                    "det_sum",
                                                    &mut sum,
                                                    "SHA-256 (optional)",
                                                    w,
                                                    false,
                                                    false,
                                                );
                                                self.hits.push((HitKind::Field("det_sum"), r));
                                            }
                                        });
                                        let norm = crate::model::normalize_sha256(&sum);
                                        if !sum.trim().is_empty()
                                            && !crate::model::is_valid_sha256(&norm)
                                        {
                                            ui.add_space(2.0);
                                            dp_value(
                                                ui,
                                                "Invalid SHA-256 — 64 hex characters expected.",
                                                pl().error,
                                                10.0,
                                                row_w,
                                            );
                                        }
                                        ui.add_space(4.0);
                                    }

                                    dp_rule_soft(ui, soft);
                                    ui.add_space(5.0);

                                    // ---- Login + Password side by side ----
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        let half = ((row_w - 8.0) / 2.0).max(60.0);
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(half, DP_ROW_H),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.spacing_mut().item_spacing.x = 7.0;
                                                ui.add(egui::Label::new(
                                                    egui::RichText::new("Login")
                                                        .font(crate::fonts::reg(10.5))
                                                        .color(pl().dim),
                                                ));
                                                let w = ui.available_width();
                                                let r = dp_input(
                                                    ui, "det_user", &mut user, "", w, false, false,
                                                );
                                                self.hits.push((HitKind::Field("det_user"), r));
                                            },
                                        );
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(half, DP_ROW_H),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.spacing_mut().item_spacing.x = 7.0;
                                                ui.add(egui::Label::new(
                                                    egui::RichText::new("Password")
                                                        .font(crate::fonts::reg(10.5))
                                                        .color(pl().dim),
                                                ));
                                                let w = ui.available_width();
                                                let r = dp_input(
                                                    ui, "det_pass", &mut pass, "", w, true, false,
                                                );
                                                self.hits.push((HitKind::Field("det_pass"), r));
                                            },
                                        );
                                    });
                                });

                            // ---- bottom bar (mock `.bottom-bar`) ----
                            let (rule, _) = ui.allocate_exact_size(
                                egui::vec2(det_w, 1.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().line_segment(
                                [rule.left_center(), rule.right_center()],
                                egui::Stroke::new(1.0, pl().modal_bd),
                            );
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: 13,
                                    right: 13,
                                    top: 9,
                                    bottom: 10,
                                })
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        if active || paused {
                                            let r = dp_button(
                                                ui,
                                                &self.man,
                                                if paused { "Resume" } else { "Pause" },
                                                false,
                                            );
                                            self.hits.push((HitKind::Btn(ButtonId::DResume), r));
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.spacing_mut().item_spacing.x = 8.0;
                                                let r =
                                                    dp_button(ui, &self.man, "OK", true);
                                                self.hits
                                                    .push((HitKind::Btn(ButtonId::DClose), r));
                                                let r =
                                                    dp_button(ui, &self.man, "Open", false);
                                                self.hits
                                                    .push((HitKind::Btn(ButtonId::DOpen), r));
                                            },
                                        );
                                    });
                                });
                        });

                    // Persist the live edits (one write per changed field).
                    self.det_start_draft.1 = start_at.clone();
                    let mut dirty = false;
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.id == did) {
                        if d.description != desc {
                            d.description = desc;
                            dirty = true;
                        }
                        if d.username != user {
                            d.username = user;
                            dirty = true;
                        }
                        if d.password != pass {
                            d.password = pass;
                            dirty = true;
                        }
                        // Bad input never sticks: the previous schedule stays.
                        let next_start = start_at.trim().to_string();
                        if crate::model::parse_hhmm(&next_start).is_some() || next_start.is_empty() {
                            if d.start_at != next_start {
                                d.start_at = next_start;
                                dirty = true;
                            }
                        }
                        let next_ref = referer.trim().to_string();
                        if d.referer != next_ref {
                            d.referer = next_ref;
                            dirty = true;
                        }
                    }
                    self.det_sum = sum;
                    let norm = crate::model::normalize_sha256(&self.det_sum);
                    if norm.is_empty() || crate::model::is_valid_sha256(&norm) {
                        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == did) {
                            if d.checksum_sha256 != norm {
                                d.checksum_sha256 = norm;
                                dirty = true;
                            }
                        }
                    }
                    if dirty {
                        self.save_queue();
                    }
                    if !open {
                        self.details_id = None;
                        self.det_for = None;
                    }
                }
            }
        }

        if self.show_about {
            // About concept: a fixed 480 px card — centered header, brand row,
            // copy, credit box, update buttons, and a bordered footer. The whole
            // card is painted from its origin so the concept's pixel geometry
            // survives; hits go on the manual table like every other modal.
            let p = pl();
            let ab_w = modal_width(ctx, 480.0);
            let pad = 26.0;
            let body_w = (ab_w - pad * 2.0).max(120.0);
            let soft = mix(p.modal_bg, p.modal_bd, 0.45);
            let accent = p.primary;
            let dark = p.txt.r() > 128;
            let accent_hi = if dark {
                mix(p.primary, egui::Color32::WHITE, 0.18)
            } else {
                mix(p.primary, egui::Color32::BLACK, 0.12)
            };
            let green = p.success;
            let header_h = 46.0;
            let footer_h = 58.0;
            let btn_h = 34.0;

            let f_title = egui::FontId::proportional(13.0);
            let f_name = crate::fonts::semi(18.0);
            let f_tag = crate::fonts::semi(9.0);
            let f_body = egui::FontId::proportional(12.0);
            let f_small = egui::FontId::proportional(10.5);
            let f_ver = egui::FontId::proportional(11.0);
            let f_ver_b = crate::fonts::semi(11.0);
            let mono = egui::FontId::monospace(10.0);

            // Galleys are measured before the window opens so the card's height
            // is known exactly (the concept is a pixel layout, not a flow one).
            let lay = |t: &str, f: egui::FontId, c: egui::Color32, w: f32| {
                ctx.fonts(|fo| fo.layout(t.to_owned(), f, c, w))
            };
            let para = |spans: &[(&str, egui::Color32)]| {
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = body_w;
                for (t, c) in spans {
                    job.append(
                        t,
                        0.0,
                        egui::text::TextFormat {
                            font_id: f_body.clone(),
                            color: *c,
                            line_height: Some(19.8),
                            ..Default::default()
                        },
                    );
                }
                ctx.fonts(|fo| fo.layout_job(job))
            };
            let desc1 = para(&[
                ("A lightweight download manager written in ", p.dim),
                ("Rust + egui", p.txt),
                (
                    ". MIT licensed — free for open-source, closed-source, and commercial use.",
                    p.dim,
                ),
            ]);
            let desc2 = para(&[(
                "Multi-connection segmented downloads, queue, scheduler, browser extension host.",
                p.dim,
            )]);
            let desc3 = para(&[("Interface design inspired by modern download managers.", p.dim)]);
            let credit = {
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = body_w - 13.0 - 14.0 - 9.0 - 13.0;
                job.append(
                    "Icons: Material Icons font (Apache-2.0, © Google). See ",
                    0.0,
                    egui::text::TextFormat {
                        font_id: f_small.clone(),
                        color: p.dm_lbl,
                        line_height: Some(16.3),
                        ..Default::default()
                    },
                );
                job.append(
                    " NOTICE.md ",
                    0.0,
                    egui::text::TextFormat {
                        font_id: mono.clone(),
                        color: p.dm_dim,
                        background: p.dm_card,
                        line_height: Some(16.3),
                        ..Default::default()
                    },
                );
                job.append(
                    ".",
                    0.0,
                    egui::text::TextFormat {
                        font_id: f_small.clone(),
                        color: p.dm_lbl,
                        line_height: Some(16.3),
                        ..Default::default()
                    },
                );
                ctx.fonts(|fo| fo.layout_job(job))
            };
            let g_name = lay("Download Craft", f_name.clone(), p.txt, body_w);
            let g_ver = {
                let mut job = egui::text::LayoutJob::default();
                job.wrap.max_width = body_w;
                for (t, f, c) in [
                    ("Version ", f_ver.clone(), p.dim),
                    (crate::model::APP_VERSION, f_ver_b.clone(), accent_hi),
                    (" — check this to confirm your build.", f_ver.clone(), p.dim),
                ] {
                    job.append(
                        t,
                        0.0,
                        egui::text::TextFormat {
                            font_id: f,
                            color: c,
                            ..Default::default()
                        },
                    );
                }
                ctx.fonts(|fo| fo.layout_job(job))
            };
            let status_g = if self.update_status.trim().is_empty() {
                None
            } else {
                Some(lay(&self.update_status, f_small.clone(), p.dim, body_w))
            };
            let hint_g = if self.update_asset.is_some() && !self.update_restart_ready {
                Some(lay(
                    "Update now downloads the new version, checks it, replaces this copy's files and restarts. Your list and settings stay.",
                    f_small.clone(),
                    p.dm_lbl,
                    body_w,
                ))
            } else {
                None
            };

            let brand_h = g_name.size().y.max(46.0) + 4.0 + g_ver.size().y;
            let credit_h = credit.size().y.max(14.0) + 18.0;
            let mut body_h = 18.0 + brand_h + 16.0 + 1.0 + 14.0;
            body_h += desc1.size().y + 10.0 + desc2.size().y + 10.0 + desc3.size().y + 14.0;
            body_h += credit_h + 14.0 + btn_h;
            if let Some(g) = &status_g {
                body_h += 8.0 + g.size().y;
            }
            if let Some(g) = &hint_g {
                body_h += 8.0 + g.size().y;
            }
            body_h += 16.0;
            // Natural height: the layout is absolute, so clamping it would push
            // the footer over the copy. It is sized to fit the app's smallest
            // window (840x480) instead.
            let ab_h = header_h + body_h + footer_h;

            let mut open = true;
            egui::Window::new("About")
                .id(egui::Id::new("about_modal"))
                .order(egui::Order::Foreground)
                .open(&mut open)
                .title_bar(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .pivot(egui::Align2::CENTER_CENTER)
                .collapsible(false)
                .resizable(false)
                .fixed_size(egui::vec2(ab_w, ab_h))
                .frame(egui::Frame::new().inner_margin(egui::Margin::ZERO))
                .show(ctx, |ui| {
                    // Painting alone leaves the window's used rect empty, which
                    // would anchor a zero-sized area at the screen centre. Claim
                    // the card so the anchor/pivot place it exactly.
                    ui.set_min_size(egui::vec2(ab_w, ab_h));
                    let card = ui.max_rect();
                    let pf = ui.painter().clone();
                    // ---- card: faint top-lit gradient, 1 px border ----
                    crate::dm::dm_gradient(
                        &pf,
                        card,
                        14.0,
                        mix(p.modal_bg, p.primary, 0.05),
                        p.modal_bg,
                    );
                    pf.rect_stroke(
                        card,
                        14.0,
                        egui::Stroke::new(1.0, p.modal_bd),
                        egui::StrokeKind::Inside,
                    );
                    pf.hline(
                        card.x_range(),
                        card.top() + 0.5,
                        egui::Stroke::new(1.0, wash(accent, 110)),
                    );

                    // ---- header: centered title + close ----
                    let head = egui::Rect::from_min_size(card.min, egui::vec2(ab_w, header_h));
                    pf.hline(
                        card.x_range(),
                        head.bottom() - 0.5,
                        egui::Stroke::new(1.0, soft),
                    );
                    let title_g = pf.layout_no_wrap("About".to_owned(), f_title.clone(), p.txt);
                    pf.galley(
                        egui::pos2(
                            head.center().x - title_g.size().x / 2.0,
                            head.center().y - title_g.size().y / 2.0,
                        ),
                        title_g,
                        p.txt,
                    );
                    let xr = egui::Rect::from_center_size(
                        egui::pos2(head.right() - 24.0, head.center().y),
                        egui::vec2(28.0, 28.0),
                    );
                    let xh = self.man.hover(&xr);
                    if xh {
                        pf.rect_filled(xr, 7.0, p.dm_hover);
                    }
                    pf.text(
                        xr.center(),
                        egui::Align2::CENTER_CENTER,
                        "\u{e5cd}".to_string(),
                        crate::fonts::icon(12.0),
                        if xh { p.txt } else { p.dm_lbl },
                    );
                    self.hits.push((HitKind::Btn(ButtonId::AbClose), xr));

                    // ---- body ----
                    let bx = card.left() + pad;
                    let mut y = head.bottom() + 18.0;
                    // brand tile
                    let tile = egui::Rect::from_min_size(egui::pos2(bx, y), egui::vec2(46.0, 46.0));
                    pf.rect(
                        tile,
                        12.0,
                        wash(accent, 26),
                        egui::Stroke::new(1.0, wash(accent, 72)),
                        egui::StrokeKind::Inside,
                    );
                    pf.text(
                        tile.center(),
                        egui::Align2::CENTER_CENTER,
                        "\u{e92d}".to_string(),
                        crate::fonts::icon(26.0),
                        accent,
                    );
                    // name + MIT tag, version under it
                    let tx = tile.right() + 14.0;
                    let block_h = g_name.size().y + 4.0 + g_ver.size().y;
                    let ty = tile.center().y - block_h / 2.0;
                    pf.galley(egui::pos2(tx, ty), g_name.clone(), p.txt);
                    let tag_g = pf.layout_no_wrap("MIT".to_owned(), f_tag.clone(), green);
                    let tag = egui::Rect::from_min_size(
                        egui::pos2(
                            tx + g_name.size().x + 8.0,
                            ty + g_name.size().y / 2.0 - 8.5,
                        ),
                        egui::vec2(tag_g.size().x + 16.0, 17.0),
                    );
                    pf.rect(
                        tag,
                        8.5,
                        wash(green, 30),
                        egui::Stroke::new(1.0, wash(green, 64)),
                        egui::StrokeKind::Inside,
                    );
                    pf.galley(tag.center() - tag_g.size() / 2.0, tag_g, green);
                    pf.galley(
                        egui::pos2(tx, ty + g_name.size().y + 4.0),
                        g_ver.clone(),
                        p.dim,
                    );
                    y += brand_h + 16.0;
                    // divider
                    pf.hline(bx..=card.right() - pad, y, egui::Stroke::new(1.0, soft));
                    y += 1.0 + 14.0;
                    // copy
                    for g in [&desc1, &desc2, &desc3] {
                        pf.galley(egui::pos2(bx, y), g.clone(), p.dim);
                        y += g.size().y + 10.0;
                    }
                    y += 4.0; // the last paragraph's 14 px margin
                    // credit box
                    let cb = egui::Rect::from_min_size(egui::pos2(bx, y), egui::vec2(body_w, credit_h));
                    pf.rect(
                        cb,
                        9.0,
                        mix(p.modal_bg, p.surface_lt, 0.30),
                        egui::Stroke::new(1.0, soft),
                        egui::StrokeKind::Inside,
                    );
                    pf.text(
                        egui::pos2(bx + 13.0 + 7.0, y + 9.0 + 7.0),
                        egui::Align2::CENTER_CENTER,
                        "\u{e88e}".to_string(),
                        crate::fonts::icon(14.0),
                        p.dm_lbl,
                    );
                    pf.galley(egui::pos2(bx + 13.0 + 14.0 + 9.0, y + 9.0), credit.clone(), p.dm_lbl);
                    y += credit_h + 14.0;
                    // ---- update buttons (opt-in; the asset queues like any
                    // other file, never a silent self-replace) ----
                    let b1 = about_btn(
                        &pf,
                        &self.man,
                        bx,
                        y,
                        if self.update_busy {
                            "Checking…"
                        } else {
                            "Check for updates"
                        },
                        Some("\u{e5d5}"),
                        false,
                    );
                    self.hits.push((HitKind::Btn(ButtonId::UpdCheck), b1));
                    if self.update_restart_ready {
                        let b2 = about_btn(&pf, &self.man, b1.right() + 8.0, y, "Restart now", Some("\u{e5d5}"), true);
                        self.hits.push((HitKind::Btn(ButtonId::UpdRestart), b2));
                    } else if self.update_asset.is_some() && !self.update_busy {
                        let b2 = about_btn(
                            &pf,
                            &self.man,
                            b1.right() + 8.0,
                            y,
                            "Update now",
                            Some("\u{e171}"),
                            true,
                        );
                        self.hits.push((HitKind::Btn(ButtonId::UpdDownload), b2));
                    }
                    y += btn_h;
                    if let Some(g) = &status_g {
                        y += 8.0;
                        pf.galley(egui::pos2(bx, y), g.clone(), p.dim);
                        y += g.size().y;
                    }
                    if let Some(g) = &hint_g {
                        y += 8.0;
                        pf.galley(egui::pos2(bx, y), g.clone(), p.dm_lbl);
                    }

                    // ---- footer ----
                    let foot = egui::Rect::from_min_max(
                        egui::pos2(card.left(), card.bottom() - footer_h),
                        card.max,
                    );
                    pf.rect_filled(foot, 0.0, mix(p.modal_bg, egui::Color32::BLACK, 0.12));
                    pf.hline(
                        card.x_range(),
                        foot.top() + 0.5,
                        egui::Stroke::new(1.0, soft),
                    );
                    let cw = {
                        let g = pf.layout_no_wrap("Close".to_owned(), f_body.clone(), egui::Color32::WHITE);
                        30.0 + g.size().x
                    };
                    let close_r = egui::Rect::from_min_size(
                        egui::pos2(card.right() - pad - cw, foot.center().y - btn_h / 2.0),
                        egui::vec2(cw, btn_h),
                    );
                    about_btn(
                        &pf,
                        &self.man,
                        close_r.left(),
                        close_r.top(),
                        "Close",
                        None,
                        true,
                    );
                    self.hits.push((HitKind::Btn(ButtonId::AbClose), close_r));
                });
        }

        // ================= manual popups (topmost) + input processing ====
        // Dropdown (menubar) and row context menu render here so their hit
        // rects are recorded last (processed first = topmost wins).
        if let Some(m) = self.drop_menu {
            self.render_drop_menu(ctx, m);
        }
        if let Some((id, pos)) = self.ctx_menu {
            self.render_ctx_menu(ctx, id, pos);
        }
        self.process_hits(ctx);
        self.diag_hits_n = self.hits.len();
    }
}

impl CraftApp {
    /// Menubar dropdown items for `render_popup`. (label, MenuItem act.)
    /// The Theme menu marks the active theme (19 system, 20 dark, 22 light).
    fn drop_rows(&self, menu: u8) -> Vec<(String, Option<HitKind>)> {
        let mut out = Vec::new();
        for (label, act) in Self::menu_items(menu) {
            if label.is_empty() {
                out.push((String::new(), None));
                continue;
            }
            let mut text = label.clone();
            if menu == 3 {
                let key = match act {
                    19 => "system",
                    20 => "dark",
                    22 => "light",
                    _ => "",
                };
                // Marked here, drawn as a tick icon by `render_popup` (the
                // text font has no check mark glyph: it showed as a box).
                if !key.is_empty() && self.theme == key {
                    text = format!("{POPUP_CHECK}{text}");
                } else if !key.is_empty() {
                    text = format!("{POPUP_INDENT}{text}");
                }
            }
            out.push((text, Some(HitKind::MenuItem(act))));
        }
        out
    }

    /// Row context-menu rows for `render_popup`.
    fn frac_to_val(frac: f64, min: f64, max: f64, log: bool) -> f64 {
        let f = frac.clamp(0.0, 1.0);
        if log {
            (min.ln() + f * (max.ln() - min.ln())).exp()
        } else {
            min + f * (max - min)
        }
    }

    fn slider_get(&self, id: SliderId) -> f64 {
        match id {
            SliderId::AddConns => self.add_conns as f64,
            SliderId::LimitKB => self.speed_limit_kbps.max(50) as f64,
            SliderId::Timeout => self.timeout_secs as f64,
            SliderId::Retries => self.max_retries as f64,
            SliderId::DefConns => self.connections_default as f64,
            SliderId::QMax => self.max_concurrent as f64,
        }
    }

    fn slider_set(&mut self, id: SliderId, v: f64) {
        match id {
            SliderId::AddConns => self.add_conns = (v as usize).clamp(1, 32),
            SliderId::LimitKB => self.speed_limit_kbps = (v as u64).clamp(50, 20000),
            SliderId::Timeout => self.timeout_secs = (v as u64).clamp(5, 600),
            SliderId::Retries => self.max_retries = (v as u32).clamp(1, 20),
            SliderId::DefConns => self.connections_default = (v as usize).clamp(1, 32),
            SliderId::QMax => {
                self.max_concurrent = (v as usize).clamp(1, 16);
                self.fill_next();
            }
        }
    }

    /// A manually opened Add dialog starts without the last browser
    /// capture's session: its cookie, referer, captured headers and login
    /// would otherwise be sent to whatever URL is typed next.
    fn clear_add_credentials(&mut self) {
        self.add_referer.clear();
        self.add_cookie.clear();
        self.add_headers.clear();
        self.add_browser = None;
        self.add_user.clear();
        self.add_pass.clear();
    }

    /// Manual dialog/toast button dispatcher (driven by process_hits).
    /// Validate + queue the Add dialog contents (shared by Add/Download).
    fn commit_add(&mut self, download: bool) {
        // A batch pattern (img[001-120].jpg): every URL it names goes to the
        // pick-and-queue window instead of becoming one row.
        if let Some(urls) = crate::model::expand_batch(self.add_url.trim()) {
            let links: Vec<crate::native_host::PageLink> = urls
                .into_iter()
                .map(|url| crate::native_host::PageLink { url, text: String::new() })
                .collect();
            let n = links.len();
            match crate::native_host::open_links_window(self.add_url.trim(), "Batch download", &self.add_cookie, &links) {
                Ok(()) => self.push_log(format!("Batch pattern: {n} links - pick them in the window that opened.")),
                Err(e) => self.push_log(format!("Batch pattern: {e}")),
            }
            self.show_add = false;
            return;
        }
        // "url | mirror | mirror": one download, several sources.
        if self.add_url.contains('|') {
            let (main, mirrors) = crate::model::split_mirrors(&self.add_url);
            self.add_url = main;
            self.add_mirrors = mirrors;
        }
        if !is_download_url(self.add_url.trim()) {
            self.show_add = true;
            self.push_log("Add download: invalid URL (need http/https).");
            return;
        }
        // A YouTube playlist or channel: list its videos to pick from.
        if crate::engine::is_youtube_list(self.add_url.trim()) {
            let url = self.add_url.trim().to_string();
            let opts = EngineOpts { proxy_url: self.proxy_url.clone(), user_agent: self.user_agent.clone(), ..Default::default() };
            let (tx, rx) = std::sync::mpsc::channel();
            let u = url.clone();
            self.rt.spawn(async move {
                let _ = tx.send(crate::engine::playlist_entries(&u, &opts).await.map_err(|e| format!("{e:#}")));
            });
            self.playlist_rx = Some((url, rx));
            self.show_add = false;
            self.push_log("Listing the playlist with yt-dlp… a window opens to pick the videos.");
            return;
        }
        let csum = crate::model::normalize_sha256(&self.add_checksum);
        if !csum.is_empty() && !crate::model::is_valid_sha256(&csum) {
            self.show_add = true;
            self.push_log("Add download: invalid SHA-256 (want 64 hex chars).");
            return;
        }
        // Same URL already tracked: a live row (queued/downloading/paused, or
        // finished with its file present) blocks a duplicate, but a DEAD row —
        // failed, or "Completed" with the file gone — is refreshed in place
        // below so a stale entry never dead-ends the user.
        let existing = self
            .downloads
            .iter()
            // Same download = same URL at the same quality (one page at
            // 1080p and 720p is two files), as everywhere else.
            .position(|d| {
                d.url == self.add_url.trim() && crate::model::same_quality(&d.quality_pref, &self.add_quality)
            });
        if let Some(pos) = existing {
            let dead = match &self.downloads[pos].status {
                Status::Failed(_) => true,
                Status::Completed => !self.downloads[pos].dest_path().exists(),
                _ => false,
            };
            if !dead {
                self.show_add = true;
                self.push_log("Add download: that URL is already queued.");
                return;
            }
        }
        if self.add_file.trim().is_empty() {
            self.add_file = filename_from_url(&self.add_url);
        }
        if self.add_dir.trim().is_empty() {
            self.add_dir = dirs::download_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
        }
        let final_dir = self.save_dir_for(&self.add_file);
        let mut id = match existing {
            Some(pos) => {
                // Refresh the dead row in place (same id): drop any stale
                // resume state so a mixed-URL resume can't corrupt the file.
                let id = self.downloads[pos].id;
                let dp = self.downloads[pos].dest_path();
                crate::model::drop_resume_state(&dp);
                id
            }
            None => 0,
        };
        let mut item = DownloadItem::new(
            id,
            self.add_url.trim().to_string(),
            self.add_file.trim().to_string(),
            final_dir,
            self.add_conns,
        );
        item.referer = self.add_referer.clone();
        item.cookie = self.add_cookie.clone();
        // Captured headers belong to that one capture: never reuse them for
        // a later manual add.
        item.extra_headers = std::mem::take(&mut self.add_headers);
        item.browser = self.add_browser.take();
        item.mirrors = std::mem::take(&mut self.add_mirrors);
        item.username = self.add_user.clone();
        item.password = self.add_pass.clone();
        item.checksum_sha256 = csum;
        item.overwrite_existing = self.add_overwrite;
        item.quality_pref = self.add_quality.trim().to_string();
        // Probed (or browser-measured) size survives queueing so the engine
        // and progress UI know the total even if a re-probe would 403.
        item.total_bytes = self.probe_size.filter(|s| *s > 0);
        if !self.add_overwrite {
            let dest = item.dest_path();
            if dest.exists() {
                self.push_log(format!(
                    "Note: {} exists — will save as a numbered copy unless overwritten.",
                    item.filename
                ));
            }
        }
        // What the dialog already downloaded continues as this file, so the
        // progress window opens at those bytes instead of starting over.
        let mut finished_early = false;
        let dest = item.dest_path();
        if self.add_pre.as_ref().is_some_and(|p| p.url == item.url)
            && !dest.exists()
            && !crate::engine::has_partial(&dest)
        {
            let got = self.add_pre.as_ref().map_or(0, |p| p.shared.downloaded.load(Ordering::SeqCst));
            if self.stop_add_prestart(Some(&dest)) {
                let len = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                item.total_bytes = Some(len);
                item.downloaded_bytes = len;
                item.status = Status::Completed;
                finished_early = true;
            } else {
                item.downloaded_bytes = got;
            }
        } else {
            self.stop_add_prestart(None);
        }
        match existing {
            Some(pos) => {
                self.downloads[pos] = item;
                self.push_log(format!("Re-queued #{id}"));
            }
            None => {
                if !self.persist_new_item(&mut item) { self.show_add=true; return; }
                id=item.id;
                self.downloads.retain(|d|d.id!=id);
                self.downloads.push(item);
                self.push_log(format!("Queued #{id}"));
            }
        }
        self.save_queue();
        self.show_add = false;
        self.add_focus_req = false;
        self.add_url.clear();
        self.add_file.clear();
        self.add_file_auto.clear();
        self.add_checksum.clear();
        self.add_quality.clear();
        self.clear_add_credentials();
        if finished_early {
            self.push_log(format!("#{id} finished while the dialog was open."));
        } else if download {
            self.queue_running = true;
            self.start(id);
        }
    }

    /// Start downloading in the background while the Add dialog is open: a
    /// plain file whose size is known (not a page, a stream or a quality
    /// choice: those the engine resolves itself). Once per link.
    fn maybe_add_prestart(&mut self) {
        let url = self.add_url.trim().to_string();
        if self.add_pre.is_some() || self.probe_size.unwrap_or(0) == 0 || !is_download_url(&url) {
            return;
        }
        let plain = self.add_quality.trim().is_empty()
            && self.add_mirrors.is_empty()
            && !crate::engine::is_stream_manifest(&url, None, "")
            && !crate::engine::is_youtube_page(&url)
            && self.add_browser.as_ref().map_or(true, |b| !b.is_post());
        if !plain {
            return;
        }
        let name = crate::engine::sanitize_filename(if self.add_file.trim().is_empty() { "download" } else { self.add_file.trim() });
        let dir = std::env::temp_dir().join("download-craft-pre").join(format!(
            "main-{}-{}",
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
            referer: self.add_referer.clone(),
            cookie: self.add_cookie.clone(),
            username: self.add_user.clone(),
            password: self.add_pass.clone(),
            browser: self.add_browser.clone(),
            allow_rename: false,
            ..Default::default()
        };
        let shared = Arc::new(SharedState::new());
        if let Some(t) = self.probe_size {
            shared.total.store(t, Ordering::SeqCst);
            shared.has_total.store(true, Ordering::SeqCst);
        }
        let (s2, d2, u2) = (shared.clone(), dest.clone(), url.clone());
        let conns = self.add_conns.max(1);
        let handle = self.rt.spawn(async move { crate::engine::download(u2, d2, conns, opts, s2).await });
        self.add_pre = Some(AddPreStart { url, shared, handle, dest });
    }

    /// Stop the Add dialog's background download. `keep_as`: continue it as
    /// this file (data moved there; a file that already finished is moved
    /// whole, and the return says so). Without it everything is deleted.
    fn stop_add_prestart(&mut self, keep_as: Option<&std::path::Path>) -> bool {
        let Some(pre) = self.add_pre.take() else { return false };
        pre.shared.cancelled.store(true, Ordering::SeqCst);
        let finished = pre.handle.is_finished() && !crate::engine::has_partial(&pre.dest) && pre.dest.is_file();
        pre.handle.abort();
        // Let the transfer write its journal and let go of the files.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut whole = false;
        if let Some(to) = keep_as {
            if let Some(dir) = to.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if finished && !to.exists() {
                whole = std::fs::rename(&pre.dest, to).is_ok()
                    || (std::fs::copy(&pre.dest, to).is_ok() && std::fs::remove_file(&pre.dest).is_ok());
            } else if !finished {
                crate::engine::move_partial(&pre.dest, to);
            }
        }
        crate::engine::remove_partial(&pre.dest);
        if let Some(dir) = pre.dest.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
        whole
    }

    /// Candidate save folders for the path field's caret menu: the standard
    /// user folders first, then whatever the dialog is currently pointed at.
    fn add_recent_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
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
        for s in [self.add_dir.trim().to_string()] {
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    fn do_button(&mut self, b: ButtonId) {
        self.diag_acts += 1;
        match b {
            ButtonId::RfOpen => {
                if let Some(page) = self.refresh_dlg.as_ref().map(|d| d.page.clone()) {
                    if let Ok(u) = url::Url::parse(&page) {
                        crate::platform::open_url(u.as_str());
                    }
                }
            }
            ButtonId::RfApply => self.apply_typed_link(),
            ButtonId::RfCancel => {
                crate::model::set_refresh_wait(None);
                self.refresh_dlg = None;
            }
            ButtonId::AddPaste => {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(t) = cb.get_text() {
                        let t = t.trim().to_string();
                        if is_download_url(&t) {
                            self.add_url = t;
                        }
                    }
                }
            }
            ButtonId::AddFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.add_dir = p.to_string_lossy().to_string();
                }
            }
            ButtonId::AddRefresh => {
                if !self.add_url.trim().is_empty() {
                    self.probe_done_for = self.add_url.trim().to_string();
                    self.start_probe(self.add_url.trim().to_string());
                }
            }
            ButtonId::AddAdv => self.add_adv = !self.add_adv,
            ButtonId::AddAdd => self.commit_add(false),
            ButtonId::AddDownload => self.commit_add(true),
            ButtonId::AddCancel => {
                self.show_add = false;
                self.add_focus_req = false;
            }
            ButtonId::GrabStart => self.start_grab(),
            ButtonId::GrabQueue => self.queue_grab_results(),
            ButtonId::GrabClose => {
                self.show_grabber = false;
            }
            ButtonId::GrabPresetPick(i) => {
                const PRESETS: &[&str] =
                    &["Mixed", "Video", "Audio", "Images", "Documents", "Archives"];
                self.grab_preset = (i as usize).min(PRESETS.len() - 1);
                self.grab_exts = crate::grabber::preset_exts(PRESETS[self.grab_preset]);
            }
            ButtonId::GrabLevelsDown => self.grab_levels = self.grab_levels.saturating_sub(1).max(1),
            ButtonId::GrabLevelsUp => self.grab_levels = (self.grab_levels + 1).min(5),
            ButtonId::GrabSave => self.grab_save_project(),
            ButtonId::GrabPreset => {
                self.grab_preset = (self.grab_preset + 1) % 6;
                const PRESETS: &[&str] =
                    &["Mixed", "Video", "Audio", "Images", "Documents", "Archives"];
                self.grab_exts = crate::grabber::preset_exts(PRESETS[self.grab_preset]);
            }
            ButtonId::RuleAdd => {
                if self.folder_rules.len() < 8 {
                    self.folder_rules.push(crate::model::FolderRule {
                        exts: String::new(),
                        folder: String::new(),
                    });
                    self.save_ui_state();
                }
            }
            ButtonId::RuleReset => {
                self.folder_rules = crate::model::default_folder_rules();
                self.save_ui_state();
            }
            ButtonId::RuleDel(i) => {
                if i < self.folder_rules.len() {
                    self.folder_rules.remove(i);
                    self.save_ui_state();
                }
            }
            ButtonId::RuleUp(i) => {
                if i > 0 && i < self.folder_rules.len() {
                    self.folder_rules.swap(i - 1, i);
                    self.save_ui_state();
                }
            }
            ButtonId::RuleDown(i) => {
                if i + 1 < self.folder_rules.len() {
                    self.folder_rules.swap(i, i + 1);
                    self.save_ui_state();
                }
            }
            ButtonId::WinAdd => {
                if self.speed_windows.len() < 6 {
                    self.speed_windows.push(crate::model::SpeedWindow {
                        start: "02:00".to_string(),
                        end: "06:00".to_string(),
                        limit_kbps: 0,
                    });
                    self.save_ui_state();
                }
            }
            ButtonId::WinDel(i) => {
                if i < self.speed_windows.len() {
                    self.speed_windows.remove(i);
                    self.save_ui_state();
                }
            }
            ButtonId::UpdCheck => self.start_update_check(),
            ButtonId::UpdDownload => {
                self.update_auto = false;
                self.start_self_update();
            }
            ButtonId::UpdRestart => self.restart_for_update(),
            ButtonId::QStart => self.start_queue(),
            ButtonId::QStop => self.stop_queue(),
            ButtonId::QSchedReset => self.scheduler_started = false,
            ButtonId::QClose => {
                self.show_queues = false;
                self.save_ui_state();
            }
            ButtonId::SFolder => {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.add_dir = p.to_string_lossy().to_string();
                    self.save_ui_state();
                }
            }
            ButtonId::SChrome => self.pending_browser_install = Some("chrome".to_string()),
            ButtonId::SEdge => self.pending_browser_install = Some("edge".to_string()),
            ButtonId::SFirefox => self.pending_browser_install = Some("firefox".to_string()),
            ButtonId::SBrave => self.pending_browser_install = Some("brave".to_string()),
            ButtonId::SOpera => self.pending_browser_install = Some("opera".to_string()),
            ButtonId::SClose => {
                // Legacy in-app Settings close (Settings now lives in its own
                // window; kept so the match stays exhaustive).
                if self.add_dir.trim().is_empty() {
                    self.add_dir = dirs::download_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| crate::model::default_download_dir().to_string_lossy().into_owned());
                }
                self.save_ui_state();
            }
            ButtonId::ThemeSystem => {
                self.theme = "system".to_string();
                self.save_ui_state();
            }
            ButtonId::ThemeDark => {
                self.theme = "dark".to_string();
                self.save_ui_state();
            }
            ButtonId::ThemeLight => {
                self.theme = "light".to_string();
                self.save_ui_state();
            }
            ButtonId::DResume => {
                if let Some(did) = self.details_id {
                    let paused = matches!(
                        self.downloads.iter().find(|d| d.id == did).map(|d| &d.status),
                        Some(Status::Paused)
                    );
                    if paused {
                        self.resume(did);
                    } else {
                        self.pause(did);
                    }
                }
            }
            ButtonId::DClose => {
                self.save_queue();
                self.details_id = None;
                self.det_for = None;
            }
            ButtonId::DOpen => {
                if let Some(did) = self.details_id {
                    if let Some(d) = self.downloads.iter().find(|d| d.id == did) {
                        let path = d.dest_path();
                        // Finished file -> open it; gone/partial -> show the
                        // folder it lives (or will live) in.
                        open_path(&path, !path.exists());
                    }
                }
            }
            ButtonId::DCopyAddr => {
                let url = self
                    .details_id
                    .and_then(|id| self.downloads.iter().find(|d| d.id == id))
                    .map(|d| d.url.clone());
                if let Some(url) = url {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(url);
                    }
                    self.push_log("Address copied to clipboard.");
                }
            }
            ButtonId::DCopyPage => {
                let url = self
                    .details_id
                    .and_then(|id| self.downloads.iter().find(|d| d.id == id))
                    .map(|d| page_url_of(&d.referer))
                    .filter(|u| !u.is_empty());
                if let Some(url) = url {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(url);
                    }
                    self.push_log("Web page copied to clipboard.");
                }
            }
            ButtonId::DMove => {
                if let Some(did) = self.details_id {
                    let info = self.downloads.iter().find(|d| d.id == did).map(|d| {
                        (
                            d.dest_path(),
                            d.filename.clone(),
                            d.save_dir.clone(),
                            matches!(d.status, Status::Completed),
                        )
                    });
                    if let Some((src, name, old_dir, finished)) = info {
                        if !finished {
                            self.push_log("Move: only finished files can be moved.");
                        } else if !src.exists() {
                            self.push_log("Move: the file is no longer at its saved location.");
                        } else {
                            let mut dlg = rfd::FileDialog::new().set_title("Move file to folder");
                            if !old_dir.is_empty() {
                                dlg = dlg.set_directory(&old_dir);
                            }
                            if let Some(dir) = dlg.pick_folder() {
                                let dst = dir.join(&name);
                                if dst == src {
                                    // Same folder: nothing to move.
                                } else if dst.exists() {
                                    self.push_log("Move: a file with that name already exists there.");
                                } else {
                                    let moved = std::fs::rename(&src, &dst).or_else(|_| {
                                        std::fs::copy(&src, &dst)
                                            .and_then(|_| std::fs::remove_file(&src))
                                    });
                                    match moved {
                                        Ok(_) => {
                                            if let Some(d) =
                                                self.downloads.iter_mut().find(|d| d.id == did)
                                            {
                                                d.save_dir = dir.to_string_lossy().to_string();
                                            }
                                            self.save_queue();
                                            self.push_log(format!(
                                                "Moved {name} to {}.",
                                                dir.to_string_lossy()
                                            ));
                                        }
                                        Err(e) => self.push_log(format!("Move failed: {e}")),
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ButtonId::DVerify => {
                if let Some(did) = self.details_id {
                    self.verify_checksum(did);
                }
            }
            ButtonId::AbClose => self.show_about = false,
        }
    }

    /// Generic manual popup list. Records item hits; clicks dispatch from
    /// process_hits. Empty labels render as separators.
    #[allow(clippy::too_many_lines)]
    fn render_popup(
        &mut self,
        ctx: &egui::Context,
        area_id: &str,
        pos: egui::Pos2,
        width: f32,
        rows: Vec<(String, Option<HitKind>)>,
    ) {
        const H: f32 = 28.0;
        let h: f32 = rows
            .iter()
            .map(|(l, _)| if l.is_empty() { 9.0 } else { H })
            .sum::<f32>()
            + 12.0;
        let scr = ctx.screen_rect();
        let pos = egui::pos2(
            pos.x.min(scr.max.x - width - 8.0).max(8.0),
            pos.y.min(scr.max.y - h - 8.0).max(8.0),
        );
        egui::Area::new(egui::Id::new(area_id))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pl().modal_bg)
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        pl().modal_bd,
                    ))
                    .corner_radius(RADIUS)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (label, hit) in rows {
                            if label.is_empty() {
                                ui.add_space(3.0);
                                ui.separator();
                                ui.add_space(3.0);
                                continue;
                            }
                            let (r, _) = ui.allocate_exact_size(
                                egui::vec2(width - 12.0, H),
                                egui::Sense::hover(),
                            );
                            if self.man.hover(&r) {
                                ui.painter().rect_filled(r, 4.0, pl().surface_lt);
                            }
                            let (checked, label) = match label.strip_prefix(POPUP_CHECK) {
                                Some(rest) => (Some(true), rest.to_string()),
                                None => match label.strip_prefix(POPUP_INDENT) {
                                    Some(rest) => (Some(false), rest.to_string()),
                                    None => (None, label),
                                },
                            };
                            let mut tx = r.min.x + 10.0;
                            if let Some(on) = checked {
                                if on {
                                    ui.painter().text(
                                        egui::pos2(r.min.x + 17.0, r.center().y),
                                        egui::Align2::CENTER_CENTER,
                                        MI_CHECK,
                                        crate::fonts::icon(15.0),
                                        pl().primary,
                                    );
                                }
                                tx = r.min.x + 30.0;
                            }
                            ui.painter().text(
                                egui::pos2(tx, r.center().y),
                                egui::Align2::LEFT_CENTER,
                                label,
                                egui::FontId::proportional(12.5),
                                pl().txt,
                            );
                            if let Some(h) = hit {
                                self.hits.push((h, r));
                            }
                        }
                    });
            });
    }

    fn render_drop_menu(&mut self, ctx: &egui::Context, menu: u8) {
        // Position below the menubar button (recorded this frame).
        let pos = self
            .hits
            .iter()
            .find(|(k, _)| matches!(k, HitKind::MenuBtn(m) if *m == menu))
            .map(|(_, r)| egui::pos2(r.min.x, r.max.y + 4.0))
            .unwrap_or(egui::pos2(100.0, 100.0));
        let rows = self.drop_rows(menu);
        self.render_popup(ctx, "drop_menu", pos, 240.0, rows);
    }

    /// What the right-click menu offers for row `id`, grouped, with icons,
    /// the shortcuts the list really has, and two submenus.
    fn ctx_entries(&self, id: u64, shift: bool) -> Vec<CtxEntry> {
        use RowAct::*;
        let t = |s: &str| crate::i18n::t(s).to_string();
        let item = |label: &str, icon: &'static str, hint: &'static str, act: RowAct| CtxEntry::Item {
            label: t(label),
            icon,
            hint,
            act,
            danger: false,
        };
        let Some(d) = self.downloads.iter().find(|d| d.id == id) else { return Vec::new() };
        let ext = std::path::Path::new(&d.filename)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut v = Vec::new();
        if self.tasks.contains_key(&id) {
            v.push(item("Show download window", MI_SPEED, "", ShowProgress));
            v.push(CtxEntry::Sep);
        }
        v.extend([
            item("Open", MI_OPEN, "Enter", OpenFile),
            item("Open folder", MI_FOLDER_OPEN, "", OpenFolder),
            item("Details", MI_INFO, "", Details),
            CtxEntry::Sep,
            item("Resume / Retry", MI_PLAY, "Space", Resume),
            item("Pause", MI_PAUSE, "", Pause),
            item("Redownload", MI_REPLAY, "", Redownload),
        ]);
        const MEDIA: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "flv", "wmv", "m4v", "ts", "3gp", "mp3", "m4a", "aac", "opus", "ogg", "wav", "flac", "wma"];
        const AUDIO: &[&str] = &["mp3", "m4a", "aac", "opus", "ogg", "wav", "flac", "wma"];
        if matches!(d.status, Status::Completed) && MEDIA.contains(&ext.as_str()) {
            let mut sub = Vec::new();
            if ext != "mp3" {
                sub.push((t("Audio only"), "MP3", Convert(0)));
            }
            if !AUDIO.contains(&ext.as_str()) {
                if ext != "mp4" {
                    sub.push((t("Same quality"), "MP4", Convert(1)));
                }
                sub.push((t("Smaller file"), "720p", Convert(2)));
            }
            if !sub.is_empty() {
                v.push(CtxEntry::Sub { label: t("Convert"), icon: MI_TRANSFORM, key: 0, items: sub });
            }
        }
        if matches!(d.status, Status::Completed) && crate::archive::volume_of(&d.filename).is_some() {
            v.push(item("Extract", MI_UNARCHIVE, "", Extract));
        }
        if ext == "zip" && d.url.starts_with("http") {
            v.push(item("ZIP: pick files…", MI_UNARCHIVE, "", ZipPick));
        }
        if !self.queues.is_empty() {
            let mut sub = vec![(t("Main"), "", MoveToQueue(0))];
            for (i, q) in self.queues.iter().enumerate().take(20) {
                sub.push((q.name.clone(), "", MoveToQueue(i as u8 + 1)));
            }
            v.push(CtxEntry::Sub { label: t("Move to queue"), icon: MI_PLAYLIST_ADD, key: 1, items: sub });
        }
        v.push(CtxEntry::Sep);
        v.push(item("Copy URL", MI_LINK, "", CopyUrl));
        v.push(item("Refresh download address", MI_REFRESH, "", RefreshAddress));
        v.push(item("Verify checksum", MI_VERIFIED, "", VerifyChecksum));
        v.push(CtxEntry::Sep);
        let _ = shift;
        v.push(CtxEntry::Item { label: t("Remove"), icon: MI_DELETE, hint: "Del", act: Remove, danger: true });
        v.push(CtxEntry::Item { label: t("Remove + delete file"), icon: MI_DELETE_FOREVER, hint: "", act: RemoveFile, danger: true });
        v
    }

    fn render_ctx_menu(&mut self, ctx: &egui::Context, id: u64, pos: egui::Pos2) {
        const W: f32 = 214.0;
        const ROW: f32 = 26.0;
        const SEP: f32 = 9.0;
        const PAD: f32 = 5.0;
        let shift = ctx.input(|i| i.modifiers.shift);
        let entries = self.ctx_entries(id, shift);
        let h: f32 = entries.iter().map(|e| if matches!(e, CtxEntry::Sep) { SEP } else { ROW }).sum::<f32>() + PAD * 2.0 + 2.0;
        let scr = ctx.screen_rect();
        let pos = egui::pos2(pos.x.min(scr.max.x - W - 8.0).max(8.0), pos.y.min(scr.max.y - h - 8.0).max(8.0));
        let mut sub_at: Option<(egui::Rect, Vec<(String, &'static str, RowAct)>)> = None;
        let open_sub = self.ctx_sub;
        egui::Area::new(egui::Id::new("ctx_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(W, h), egui::Sense::hover());
                let p = ui.painter();
                p.rect(rect, 10.0, pl().modal_bg, egui::Stroke::new(1.0_f32, pl().modal_bd), egui::StrokeKind::Inside);
                let mut y = rect.min.y + PAD + 1.0;
                for e in entries {
                    match e {
                        CtxEntry::Sep => {
                            p.hline(
                                (rect.min.x + PAD + 6.0)..=(rect.max.x - PAD - 6.0),
                                y + SEP / 2.0,
                                egui::Stroke::new(1.0_f32, pl().card_bd),
                            );
                            y += SEP;
                        }
                        CtxEntry::Item { label, icon, hint, act, danger } => {
                            let r = egui::Rect::from_min_size(egui::pos2(rect.min.x + PAD, y), egui::vec2(W - PAD * 2.0, ROW));
                            let hov = self.man.hover(&r);
                            if hov {
                                p.rect_filled(r, 6.0, pl().surface_lt);
                                self.ctx_sub = None;
                            }
                            let col = if danger { pl().error } else if hov { pl().txt } else { pl().txt.gamma_multiply(0.85) };
                            ctx_row(p, r, icon, &label, hint, col, false);
                            self.hits.push((HitKind::CtxItem(act, id), r));
                            y += ROW;
                        }
                        CtxEntry::Sub { label, icon, key, items } => {
                            let r = egui::Rect::from_min_size(egui::pos2(rect.min.x + PAD, y), egui::vec2(W - PAD * 2.0, ROW));
                            let on = self.man.hover(&r) || open_sub == Some(key);
                            if on {
                                p.rect_filled(r, 6.0, pl().surface_lt);
                            }
                            ctx_row(p, r, icon, &label, "", pl().txt, true);
                            self.hits.push((HitKind::CtxSub(key), r));
                            if open_sub == Some(key) {
                                sub_at = Some((r, items));
                            }
                            y += ROW;
                        }
                    }
                }
            });
        // The open submenu, beside its row (to the left when there is no
        // room on the right).
        if let Some((row, items)) = sub_at {
            const SW: f32 = 196.0;
            let sh = items.len() as f32 * ROW + PAD * 2.0 + 2.0;
            let mut x = row.max.x + PAD + 2.0;
            if x + SW > scr.max.x - 8.0 {
                x = (pos.x - SW - 2.0).max(8.0);
            }
            let y = (row.min.y - PAD - 1.0).min(scr.max.y - sh - 8.0).max(8.0);
            egui::Area::new(egui::Id::new("ctx_sub_menu"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(x, y))
                .show(ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(SW, sh), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect(rect, 10.0, pl().modal_bg, egui::Stroke::new(1.0_f32, pl().modal_bd), egui::StrokeKind::Inside);
                    let mut y = rect.min.y + PAD + 1.0;
                    for (label, tag, act) in items {
                        let r = egui::Rect::from_min_size(egui::pos2(rect.min.x + PAD, y), egui::vec2(SW - PAD * 2.0, ROW));
                        let hov = self.man.hover(&r);
                        if hov {
                            p.rect_filled(r, 6.0, pl().surface_lt);
                        }
                        let mut tx = r.min.x + 10.0;
                        if !tag.is_empty() {
                            p.text(egui::pos2(tx, r.center().y), egui::Align2::LEFT_CENTER, tag, egui::FontId::proportional(10.0), pl().primary);
                            tx += 36.0;
                        }
                        let name = truncate_to_width(p, &label, egui::FontId::proportional(12.0), pl().txt, r.max.x - tx - 8.0);
                        crate::shaped::draw(p, egui::pos2(tx, r.center().y), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(12.0), if hov { pl().txt } else { pl().txt.gamma_multiply(0.85) });
                        self.hits.push((HitKind::CtxItem(act, id), r));
                        y += ROW;
                    }
                });
        }
    }

    /// Match recorded hit rects (reverse = topmost first) against manual
    /// pointer state and dispatch. The single place anything "clicks".
    fn process_hits(&mut self, ctx: &egui::Context) {
        // A divider drag is over once the button is up and its release frame
        // (which still holds the capture) has passed.
        if self.resize_cap.is_none() && !self.man.down_left() {
            self.col_drag_used = false;
        }
        // Active divider drag continues here (movement, not clicks).
        if let Some(cap) = self.resize_cap.take() {
            match cap {
                ResizeCap::Col { col, start_x, start_w } => {
                    if self.man.down_left() {
                        if let Some(p) = self.man.pos() {
                            self.col_w.insert(col, dragged_width(col, start_w, p.x - start_x));
                        }
                        self.resize_cap = Some(cap);
                    } else {
                        self.save_ui_state();
                    }
                }
            }
        }
        // Drag finished downloads out of the list, as from Explorer: onto
        // a folder, the desktop, a chat. A press that began on a finished
        // row and moved a little starts it; a selected row takes every
        // selected finished file along.
        if !self.man.down_left() {
            self.drag_out_done = false;
        }
        if self.man.down_left() && !self.drag_out_done && self.resize_cap.is_none() && !self.show_add && self.details_id.is_none() {
            if let (Some(o), Some(p)) = (self.man.press_origin(), self.man.pos()) {
                if o.distance(p) > 8.0 {
                    let row = self.hits.iter().rev().find_map(|(k, r)| match k {
                        HitKind::Row(id) if r.contains(o) => Some(*id),
                        _ => None,
                    });
                    if let Some(id) = row {
                        self.drag_out_done = true;
                        let selected = self.downloads.iter().find(|d| d.id == id).map(|d| d.selected).unwrap_or(false);
                        let files: Vec<std::path::PathBuf> = self
                            .downloads
                            .iter()
                            .filter(|d| if selected { d.selected } else { d.id == id })
                            .filter(|d| matches!(d.status, Status::Completed))
                            .map(|d| d.dest_path())
                            .filter(|p| p.is_file())
                            .collect();
                        if !files.is_empty() {
                            crate::platform::drag_files_out(&files);
                        }
                    }
                }
            }
        }
        // Modal dialogs block main-UI hits beneath them; popups stay live.
        let modal_open = self.show_add
            || self.show_queues
            || self.show_about
            || self.details_id.is_some();
        let mut hover_dbg = "-".to_string();
        let mut fired_any = false;
        let mut fired_menu = false;
        let mut fired_ctx = false;
        let mut open_ctx: Option<(u64, egui::Pos2)> = None;
        let mut open_drop: Option<u8> = None;
        let mut close_drop = false;
        // Snapshot hover for cursor icons (resize zones).
        let mut zone_cursor: Option<egui::CursorIcon> = None;
        for (kind, rect) in self.hits.clone().into_iter().rev() {
            // When a modal dialog is open, main-list hits beneath it are
            // blocked — but the dialog's OWN controls must stay live.
            // Otherwise every Close/Pause/slider inside Queues/Details/About
            // is skipped and the buttons look dead. Block only the main
            // window surface; allow popups and all dialog kinds.
            let dialog_kind = matches!(
                kind,
                HitKind::MenuItem(_)
                    | HitKind::CtxItem(_, _)
                    | HitKind::CtxSub(_)
                    | HitKind::Btn(_)
                    | HitKind::Check(_)
                    | HitKind::Slider(..)
                    | HitKind::Field(_)
                    | HitKind::ComboAnchor
                    | HitKind::QMove(_, _)
            );
            if modal_open && !dialog_kind && !matches!(kind, HitKind::Tool(ToolId::Settings)) {
                continue;
            }
            let hov = self.man.hover(&rect);
            let clk = self.man.clicked(&rect);
            let sec = self.man.secondary(&rect);
            if hov && hover_dbg == "-" {
                hover_dbg = match &kind {
                    HitKind::Tool(t) => format!("tool:{t:?}"),
                    HitKind::SideRow(k) => format!("side:{k}"),
                    HitKind::SideToggle(n) => format!("sidetoggle:{n}"),
                    HitKind::Row(id) => format!("row:#{id}"),
                    HitKind::RowCheck(id) => format!("check:#{id}"),
                    HitKind::HdrCol(c) => format!("hdr:{}", c.key()),
                    HitKind::HdrCheck => "hdrcheck".to_string(),
                    HitKind::ResizeCol(c) => format!("rz:{}", c.key()),
                    HitKind::MenuBtn(m) => format!("menu:{m}"),
                    HitKind::MenuItem(a) => format!("menuitem:{a}"),
                    HitKind::CtxItem(_, id) => format!("ctx:#{id}"),
                    HitKind::CtxSub(n) => format!("ctxsub:{n}"),
                    HitKind::Btn(_) => "btn".to_string(),
                    HitKind::Check(_) => "check".to_string(),
                    HitKind::Slider(..) => "slider".to_string(),
                    HitKind::Field(f) => format!("field:{f}"),
                    HitKind::ComboAnchor => "combo".to_string(),
                    HitKind::QMove(id, d) => format!("qmove:#{id}:{d}"),
                };
            }
            if clk || sec {
                fired_any = true;
            }
            match kind {
                HitKind::Tool(t) => {
                    if clk {
                        self.diag_acts += 1;
                        // Dimmed actions are inert (a disabled button must
                        // never act, even if its rect is clicked).
                        let idle = self.tasks.is_empty();
                        match t {
                            ToolId::AddUrl => {
                                self.add_url.clear();
                                self.add_file.clear();
                                self.add_checksum.clear();
                                self.add_quality.clear();
                                self.clear_add_credentials();
                                // A link picked up from the clipboard is
                                // pre-filled here: the lit Add URL button is
                                // the only affordance (no banner row).
                                if let Some(url) = self.clipboard_url.take() {
                                    self.add_file = filename_from_url(&url);
                                    self.add_url = url;
                                    self.add_focus_req = true;
                                }
                                self.show_add = true;
                            }
                            ToolId::StartQueue => self.start_queue(),
                            ToolId::StopQueue if !idle => self.stop_queue(),
                            ToolId::StopAll if !idle => self.stop_all(),
                            ToolId::StopQueue | ToolId::StopAll => {}
                            ToolId::Queues => self.show_queues = true,
                            ToolId::Settings => Self::open_settings_window(),
                            ToolId::Grabber => {
                                self.grab_notice.clear();
                                self.show_grabber = true;
                            }
                            ToolId::ResumeSel => {
                                // Resume what is checked; with nothing checked,
                                // every paused row (so the button never sits
                                // inert on a fresh window).
                                let mut ids = self.selected_ids();
                                if ids.is_empty() {
                                    ids = self
                                        .downloads
                                        .iter()
                                        .filter(|d| matches!(d.status, Status::Paused))
                                        .map(|d| d.id)
                                        .collect();
                                }
                                let n = ids.len();
                                for id in ids {
                                    self.resume(id);
                                }
                                if n > 0 {
                                    self.push_log(format!("Resumed {n} download(s)."));
                                }
                            }
                            ToolId::StopSel => {
                                // Pause what is checked; with nothing checked,
                                // every live transfer.
                                let mut ids = self.selected_ids();
                                if ids.is_empty() {
                                    ids = self.tasks.keys().copied().collect();
                                }
                                let n = ids.len();
                                for id in ids {
                                    self.pause(id);
                                }
                                if n > 0 {
                                    self.push_log(format!("Paused {n} download(s)."));
                                }
                            }
                            ToolId::Delete => {
                                let ids = self.selected_ids();
                                let n = ids.len();
                                for id in ids {
                                    self.remove(id);
                                }
                                self.push_log(format!("Removed {n} selected download(s)."));
                            }
                        }
                    }
                }
                HitKind::MenuBtn(m) => {
                    if clk {
                        fired_menu = true;
                        if self.drop_menu == Some(m) {
                            close_drop = true;
                        } else {
                            open_drop = Some(m);
                        }
                        self.ctx_menu = None;
                    }
                }
                HitKind::MenuItem(a) => {
                    if clk {
                        fired_menu = true;
                        self.drop_menu = None;
                        self.do_menu_act(ctx, a);
                    }
                }
                HitKind::SideRow(key) => {
                    if clk {
                        if key == "Queues" {
                            self.show_queues = true;
                        } else {
                            self.filter = key.clone();
                        }
                    }
                }
                HitKind::SideToggle(n) => {
                    if clk {
                        if n == 0 {
                            self.cats_open = !self.cats_open;
                        } else {
                            self.status_open = !self.status_open;
                        }
                    }
                }
                HitKind::RowCheck(id) => {
                    if clk {
                        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                            d.selected = !d.selected;
                        }
                    }
                }
                HitKind::QMove(id, dir) => {
                    if clk {
                        self.move_in_queue(id, dir);
                    }
                }
                HitKind::Row(id) => {
                    if sec {
                        fired_ctx = true;
                        // Keep a multi-selection intact when right-clicking a
                        // row that is already checked; otherwise select just
                        // the clicked row.
                        let already = self
                            .downloads
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| d.selected)
                            .unwrap_or(false);
                        if !already {
                            for d in self.downloads.iter_mut() {
                                d.selected = d.id == id;
                            }
                        }
                        if let Some(p) = self.man.pos() {
                            open_ctx = Some((id, p));
                        }
                    } else if clk {
                        for d in self.downloads.iter_mut() {
                            d.selected = d.id == id;
                        }
                    }
                    if clk && self.man.take_double() {
                        // A running download opens its progress window (as
                        // IDM does); anything else its details.
                        let act = if self.tasks.contains_key(&id) { RowAct::ShowProgress } else { RowAct::Details };
                        self.do_row_act(act, id);
                    }
                }
                HitKind::HdrCol(c) => {
                    if clk && !self.col_drag_used {
                        if self.sort_col == c {
                            self.sort_asc = !self.sort_asc;
                        } else {
                            self.sort_col = c;
                            self.sort_asc = true;
                        }
                    }
                }
                HitKind::HdrCheck => {
                    if clk {
                        let ids = self.visible_ids();
                        let all = !ids.is_empty()
                            && ids.iter().all(|id| {
                                self.downloads
                                    .iter()
                                    .find(|d| d.id == *id)
                                    .map(|d| d.selected)
                                    .unwrap_or(false)
                            });
                        for id in &ids {
                            if let Some(d) =
                                self.downloads.iter_mut().find(|d| d.id == *id)
                            {
                                d.selected = !all;
                            }
                        }
                    }
                }
                HitKind::ResizeCol(c) => {
                    if hov {
                        zone_cursor = Some(egui::CursorIcon::ResizeColumn);
                    }
                    if self.man.press_started_in(&rect) {
                        if let Some(p) = self.man.pos() {
                            self.col_drag_used = true;
                            // From the width on screen: Name may still be
                            // filling the window, with no stored width yet.
                            let shown = self.col_shown.iter().find(|(k, _)| *k == c).map(|(_, w)| *w);
                            self.resize_cap = Some(ResizeCap::Col {
                                col: c,
                                start_x: p.x,
                                start_w: shown.unwrap_or_else(|| col_w_of(&self.col_w, c)),
                            });
                        }
                    }
                }
                HitKind::CtxItem(a, id) => {
                    if clk {
                        fired_ctx = true;
                        self.ctx_menu = None;
                        self.ctx_sub = None;
                        self.do_row_act(a, id);
                    }
                }
                HitKind::CtxSub(n) => {
                    if hov {
                        self.ctx_sub = Some(n);
                    }
                    if clk {
                        fired_ctx = true;
                        self.ctx_sub = Some(n);
                    }
                }
                HitKind::Btn(b) => {
                    if clk {
                        self.do_button(b);
                    }
                }
                HitKind::Check(c) => {
                    if clk {
                        self.diag_acts += 1;
                        match c {
                            CheckId::CatFold => {
                                self.category_folders = !self.category_folders
                            }
                            CheckId::Tray => self.close_to_tray = !self.close_to_tray,
                            CheckId::Unlimited => {
                                self.speed_limit_kbps =
                                    if self.speed_limit_kbps == 0 { 1024 } else { 0 }
                            }
                            CheckId::SchedOn => {
                                self.scheduler_enabled = !self.scheduler_enabled
                            }
                            CheckId::AddOverwrite => {
                                self.add_overwrite = !self.add_overwrite
                            }
                            CheckId::GrabDepth => self.grab_depth = !self.grab_depth,
                            CheckId::GrabStay => self.grab_stay = !self.grab_stay,
                            CheckId::WinOn => {
                                self.speed_windows_on = !self.speed_windows_on;
                                self.refresh_speed_limit();
                            }
                            CheckId::UpdOn => self.update_check_on = !self.update_check_on,
                        }
                        self.save_ui_state();
                    }
                }
                HitKind::Slider(id, min, max, log) => {
                    if self.man.press_started_in(&rect) {
                        if let Some(p) = self.man.pos() {
                            let (tx0, tw) = slider_track(&rect);
                            let frac = ((p.x - tx0) / tw).clamp(0.0, 1.0) as f64;
                            self.slider_set(id, Self::frac_to_val(frac, min, max, log));
                            self.slider_cap = Some(SliderCap {
                                id,
                                min,
                                max,
                                log,
                                x0: tx0,
                                tw,
                            });
                        }
                    }
                }
                HitKind::Field(fid) => {
                    if clk {
                        focus_field(ctx, fid);
                    }
                }
                HitKind::ComboAnchor => {
                    if clk {
                        fired_menu = true;
                        self.finish_open = !self.finish_open;
                    }
                }
            }
        }
        if let Some(cur) = zone_cursor {
            ctx.set_cursor_icon(cur);
        }
        if let Some((id, pos)) = open_ctx {
            self.ctx_menu = Some((id, pos));
        }
        if let Some(m) = open_drop {
            self.drop_menu = Some(m);
            self.ctx_menu = None;
        }
        if close_drop {
            self.drop_menu = None;
        }
        // Clicks elsewhere dismiss open popups (standard behavior).
        if fired_any && !fired_menu && !fired_ctx {
            self.drop_menu = None;
            self.ctx_menu = None;
        }
        self.man_hover_dbg = hover_dbg;
    }
}

/// Byte counts cannot override a failed audio/duration/checksum validation.
fn task_can_complete(shared: &SharedState, bytes_landed: bool) -> bool {
    bytes_landed && shared.succeeded.load(Ordering::SeqCst)
        && !shared.cancelled.load(Ordering::SeqCst)
}
#[cfg(test)]
mod audit_completion_tests {
    #[test]
    fn column_dividers_follow_the_pointer() {
        use super::{dragged_width, col_min, SortCol};
        // Only the dragged column changes; the others keep their widths and
        // move along (IDM).
        assert_eq!(dragged_width(SortCol::Size, 90.0, 15.0), 105.0);
        assert_eq!(dragged_width(SortCol::Name, 300.0, 120.0), 420.0);
        // Never below the column's minimum, never absurdly wide.
        assert_eq!(dragged_width(SortCol::Size, 90.0, -500.0), col_min(SortCol::Size));
        assert_eq!(dragged_width(SortCol::Status, 110.0, 5000.0), 900.0);
    }
    use super::*;
    #[test]
    fn dead_links_open_the_refresh_window_and_other_failures_do_not() {
        assert!(dead_link_reason("error sending request for url (https://x/f.zip): client error (Connect): dns error: No such host is known. (os error 11001)").unwrap().contains("no longer exists"));
        assert!(dead_link_reason("server answered 404 Not Found").unwrap().contains("404"));
        assert!(dead_link_reason("HTTP 403 Forbidden").unwrap().contains("403"));
        assert!(dead_link_reason("connection ended early (got 1 of 2) — resume to retry").is_none());
        assert!(dead_link_reason("not enough disk space").is_none());
    }
    #[test]
    fn validation_failure_never_becomes_completed_from_full_byte_count() {
        let s=SharedState::new();
        s.downloaded.store(100,Ordering::SeqCst);
        s.total.store(100,Ordering::SeqCst);
        s.set_error("finished media has no audio track");
        assert!(!task_can_complete(&s,true));
        let success=SharedState::new();
        success.succeeded.store(true,Ordering::SeqCst);
        assert!(task_can_complete(&success,true));
        assert!(!task_can_complete(&success,false));
        success.cancelled.store(true,Ordering::SeqCst);
        assert!(!task_can_complete(&success,true));
    }
}
