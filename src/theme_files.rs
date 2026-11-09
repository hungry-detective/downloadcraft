//! Themes from files: a folder of small theme files (`themes` in the
//! settings folder) that appear in the Theme menu under the layouts.
//!
//! - **A Download Craft theme** (`*.json`): a layout to start from and a few
//!   colours for its light side, its dark side, or both:
//!
//!   ```json
//!   { "name": "Harbour", "layout": "petal",
//!     "light": { "background": "#eef3f6", "surface": "#ffffff", "text": "#11202b", "accent": "#0f6c8c" },
//!     "dark":  { "background": "#14212a", "surface": "#0e171d", "text": "#e8f0f4", "accent": "#5cc3e6" } }
//!   ```
//!
//!   Optional colours: `secondary` (secondary text), `error`, `warning`,
//!   `success`, `selection`.
//! - **A Base16 colour scheme** (`*.yaml`, the format of the Base16/Base24
//!   scheme collections): `base00` background, `base01` surface, `base05`
//!   text, `base04` secondary text, `base0D` accent, `base08` error, `base0A`
//!   warning, `base0B` success. A dark scheme is dark and a light one light,
//!   whatever the mode says.
//!
//! Every other colour is mixed from these, so a file names only what it
//! changes. Text that would be hard to read is pushed toward black or white.

use crate::app::Palette;
use egui::Color32;
use std::path::PathBuf;

/// The folder (made on first use).
pub fn folder() -> Option<PathBuf> {
    let d = crate::model::config_dir()?.join("themes");
    let _ = std::fs::create_dir_all(&d);
    Some(d)
}

/// A theme file found in the folder: its key (`user.<file stem>`) and name.
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub name: String,
}

/// Every theme file, sorted by name (at most 20).
pub fn list() -> Vec<Entry> {
    let Some(dir) = folder() else { return Vec::new() };
    let mut v: Vec<Entry> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
                    if !matches!(ext.as_str(), "json" | "yaml" | "yml") {
                        return None;
                    }
                    let stem = p.file_stem()?.to_str()?.to_string();
                    let t = load(&format!("user.{stem}"))?;
                    Some(Entry { key: format!("user.{stem}"), name: t.name })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v.truncate(20);
    v
}

#[derive(Clone, Debug, Default)]
struct Seeds {
    background: Option<Color32>,
    surface: Option<Color32>,
    text: Option<Color32>,
    secondary: Option<Color32>,
    accent: Option<Color32>,
    error: Option<Color32>,
    warning: Option<Color32>,
    success: Option<Color32>,
    selection: Option<Color32>,
}

/// A loaded theme file.
#[derive(Clone, Debug)]
pub struct ThemeFile {
    pub name: String,
    /// The layout it is drawn with: craft, ledger or petal.
    pub layout: String,
    light: Option<Seeds>,
    dark: Option<Seeds>,
    /// A Base16 scheme is one side only, dark or light by its background.
    fixed_dark: Option<bool>,
}

fn hex(s: &str) -> Option<Color32> {
    let t = s.trim().trim_matches('"').trim_matches('\'').trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn seeds_json(v: &serde_json::Value) -> Option<Seeds> {
    let o = v.as_object()?;
    let c = |k: &str| o.get(k).and_then(|x| x.as_str()).and_then(hex);
    Some(Seeds {
        background: c("background"),
        surface: c("surface"),
        text: c("text"),
        secondary: c("secondary"),
        accent: c("accent"),
        error: c("error"),
        warning: c("warning"),
        success: c("success"),
        selection: c("selection"),
    })
}

/// Read `user.<stem>` from the folder.
pub fn load(key: &str) -> Option<ThemeFile> {
    let stem = key.strip_prefix("user.")?;
    let dir = folder()?;
    for ext in ["json", "yaml", "yml"] {
        let p = dir.join(format!("{stem}.{ext}"));
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let text = text.trim_start_matches('\u{feff}');
        return if ext == "json" { parse_json(text, stem) } else { parse_base16(text, stem) };
    }
    None
}

fn parse_json(text: &str, stem: &str) -> Option<ThemeFile> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let layout = v.get("layout").and_then(|x| x.as_str()).filter(|l| ["craft", "ledger", "petal"].contains(l)).unwrap_or("craft");
    let light = v.get("light").and_then(seeds_json);
    let dark = v.get("dark").and_then(seeds_json);
    // Colours at the top level count for both sides.
    let both = seeds_json(&v).filter(|s| s.background.is_some() || s.accent.is_some() || s.text.is_some());
    let (light, dark) = match (light, dark, both) {
        (None, None, Some(b)) => (Some(b.clone()), Some(b)),
        (l, d, _) => (l, d),
    };
    if light.is_none() && dark.is_none() {
        return None;
    }
    Some(ThemeFile {
        name: v.get("name").and_then(|x| x.as_str()).unwrap_or(stem).chars().take(40).collect(),
        layout: layout.to_string(),
        light,
        dark,
        fixed_dark: None,
    })
}

fn parse_base16(text: &str, stem: &str) -> Option<ThemeFile> {
    let mut map = std::collections::HashMap::new();
    let mut name = stem.to_string();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or(line).trim();
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').trim_matches('\'').to_string());
        if k == "scheme" || k == "name" {
            if !v.is_empty() {
                name = v.chars().take(40).collect();
            }
        } else if k.starts_with("base") && k.len() == 6 {
            map.insert(k, v);
        }
    }
    let c = |k: &str| map.get(k).and_then(|v| hex(v));
    let bg = c("base00")?;
    let s = Seeds {
        background: Some(bg),
        surface: c("base01"),
        text: c("base05"),
        secondary: c("base04"),
        accent: c("base0d"),
        error: c("base08"),
        warning: c("base0a"),
        success: c("base0b"),
        selection: None,
    };
    let dark = luminance(bg) < 0.4;
    Some(ThemeFile { name, layout: "craft".into(), light: (!dark).then(|| s.clone()), dark: dark.then_some(s), fixed_dark: Some(dark) })
}

fn lin(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color32) -> f32 {
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Push `fg` toward black or white until it reads on `bg` at `want`:1.
fn readable(fg: Color32, bg: Color32, want: f32) -> Color32 {
    let target = if luminance(bg) < 0.4 { Color32::WHITE } else { Color32::BLACK };
    let mut c = fg;
    for step in 1..=10 {
        if contrast(c, bg) >= want {
            break;
        }
        c = mix(fg, target, step as f32 / 10.0);
    }
    c
}

fn wash(c: Color32, a: u8) -> Color32 {
    let m = |v: u8| ((v as u16 * a as u16) / 255) as u8;
    Color32::from_rgba_premultiplied(m(c.r()), m(c.g()), m(c.b()), a)
}

impl ThemeFile {
    /// Whether the theme decides light or dark by itself (a Base16 scheme).
    pub fn fixed_dark(&self) -> Option<bool> {
        self.fixed_dark.or_else(|| match (&self.light, &self.dark) {
            (Some(_), None) => Some(false),
            (None, Some(_)) => Some(true),
            _ => None,
        })
    }

    /// The palette: the layout's own, with this file's colours spread over it.
    pub fn palette(&self, dark: bool) -> Palette {
        let dark = self.fixed_dark().unwrap_or(dark);
        let mut p = crate::skins::palette(&self.layout, dark);
        let Some(s) = (if dark { self.dark.as_ref() } else { self.light.as_ref() }) else { return p };
        let bg = s.background.unwrap_or(p.bg);
        let surface = s.surface.unwrap_or_else(|| if dark { mix(bg, Color32::WHITE, 0.06) } else { mix(bg, Color32::WHITE, 0.6) });
        let text = readable(s.text.unwrap_or(p.txt), surface, 7.0);
        let dim = readable(s.secondary.unwrap_or_else(|| mix(text, surface, 0.35)), surface, 4.5);
        let accent = s.accent.unwrap_or(p.primary);
        let line = mix(surface, text, 0.16);
        p.bg = bg;
        p.surface = surface;
        p.surface_lt = mix(surface, text, 0.08);
        p.header_bg = surface;
        p.central_bg = surface;
        p.card_bd = line;
        p.primary = accent;
        p.secondary = mix(accent, text, 0.2);
        p.txt = text;
        p.dim = dim;
        if let Some(e) = s.error {
            p.error = e;
            p.fail = e;
            p.dm_warn = e;
        }
        if let Some(w) = s.warning {
            p.warning = w;
        }
        if let Some(g) = s.success {
            p.success = g;
            p.done = g;
        }
        p.sel_bg = wash(s.selection.unwrap_or(accent), if dark { 70 } else { 45 });
        p.sel_bd = accent;
        p.hover_bg = wash(text, 16);
        p.modal_bg = surface;
        p.modal_bd = line;
        p.track = mix(surface, text, 0.14);
        p.dm_card = surface;
        p.dm_line = line;
        p.dm_box_bd = mix(surface, text, 0.25);
        p.dm_txt = text;
        p.dm_dim = dim;
        p.dm_hover = mix(surface, text, 0.07);
        p.dm_hover2 = mix(surface, text, 0.12);
        p.dm_trough = bg;
        p.dm_lbl = dim;
        p.dm_head = dim;
        p.cg_bg = surface;
        p.cg_bd = line;
        p.cg_txt = text;
        p.cg_dim = dim;
        p.cg_box_bg = bg;
        p.cg_box_bd = line;
        p.cg_btn_bd = mix(surface, text, 0.25);
        p.cg_box_txt = text;
        p.cg_hover = mix(surface, text, 0.08);
        // The accent fills buttons with white text on them.
        p.acc = readable(accent, Color32::WHITE, 4.5);
        p.acc2 = accent;
        p
    }
}

/// Copy a file the user picked into the folder; its key when it is a theme.
pub fn add(path: &std::path::Path) -> Result<String, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !matches!(ext.as_str(), "json" | "yaml" | "yml") {
        return Err("a theme is a .json file or a Base16 .yaml scheme".into());
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).ok_or("no file name")?;
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let to = folder().ok_or("no settings folder")?.join(format!("{stem}.{ext}"));
    std::fs::copy(path, &to).map_err(|e| e.to_string())?;
    let key = format!("user.{stem}");
    if load(&key).is_none() {
        let _ = std::fs::remove_file(&to);
        return Err("that file is not a theme Download Craft can read".into());
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_theme_spreads_and_stays_readable() {
        let t = parse_json(r##"{"name":"Harbour","layout":"petal","light":{"background":"#eef3f6","surface":"#ffffff","text":"#bbbbbb","accent":"#0f6c8c"}}"##, "h").unwrap();
        assert_eq!(t.layout, "petal");
        assert_eq!(t.fixed_dark(), Some(false));
        let p = t.palette(true);
        assert!(contrast(p.txt, p.surface) >= 7.0, "text pushed to readable");
        assert!(contrast(Color32::WHITE, p.acc) >= 4.5);
    }

    #[test]
    fn base16_scheme_reads() {
        let y = "scheme: \"Gruvbox dark\"\nbase00: \"282828\"\nbase01: \"3c3836\"\nbase04: \"bdae93\"\nbase05: \"d5c4a1\"\nbase08: \"fb4934\"\nbase0A: \"fabd2f\"\nbase0B: \"b8bb26\"\nbase0D: \"83a598\"\n";
        let t = parse_base16(y, "g").unwrap();
        assert_eq!(t.name, "Gruvbox dark");
        assert_eq!(t.fixed_dark(), Some(true));
        let p = t.palette(false);
        assert_eq!(p.bg, Color32::from_rgb(0x28, 0x28, 0x28));
    }
}
