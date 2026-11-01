//! Font registration shared by every window.
//!
//! The Add-download dialog and the browser-capture popup are drawn to the
//! concept in `popup_concepts.html`, which asks for Segoe UI. Loading it as a
//! *named* family lets those dialogs use the exact face the design was drawn in
//! while the manager window keeps its own font. Missing system fonts fall back
//! to the bundled defaults, so this is safe on any machine.
//!
//! On top of that, every window gets the system's script fonts as *fallbacks*.
//! egui's bundled face is Latin only, so anything else - Bengali, Devanagari,
//! Tamil, Thai, Korean, Chinese - used to come out as tofu boxes: the file
//! name of a video titled in Bangla showed up as "Kalachan _ ▯▯▯▯▯▯ _ Tosiba".
//! Fallbacks are consulted per glyph, so the app's own typography is untouched
//! and only the characters it cannot draw are picked up from elsewhere.
//!
//! Fallbacks fix tofu, but they cannot fix *shaping*: epaint maps codepoints to
//! glyphs one at a time, so Bengali matras landed after their consonant and the
//! virama stayed visible instead of forming a conjunct. The Indic scripts
//! (`shaped`) therefore take a second route: rustybuzz shapes the run, ab_glyph
//! rasterises it and egui paints the result as a texture.

/// Named family holding Segoe UI regular (falls back to the app font).
pub const FONT_UI: &str = "segoe";
/// Named family holding Segoe UI Semibold (falls back to the app font).
pub const FONT_SEMI: &str = "popup-semibold";
/// Named family holding *only* Material Icons (see `install`).
pub const FONT_ICON: &str = "icons";

/// Segoe UI regular at `size`.
pub fn reg(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_UI.into()))
}

/// Material Icons glyph at `size`.
///
/// Icon codepoints sit in the Unicode Private Use Area, and the script
/// fallbacks deliberately include Segoe UI Symbol — which also maps PUA points
/// and therefore shadows the icon font. That is how U+E2C4 ("file_download")
/// came out as a little phone instead of a download arrow. Icons must go
/// through this family, where Material Icons is the only candidate.
pub fn icon(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_ICON.into()))
}

/// Segoe UI Semibold at `size` — the concept's 500/600 weights.
pub fn semi(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name(FONT_SEMI.into()))
}

/// System files tried as script fallbacks: `(key, file, face index)`.
///
/// `Nirmala.ttc` carries the Indic scripts in one collection — Bengali
/// (also the national language of Bangladesh), Devanagari, Gujarati,
/// Gurmukhi, Kannada, Malayalam, Odia, Sinhala, Tamil and Telugu. The rest
/// cover the other scripts Windows ships fonts for.
#[cfg(windows)]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("nirmala", "Nirmala.ttc", 0),
    ("leelawadee", "LeelawUI.ttf", 0),
    ("myanmar", "mmrtext.ttf", 0),
    ("himalaya", "himalaya.ttf", 0),
    ("sylfaen", "sylfaen.ttf", 0),
    ("ebrima", "ebrima.ttf", 0),
    ("malgun", "malgun.ttf", 0),
    ("msyh", "msyh.ttc", 0),
    ("yugoth", "YuGothR.ttc", 0),
    ("seguisym", "seguisym.ttf", 0),
];

#[cfg(target_os = "macos")]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("arabic", "GeezaPro.ttc", 0), ("devanagari", "KohinoorDevanagari.ttc", 0),
    ("bengali", "KohinoorBangla.ttc", 0), ("cjk", "PingFang.ttc", 0),
];
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) const SYSTEM_FONTS: &[(&str, &str, u32)] = &[
    ("arabic", "NotoSansArabic-Regular.ttf", 0), ("devanagari", "NotoSansDevanagari-Regular.ttf", 0),
    ("bengali", "NotoSansBengali-Regular.ttf", 0), ("thai", "NotoSansThai-Regular.ttf", 0),
    ("cjk", "NotoSansCJK-Regular.ttc", 0),
];

/// Where the OS keeps its fonts. Missing optional fonts are skipped.
#[cfg(windows)]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    // A per-user font (`%LOCALAPPDATA%\Microsoft\Windows\Fonts`) is where a
    // user-installed script font would land, and it is checked first so it can
    // override the machine copy.
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(
            std::path::Path::new(&local)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    let win = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    dirs.push(std::path::Path::new(&win).join("Fonts"));
    dirs
}

#[cfg(target_os = "macos")]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut paths = vec!["/System/Library/Fonts".into(), "/System/Library/Fonts/Supplemental".into(), "/Library/Fonts".into()];
    if let Some(h) = dirs::home_dir() { paths.insert(0, h.join("Library/Fonts")); }
    paths
}
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut paths = vec!["/usr/share/fonts/truetype/noto".into(), "/usr/share/fonts/opentype/noto".into(), "/usr/share/fonts/truetype/dejavu".into(), "/usr/local/share/fonts".into()];
    if let Some(h) = dirs::home_dir() { paths.insert(0, h.join(".local/share/fonts")); paths.push(h.join(".fonts")); }
    paths
}

/// Read one system font and register it under `key`.
///
/// Returns whether it was registered. epaint *panics* on a font it cannot
/// parse (it builds the faces on first use, outside any `catch_unwind`), so a
/// file that fails to parse is dropped here instead — a broken or exotic font
/// in the system directory must not take the app down with it.
fn load_system(
    fonts: &mut egui::FontDefinitions,
    key: &str,
    file: &str,
    index: u32,
) -> bool {
    {
        let bytes = font_dirs()
            .into_iter()
            .find_map(|dir| std::fs::read(dir.join(file)).ok());
        let Some(bytes) = bytes else {
            return false;
        };
        if ab_glyph::FontVec::try_from_vec_and_index(bytes.clone(), index).is_err() {
            return false;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert(key.to_owned(), std::sync::Arc::new(data));
        true
    }
}

/// Which script-fallback key (from `SYSTEM_FONTS`) draws `c`; None when the
/// base fonts already cover it (Latin, Greek, Cyrillic, Arabic, Hebrew on
/// Windows via Segoe UI; Bengali via the bundled face).
fn wanted_key(c: char) -> Option<&'static str> {
    let u = c as u32;
    if cfg!(windows) {
        Some(match u {
            0x0530..=0x058F | 0x10A0..=0x10FF => "sylfaen",
            0x07C0..=0x07FF | 0x1200..=0x139F | 0x2D80..=0x2DDF => "ebrima",
            0x0900..=0x097F | 0x0A00..=0x0DFF => "nirmala",
            0x0E00..=0x0EFF => "leelawadee",
            0x0F00..=0x0FFF => "himalaya",
            0x1000..=0x109F => "myanmar",
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => "malgun",
            0x3040..=0x30FF | 0x31F0..=0x31FF => "yugoth",
            0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF => "msyh",
            0x2190..=0x2BFF | 0x1F000..=0x1FAFF => "seguisym",
            _ => return None,
        })
    } else {
        Some(match u {
            0x0600..=0x06FF => "arabic",
            0x0900..=0x097F => "devanagari",
            0x0980..=0x09FF => "bengali",
            0x0E00..=0x0E7F => "thai",
            0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF => "cjk",
            _ => return None,
        })
    }
}

/// Fonts of this process: what is registered, and which script fallbacks
/// were already looked for. Script fonts are big (Microsoft YaHei alone is
/// ~20 MB) and every window is its own process, so each one is loaded the
/// first time a character of its script is drawn, not up front.
struct FontState {
    defs: egui::FontDefinitions,
    /// egui's own family lists, which every rebuild starts from.
    base_families: std::collections::BTreeMap<egui::FontFamily, Vec<String>>,
    fallbacks: Vec<String>,
    has_reg: bool,
    has_semi: bool,
    primary: bool,
    tried: std::collections::HashSet<&'static str>,
}

static FONTS: std::sync::Mutex<Option<FontState>> = std::sync::Mutex::new(None);

impl FontState {
    /// (Re)build the family lists from the fonts registered so far.
    fn apply_families(&mut self) {
        let fonts = &mut self.defs;
        fonts.families = self.base_families.clone();
        {
            let prop = fonts.families.entry(egui::FontFamily::Proportional).or_default();
            if self.primary && self.has_reg {
                prop.insert(0, "segoe".to_owned());
            }
            for key in &self.fallbacks {
                if !prop.contains(key) {
                    prop.push(key.clone());
                }
            }
            prop.push("material".to_owned());
        }
        // Monospace falls back the same way: the URL and destination boxes
        // are drawn in it, and a name in any script has to stay readable.
        {
            let mono = fonts.families.entry(egui::FontFamily::Monospace).or_default();
            for key in &self.fallbacks {
                if !mono.contains(key) {
                    mono.push(key.clone());
                }
            }
        }
        // Named families: the regular cut prefers Segoe UI, then the app
        // font, so icon glyphs and every script keep resolving in dialogs.
        let base = fonts.families[&egui::FontFamily::Proportional].clone();
        let regular = if self.has_reg {
            let mut v = vec!["segoe".to_owned()];
            v.extend(base.iter().filter(|f| f.as_str() != "segoe").cloned());
            v
        } else {
            base
        };
        // Semibold keeps the concept's weight for Latin and falls through to
        // the same fallbacks for everything else.
        let semibold = if self.has_semi {
            let mut v = vec!["segoe_semi".to_owned()];
            v.extend(regular.iter().filter(|f| f.as_str() != "segoe").cloned());
            v
        } else {
            regular.clone()
        };
        fonts.families.insert(egui::FontFamily::Name(FONT_UI.into()), regular);
        fonts.families.insert(egui::FontFamily::Name(FONT_SEMI.into()), semibold);
        // Icon-only family: Material Icons and nothing else, so no system
        // font can shadow a PUA icon codepoint (see `icon`).
        fonts.families.insert(egui::FontFamily::Name(FONT_ICON.into()), vec!["material".to_owned()]);
    }
}

/// Register Material Icons, Segoe UI and the bundled Bengali face. Other
/// script fonts follow on demand (`ensure_text`).
///
/// `primary` also promotes Segoe UI to the head of the proportional family,
/// which is what the standalone dialogs want (their whole surface is the
/// concept). The manager window passes `false`, so only the named families are
/// added and its own typography stays untouched.
/// Interface fonts offered in Settings → Appearance: (name, regular file,
/// semibold file), all shipped with Windows. Any other .ttf can be picked.
pub const UI_FONTS: &[(&str, &str, &str)] = &[
    ("Segoe UI", "segoeui.ttf", "seguisb.ttf"),
    ("Arial", "arial.ttf", "arialbd.ttf"),
    ("Calibri", "calibri.ttf", "calibrib.ttf"),
    ("Tahoma", "tahoma.ttf", "tahomabd.ttf"),
    ("Verdana", "verdana.ttf", "verdanab.ttf"),
    ("Trebuchet MS", "trebuc.ttf", "trebucbd.ttf"),
];

fn default_font_files() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("segoeui.ttf", "seguisb.ttf")
    } else if cfg!(target_os = "macos") {
        ("Helvetica.ttc", "Helvetica.ttc")
    } else {
        ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf")
    }
}

/// The regular and semibold files for the `ui_font` preference: a name from
/// `UI_FONTS`, or a font file's path (used for both weights). Empty = the
/// default. Read once at start, like the rest of the typography.
fn ui_font_files() -> (String, String) {
    let pick = crate::model::ui_pref_str("ui_font", "");
    let pick = pick.trim();
    if let Some((_, r, b)) = UI_FONTS.iter().find(|f| f.0.eq_ignore_ascii_case(pick)) {
        return (r.to_string(), b.to_string());
    }
    if !pick.is_empty() && std::path::Path::new(pick).is_file() {
        return (pick.to_string(), pick.to_string());
    }
    let (r, b) = default_font_files();
    (r.to_string(), b.to_string())
}

pub fn install(cc: &eframe::CreationContext<'_>, primary: bool) {
    let mut defs = egui::FontDefinitions::default();
    let base_families = defs.families.clone();
    defs.font_data.insert(
        "material".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/MaterialIcons-Regular.ttf"
        ))),
    );
    let (reg_file, semi_file) = ui_font_files();
    let mut has_reg = load_system(&mut defs, "segoe", &reg_file, 0);
    let mut has_semi = load_system(&mut defs, "segoe_semi", &semi_file, 0);
    // A picked font that is missing or unreadable: back to the default.
    let (def_reg, def_semi) = default_font_files();
    if !has_reg {
        has_reg = load_system(&mut defs, "segoe", def_reg, 0);
    }
    if !has_semi {
        has_semi = load_system(&mut defs, "segoe_semi", def_semi, 0);
    }
    defs.font_data.insert("bundled-bengali".into(), std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansBengali.ttf"))));
    // Segoe UI leads the fallbacks: it covers Greek, Cyrillic, Arabic,
    // Hebrew and the wider Latin ranges.
    let mut fallbacks: Vec<String> = Vec::new();
    if has_reg {
        fallbacks.push("segoe".to_owned());
    }
    fallbacks.push("bundled-bengali".into());
    let mut st = FontState { defs, base_families, fallbacks, has_reg, has_semi, primary, tried: Default::default() };
    st.apply_families();
    cc.egui_ctx.set_fonts(st.defs.clone());
    *FONTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(st);
}

/// Make sure `text` can be drawn: loads the script fallback fonts its
/// characters need (once per process). Cheap for text already covered -
/// call it with whatever is about to be shown.
pub fn ensure_text(ctx: &egui::Context, text: &str) {
    if text.is_ascii() {
        return;
    }
    let mut guard = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(st) = guard.as_mut() else { return };
    let mut changed = false;
    for c in text.chars() {
        if (c as u32) < 0x0530 {
            continue;
        }
        let Some(key) = wanted_key(c) else { continue };
        if !st.tried.insert(key) {
            continue;
        }
        if let Some(&(k, file, index)) = SYSTEM_FONTS.iter().find(|(k, _, _)| *k == key) {
            if load_system(&mut st.defs, k, file, index) {
                // Before the bundled Bengali face, which is a last resort.
                let at = st.fallbacks.iter().position(|f| f == "bundled-bengali").unwrap_or(st.fallbacks.len());
                st.fallbacks.insert(at, k.to_owned());
                changed = true;
            }
        }
    }
    if changed {
        st.apply_families();
        ctx.set_fonts(st.defs.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_map_to_their_fallback_font() {
        if cfg!(windows) {
            assert_eq!(wanted_key('中'), Some("msyh"));
            assert_eq!(wanted_key('한'), Some("malgun"));
            assert_eq!(wanted_key('ひ'), Some("yugoth"));
            assert_eq!(wanted_key('क'), Some("nirmala"));
            assert_eq!(wanted_key('ไ'), Some("leelawadee"));
        }
        // Covered by the base fonts: nothing to load.
        assert_eq!(wanted_key('a'), None);
        assert_eq!(wanted_key('Ж'), None);
    }
}
