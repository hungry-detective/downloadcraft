//! Image galleries through gallery-dl (GPL-2.0, run as its own program, as
//! yt-dlp is): an album, a gallery, an artist's page or a category becomes
//! one row that saves every picture into a folder of its own.
//!
//! A link is a gallery when its site and path are one of the known kinds
//! below, or when it starts with `gallery:` (any site gallery-dl knows).
//! gallery-dl is fetched once into the helpers folder (Windows, Linux, macOS);
//! pause stops it and resume starts it again, and it skips what is already
//! in the folder.

use crate::engine::SharedState;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Is `url` a gallery page this module takes?
pub fn is_gallery(url: &str) -> bool {
    let t = url.trim();
    if t.starts_with("gallery:") {
        return true;
    }
    let Ok(u) = url::Url::parse(t) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
    let path = u.path().to_ascii_lowercase();
    match host.as_str() {
        "imgur.com" => path.starts_with("/a/") || path.starts_with("/gallery/"),
        "flickr.com" => path.contains("/albums/") || path.contains("/sets/") || path.contains("/galleries/"),
        "commons.wikimedia.org" => path.starts_with("/wiki/category:"),
        "artstation.com" => path.split('/').filter(|s| !s.is_empty()).count() == 1,
        "pinterest.com" => path.split('/').filter(|s| !s.is_empty()).count() == 2,
        h if h.ends_with("deviantart.com") => path.contains("/gallery"),
        h if h.ends_with(".tumblr.com") => true,
        "bsky.app" => path.contains("/profile/") && !path.contains("/post/"),
        _ => false,
    }
}

/// The address gallery-dl is given (without the `gallery:` prefix).
fn target(url: &str) -> &str {
    url.trim().strip_prefix("gallery:").unwrap_or(url.trim())
}

/// A folder name for the gallery, from its address.
pub fn folder_name(url: &str) -> String {
    let t = target(url);
    let name = url::Url::parse(t)
        .ok()
        .map(|u| {
            let host = u.host_str().unwrap_or("gallery").trim_start_matches("www.").split('.').next().unwrap_or("gallery").to_string();
            let last = u.path_segments().and_then(|s| s.filter(|p| !p.is_empty()).last().map(str::to_string)).unwrap_or_default();
            let last = crate::engine::percent_decode_owned(&last).replace("Category:", "").replace('_', " ");
            if last.is_empty() { host } else { format!("{host} - {last}") }
        })
        .unwrap_or_else(|| "gallery".into());
    crate::engine::sanitize_filename(&name)
}

const BIN: &str = if cfg!(windows) { "gallery-dl.exe" } else { "gallery-dl" };

fn local_path() -> Option<PathBuf> {
    Some(crate::platform::helper_dir()?.join(BIN))
}

/// gallery-dl on this computer, fetched from its releases the first time
/// (and again when the copy is over 30 days old).
async fn ensure(client: &reqwest::Client) -> Result<PathBuf> {
    let dst = local_path().context("no folder for helpers")?;
    let fresh = std::fs::metadata(&dst)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|a| a.as_secs() < 30 * 86_400);
    if fresh {
        return Ok(dst);
    }
    // The project's standalone builds (its own GitHub organisation).
    let asset = if cfg!(windows) {
        "gallery-dl_windows.exe"
    } else if cfg!(target_os = "macos") {
        "gallery-dl_macos"
    } else {
        "gallery-dl_linux"
    };
    let url = format!("https://github.com/gdl-org/builds/releases/latest/download/{asset}");
    let fetched = async {
        let r = client.get(&url).send().await?;
        anyhow::ensure!(r.status().is_success(), "HTTP {}", r.status());
        let b = r.bytes().await?;
        anyhow::ensure!(b.len() > 1_000_000, "suspicious download ({} bytes)", b.len());
        Ok::<_, anyhow::Error>(b)
    }
    .await;
    match fetched {
        Ok(bytes) => {
            if let Some(d) = dst.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let tmp = dst.with_extension("part");
            std::fs::write(&tmp, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            }
            let _ = std::fs::remove_file(&dst);
            std::fs::rename(&tmp, &dst)?;
            Ok(dst)
        }
        Err(_) if dst.is_file() => Ok(dst),
        Err(e) => Err(anyhow!("could not get gallery-dl: {e:#}")),
    }
}

/// Save every picture of the gallery `url` into a folder next to `dest`.
pub async fn download(url: &str, dest: &Path, client: reqwest::Client, shared: Arc<SharedState>, log: &str) -> Result<()> {
    crate::netlock::bind_ip().map_err(|e| anyhow!(e))?;
    if let Ok(mut p) = shared.phase.lock() {
        *p = "Getting gallery-dl".into();
    }
    let exe = ensure(&client).await?;
    let name = folder_name(url);
    let folder = dest.parent().map(Path::to_path_buf).unwrap_or_default().join(&name);
    std::fs::create_dir_all(&folder)?;
    if let Ok(mut n) = shared.resolved_name.lock() {
        *n = Some(name.clone());
    }
    // Pause stops the program; Resume starts it again (it skips files that
    // are already there).
    shared.external_helper.store(true, Ordering::SeqCst);
    crate::engine::job_log_pub(log, format!("gallery: {} into {}", target(url), folder.display()));
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("--directory").arg(&folder).arg("--no-colors").arg(target(url));
    if let Ok(Some(ip)) = crate::netlock::bind_ip() {
        cmd.arg("--source-address").arg(ip.to_string());
    }
    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().context("could not start gallery-dl")?;
    let stdout = child.stdout.take().context("no output")?;
    let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut files = 0u64;
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    // gallery-dl prints each file it saves ("# " marks one it skipped).
                    let path = l.trim().trim_start_matches("# ").trim();
                    if !path.is_empty() {
                        files += 1;
                        shared.downloaded.store(crate::model::size_on_disk(&folder), Ordering::SeqCst);
                        if let Ok(mut p) = shared.phase.lock() {
                            *p = format!("{files} picture(s)");
                        }
                    }
                }
                _ => break,
            },
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {
                if shared.cancelled.load(Ordering::SeqCst) || shared.paused.load(Ordering::SeqCst) {
                    let _ = child.kill().await;
                    return Err(anyhow!("cancelled"));
                }
            }
        }
    }
    let status = child.wait().await?;
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut e, &mut err).await;
    }
    if let Ok(mut p) = shared.phase.lock() {
        p.clear();
    }
    let size = crate::model::size_on_disk(&folder);
    shared.downloaded.store(size, Ordering::SeqCst);
    shared.total.store(size, Ordering::SeqCst);
    shared.has_total.store(true, Ordering::SeqCst);
    crate::engine::job_log_pub(log, format!("gallery: {files} file(s), {size} bytes, exit {status}"));
    if files == 0 && size == 0 {
        let why = err.lines().rev().find(|l| l.contains("error") || l.contains("Error")).unwrap_or("no pictures found").trim().to_string();
        return Err(anyhow!("gallery-dl: {why}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_galleries() {
        assert!(is_gallery("https://imgur.com/a/abc123"));
        assert!(is_gallery("https://commons.wikimedia.org/wiki/Category:Sunsets"));
        assert!(is_gallery("https://www.flickr.com/photos/someone/albums/7215"));
        assert!(is_gallery("gallery:https://example.com/anything"));
        assert!(!is_gallery("https://imgur.com/abc123.jpg"));
        assert!(!is_gallery("https://example.com/file.zip"));
        assert_eq!(folder_name("https://commons.wikimedia.org/wiki/Category:Red_pandas"), "commons - Red pandas");
    }
}
