//! Translations. The English text is the key: `t("Settings")` returns the
//! chosen language's text, or the English when there is none. A language is
//! one JSON object of {"English": "translation"} in a `lang` folder, either
//! next to the program or in the config folder (which wins), named by its
//! code: `lang/bn.json`, `lang/es.json`, ...
//!
//! Texts a language file is missing are collected in `<code>.missing.json`
//! in the config folder's `lang`, so a translator can run the app, open that
//! file and fill it in.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Languages that ship inside the program; a file with the same code in a
/// `lang` folder adds to and overrides them.
const BUILTIN: &[(&str, &str)] = &[
    ("bn", include_str!("../lang/bn.json")),
    ("es", include_str!("../lang/es.json")),
    ("fr", include_str!("../lang/fr.json")),
    ("de", include_str!("../lang/de.json")),
    ("pt", include_str!("../lang/pt.json")),
    ("ru", include_str!("../lang/ru.json")),
    ("hi", include_str!("../lang/hi.json")),
    ("zh", include_str!("../lang/zh.json")),
    ("id", include_str!("../lang/id.json")),
];

struct Lang {
    code: String,
    map: HashMap<String, &'static str>,
    missing: BTreeMap<String, String>,
}

fn state() -> &'static Mutex<Lang> {
    static S: OnceLock<Mutex<Lang>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(load(&crate::model::ui_pref_str("language", "en"))))
}

/// Folders that can hold language files, the config folder's first.
fn lang_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(c) = crate::model::config_dir() {
        v.push(c.join("lang"));
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        v.push(d.join("lang"));
    }
    v
}

fn load(code: &str) -> Lang {
    load_from(code, &lang_dirs())
}

fn load_from(code: &str, dirs: &[PathBuf]) -> Lang {
    let code = code.trim().to_ascii_lowercase();
    let mut map = HashMap::new();
    if code != "en" && !code.is_empty() {
        let builtin = BUILTIN.iter().filter(|(c, _)| *c == code).map(|(_, j)| j.to_string());
        // Built in, then the program folder, then the config folder's file.
        let files = dirs.iter().rev().filter_map(|d| std::fs::read_to_string(d.join(format!("{code}.json"))).ok());
        for text in builtin.chain(files) {
            let Ok(obj) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) else { continue };
            for (k, v) in obj {
                if k.starts_with('@') {
                    continue;
                }
                if let Some(s) = v.as_str().filter(|s| !s.trim().is_empty()) {
                    map.insert(k, &*Box::leak(s.to_string().into_boxed_str()));
                }
            }
        }
    }
    Lang { code, map, missing: BTreeMap::new() }
}

/// `english` in the chosen language (or as is).
pub fn t(english: &str) -> &str {
    let Ok(mut l) = state().lock() else { return english };
    if l.code == "en" || l.code.is_empty() {
        return english;
    }
    if let Some(s) = l.map.get(english) {
        return s;
    }
    if !l.missing.contains_key(english) && l.missing.len() < 5000 {
        l.missing.insert(english.to_string(), String::new());
        if let Some(dir) = crate::model::config_dir().map(|c| c.join("lang")) {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(j) = serde_json::to_string_pretty(&l.missing) {
                let _ = std::fs::write(dir.join(format!("{}.missing.json", l.code)), j);
            }
        }
    }
    english
}

/// Every translated text once (the first call only), so the fonts for its
/// script can be loaded before it is drawn.
pub fn take_font_text() -> Option<String> {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let l = state().lock().ok()?;
    (!l.map.is_empty()).then(|| l.map.values().copied().collect::<Vec<_>>().join(" "))
}

/// The languages there are files for: (code, name shown in the picker).
/// English is always first.
pub fn available() -> Vec<(String, String)> {
    let mut v = vec![("en".to_string(), "English".to_string())];
    for (code, json) in BUILTIN {
        let name = serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|j| j.get("@name").and_then(|n| n.as_str()).map(str::to_string))
            .unwrap_or_else(|| code.to_string());
        v.push((code.to_string(), name));
    }
    for dir in lang_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(code) = name.strip_suffix(".json").filter(|c| !c.contains('.')) else { continue };
            if v.iter().any(|(c, _)| c == code) {
                continue;
            }
            // A file may name itself: {"@name": "বাংলা"}.
            let label = std::fs::read_to_string(e.path())
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|j| j.get("@name").and_then(|n| n.as_str()).map(str::to_string))
                .unwrap_or_else(|| code.to_string());
            v.push((code.to_string(), label));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_translate_and_the_config_folder_wins() {
        let base = std::env::temp_dir().join(format!("dc-lang-{}", std::process::id()));
        let (cfg, prog) = (base.join("cfg"), base.join("prog"));
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::create_dir_all(&prog).unwrap();
        std::fs::write(prog.join("xx.json"), r#"{"Settings":"Old","Delete":"Supprimer","Empty":""}"#).unwrap();
        std::fs::write(cfg.join("xx.json"), r#"{"Settings":"Réglages"}"#).unwrap();
        let l = load_from("XX", &[cfg, prog]);
        assert_eq!(l.code, "xx");
        assert_eq!(l.map.get("Settings").copied(), Some("Réglages"));
        assert_eq!(l.map.get("Delete").copied(), Some("Supprimer"));
        assert!(!l.map.contains_key("Empty"));
        assert!(load_from("en", &[base.join("cfg")]).map.is_empty());
        // The built-in Bengali file parses and names itself.
        assert_eq!(load_from("bn", &[]).map.get("Settings").copied(), Some("সেটিংস"));
        assert!(available().iter().any(|(c, n)| c == "bn" && n == "বাংলা"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
