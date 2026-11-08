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

// ---- the font library: any installed font, a file, or a link (issue #10) ----

/// Free fonts offered by name and downloaded the first time they are picked
/// (Google Fonts' own repository, SIL Open Font License): (name, file URL).
pub const DOWNLOADABLE_FONTS: &[(&str, &str)] = &[
    ("Inter", "https://github.com/google/fonts/raw/main/ofl/inter/Inter%5Bopsz,wght%5D.ttf"),
    ("Roboto", "https://github.com/google/fonts/raw/main/ofl/roboto/Roboto%5Bwdth,wght%5D.ttf"),
    ("Open Sans", "https://github.com/google/fonts/raw/main/ofl/opensans/OpenSans%5Bwdth,wght%5D.ttf"),
    ("Noto Sans", "https://github.com/google/fonts/raw/main/ofl/notosans/NotoSans%5Bwdth,wght%5D.ttf"),
    ("Poppins", "https://github.com/google/fonts/raw/main/ofl/poppins/Poppins-Regular.ttf"),
    ("Lato", "https://github.com/google/fonts/raw/main/ofl/lato/Lato-Regular.ttf"),
];

/// The app's own font folder: fonts picked from a file or a link are copied
/// here, so the choice keeps working when the original moves.
pub fn library_dir() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|d| d.join("fonts"))
}

/// The family and style a TrueType/OpenType file names in its `name` table
/// (IDs 16/17 when present, else 1/2), or `None` for anything unreadable.
pub fn font_names(data: &[u8]) -> Option<(String, String)> {
    let be16 = |o: usize| data.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let be32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // A collection: its first face.
    let base = if data.get(0..4) == Some(b"ttcf") { be32(12)? } else { 0 };
    let tables = be16(base + 4)?;
    let mut name_at = None;
    for i in 0..tables.min(64) {
        let rec = base + 12 + i * 16;
        if data.get(rec..rec + 4) == Some(b"name") {
            name_at = Some(be32(rec + 8)?);
            break;
        }
    }
    let t = name_at?;
    let count = be16(t + 2)?;
    let strings = t + be16(t + 4)?;
    let mut found: [Option<String>; 18] = Default::default();
    for i in 0..count.min(512) {
        let r = t + 6 + i * 12;
        let (platform, encoding, language, id, len, off) =
            (be16(r)?, be16(r + 2)?, be16(r + 4)?, be16(r + 6)?, be16(r + 8)?, be16(r + 10)?);
        if !matches!(id, 1 | 2 | 16 | 17) {
            continue;
        }
        let bytes = data.get(strings + off..strings + off + len)?;
        let text = if platform == 3 || platform == 0 {
            // UTF-16BE (Windows / Unicode), English preferred.
            if platform == 3 && encoding == 1 && language != 0x409 && found[id].is_some() {
                continue;
            }
            let units: Vec<u16> = bytes.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        } else if platform == 1 && encoding == 0 {
            bytes.iter().map(|&b| b as char).collect()
        } else {
            continue;
        };
        let text = text.trim().to_string();
        if !text.is_empty() && (found[id].is_none() || (platform == 3 && language == 0x409)) {
            found[id] = Some(text);
        }
    }
    let family = found[16].clone().or_else(|| found[1].clone())?;
    let style = found[17].clone().or_else(|| found[2].clone()).unwrap_or_else(|| "Regular".into());
    Some((family, style))
}

/// Every font on this computer (and in the app's library) that can be the
/// interface font: one entry per family, its regular face, as (family, file),
/// sorted by name. Read from the fonts' own name tables.
pub fn installed_fonts() -> Vec<(String, std::path::PathBuf)> {
    let mut dirs = font_dirs();
    #[cfg(all(unix, not(target_os = "macos")))]
    dirs.push("/usr/share/fonts".into());
    dirs.extend(library_dir());
    let mut files = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> = dirs.into_iter().map(|d| (d, 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 4 {
                    stack.push((p, depth + 1));
                }
            } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "ttf" | "otf")) {
                files.push(p);
            }
        }
    }
    let mut best: std::collections::BTreeMap<String, (u8, std::path::PathBuf)> = Default::default();
    for p in files {
        // The name table sits in the first part of the file: no need to read
        // a 20 MB CJK font whole.
        let Ok(mut f) = std::fs::File::open(&p) else { continue };
        let mut head = Vec::new();
        use std::io::Read;
        if (&mut f).take(1 << 20).read_to_end(&mut head).is_err() {
            continue;
        }
        let Some((family, style)) = font_names(&head) else { continue };
        let s = style.to_ascii_lowercase();
        let rank = if s == "regular" || s == "normal" || s == "book" || s == "roman" { 0 } else if s.contains("italic") || s.contains("oblique") { 3 } else if s.contains("bold") { 2 } else { 1 };
        let key = family.trim().to_string();
        if key.is_empty() || key.starts_with('.') {
            continue;
        }
        match best.get(&key) {
            Some((r, _)) if *r <= rank => {}
            _ => {
                best.insert(key, (rank, p));
            }
        }
    }
    best.into_iter().map(|(k, (_, p))| (k, p)).collect()
}

/// Copy a font file into the app's library and return its new path.
pub fn add_to_library(src: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let data = std::fs::read(src).map_err(|e| format!("could not read the font: {e}"))?;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "font.ttf".into());
    save_to_library(&name, &data)
}

fn save_to_library(name: &str, data: &[u8]) -> Result<std::path::PathBuf, String> {
    if font_names(data).is_none() {
        return Err("this is not a font file the app can read (.ttf or .otf)".into());
    }
    let dir = library_dir().ok_or("no settings folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let clean: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' })
        .collect();
    let path = dir.join(clean);
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Download a font into the library: a .ttf/.otf link, or a .zip holding
/// fonts (Google Fonts' "Download family"), whose regular face is kept.
/// Blocking; call it off the UI thread.
pub fn install_from_link(url: &str) -> Result<std::path::PathBuf, String> {
    let url = url.trim();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("paste a link that starts with https://".into());
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let bytes = rt.block_on(async {
        let client = crate::engine::EngineOpts::default().build_client().map_err(|e| e.to_string())?;
        let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("the link answered HTTP {}", resp.status().as_u16()));
        }
        if resp.content_length().is_some_and(|n| n > (60 << 20)) {
            return Err("that file is too large to be a font".into());
        }
        resp.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
    })?;
    let name = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .map(|n| crate::engine::percent_decode_owned(n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "font.ttf".into());
    if bytes.starts_with(b"PK") {
        // A zip of a family: its regular face (or its first font).
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("could not open the zip: {e}"))?;
        let mut pick: Option<(u8, usize, String)> = None;
        for i in 0..zip.len() {
            let Ok(f) = zip.by_index(i) else { continue };
            let n = f.name().to_string();
            let low = n.to_ascii_lowercase();
            if !(low.ends_with(".ttf") || low.ends_with(".otf")) {
                continue;
            }
            let rank = if low.contains("regular") || low.contains("[") { 0 } else if low.contains("italic") { 2 } else { 1 };
            if pick.as_ref().is_none_or(|(r, _, _)| rank < *r) {
                pick = Some((rank, i, n));
            }
        }
        let (_, i, n) = pick.ok_or("the zip holds no .ttf or .otf font")?;
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        use std::io::Read;
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        let base = std::path::Path::new(&n).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or(n);
        return save_to_library(&base, &data);
    }
    let name = if name.to_ascii_lowercase().ends_with(".ttf") || name.to_ascii_lowercase().ends_with(".otf") { name } else { format!("{name}.ttf") };
    save_to_library(&name, &bytes)
}

/// The library copy of a downloadable font, fetched the first time.
pub fn downloadable_font(name: &str) -> Result<std::path::PathBuf, String> {
    let (_, url) = DOWNLOADABLE_FONTS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).ok_or("unknown font")?;
    if let Some(dir) = library_dir() {
        if let Some((_, p)) = installed_in(&dir).into_iter().find(|(f, _)| f.eq_ignore_ascii_case(name)) {
            return Ok(p);
        }
    }
    install_from_link(url)
}

fn installed_in(dir: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let data = std::fs::read(&p).ok()?;
            Some((font_names(&data)?.0, p))
        })
        .collect()
}

#[cfg(test)]
mod library_tests {
    use super::*;

    #[test]
    fn reads_family_names_from_a_real_font() {
        let data = include_bytes!("../assets/fonts/MaterialIcons-Regular.ttf");
        let (family, style) = font_names(data).expect("name table");
        assert_eq!(family, "Material Icons");
        assert_eq!(style, "Regular");
        assert!(font_names(b"not a font at all").is_none());
    }

    /// Network and the machine's own fonts: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lists_installed_fonts_and_fetches_inter() {
        let all = installed_fonts();
        eprintln!("{} font families, e.g. {:?}", all.len(), all.iter().take(8).map(|(f, _)| f).collect::<Vec<_>>());
        assert!(all.len() > 5);
        let inter = downloadable_font("Inter").expect("Inter");
        let (family, _) = font_names(&std::fs::read(&inter).unwrap()).unwrap();
        eprintln!("Inter at {} = {family}", inter.display());
        assert_eq!(family, "Inter");
    }
}
