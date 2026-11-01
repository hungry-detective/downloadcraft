//! The system's automatic proxy (a PAC script or WPAD auto-detect), as set
//! in Windows' "Proxy settings". A fixed system proxy is already picked up
//! by reqwest; a PAC script decides per URL, so it is asked per host here
//! through WinHTTP, which runs the script the same way the browsers do.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// How long a host's answer is kept (a PAC script can depend on time of day).
const CACHE_SECS: u64 = 300;

/// Whether the system uses a PAC script or auto-detect at all (read once).
pub fn auto_configured() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(imp::auto_configured)
}

/// The proxy the system's PAC / auto-detect chooses for `url`, as a URL
/// reqwest and yt-dlp understand ("http://host:port"); None = go direct.
pub fn proxy_for(url: &str) -> Option<String> {
    if !auto_configured() {
        return None;
    }
    let key = reqwest::Url::parse(url).ok().map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))?;
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, v)) = cache.lock().ok()?.get(&key) {
        if at.elapsed().as_secs() < CACHE_SECS {
            return v.clone();
        }
    }
    let v = imp::resolve(url).and_then(|list| pick(&list, url));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, (std::time::Instant::now(), v.clone()));
    }
    v
}

/// The proxy a given PAC script (Settings → Proxy → automatic configuration
/// script) picks for `url`; None = go direct. Cached like `proxy_for`.
pub fn pac_proxy_for(script: &str, url: &str) -> Option<String> {
    let host = reqwest::Url::parse(url).ok().map(|u| format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")))?;
    let key = format!("{script}|{host}");
    static CACHE: OnceLock<Mutex<HashMap<String, (std::time::Instant, Option<String>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, v)) = cache.lock().ok()?.get(&key) {
        if at.elapsed().as_secs() < CACHE_SECS {
            return v.clone();
        }
    }
    let v = imp::resolve_with(url, (false, Some(script.to_string()))).and_then(|list| pick(&list, url));
    if let Ok(mut c) = cache.lock() {
        c.insert(key, (std::time::Instant::now(), v.clone()));
    }
    v
}

/// The fixed proxy in the system's settings (Settings → Proxy → Get system):
/// (proxy list as Windows stores it, PAC script URL).
pub fn system_settings() -> (Option<String>, Option<String>) {
    imp::system_settings()
}

/// WinHTTP's answer is a list ("a:8080; b:3128" or "http=a:80;https=b:443"):
/// take the first entry that fits the URL's scheme.
fn pick(list: &str, url: &str) -> Option<String> {
    let https = url.starts_with("https:");
    let mut fallback = None;
    for item in list.split([';', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
        let (scheme, addr) = match item.split_once('=') {
            Some((s, a)) => (Some(s.to_ascii_lowercase()), a),
            None => (None, item),
        };
        let addr = if addr.contains("://") { addr.to_string() } else { format!("http://{addr}") };
        match scheme.as_deref() {
            None => return Some(addr),
            Some("https") if https => return Some(addr),
            Some("http") if !https => return Some(addr),
            Some("http") => {
                fallback.get_or_insert(addr);
            }
            _ => {}
        }
    }
    fallback
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Networking::WinHttp::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Read a WinHTTP-allocated string and free it.
    unsafe fn take(p: *mut u16) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        GlobalFree(p as _);
        Some(s)
    }

    /// (auto-detect on, PAC URL) from the user's proxy settings.
    fn ie_config() -> Option<(bool, Option<String>)> {
        unsafe {
            let mut c: WINHTTP_CURRENT_USER_IE_PROXY_CONFIG = std::mem::zeroed();
            if WinHttpGetIEProxyConfigForCurrentUser(&mut c) == 0 {
                return None;
            }
            let pac = take(c.lpszAutoConfigUrl).filter(|s| !s.trim().is_empty());
            let _ = take(c.lpszProxy);
            let _ = take(c.lpszProxyBypass);
            Some((c.fAutoDetect != 0, pac))
        }
    }

    pub fn auto_configured() -> bool {
        matches!(ie_config(), Some((auto, pac)) if auto || pac.is_some())
    }

    pub fn system_settings() -> (Option<String>, Option<String>) {
        unsafe {
            let mut c: WINHTTP_CURRENT_USER_IE_PROXY_CONFIG = std::mem::zeroed();
            if WinHttpGetIEProxyConfigForCurrentUser(&mut c) == 0 {
                return (None, None);
            }
            let pac = take(c.lpszAutoConfigUrl).filter(|s| !s.trim().is_empty());
            let proxy = take(c.lpszProxy).filter(|s| !s.trim().is_empty());
            let _ = take(c.lpszProxyBypass);
            (proxy, pac)
        }
    }

    pub fn resolve(url: &str) -> Option<String> {
        resolve_with(url, ie_config()?)
    }

    pub fn resolve_with(url: &str, (auto, pac): (bool, Option<String>)) -> Option<String> {
        unsafe {
            let agent = wide("Download Craft");
            let session = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0);
            if session.is_null() {
                return None;
            }
            // Cap the script download / WPAD lookup: a missing PAC host must
            // not hold a download back for long.
            WinHttpSetTimeouts(session, 3000, 3000, 3000, 3000);
            let pac_w = pac.as_deref().map(wide);
            let mut o: WINHTTP_AUTOPROXY_OPTIONS = std::mem::zeroed();
            if let Some(p) = &pac_w {
                o.dwFlags = WINHTTP_AUTOPROXY_CONFIG_URL;
                o.lpszAutoConfigUrl = p.as_ptr();
            } else if auto {
                o.dwFlags = WINHTTP_AUTOPROXY_AUTO_DETECT;
                o.dwAutoDetectFlags = WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
            }
            let url_w = wide(url);
            let mut info: WINHTTP_PROXY_INFO = std::mem::zeroed();
            let ok = WinHttpGetProxyForUrl(session, url_w.as_ptr(), &mut o, &mut info) != 0;
            WinHttpCloseHandle(session);
            if !ok {
                return None;
            }
            let list = take(info.lpszProxy);
            let _ = take(info.lpszProxyBypass);
            if info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY {
                list.filter(|s| !s.trim().is_empty())
            } else {
                None
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    // Elsewhere reqwest's environment variables cover the system proxy.
    pub fn auto_configured() -> bool {
        false
    }
    pub fn resolve(_url: &str) -> Option<String> {
        None
    }
    // No WinHTTP to run a PAC script here: such downloads go direct.
    pub fn resolve_with(_url: &str, _cfg: (bool, Option<String>)) -> Option<String> {
        None
    }
    pub fn system_settings() -> (Option<String>, Option<String>) {
        let env = std::env::var("https_proxy").or_else(|_| std::env::var("HTTPS_PROXY"))
            .or_else(|_| std::env::var("http_proxy")).or_else(|_| std::env::var("HTTP_PROXY")).ok();
        (env.filter(|s| !s.trim().is_empty()), None)
    }
}

#[cfg(test)]
mod tests {
    use super::pick;

    #[test]
    fn pac_answers_are_read_per_scheme() {
        assert_eq!(pick("proxy:8080; backup:3128", "https://x/").as_deref(), Some("http://proxy:8080"));
        assert_eq!(pick("http=a:80;https=b:443", "https://x/").as_deref(), Some("http://b:443"));
        assert_eq!(pick("http=a:80;https=b:443", "http://x/").as_deref(), Some("http://a:80"));
        assert_eq!(pick("http=a:80", "https://x/").as_deref(), Some("http://a:80"));
        assert_eq!(pick("socks=s:1080", "https://x/"), None);
        assert_eq!(pick("", "https://x/"), None);
    }
}

