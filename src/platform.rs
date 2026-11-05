//! OS-specific integration. Windows keeps its existing implementation;
//! these functions provide the native equivalents on macOS and Linux.
use std::path::{Path, PathBuf};
use std::process::Command;

/// The identity every window of this app reports: the main manager, About,
/// Settings, and the standalone capture/progress dialogs. All of them are
/// separate processes, so without a shared id the desktop lists each one as
/// its own application (four dock icons, four "new window" entries). This is
/// the installed desktop file's basename, which is what Wayland matches an
/// `app_id` against; X11 gets the same string as `WM_CLASS`, and the packaged
/// `.desktop` names it in `StartupWMClass`.
pub fn app_id() -> &'static str {
    "com.downloadcraft.app"
}

/// AppImage mount paths disappear after exit. Relaunch the permanent image.
pub fn launch_path() -> std::io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("APPIMAGE").map(PathBuf::from) {
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
    }
    std::env::current_exe()
}

pub fn executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    true
}

pub fn helper_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join(crate::model::APP_SLUG).join("bin"))
}

/// A broken custom executable must not freeze Settings indefinitely.
pub fn version_output(command: Command) -> Option<std::process::Output> {
    version_output_timeout(command, std::time::Duration::from_secs(5))
}
/// The first run of a just-downloaded helper: a frozen build unpacks itself
/// and antivirus scans it first, which can take far longer than 5 s on a slow
/// or busy machine.
pub fn first_run_output(command: Command) -> Option<std::process::Output> {
    version_output_timeout(command, std::time::Duration::from_secs(120))
}
fn version_output_timeout(mut command: Command, timeout: std::time::Duration) -> Option<std::process::Output> {
    // A file avoids pipe deadlock and reader threads left behind by a tool's
    // descendants inheriting stdout. Keep only a bounded version prefix in RAM.
    static NEXT: std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
    struct Capture(PathBuf);
    impl Drop for Capture { fn drop(&mut self) { let _=std::fs::remove_file(&self.0); } }
    let mut capture=None;
    for _ in 0..100 {
        let id=NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let path=std::env::temp_dir().join(format!("dc-version-{}-{id}",std::process::id()));
        let mut options=std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => { capture=Some((Capture(path),file)); break; },
            Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    let (guard, mut file)=capture?;
    #[cfg(windows)]
    {use std::os::windows::process::CommandExt;command.creation_flags(0x08000000);}
    command.stdin(std::process::Stdio::null()).stdout(file.try_clone().ok()?)
        .stderr(std::process::Stdio::null());
    let mut child=command.spawn().ok()?;
    let deadline=std::time::Instant::now()+timeout;
    let status=loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now()<deadline =>
                std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => { let _=child.kill();let _=child.wait();return None; }
        }
    };
    use std::io::{Read,Seek};
    file.rewind().ok()?;
    let mut stdout=Vec::new();
    file.by_ref().take(65536).read_to_end(&mut stdout).ok()?;
    drop(file);drop(child);drop(command);drop(guard);
    Some(std::process::Output {status,stdout,stderr:Vec::new()})
}

/// Open a web page in the default browser.
pub fn open_url(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    #[cfg(windows)]
    let _ = Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

pub fn open(path: &Path, reveal: bool) {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        if reveal && !path.is_dir() {
            c.arg("-R");
        }
        c.arg(path);
        let _ = c.spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if reveal && !path.is_dir() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let _ = Command::new("xdg-open").arg(target).spawn();
    }
    #[cfg(windows)]
    {
        let _ = (path, reveal);
    }
}
/// A short system sound: "done" or "failed" (best effort, never blocks).
pub fn play_sound(ok: bool) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONHAND};
        unsafe {
            MessageBeep(if ok { MB_ICONASTERISK } else { MB_ICONHAND });
        }
    }
    #[cfg(target_os = "macos")]
    {
        let f = if ok { "/System/Library/Sounds/Glass.aiff" } else { "/System/Library/Sounds/Basso.aiff" };
        let _ = std::process::Command::new("afplay").arg(f).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let id = if ok { "complete" } else { "dialog-error" };
        let _ = std::process::Command::new("canberra-gtk-play").args(["-i", id]).spawn();
    }
}

/// One of the Sounds page's events ("complete", "failed", "queue_start",
/// "queue_done"), when it is switched on there.
pub fn play_event(event: &str) {
    let legacy = crate::model::ui_pref_bool("sounds", false) && (event == "complete" || event == "failed");
    if !crate::model::ui_pref_bool(&format!("sound_{event}"), legacy) {
        return;
    }
    play_sound_file(event, &crate::model::ui_pref_str(&format!("sound_{event}_file"), ""));
}

/// Play `file` (a .wav), or the system sound for `event` when it is empty
/// or missing.
pub fn play_sound_file(event: &str, file: &str) {
    let file = file.trim();
    if file.is_empty() || !Path::new(file).is_file() {
        play_sound(event != "failed");
        return;
    }
    #[cfg(windows)]
    {
        #[link(name = "winmm")]
        extern "system" {
            fn PlaySoundW(sound: *const u16, module: isize, flags: u32) -> i32;
        }
        const SND_ASYNC: u32 = 0x0001;
        const SND_NODEFAULT: u32 = 0x0002;
        const SND_FILENAME: u32 = 0x0002_0000;
        let wide: Vec<u16> = file.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is NUL-terminated; SND_ASYNC copies what it needs.
        unsafe { PlaySoundW(wide.as_ptr(), 0, SND_FILENAME | SND_ASYNC | SND_NODEFAULT) };
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("afplay").arg(file).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if Command::new("paplay").arg(file).spawn().is_err() {
            let _ = Command::new("aplay").arg("-q").arg(file).spawn();
        }
    }
}

pub fn power(action: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new(if action == "sleep" {
            "pmset"
        } else {
            "osascript"
        });
        if action == "sleep" {
            c.arg("sleepnow");
        } else if action == "shutdown" {
            c.args(["-e", "tell application \"System Events\" to shut down"]);
        } else {
            return Err("Hibernation is not offered by this macOS build".into());
        }
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("systemctl");
        c.arg(match action {
            "shutdown" => "poweroff",
            "sleep" => "suspend",
            "hibernate" => "hibernate",
            _ => return Err("Unknown power action".into()),
        });
        c
    };
    #[cfg(windows)]
    return Err("Windows uses the existing power integration".into());
    #[cfg(unix)]
    {
        let out = command.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }
}
/// Browsers whose native-host registration the app keeps current. Linux
/// also has Chromium as a separate browser (Fedora's default Chromium build).
#[cfg(windows)]
pub const HOST_BROWSERS: &[&str] = &["chrome", "edge", "brave", "opera", "vivaldi", "firefox"];
#[cfg(not(windows))]
pub const HOST_BROWSERS: &[&str] = &["chrome", "chromium", "edge", "brave", "opera", "vivaldi", "firefox"];

pub fn native_host_directory(browser: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support");
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = dirs::config_dir().ok_or("No configuration directory")?;
    #[cfg(windows)]
    let base = home.clone();
    let relative = match browser {
        "chrome" => {
            if cfg!(target_os = "macos") {
                "Google/Chrome/NativeMessagingHosts"
            } else {
                "google-chrome/NativeMessagingHosts"
            }
        }
        "chromium" => {
            if cfg!(target_os = "macos") {
                "Chromium/NativeMessagingHosts"
            } else {
                "chromium/NativeMessagingHosts"
            }
        }
        "brave" => "BraveSoftware/Brave-Browser/NativeMessagingHosts",
        "edge" => {
            if cfg!(target_os = "macos") {
                "Microsoft Edge/NativeMessagingHosts"
            } else {
                "microsoft-edge/NativeMessagingHosts"
            }
        }
        "firefox" => {
            if cfg!(target_os = "macos") {
                "Mozilla/NativeMessagingHosts"
            } else {
                return Ok(home.join(".mozilla/native-messaging-hosts"));
            }
        }
        "vivaldi" => {
            if cfg!(target_os = "macos") {
                "Vivaldi/NativeMessagingHosts"
            } else {
                "vivaldi/NativeMessagingHosts"
            }
        }
        "opera" => {
            if cfg!(target_os = "macos") {
                "com.operasoftware.Opera/NativeMessagingHosts"
            } else {
                "opera/NativeMessagingHosts"
            }
        }
        _ => return Err(format!("Unsupported browser: {browser}")),
    };
    Ok(base.join(relative))
}
pub fn install_native_host(browser: &str) -> Result<String, String> {
    let _ = extension_directory(browser == "firefox");
    let dir = native_host_directory(browser)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exe = launch_path().map_err(|e| e.to_string())?;
    let launcher = crate::model::config_dir()
        .ok_or("No config directory")?
        .join("native-host/download_craft_host.sh");
    std::fs::create_dir_all(launcher.parent().unwrap()).map_err(|e| e.to_string())?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let tools = dirs::home_dir()
        .map(|h| h.join(".local/bin"))
        .unwrap_or_default();
    let body=format!("#!/bin/sh\n# Browsers do not inherit a terminal's PATH.\nexport PATH=/opt/homebrew/bin:/usr/local/bin:{}:$PATH\nexport APPIMAGE_EXTRACT_AND_RUN=1\nexec {} --native-host \"$@\"\n",quote(&tools.to_string_lossy()),quote(&exe.to_string_lossy()));
    std::fs::write(&launcher, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let mut manifest = serde_json::json!({"name":"com.download_craft","description":"Download Craft native host","path":launcher,"type":"stdio"});
    if browser == "firefox" {
        manifest["allowed_extensions"] = serde_json::json!(["send-to-download-craft@example"]);
    } else {
        manifest["allowed_origins"] =
            serde_json::json!([format!("chrome-extension://{}/", crate::app::EXTENSION_ID)]);
    }
    let path = dir.join("com.download_craft.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn native_manifests_use_vendor_directories() {
        let brave = native_host_directory("brave").unwrap();
        assert!(brave.ends_with("BraveSoftware/Brave-Browser/NativeMessagingHosts"));
        let firefox = native_host_directory("firefox").unwrap();
        assert!(firefox.ends_with(if cfg!(target_os = "macos") {
            "Mozilla/NativeMessagingHosts"
        } else {
            ".mozilla/native-messaging-hosts"
        }));
        assert!(native_host_directory("unknown").is_err());
    }
    #[test]
    fn window_lock_is_exclusive_and_released_when_closed() {
        let name = format!("portable-lock-test-{}", std::process::id());
        let first = crate::model::claim_window(&name).expect("first lock");
        assert!(crate::model::claim_window(&name).is_none());
        drop(first);
        assert!(crate::model::claim_window(&name).is_some());
        let _ = std::fs::remove_file(crate::model::config_dir().unwrap().join("win").join(name));
    }
}

#[cfg(all(test, unix))]
mod executable_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn data_files_are_not_offered_as_executables() {
        let p = std::env::temp_dir().join(format!("dc-executable-{}", std::process::id()));
        std::fs::write(&p, "#!/bin/sh\necho version\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!executable_file(&p));
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(executable_file(&p));
        std::fs::remove_file(p).unwrap();
    }
}

pub fn default_user_agent() -> &'static str {
    if cfg!(windows) {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
    } else if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"
    }
}

/// Extension files in an AppImage mount are temporary. Stage browser-loadable
/// copies in user data; macOS bundles keep the source files in Resources.
pub fn extension_directory(firefox: bool) -> Result<PathBuf, String> {
    let name = if firefox {
        "browser-extension-firefox"
    } else {
        "browser-extension"
    };
    let target = dirs::data_local_dir()
        .ok_or("No application data directory")?
        .join(crate::model::APP_SLUG)
        .join("extensions")
        .join(name);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("No application directory")?;
    let candidates = [
        parent.join(name),
        parent.join("../Resources").join(name),
        parent.join("../lib/downloadcraft").join(name),
    ];
    if let Some(source) = candidates
        .iter()
        .find(|p| p.join("manifest.json").is_file())
    {
        fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
            std::fs::create_dir_all(to)?;
            for entry in std::fs::read_dir(from)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    copy_tree(&entry.path(), &to.join(entry.file_name()))?;
                } else if kind.is_file() {
                    let target = to.join(entry.file_name());
                    let bytes = std::fs::read(entry.path())?;
                    if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
                        std::fs::write(target, bytes)?;
                    }
                }
            }
            Ok(())
        }
        copy_tree(source, &target).map_err(|e| e.to_string())?;
    }
    if target.join("manifest.json").is_file() {
        Ok(target)
    } else {
        Err("Extension files were not included with this build; use the browser-extension folders from the download package".into())
    }
}

#[cfg(all(test, unix))]
mod audit_tests {
    use super::*;
    #[test]
    fn audit_version_probe_drains_large_output() {
        let mut c = Command::new("python3");
        c.args(["-c", "import sys;sys.stdout.write('v' * 262144)"]);
        let out = version_output(c).expect("a noisy working tool must not deadlock");
        assert!(out.status.success());
        assert!(!out.stdout.is_empty());
    }
}

/// Parse an executable and arguments before substituting an untrusted filename.
/// Downloaded names must never become shell commands.
pub fn antivirus_command(template: &str, path: &Path) -> Result<Command,String> {
    #[cfg(unix)]
    let args=shlex::split(template).ok_or("Antivirus command has unmatched quotes")?;
    #[cfg(windows)]
    let args=windows_command_args(template)?;
    let (program,args)=args.split_first().ok_or("Antivirus command is empty")?;
    if program.is_empty() {return Err("Antivirus executable is empty".into());}
    let mut cmd=Command::new(program);
    for arg in args { cmd.arg(arg.replace("{file}",&path.to_string_lossy())); }
    #[cfg(windows)]
    {use std::os::windows::process::CommandExt;cmd.creation_flags(0x08000000);}
    cmd.stdin(std::process::Stdio::null());
    Ok(cmd)
}
#[cfg(windows)]
fn windows_command_args(template:&str) -> Result<Vec<String>,String> {
    #[link(name="shell32")]
    unsafe extern "system" {fn CommandLineToArgvW(command:*const u16,count:*mut i32)->*mut *mut u16;}
    #[link(name="kernel32")]
    unsafe extern "system" {fn LocalFree(memory:*mut std::ffi::c_void)->*mut std::ffi::c_void;}
    if template.trim().is_empty() {return Ok(Vec::new());}
    if template.contains('\0') {return Err("Antivirus command contains a null character".into());}
    let text:Vec<u16>=template.encode_utf16().chain(Some(0)).collect();
    let mut count=0;
    unsafe {
        let ptr=CommandLineToArgvW(text.as_ptr(),&mut count);
        if ptr.is_null() {return Err("Could not parse antivirus command".into());}
        let result=std::slice::from_raw_parts(ptr,count as usize).iter().map(|arg|{
            let mut len=0;while *arg.add(len)!=0 {len+=1;}
            String::from_utf16_lossy(std::slice::from_raw_parts(*arg,len))
        }).collect();
        LocalFree(ptr.cast());
        Ok(result)
    }
}
#[cfg(all(test,unix))]
mod audit_antivirus_tests {
    use super::*;
    #[test]
    fn downloaded_filename_is_passed_as_one_literal_argument() {
        let path=Path::new("/tmp/a name;$(printf HACKED)&'file'");
        for template in ["printf '%s' {file}","printf '%s' \"{file}\""] {
            let out=antivirus_command(template,path).unwrap().output().unwrap();
            assert!(out.status.success());
            assert_eq!(out.stdout,path.as_os_str().as_encoded_bytes());
        }
        assert!(antivirus_command("'unterminated",path).is_err());
    }
}

/// Keep helper descendants (notably yt-dlp's FFmpeg merger) in the same
/// cancellation scope as their parent. Closing a Windows job or a Unix group
/// prevents an orphaned merger from writing after Pause/Remove.
pub struct HelperProcessTree {
    #[cfg(unix)]
    group: i32,
    #[cfg(windows)]
    job: usize,
}
impl HelperProcessTree {
    pub fn attach(child:&tokio::process::Child) -> std::io::Result<Self> {
        #[cfg(unix)]
        { Ok(Self {group:child.id().ok_or_else(||std::io::Error::other("Helper already exited"))? as i32}) }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::JobObjects::*;
            use windows_sys::Win32::Foundation::CloseHandle;
            let job=CreateJobObjectW(std::ptr::null(),std::ptr::null());
            if job.is_null() {return Err(std::io::Error::last_os_error());}
            let mut info:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let process=child.raw_handle().ok_or_else(||std::io::Error::other("Helper already exited"));
            let result=process.and_then(|process| {
                if SetInformationJobObject(job,JobObjectExtendedLimitInformation,(&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),std::mem::size_of_val(&info) as u32)==0
                    || AssignProcessToJobObject(job,process)==0 {
                    Err(std::io::Error::last_os_error())
                } else {Ok(())}
            });
            if let Err(e)=result {CloseHandle(job);return Err(e);}
            Ok(Self {job:job as usize})
        }
    }
}
impl Drop for HelperProcessTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {fn kill(pid:i32,signal:i32)->i32;}
            kill(-self.group,9);
        }
        #[cfg(windows)]
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job as *mut std::ffi::c_void); }
    }
}

/// Drag files out of the app, as Explorer does: dropped on Explorer, the
/// desktop, a chat or a mail, they are copied there. Blocks until the drop
/// (the system runs the drag), so call it while the mouse button is held.
#[cfg(windows)]
pub fn drag_files_out(files: &[PathBuf]) {
    use windows_sys::Win32::System::Ole::{OleInitialize, DROPEFFECT_COPY, DROPEFFECT_LINK};
    use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows_sys::Win32::UI::Shell::{ILFree, SHCreateDataObject, SHDoDragDrop, SHParseDisplayName};
    // IID_IDataObject {0000010e-0000-0000-C000-000000000046}
    const IID_IDATAOBJECT: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0x0000_010e,
        data2: 0,
        data3: 0,
        data4: [0xC0, 0, 0, 0, 0, 0, 0, 0x46],
    };
    let hwnd = crate::tray::hwnd();
    if files.is_empty() || hwnd == 0 {
        return;
    }
    unsafe {
        // Already set up by the window on this thread; harmless if so.
        use std::os::windows::ffi::OsStrExt;
        let _ = OleInitialize(std::ptr::null());
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for f in files {
            let wide: Vec<u16> = f.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(wide.as_ptr(), std::ptr::null_mut(), &mut pidl, 0, std::ptr::null_mut()) == 0 && !pidl.is_null() {
                pidls.push(pidl);
            }
        }
        if !pidls.is_empty() {
            let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const ITEMIDLIST).collect();
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            // Absolute item lists with no parent folder: the desktop's.
            if SHCreateDataObject(std::ptr::null(), list.len() as u32, list.as_ptr(), std::ptr::null_mut(), &IID_IDATAOBJECT, &mut data) == 0
                && !data.is_null()
            {
                let mut effect = 0u32;
                // No drop source of our own: the shell supplies the standard one.
                let _ = SHDoDragDrop(hwnd as _, data, std::ptr::null_mut(), DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
                // IUnknown::Release is the third entry of the object's table.
                let vtbl = *(data as *const *const usize);
                let release: extern "system" fn(*mut std::ffi::c_void) -> u32 = std::mem::transmute(*vtbl.add(2));
                release(data);
            }
        }
        for p in pidls {
            ILFree(p);
        }
    }
}

#[cfg(not(windows))]
pub fn drag_files_out(_files: &[PathBuf]) {}

/// Whether windows should keep the OS title bar instead of painting their
/// own. The borderless window relies on DWM drawing the client area over the
/// whole frame; stripped Windows builds (Themes service off, "Micro 10" and
/// similar) still reserve the caption, so the picture lands one caption lower
/// than where clicks are read and every button answers above itself. The
/// `system_title_bar` key in ui.json wins; without it this follows the theme
/// state.
pub fn system_title_bar() -> bool {
    if std::env::var_os("DOWNLOADCRAFT_SYSTEM_FRAME").is_some_and(|v| v != "0") {
        return true;
    }
    crate::model::ui_pref_bool("system_title_bar", frame_unsupported())
}

/// The OS cannot host a borderless window cleanly: visual styles or desktop
/// composition are off.
#[cfg(windows)]
pub fn frame_unsupported() -> bool {
    #[link(name = "uxtheme")]
    extern "system" {
        fn IsThemeActive() -> i32;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmIsCompositionEnabled(enabled: *mut i32) -> i32;
    }
    let mut on = 0i32;
    // SAFETY: plain queries; `on` outlives the call.
    unsafe { IsThemeActive() == 0 || DwmIsCompositionEnabled(&mut on) != 0 || on == 0 }
}

#[cfg(not(windows))]
pub fn frame_unsupported() -> bool {
    false
}
