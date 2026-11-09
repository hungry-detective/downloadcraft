//! Where downloads go out, and who answers the DNS.
//!
//! - **Network adapter lock (VPN only).** Settings → Network can hold every
//!   download to one adapter, usually the VPN's. Each connection is bound to
//!   that adapter's address. It doubles as a kill switch: while the adapter is
//!   not connected (the VPN is off), a download does not start at all, and a
//!   running one fails instead of falling back to the normal connection. The
//!   app starts those again once the adapter is back.
//! - **Encrypted DNS.** Server names can be looked up over DNS-over-HTTPS or
//!   DNS-over-TLS (Cloudflare, Google, Quad9, AdGuard) instead of the
//!   system's DNS, which the network can read and change. With an adapter
//!   chosen, the DNS queries go through it too.
//!
//! Both are read from `ui.json` (`net_adapter`, `dns_mode`) wherever an HTTP
//! client is built, so the app, the command line and server mode all follow.

use hickory_resolver::config::{NameServerConfig, ResolverConfig, CLOUDFLARE, GOOGLE, QUAD9};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

/// The text a failed download carries while the chosen adapter is down; the
/// app looks for it to start those downloads again later.
pub const NOT_CONNECTED: &str = "is not connected (Settings → Network)";

/// The computer's network adapters with an IPv4 address: (name, address).
pub fn adapters() -> Vec<(String, IpAddr)> {
    let mut v: Vec<(String, IpAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && i.ip().is_ipv4())
        .map(|i| (i.name.clone(), i.ip()))
        .collect();
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

/// The adapter Settings holds downloads to (empty: any).
pub fn chosen() -> String {
    crate::model::ui_pref_str("net_adapter", "").trim().to_string()
}

/// The address to bind to: `Ok(None)` when no adapter is chosen, `Err` with
/// the kill switch's message when the chosen one is not connected now.
pub fn bind_ip() -> Result<Option<IpAddr>, String> {
    let name = chosen();
    if name.is_empty() {
        return Ok(None);
    }
    adapters()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ip)| Some(ip))
        .ok_or_else(|| format!("the network adapter \"{name}\" {NOT_CONNECTED}"))
}

/// DNS choices: (key saved in ui.json, name shown in Settings).
pub const DNS_CHOICES: [(&str, &str); 7] = [
    ("system", "System's DNS"),
    ("cloudflare-https", "Cloudflare, over HTTPS"),
    ("google-https", "Google, over HTTPS"),
    ("quad9-https", "Quad9, over HTTPS"),
    ("adguard-https", "AdGuard, over HTTPS"),
    ("cloudflare-tls", "Cloudflare, over TLS"),
    ("quad9-tls", "Quad9, over TLS"),
];

fn dns_config(mode: &str, bind: Option<IpAddr>) -> Option<ResolverConfig> {
    let mut cfg = match mode {
        "cloudflare-https" => ResolverConfig::https(&CLOUDFLARE),
        "google-https" => ResolverConfig::https(&GOOGLE),
        "quad9-https" => ResolverConfig::https(&QUAD9),
        "cloudflare-tls" => ResolverConfig::tls(&CLOUDFLARE),
        "quad9-tls" => ResolverConfig::tls(&QUAD9),
        "adguard-https" => ResolverConfig::from_name_servers(
            ["94.140.14.14", "94.140.15.15"]
                .iter()
                .filter_map(|ip| ip.parse::<IpAddr>().ok())
                .map(|ip| NameServerConfig::https(ip, Arc::from("dns.adguard-dns.com"), Some(Arc::from("/dns-query"))))
                .collect(),
        ),
        _ => return None,
    };
    // IPv4 servers only (downloads go out over IPv4), and bound to the
    // chosen adapter when there is one.
    let (domain, search, servers) = cfg.clone().into_parts();
    let servers: Vec<NameServerConfig> = servers
        .into_iter()
        .filter(|s| s.ip.is_ipv4())
        .map(|mut s| {
            if let Some(ip) = bind {
                for c in s.connections.iter_mut() {
                    c.bind_addr = Some(SocketAddr::new(ip, 0));
                }
            }
            s
        })
        .collect();
    cfg = ResolverConfig::from_parts(domain, search, servers);
    Some(cfg)
}

/// reqwest's resolver over an encrypted DNS service.
pub(crate) struct Encrypted(TokioResolver);

impl reqwest::dns::Resolve for Encrypted {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let r = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = r.lookup_ip(host.as_str()).await.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
            let addrs: Vec<SocketAddr> = found.iter().filter(|ip| ip.is_ipv4()).map(|ip| SocketAddr::new(ip, 0)).collect();
            if addrs.is_empty() {
                return Err(format!("no IPv4 address for {host}").into());
            }
            let it: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(it)
        })
    }
}

/// One resolver per (DNS choice, bound address), kept for reuse.
static CACHE: Mutex<Option<(String, Arc<Encrypted>)>> = Mutex::new(None);

/// The resolver for Settings' DNS choice, or `None` for the system's.
pub(crate) fn resolver(bind: Option<IpAddr>) -> Option<Arc<Encrypted>> {
    let mode = crate::model::ui_pref_str("dns_mode", "system");
    let key = format!("{mode}|{bind:?}");
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, r)) = c.as_ref() {
        if *k == key {
            return Some(r.clone());
        }
    }
    let cfg = dns_config(&mode, bind)?;
    let resolver = TokioResolver::builder_with_config(cfg, TokioRuntimeProvider::default()).build().ok()?;
    let r = Arc::new(Encrypted(resolver));
    *c = Some((key, r.clone()));
    Some(r)
}

/// One line for the status bar: the adapter downloads are held to, and
/// whether it is connected (empty when none is chosen).
pub fn status_line() -> String {
    let name = chosen();
    if name.is_empty() {
        return String::new();
    }
    match bind_ip() {
        Ok(Some(ip)) => format!("Via {name} ({ip})"),
        _ => format!("{name} is not connected — downloads wait"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encrypted_choice_has_ipv4_servers() {
        for (key, _) in DNS_CHOICES.iter().skip(1) {
            let cfg = dns_config(key, None).expect(key);
            assert!(!cfg.name_servers().is_empty(), "{key}");
            assert!(cfg.name_servers().iter().all(|s| s.ip.is_ipv4()), "{key}");
        }
        assert!(dns_config("system", None).is_none());
    }

    #[test]
    fn binding_reaches_every_dns_connection() {
        let ip: IpAddr = "10.8.0.2".parse().unwrap();
        let cfg = dns_config("quad9-tls", Some(ip)).unwrap();
        for s in cfg.name_servers() {
            for c in &s.connections {
                assert_eq!(c.bind_addr, Some(SocketAddr::new(ip, 0)));
            }
        }
    }
}
