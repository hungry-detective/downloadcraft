//! Managed media tools: per-user downloads, no sudo, registry or shell installer.
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn managed_ffmpeg_path() -> Option<PathBuf> {
    crate::platform::helper_dir().map(|p| managed_pair_path(&p))
}
fn managed_pair_path(base: &Path) -> PathBuf {
    let current=base.join("ffmpeg/ffmpeg");
    let backup=base.join("ffmpeg.previous");
    if !current.is_file() && crate::platform::executable_file(&backup.join("ffmpeg"))
        && crate::platform::executable_file(&backup.join("ffprobe")) {
        backup.join("ffmpeg")
    } else { current }
}
pub fn install_supported() -> bool {
    cfg!(unix) && cfg!(any(target_arch = "x86_64", target_arch = "aarch64"))
}
fn asset_key(tool: &str) -> String {
    format!(
        "{tool}-{}-{}",
        if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        },
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x64"
        }
    )
}
fn download_spec(tool: &str) -> Result<(String, String)> {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../assets/helper-downloads.json"))?;
    let entry = &data[asset_key(tool)];
    Ok((
        entry["url"]
            .as_str()
            .ok_or_else(|| anyhow!("No download for this platform"))?
            .to_string(),
        entry["sha256"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing download checksum"))?
            .to_string(),
    ))
}
fn verify_digest(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        return Err(anyhow!(
            "Media tool checksum mismatch; previous tools retained"
        ));
    }
    Ok(())
}
fn check_tool(path: &Path, name: &str) -> bool {
    if !crate::platform::executable_file(path) {
        return false;
    }
    let mut c = std::process::Command::new(path);
    c.arg("-version");
    crate::platform::first_run_output(c)
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).starts_with(&format!("{name} version "))
        })
        .unwrap_or(false)
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn install_ffmpeg() -> Result<PathBuf> {
    if !install_supported() {
        return Err(anyhow!("Use the system FFmpeg installer on this platform"));
    }
    let _lock = crate::model::claim_window("install-ffmpeg")
        .ok_or_else(|| anyhow!("Another FFmpeg install is running; retry when it finishes"))?;
    let dest = crate::platform::helper_dir()
        .ok_or_else(|| anyhow!("No writable application data directory"))?.join("ffmpeg");
    // Recover an interrupted previous swap before starting another install.
    let backup=dest.with_extension("previous");
    if !dest.exists() && backup.is_dir() { std::fs::rename(&backup,&dest)?; }
    let stage = Stage(dest.with_extension(format!("{}.part", std::process::id())));
    std::fs::create_dir_all(&stage.0)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(format!("DownloadCraft/{}",crate::model::APP_VERSION))
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        for name in ["ffmpeg", "ffprobe"] {
            let (url, digest) = download_spec(name)?;
            let bytes = client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            verify_digest(&bytes, &digest)?;
            let target = stage.0.join(name);
            std::fs::write(&target, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700))?;
            }
            if !check_tool(&target, name) {
                return Err(anyhow!(
                    "Downloaded {name} could not run on this computer; previous tools retained"
                ));
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;
    // Replace the pair together. Restore the prior pair if the swap fails.
    let backup = dest.with_extension("previous");
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    let had_old = dest.exists();
    if had_old {
        std::fs::rename(&dest, &backup)?;
    }
    if let Err(e) = std::fs::rename(&stage.0, &dest) {
        if had_old {
            let _ = std::fs::rename(&backup, &dest);
        }
        return Err(e.into());
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&backup);
    }
    Ok(dest.join("ffmpeg"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_rejects_corrupt_download() {
        assert!(verify_digest(b"abc", "bad").is_err());
        assert!(verify_digest(
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )
        .is_ok());
    }
    #[test]
    fn both_tools_have_pinned_downloads_for_this_platform() {
        if install_supported() {
            for tool in ["ffmpeg", "ffprobe"] {
                let (u, d) = download_spec(tool).unwrap();
                assert!(u.starts_with("https://github.com/descriptinc/"));
                assert_eq!(d.len(), 64);
                assert!(u.ends_with(&asset_key(tool)));
            }
        }
    }
}

#[cfg(all(test, unix))]
mod audit_recovery_tests {
    use super::*;
    #[test]
    fn interrupted_swap_keeps_the_previous_pair_discoverable() {
        use std::os::unix::fs::PermissionsExt;
        let base=std::env::temp_dir().join(format!("dc-ffmpeg-recovery-{}",std::process::id()));
        let backup=base.join("ffmpeg.previous");std::fs::create_dir_all(&backup).unwrap();
        for name in ["ffmpeg","ffprobe"] {
            let p=backup.join(name);std::fs::write(&p,b"test").unwrap();
            std::fs::set_permissions(p,std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert_eq!(managed_pair_path(&base),backup.join("ffmpeg"));
        std::fs::create_dir_all(base.join("ffmpeg")).unwrap();
        std::fs::write(base.join("ffmpeg/ffmpeg"),b"new").unwrap();
        assert_eq!(managed_pair_path(&base),base.join("ffmpeg/ffmpeg"));
        std::fs::remove_dir_all(base).unwrap();
    }
}
