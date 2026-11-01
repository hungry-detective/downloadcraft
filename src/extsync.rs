//! The browser extension in a folder that never moves.
//!
//! A portable copy of the app can live anywhere and move with each update (a
//! new zip unpacked somewhere else, the old folder deleted), and Chrome keeps
//! an unpacked extension only at the path it was loaded from. So the app
//! copies the extension it ships into its own settings folder
//! (`%APPDATA%\download_craft\browser-extension`, and `…-firefox`) on every
//! start: loaded from there once, the extension stays in place whatever
//! happens to the app's folder, and its files are always the app's own
//! version. The native host tells the extension which version that is
//! (`pong`), and an older running copy reloads itself onto the new files.

use std::path::{Path, PathBuf};

/// Extension folders shipped next to the app, and where each is kept.
const FOLDERS: [&str; 2] = ["browser-extension", "browser-extension-firefox"];

/// The fixed folder the Chrome / Edge / Brave extension is loaded from.
pub fn stable_dir() -> Option<PathBuf> {
    crate::model::config_dir().map(|d| d.join(FOLDERS[0]))
}

/// The version in `dir`'s manifest.
pub fn manifest_version(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("version")?.as_str().map(str::to_string)
}

/// The version of the extension the app keeps in its fixed folder.
pub fn kept_version() -> Option<String> {
    manifest_version(&stable_dir()?)
}

/// The folder `name` shipped with this app: beside the program (the
/// portable package), or a few levels up for a build in a source checkout.
fn shipped(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|d| d.join(name))
        .find(|d| d.join("manifest.json").is_file())
}

/// Files that belong in an installed extension (tests, notes and the other
/// browser's manifest stay out, as in the release packages).
fn wanted(rel: &Path) -> bool {
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    !(name.ends_with(".test.js")
        || name.ends_with(".md")
        || name.ends_with(".ps1")
        || name == "manifest-firefox.json"
        || rel.components().any(|c| c.as_os_str() == "testdata"))
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out
}

/// Makes `dest` hold exactly `src`'s extension files: changed ones copied,
/// ones the new version no longer has removed. Returns how many files
/// changed (0 = already the same).
fn mirror(src: &Path, dest: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut changed = 0;
    let want: Vec<PathBuf> = files_under(src).into_iter().filter(|r| wanted(r)).collect();
    for rel in &want {
        let (from, to) = (src.join(rel), dest.join(rel));
        let new = std::fs::read(&from)?;
        if std::fs::read(&to).ok().as_deref() == Some(&new[..]) {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&to, &new)?;
        changed += 1;
    }
    for rel in files_under(dest) {
        if !want.contains(&rel) {
            let _ = std::fs::remove_file(dest.join(&rel));
            changed += 1;
        }
    }
    Ok(changed)
}

/// Brings the fixed extension folders up to the app's own version. Called
/// once at start (off the window's thread); an error only means the old
/// copy stays.
pub fn sync() -> Vec<String> {
    let mut notes = Vec::new();
    let Some(base) = crate::model::config_dir() else { return notes };
    for name in FOLDERS {
        let Some(src) = shipped(name) else { continue };
        let dest = base.join(name);
        match mirror(&src, &dest) {
            Ok(0) => {}
            Ok(n) => notes.push(format!(
                "Browser extension {} kept up to date in {} ({n} files changed)",
                manifest_version(&dest).unwrap_or_default(),
                dest.display()
            )),
            Err(e) => notes.push(format!("Could not update the browser extension in {}: {e}", dest.display())),
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_copies_changes_and_drops_old_files() {
        let root = std::env::temp_dir().join(format!("dc-extsync-{}", std::process::id()));
        let (src, dest) = (root.join("src"), root.join("dest"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(src.join("icons")).unwrap();
        std::fs::write(src.join("manifest.json"), r#"{"version":"1.2.3"}"#).unwrap();
        std::fs::write(src.join("icons/a.png"), b"png").unwrap();
        std::fs::write(src.join("x.test.js"), b"test").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("gone.js"), b"old").unwrap();
        assert_eq!(mirror(&src, &dest).unwrap(), 3);
        assert_eq!(manifest_version(&dest).as_deref(), Some("1.2.3"));
        assert!(dest.join("icons/a.png").is_file());
        assert!(!dest.join("x.test.js").exists() && !dest.join("gone.js").exists());
        assert_eq!(mirror(&src, &dest).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
