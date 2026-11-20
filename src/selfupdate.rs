//! One-click update of the portable Windows build.
//!
//! 1. Download the release's portable .zip to a temp folder and check it
//!    against the SHA-256 GitHub publishes for the asset (and its size).
//! 2. Unpack it there; the folder holding `download_craft.exe` is the new
//!    portable folder.
//! 3. Install: every file the new folder has replaces the one in the app's
//!    folder. The old file is first renamed to `<name>.dcold` (Windows lets a
//!    running program be renamed, not overwritten), then the new one is
//!    copied in. Any failure puts every old file back: the app keeps working.
//! 4. Restart: the new program starts with `--wait-pid <old pid>` and waits
//!    for this one to close before it claims the single-instance slot.
//!
//! Leftover `.dcold` files (the running program cannot delete its own) are
//! removed at the next start. Settings, the list and history live in the
//! config folder, which an update never touches.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// A release asset: where it is and what it must hash to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Asset {
    pub url: String,
    pub name: String,
    /// Lower-case hex SHA-256 from the release (`digest: "sha256:…"`), when
    /// GitHub published one.
    pub sha256: Option<String>,
    pub size: u64,
}

const OLD_SUFFIX: &str = ".dcold";

/// The app's folder when it can update itself there: a portable copy in a
/// folder this user can write to. A build run from a source checkout
/// (`…\target\…`) is never overwritten by a release.
pub fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    if dir.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target")) {
        return None;
    }
    writable(&dir).then_some(dir)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".dc-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Where downloads and unpacking happen.
fn work_dir() -> PathBuf {
    std::env::temp_dir().join("download-craft-update")
}

/// Download `asset` and check it. `progress` gets 0.0..=1.0.
pub async fn download(client: &reqwest::Client, asset: &Asset, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let dir = work_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let name = if asset.name.is_empty() { "update.zip".to_string() } else { crate::engine::sanitize_filename(&asset.name) };
    let path = dir.join(name);
    let resp = client.get(&asset.url).send().await.context("downloading the update")?;
    if !resp.status().is_success() {
        return Err(anyhow!("the update download failed (HTTP {})", resp.status()));
    }
    let total = resp.content_length().unwrap_or(asset.size).max(1);
    let mut file = std::fs::File::create(&path)?;
    let mut hasher = Sha256::new();
    let mut got = 0u64;
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.context("downloading the update")?;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        got += chunk.len() as u64;
        progress((got as f32 / total as f32).min(1.0));
    }
    file.flush()?;
    drop(file);
    if asset.size > 0 && got != asset.size {
        return Err(anyhow!("the update is {got} bytes, the release says {}", asset.size));
    }
    if let Some(want) = &asset.sha256 {
        let have = format!("{:x}", hasher.finalize());
        if !have.eq_ignore_ascii_case(want) {
            return Err(anyhow!("the update's checksum does not match the release (got {have}, expected {want})"));
        }
    }
    Ok(path)
}

/// Unpack the zip next to it and return the folder that holds the program.
pub fn unpack(zip_path: &Path) -> Result<PathBuf> {
    let out = zip_path.with_extension("unpacked");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file).context("the update is not a readable zip")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // `enclosed_name` refuses absolute paths and `..`: nothing may land
        // outside the unpack folder.
        let Some(rel) = entry.enclosed_name() else { return Err(anyhow!("the update holds an unsafe path: {}", entry.name())) };
        let dest = out.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&dest)?;
        // Reading to the end checks each file's CRC.
        std::io::copy(&mut entry, &mut f).with_context(|| format!("unpacking {}", entry.name()))?;
    }
    find_program_dir(&out).ok_or_else(|| anyhow!("the update has no {}", exe_name()))
}

fn exe_name() -> &'static str {
    if cfg!(windows) { "download_craft.exe" } else { "download_craft" }
}

/// The folder (the unpack root or one level down) holding the program.
fn find_program_dir(root: &Path) -> Option<PathBuf> {
    if root.join(exe_name()).is_file() {
        return Some(root.to_path_buf());
    }
    std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| p.join(exe_name()).is_file())
}

/// Every file under `dir`, as paths relative to it.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    Ok(out)
}

fn old_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(OLD_SUFFIX);
    PathBuf::from(s)
}

/// Where `dest` is moved aside to. A backup from an earlier update that is
/// still in use (the browser helper keeps running the old program until the
/// browser closes, sometimes for days) cannot be removed: this backup then
/// takes a numbered name of its own, so the update still goes ahead. Every
/// name ends in `.dcold`, which `cleanup` removes once nothing uses it.
fn free_old_path(dest: &Path) -> Result<PathBuf> {
    let first = old_path(dest);
    if !first.exists() || std::fs::remove_file(&first).is_ok() {
        return Ok(first);
    }
    for n in 1..100 {
        let mut s = dest.as_os_str().to_os_string();
        s.push(format!(".{n}{OLD_SUFFIX}"));
        let p = PathBuf::from(s);
        if !p.exists() || std::fs::remove_file(&p).is_ok() {
            return Ok(p);
        }
    }
    Err(anyhow!("no free name to move {} aside", dest.display()))
}

/// Copy the new folder over `target`, all or nothing.
pub fn install(new_dir: &Path, target: &Path) -> Result<usize> {
    let files = files_under(new_dir)?;
    if !files.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(exe_name())) {
        return Err(anyhow!("the update has no {}", exe_name()));
    }
    // (moved aside: original → backup) and (newly created files)
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut created: Vec<PathBuf> = Vec::new();
    let result: Result<()> = (|| {
        for rel in &files {
            let dest = target.join(rel);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            if dest.exists() {
                let bak = free_old_path(&dest)?;
                std::fs::rename(&dest, &bak).with_context(|| format!("moving {} aside", dest.display()))?;
                moved.push((dest.clone(), bak));
            }
            std::fs::copy(new_dir.join(rel), &dest).with_context(|| format!("writing {}", dest.display()))?;
            created.push(dest);
        }
        Ok(())
    })();
    if let Err(e) = result {
        // Undo: the new files go, the old ones come back.
        for c in created.iter().rev() {
            let _ = std::fs::remove_file(c);
        }
        for (orig, bak) in moved.iter().rev() {
            let _ = std::fs::rename(bak, orig);
        }
        return Err(e);
    }
    // Backups that are not in use go now; the running program's waits for
    // the next start (see `cleanup`).
    for (_, bak) in &moved {
        let _ = std::fs::remove_file(bak);
    }
    remove_dropped(target, &files);
    Ok(files.len())
}

/// The list of files the last update installed, kept in the app's folder.
const MANIFEST: &str = ".dc-files";

fn rel_key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Files the previous version installed that this one no longer has (an old
/// `DownloadCraft-Firefox-0.9.10.xpi`, a script dropped from the extension)
/// go, and the new list is written. Without a list (a copy older than the
/// list) only what is certainly the app's own goes: release packages named
/// `DownloadCraft-…` and files inside its two extension folders. Anything
/// else in the folder is the user's and is never touched.
fn remove_dropped(target: &Path, files: &[PathBuf]) {
    let new: std::collections::HashSet<String> = files.iter().map(|f| rel_key(f)).collect();
    let old: Vec<String> = match std::fs::read_to_string(target.join(MANIFEST)) {
        Ok(text) => text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
        Err(_) => files_under(target)
            .unwrap_or_default()
            .iter()
            .map(|f| rel_key(f))
            .filter(|k| {
                let top = k.split('/').next().unwrap_or("");
                let package = !k.contains('/')
                    && top.starts_with("DownloadCraft-")
                    && (k.ends_with(".xpi") || k.ends_with(".zip"));
                package || top == "browser-extension" || top == "browser-extension-firefox"
            })
            .collect(),
    };
    for k in old {
        // Never a path that leaves the folder, the program, or the list.
        if new.contains(&k) || k.contains("..") || k.starts_with('/') || k == exe_name() || k == MANIFEST {
            continue;
        }
        let p = target.join(&k);
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
            // An emptied folder of the app's goes too.
            if let Some(dir) = p.parent().filter(|d| *d != target) {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
    let mut list: Vec<&String> = new.iter().collect();
    list.sort();
    let _ = std::fs::write(target.join(MANIFEST), list.into_iter().map(|k| format!("{k}\n")).collect::<String>());
}

/// Remove `.dcold` leftovers in the app's folder (best effort, at start).
pub fn cleanup() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) else { return };
    cleanup_in(&dir);
    let _ = std::fs::remove_dir_all(work_dir());
}

/// Remove the backups in `dir` that nothing uses any more.
fn cleanup_in(dir: &Path) {
    let Ok(files) = files_under(dir) else { return };
    for rel in files {
        if rel.to_string_lossy().ends_with(OLD_SUFFIX) {
            let _ = std::fs::remove_file(dir.join(rel));
        }
    }
}

/// Start the (new) program; it waits for this process to end first.
pub fn relaunch() -> Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(&exe)
        .arg("--wait-pid")
        .arg(std::process::id().to_string())
        .spawn()
        .context("starting the new version")?;
    Ok(())
}

/// `--wait-pid <pid>`: wait (up to 20 s) for the old program to close
/// before this one starts, so it does not just wake the old window.
pub fn wait_for_previous(args: &[String]) {
    let Some(pid) = args
        .iter()
        .position(|a| a == "--wait-pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return;
    };
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 20_000);
            CloseHandle(h);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

/// The portable Windows asset of a GitHub release JSON, with its checksum.
pub fn pick_asset(v: &serde_json::Value) -> Option<Asset> {
    let url = crate::model::pick_windows_asset(v)?;
    let a = v.get("assets")?.as_array()?.iter().find(|a| a.get("browser_download_url").and_then(|u| u.as_str()) == Some(url.as_str()))?;
    let sha256 = a
        .get("digest")
        .and_then(|d| d.as_str())
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|h| h.to_ascii_lowercase());
    Some(Asset {
        name: a.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
        size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
        url,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc-selfupdate-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        // A stored zip written by hand (the crate is built read-only).
        let mut out = Vec::new();
        let mut dir = Vec::new();
        for (name, data) in files {
            let crc = crc32(data);
            let at = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            dir.extend_from_slice(&crc.to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&at.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        std::fs::File::create(path).unwrap().write_all(&out).unwrap();
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }

    #[test]
    fn release_json_gives_the_portable_zip_and_its_checksum() {
        let h = "a".repeat(64);
        let v = serde_json::json!({ "assets": [
            { "name": "DownloadCraft-1.0-x86_64.AppImage", "browser_download_url": "https://x/a", "size": 1 },
            { "name": "DownloadCraft-1.0-windows-x64-portable.zip", "browser_download_url": "https://x/p.zip", "size": 42, "digest": format!("sha256:{h}") },
        ]});
        let a = pick_asset(&v).unwrap();
        assert_eq!((a.url.as_str(), a.size, a.sha256.as_deref()), ("https://x/p.zip", 42, Some(h.as_str())));
    }

    #[test]
    fn unpack_finds_the_program_and_refuses_escaping_paths() {
        let d = tmp("unpack");
        let z = d.join("u.zip");
        let exe = format!("DC-1.0/{}", exe_name());
        make_zip(&z, &[(exe.as_str(), b"new exe"), ("DC-1.0/browser-extension/content.js", b"js")]);
        let root = unpack(&z).unwrap();
        assert!(root.ends_with("DC-1.0"));
        assert_eq!(std::fs::read(root.join("browser-extension/content.js")).unwrap(), b"js");
        let bad = d.join("bad.zip");
        make_zip(&bad, &[("../evil.txt", b"x"), (exe_name(), b"e")]);
        assert!(unpack(&bad).is_err());
        assert!(!d.join("evil.txt").exists());
    }

    #[test]
    fn install_replaces_files_and_keeps_others() {
        let d = tmp("install");
        let (new, app) = (d.join("new"), d.join("app"));
        std::fs::create_dir_all(new.join("browser-extension")).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(new.join(exe_name()), b"v2").unwrap();
        std::fs::write(new.join("browser-extension/content.js"), b"js2").unwrap();
        std::fs::write(app.join(exe_name()), b"v1").unwrap();
        std::fs::write(app.join("my-notes.txt"), b"mine").unwrap();
        assert_eq!(install(&new, &app).unwrap(), 2);
        assert_eq!(std::fs::read(app.join(exe_name())).unwrap(), b"v2");
        assert_eq!(std::fs::read(app.join("browser-extension/content.js")).unwrap(), b"js2");
        assert_eq!(std::fs::read(app.join("my-notes.txt")).unwrap(), b"mine");
        assert!(!old_path(&app.join(exe_name())).exists(), "unused backups are removed");
    }

    #[test]
    fn an_update_removes_what_the_new_version_dropped() {
        let d = tmp("dropped");
        let (new, app) = (d.join("new"), d.join("app"));
        std::fs::create_dir_all(new.join("browser-extension")).unwrap();
        std::fs::create_dir_all(app.join("browser-extension")).unwrap();
        std::fs::write(new.join(exe_name()), b"v2").unwrap();
        std::fs::write(new.join("DownloadCraft-Firefox-0.9.11.xpi"), b"x2").unwrap();
        std::fs::write(new.join("browser-extension/content.js"), b"js2").unwrap();
        std::fs::write(app.join(exe_name()), b"v1").unwrap();
        std::fs::write(app.join("DownloadCraft-Firefox-0.9.10.xpi"), b"x1").unwrap();
        std::fs::write(app.join("browser-extension/old-helper.js"), b"gone").unwrap();
        std::fs::write(app.join("my-notes.txt"), b"mine").unwrap();
        // First update: no list yet, so only the app's own leftovers go.
        install(&new, &app).unwrap();
        assert!(!app.join("DownloadCraft-Firefox-0.9.10.xpi").exists());
        assert!(!app.join("browser-extension/old-helper.js").exists());
        assert!(app.join("DownloadCraft-Firefox-0.9.11.xpi").exists());
        assert_eq!(std::fs::read(app.join("my-notes.txt")).unwrap(), b"mine");
        // Next update: the list says what the previous one installed.
        let newer = d.join("newer");
        std::fs::create_dir_all(newer.join("browser-extension")).unwrap();
        std::fs::write(newer.join(exe_name()), b"v3").unwrap();
        std::fs::write(newer.join("DownloadCraft-Firefox-0.9.12.xpi"), b"x3").unwrap();
        std::fs::write(newer.join("browser-extension/content.js"), b"js3").unwrap();
        install(&newer, &app).unwrap();
        assert!(!app.join("DownloadCraft-Firefox-0.9.11.xpi").exists());
        assert!(app.join("DownloadCraft-Firefox-0.9.12.xpi").exists());
        assert_eq!(std::fs::read(app.join("my-notes.txt")).unwrap(), b"mine");
        assert_eq!(std::fs::read(app.join(exe_name())).unwrap(), b"v3");
    }

    #[test]
    fn a_failed_install_puts_everything_back() {
        let d = tmp("rollback");
        let (new, app) = (d.join("new"), d.join("app"));
        std::fs::create_dir_all(&new).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(new.join("a.txt"), b"a2").unwrap();
        std::fs::write(new.join(exe_name()), b"v2").unwrap();
        std::fs::create_dir_all(new.join("sub")).unwrap();
        std::fs::write(new.join("sub/x.txt"), b"x2").unwrap();
        std::fs::write(app.join("a.txt"), b"a1").unwrap();
        std::fs::write(app.join(exe_name()), b"v1").unwrap();
        // A file where the update needs a folder: the last step fails after
        // the first two files were already replaced.
        std::fs::write(app.join("sub"), b"in the way").unwrap();
        assert!(install(&new, &app).is_err());
        assert_eq!(std::fs::read(app.join("a.txt")).unwrap(), b"a1");
        assert_eq!(std::fs::read(app.join(exe_name())).unwrap(), b"v1");
        assert!(!old_path(&app.join("a.txt")).exists());
    }

    #[tokio::test]
    async fn download_checks_the_checksum() {
        use sha2::{Digest, Sha256};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = b"pretend zip bytes".to_vec();
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/u.zip", l.local_addr().unwrap().port());
        let b2 = body.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { break };
                let b = b2.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).as_bytes()).await;
                    let _ = s.write_all(&b).await;
                });
            }
        });
        let client = reqwest::Client::new();
        let good = Asset { url: url.clone(), name: "u.zip".into(), sha256: Some(format!("{:x}", Sha256::digest(&body))), size: body.len() as u64 };
        let p = download(&client, &good, |_| {}).await.unwrap();
        assert_eq!(std::fs::read(p).unwrap(), body);
        let bad = Asset { sha256: Some("0".repeat(64)), ..good };
        assert!(download(&client, &bad, |_| {}).await.unwrap_err().to_string().contains("checksum"));
    }

    /// The program from two updates ago is still running (a browser helper):
    /// its backup cannot be removed, and the next update still installs.
    #[cfg(windows)]
    #[test]
    fn an_old_program_still_running_does_not_stop_an_update() {
        use std::os::windows::fs::OpenOptionsExt;
        let app = tmp("busy-app");
        let new = tmp("busy-new");
        std::fs::write(app.join(exe_name()), b"v2").unwrap();
        std::fs::write(app.join(format!("{}{OLD_SUFFIX}", exe_name())), b"v1").unwrap();
        // Held open without delete sharing, as a running program's file is.
        let busy = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 /* FILE_SHARE_READ */)
            .open(app.join(format!("{}{OLD_SUFFIX}", exe_name())))
            .unwrap();
        std::fs::write(new.join(exe_name()), b"v3").unwrap();
        install(&new, &app).unwrap();
        assert_eq!(std::fs::read(app.join(exe_name())).unwrap(), b"v3");
        drop(busy);
        cleanup_in(&app);
        let left: Vec<_> = std::fs::read_dir(&app).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert!(left.iter().all(|n| !n.to_string_lossy().ends_with(OLD_SUFFIX)), "{left:?}");
    }
}
