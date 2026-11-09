//! The looks the Theme menu offers besides Download Craft's own:
//!
//! - **Ledger**: a desktop-classic look: grey window, white list with grid
//!   lines, a large labelled toolbar, the Windows blue accent, square corners.
//! - **Petal**: an airy card look: white surfaces on a light grey page,
//!   summary tiles, downloads as cards, the blue accent, round corners.
//!
//! Both are written here from scratch (palettes, layouts, drawing). Each look has a light and a
//! dark side; the mode (system, dark, light) chooses between them.
//!
//! A theme key is `"<mode>"` (Download Craft's look) or `"<look>:<mode>"`.

use crate::app::{Palette, DARK_PAL, LIGHT_PAL};
use egui::Color32;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A colour at alpha `a` out of 255, premultiplied (as `Color32` stores it).
const fn wash(hex: u32, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(pm(hex >> 16, a), pm(hex >> 8, a), pm(hex, a), a)
}

const fn pm(c: u32, a: u8) -> u8 {
    ((c & 0xFF) * a as u32 / 255) as u8
}

/// Looks in the order the menus list them: (key, name).
pub const LOOKS: [(&str, &str); 3] = [("craft", "Craft"), ("ledger", "Ledger"), ("petal", "Petal")];

/// `"petal:light"` → `("petal", "light")`; a bare mode is Download Craft's look.
pub fn split(theme: &str) -> (&str, &str) {
    match theme.split_once(':') {
        Some((look, mode)) if LOOKS.iter().any(|(k, _)| *k == look) || look.starts_with("user.") => (look, mode),
        Some((_, mode)) => ("craft", mode),
        None => ("craft", theme),
    }
}

/// The key for `look` in `mode` (Download Craft's look keeps the bare mode,
/// so older settings and other windows read it as before).
pub fn join(look: &str, mode: &str) -> String {
    let mode = if mode.is_empty() { "system" } else { mode };
    if look == "craft" || look.is_empty() {
        mode.to_string()
    } else {
        // Pure black belongs to Download Craft's look only.
        let mode = if mode == "black" { "dark" } else { mode };
        format!("{look}:{mode}")
    }
}

/// The palette for `look` on its dark or light side.
pub fn palette(look: &str, dark: bool) -> Palette {
    // A theme file (src/theme_files.rs): its colours over its layout.
    if look.starts_with("user.") {
        return crate::theme_files::load(look).map(|t| t.palette(dark)).unwrap_or(if dark { DARK_PAL } else { LIGHT_PAL });
    }
    match (look, dark) {
        ("ledger", false) => LEDGER_LIGHT,
        ("ledger", true) => LEDGER_DARK,
        ("petal", false) => PETAL_LIGHT,
        ("petal", true) => PETAL_DARK,
        (_, true) => DARK_PAL,
        (_, false) => LIGHT_PAL,
    }
}

pub const LEDGER_LIGHT: Palette = Palette {
    bg: rgb(0xF0F0F0),
    surface: rgb(0xFFFFFF),
    surface_lt: rgb(0xE5F1FB),
    header_bg: rgb(0xF0F0F0),
    central_bg: rgb(0xFFFFFF),
    card_bd: rgb(0xD0D0D0),
    primary: rgb(0x0078D4),
    secondary: rgb(0x005A9E),
    txt: rgb(0x000000),
    dim: rgb(0x5F5F5F),
    success: rgb(0x0F7B0F),
    warning: rgb(0x9D5D00),
    error: rgb(0xC42B1C),
    sel_bg: wash(0x0078D4, 48),
    sel_bd: rgb(0x0078D4),
    hover_bg: wash(0x0078D4, 22),
    modal_bg: rgb(0xFFFFFF),
    modal_bd: rgb(0xC8C8C8),
    scrim: wash(0x000000, 70),
    bar_bd: wash(0x000000, 30),
    track: rgb(0xE6E6E6),
    notice: rgb(0xB04A08),
    dm_card: rgb(0xFFFFFF),
    dm_line: rgb(0xE5E5E5),
    dm_box_bd: rgb(0xC8C8C8),
    dm_txt: rgb(0x000000),
    dm_dim: rgb(0x5F5F5F),
    dm_hover: rgb(0xE5F1FB),
    dm_hover2: rgb(0xCCE4F7),
    dm_trough: rgb(0xF0F0F0),
    dm_lbl: rgb(0x6D6D6D),
    dm_head: rgb(0x6D6D6D),
    dm_warn: rgb(0xC42B1C),
    cg_bg: rgb(0xF0F0F0),
    cg_box_bg: rgb(0xFFFFFF),
    cg_box_bd: rgb(0xC8C8C8),
    cg_hover: rgb(0xE5F1FB),
    acc: rgb(0x0078D4),
    acc2: rgb(0x0078D4),
    radius: 3.0,
    done: rgb(0x0F7B0F),
    fail: rgb(0xC42B1C),
    ..LIGHT_PAL
};

pub const LEDGER_DARK: Palette = Palette {
    bg: rgb(0x202020),
    surface: rgb(0x2B2B2B),
    surface_lt: rgb(0x3C3C3C),
    header_bg: rgb(0x191919),
    central_bg: rgb(0x2B2B2B),
    card_bd: rgb(0x4B4B4B),
    primary: rgb(0x2E8AE6),
    secondary: rgb(0x60CDFF),
    txt: rgb(0xFFFFFF),
    dim: rgb(0xABABAB),
    success: rgb(0x6CCB5F),
    warning: rgb(0xF7B040),
    error: rgb(0xFF9999),
    sel_bg: wash(0x0078D4, 110),
    sel_bd: rgb(0x0078D4),
    hover_bg: wash(0xFFFFFF, 16),
    modal_bg: rgb(0x2B2B2B),
    modal_bd: rgb(0x4B4B4B),
    scrim: wash(0x000000, 150),
    bar_bd: wash(0xFFFFFF, 20),
    track: rgb(0x4A4A4A),
    notice: rgb(0xFFA080),
    dm_card: rgb(0x2B2B2B),
    dm_line: rgb(0x3A3A3A),
    dm_box_bd: rgb(0x555555),
    dm_txt: rgb(0xF0F0F0),
    dm_dim: rgb(0xABABAB),
    dm_hover: rgb(0x2D4A60),
    dm_hover2: rgb(0x3C3C3C),
    dm_trough: rgb(0x1E1E1E),
    dm_lbl: rgb(0x8A8A8A),
    dm_head: rgb(0x8A8A8A),
    dm_warn: rgb(0xFF9999),
    cg_bg: rgb(0x2B2B2B),
    cg_bd: rgb(0x4B4B4B),
    cg_box_bg: rgb(0x333333),
    cg_box_bd: rgb(0x4B4B4B),
    cg_btn_bd: rgb(0x555555),
    cg_hover: rgb(0x3C3C3C),
    acc: rgb(0x0078D4),
    acc2: rgb(0x0078D4),
    radius: 3.0,
    done: rgb(0x6CCB5F),
    fail: rgb(0xFF9999),
    ..DARK_PAL
};

pub const PETAL_LIGHT: Palette = Palette {
    bg: rgb(0xF7F7F7),
    surface: rgb(0xFFFFFF),
    surface_lt: rgb(0xF2F2F2),
    header_bg: rgb(0xFFFFFF),
    central_bg: rgb(0xFFFFFF),
    card_bd: rgb(0xEBEBEB),
    primary: rgb(0x2563EB),
    secondary: rgb(0x1D4ED8),
    txt: rgb(0x222222),
    dim: rgb(0x6A6A6A),
    success: rgb(0x0A7A30),
    warning: rgb(0x8A4B00),
    error: rgb(0xB32D0E),
    sel_bg: wash(0x2563EB, 30),
    sel_bd: rgb(0x2563EB),
    hover_bg: wash(0x000000, 13),
    modal_bg: rgb(0xFFFFFF),
    modal_bd: rgb(0xDDDDDD),
    scrim: wash(0x000000, 102),
    bar_bd: wash(0x000000, 20),
    track: rgb(0xEBEBEB),
    notice: rgb(0xB04A08),
    dm_card: rgb(0xFFFFFF),
    dm_line: rgb(0xEBEBEB),
    dm_box_bd: rgb(0xDDDDDD),
    dm_txt: rgb(0x222222),
    dm_dim: rgb(0x6A6A6A),
    dm_hover: rgb(0xF2F2F2),
    dm_hover2: rgb(0xE8E8E8),
    dm_trough: rgb(0xF2F2F2),
    dm_lbl: rgb(0x6A6A6A),
    dm_head: rgb(0x6A6A6A),
    dm_warn: rgb(0xB32D0E),
    cg_bg: rgb(0xFFFFFF),
    cg_bd: rgb(0xEBEBEB),
    cg_txt: rgb(0x222222),
    cg_box_bg: rgb(0xF7F7F7),
    cg_box_bd: rgb(0xDDDDDD),
    cg_hover: rgb(0xF2F2F2),
    acc: rgb(0x2563EB),
    acc2: rgb(0x3B82F6),
    radius: 14.0,
    done: rgb(0x0A7A30),
    fail: rgb(0xB32D0E),
    ..LIGHT_PAL
};

pub const PETAL_DARK: Palette = Palette {
    bg: rgb(0x0F0F10),
    surface: rgb(0x1C1C1E),
    surface_lt: rgb(0x2C2C2E),
    header_bg: rgb(0x1C1C1E),
    central_bg: rgb(0x1C1C1E),
    card_bd: rgb(0x333335),
    primary: rgb(0x3B82F6),
    secondary: rgb(0x60A5FA),
    txt: rgb(0xF5F5F7),
    dim: rgb(0xC7C7CC),
    success: rgb(0x30D158),
    warning: rgb(0xFF9F0A),
    error: rgb(0xFF8A70),
    sel_bg: wash(0x3B82F6, 46),
    sel_bd: rgb(0x3B82F6),
    hover_bg: wash(0xFFFFFF, 18),
    modal_bg: rgb(0x262628),
    modal_bd: rgb(0x3A3A3C),
    scrim: wash(0x000000, 153),
    bar_bd: wash(0xFFFFFF, 26),
    track: rgb(0x3A3A3C),
    notice: rgb(0xFFA080),
    dm_card: rgb(0x1C1C1E),
    dm_line: rgb(0x2C2C2E),
    dm_box_bd: rgb(0x48484A),
    dm_txt: rgb(0xF5F5F7),
    dm_dim: rgb(0xC7C7CC),
    dm_hover: rgb(0x2C2C2E),
    dm_hover2: rgb(0x3A3A3C),
    dm_trough: rgb(0x0F0F10),
    dm_lbl: rgb(0x98989F),
    dm_head: rgb(0x98989F),
    dm_warn: rgb(0xFF8A70),
    cg_bg: rgb(0x1C1C1E),
    cg_bd: rgb(0x333335),
    cg_box_bg: rgb(0x2C2C2E),
    cg_box_bd: rgb(0x3A3A3C),
    cg_btn_bd: rgb(0x48484A),
    cg_hover: rgb(0x2C2C2E),
    acc: rgb(0x2563EB),
    acc2: rgb(0x3B82F6),
    radius: 14.0,
    done: rgb(0x30D158),
    fail: rgb(0xFF8A70),
    ..DARK_PAL
};

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: Color32) -> f32 {
        let l = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * l(c.r()) + 0.7152 * l(c.g()) + 0.0722 * l(c.b())
    }

    fn ratio(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn keys_round_trip() {
        assert_eq!(split("dark"), ("craft", "dark"));
        assert_eq!(split("petal:light"), ("petal", "light"));
        assert_eq!(split("nope:light"), ("craft", "light"));
        assert_eq!(join("craft", "light"), "light");
        assert_eq!(join("ledger", "system"), "ledger:system");
        assert_eq!(join("petal", "black"), "petal:dark");
    }

    #[test]
    fn text_is_readable_in_every_look() {
        for (name, p) in [
            ("ledger light", LEDGER_LIGHT),
            ("ledger dark", LEDGER_DARK),
            ("petal light", PETAL_LIGHT),
            ("petal dark", PETAL_DARK),
        ] {
            for (what, bg) in [("bg", p.bg), ("surface", p.surface), ("list", p.central_bg)] {
                assert!(ratio(p.txt, bg) >= 7.0, "{name}: text on {what}");
                assert!(ratio(p.dim, bg) >= 4.5, "{name}: secondary text on {what}");
            }
            // White text on the accent buttons.
            assert!(ratio(Color32::WHITE, p.acc) >= 4.5, "{name}: text on the accent");
        }
    }
}

/// Is the active look `key`? (The palette knows: each look has its radius.)
pub fn active() -> &'static str {
    let r = crate::app::pl().radius;
    if r == LEDGER_LIGHT.radius {
        "ledger"
    } else if r == PETAL_LIGHT.radius {
        "petal"
    } else {
        "craft"
    }
}
