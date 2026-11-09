//! Where a download's server is, and how far: shown in the progress window
//! as "Server: Germany · about 24 ms (near)".
//!
//! - The country comes from DB-IP's free IP-to-Country Lite table (CC BY 4.0,
//!   <https://db-ip.com>), downloaded once into the settings folder and
//!   refreshed monthly; the lookup happens on this computer, nothing is sent
//!   anywhere to ask.
//! - The time is one TCP connection's set-up to the server (a single round
//!   trip): near under 60 ms, medium to 150 ms, far above.
//! - Through a proxy only the proxy is seen, so the line says so; a server on
//!   this computer or the local network is named as such.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, OnceLock, RwLock};

/// IPv4 ranges (start, end, country code), sorted by start.
type Table = Vec<(u32, u32, [u8; 2])>;

static TABLE: OnceLock<RwLock<Option<Arc<Table>>>> = OnceLock::new();

fn table_slot() -> &'static RwLock<Option<Arc<Table>>> {
    TABLE.get_or_init(|| RwLock::new(None))
}

fn table_path() -> Option<std::path::PathBuf> {
    crate::model::config_dir().map(|d| d.join("geo").join("dbip-country-lite.csv.gz"))
}

fn parse(gz: &[u8]) -> Option<Table> {
    use std::io::Read;
    let mut text = String::new();
    flate2::read::GzDecoder::new(gz).read_to_string(&mut text).ok()?;
    let mut t: Table = Vec::with_capacity(300_000);
    for line in text.lines() {
        let mut f = line.split(',');
        let (Some(a), Some(b), Some(c)) = (f.next(), f.next(), f.next()) else { continue };
        let (Ok(a), Ok(b)) = (a.parse::<Ipv4Addr>(), b.parse::<Ipv4Addr>()) else { continue };
        let c = c.trim().as_bytes();
        if c.len() == 2 {
            t.push((u32::from(a), u32::from(b), [c[0], c[1]]));
        }
    }
    t.sort_by_key(|r| r.0);
    (!t.is_empty()).then_some(t)
}

/// Load the table from disk, fetching it first when it is missing or older
/// than 40 days. Quiet on failure (the line then shows no country).
async fn table(client: &reqwest::Client) -> Option<Arc<Table>> {
    if let Some(t) = table_slot().read().ok().and_then(|g| g.clone()) {
        return Some(t);
    }
    let path = table_path()?;
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < 40 * 86_400);
    if !fresh {
        let now = chrono::Local::now();
        for months_back in 0..2 {
            let d = now - chrono::Duration::days(31 * months_back);
            let url = format!("https://download.db-ip.com/free/dbip-country-lite-{}.csv.gz", d.format("%Y-%m"));
            if let Ok(r) = client.get(&url).send().await {
                if r.status().is_success() {
                    if let Ok(bytes) = r.bytes().await {
                        if bytes.len() > 100_000 && parse(&bytes).is_some() {
                            if let Some(dir) = path.parent() {
                                let _ = std::fs::create_dir_all(dir);
                            }
                            let _ = crate::model::atomic_write(&path, &bytes);
                            break;
                        }
                    }
                }
            }
        }
    }
    let bytes = std::fs::read(&path).ok()?;
    let t = Arc::new(parse(&bytes)?);
    if let Ok(mut g) = table_slot().write() {
        *g = Some(t.clone());
    }
    Some(t)
}

fn lookup(t: &Table, ip: Ipv4Addr) -> Option<[u8; 2]> {
    let x = u32::from(ip);
    let i = t.partition_point(|r| r.0 <= x);
    let r = t.get(i.checked_sub(1)?)?;
    (x <= r.1).then_some(r.2)
}

/// [`measure`] with an owned address, for `tokio::spawn`.
pub async fn measure_owned(url: String, proxied: bool, client: reqwest::Client, out: Arc<std::sync::Mutex<String>>) {
    measure(&url, proxied, client, out).await
}

/// "near" / "medium" / "far" for a round trip.
pub fn reach(ms: u128) -> &'static str {
    match ms {
        0..=59 => "near",
        60..=150 => "medium",
        _ => "far",
    }
}

fn local_kind(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v) if v.is_loopback() => Some("this computer"),
        IpAddr::V4(v) if v.is_private() || v.is_link_local() => Some("the local network"),
        IpAddr::V6(v) if v.is_loopback() => Some("this computer"),
        _ => None,
    }
}

/// Work out the server line for `url` and store it in `out` (the shared
/// state the progress window reads). Runs beside the download.
pub async fn measure(url: &str, proxied: bool, client: reqwest::Client, out: Arc<std::sync::Mutex<String>>) {
    let set = |s: String| {
        if let Ok(mut g) = out.lock() {
            *g = s;
        }
    };
    if proxied {
        set("through a proxy".into());
        return;
    }
    let Ok(u) = url::Url::parse(url) else { return };
    let (Some(host), Some(port)) = (u.host_str().map(str::to_string), u.port_or_known_default()) else { return };
    let addrs: Vec<std::net::SocketAddr> = match tokio::net::lookup_host((host.as_str(), port)).await {
        Ok(a) => a.filter(|a| a.is_ipv4()).collect(),
        Err(_) => return,
    };
    let Some(addr) = addrs.first().copied() else { return };
    let started = std::time::Instant::now();
    let ok = matches!(
        tokio::time::timeout(std::time::Duration::from_secs(4), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    );
    let ms = started.elapsed().as_millis();
    let time = if ok { format!("about {ms} ms ({})", reach(ms)) } else { String::new() };
    if let Some(kind) = local_kind(addr.ip()) {
        set(if time.is_empty() { kind.to_string() } else { format!("{kind} · {time}") });
        return;
    }
    let country = match addr.ip() {
        IpAddr::V4(v4) => table(&client).await.and_then(|t| lookup(&t, v4)),
        IpAddr::V6(_) => None,
    };
    let place = country.map(|c| country_name(&c)).unwrap_or_default();
    set(match (place.is_empty(), time.is_empty()) {
        (false, false) => format!("{place} · {time}"),
        (false, true) => place,
        (true, false) => time,
        (true, true) => String::new(),
    });
}

/// A country's English name from its ISO 3166 code (the code itself when
/// it is not in the list).
pub fn country_name(code: &[u8; 2]) -> String {
    let c = std::str::from_utf8(code).unwrap_or("").to_ascii_uppercase();
    NAMES
        .split('|')
        .find_map(|e| e.strip_prefix(&format!("{c}:")).map(str::to_string))
        .unwrap_or(c)
}

const NAMES: &str = "AD:Andorra|AE:United Arab Emirates|AF:Afghanistan|AG:Antigua and Barbuda|AL:Albania|AM:Armenia|AO:Angola|AR:Argentina|AT:Austria|AU:Australia|AZ:Azerbaijan|BA:Bosnia and Herzegovina|BB:Barbados|BD:Bangladesh|BE:Belgium|BF:Burkina Faso|BG:Bulgaria|BH:Bahrain|BI:Burundi|BJ:Benin|BN:Brunei|BO:Bolivia|BR:Brazil|BS:Bahamas|BT:Bhutan|BW:Botswana|BY:Belarus|BZ:Belize|CA:Canada|CD:DR Congo|CF:Central African Republic|CG:Congo|CH:Switzerland|CI:Côte d'Ivoire|CL:Chile|CM:Cameroon|CN:China|CO:Colombia|CR:Costa Rica|CU:Cuba|CV:Cabo Verde|CY:Cyprus|CZ:Czechia|DE:Germany|DJ:Djibouti|DK:Denmark|DM:Dominica|DO:Dominican Republic|DZ:Algeria|EC:Ecuador|EE:Estonia|EG:Egypt|ER:Eritrea|ES:Spain|ET:Ethiopia|FI:Finland|FJ:Fiji|FR:France|GA:Gabon|GB:United Kingdom|GD:Grenada|GE:Georgia|GH:Ghana|GM:Gambia|GN:Guinea|GQ:Equatorial Guinea|GR:Greece|GT:Guatemala|GW:Guinea-Bissau|GY:Guyana|HK:Hong Kong|HN:Honduras|HR:Croatia|HT:Haiti|HU:Hungary|ID:Indonesia|IE:Ireland|IL:Israel|IN:India|IQ:Iraq|IR:Iran|IS:Iceland|IT:Italy|JM:Jamaica|JO:Jordan|JP:Japan|KE:Kenya|KG:Kyrgyzstan|KH:Cambodia|KM:Comoros|KN:Saint Kitts and Nevis|KP:North Korea|KR:South Korea|KW:Kuwait|KZ:Kazakhstan|LA:Laos|LB:Lebanon|LC:Saint Lucia|LI:Liechtenstein|LK:Sri Lanka|LR:Liberia|LS:Lesotho|LT:Lithuania|LU:Luxembourg|LV:Latvia|LY:Libya|MA:Morocco|MC:Monaco|MD:Moldova|ME:Montenegro|MG:Madagascar|MK:North Macedonia|ML:Mali|MM:Myanmar|MN:Mongolia|MO:Macao|MR:Mauritania|MT:Malta|MU:Mauritius|MV:Maldives|MW:Malawi|MX:Mexico|MY:Malaysia|MZ:Mozambique|NA:Namibia|NE:Niger|NG:Nigeria|NI:Nicaragua|NL:Netherlands|NO:Norway|NP:Nepal|NZ:New Zealand|OM:Oman|PA:Panama|PE:Peru|PG:Papua New Guinea|PH:Philippines|PK:Pakistan|PL:Poland|PR:Puerto Rico|PS:Palestine|PT:Portugal|PY:Paraguay|QA:Qatar|RO:Romania|RS:Serbia|RU:Russia|RW:Rwanda|SA:Saudi Arabia|SB:Solomon Islands|SC:Seychelles|SD:Sudan|SE:Sweden|SG:Singapore|SI:Slovenia|SK:Slovakia|SL:Sierra Leone|SN:Senegal|SO:Somalia|SR:Suriname|SS:South Sudan|SV:El Salvador|SY:Syria|SZ:Eswatini|TD:Chad|TG:Togo|TH:Thailand|TJ:Tajikistan|TL:Timor-Leste|TM:Turkmenistan|TN:Tunisia|TR:Türkiye|TT:Trinidad and Tobago|TW:Taiwan|TZ:Tanzania|UA:Ukraine|UG:Uganda|US:United States|UY:Uruguay|UZ:Uzbekistan|VE:Venezuela|VN:Vietnam|YE:Yemen|ZA:South Africa|ZM:Zambia|ZW:Zimbabwe";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_names() {
        let t: Table = vec![(u32::from(Ipv4Addr::new(1, 0, 0, 0)), u32::from(Ipv4Addr::new(1, 0, 0, 255)), *b"AU"), (u32::from(Ipv4Addr::new(8, 8, 8, 0)), u32::from(Ipv4Addr::new(8, 8, 8, 255)), *b"US")];
        assert_eq!(lookup(&t, Ipv4Addr::new(8, 8, 8, 8)), Some(*b"US"));
        assert_eq!(lookup(&t, Ipv4Addr::new(1, 0, 0, 9)), Some(*b"AU"));
        assert_eq!(lookup(&t, Ipv4Addr::new(2, 0, 0, 1)), None);
        assert_eq!(country_name(b"bd"), "Bangladesh");
        assert_eq!(country_name(b"ZZ"), "ZZ");
        assert_eq!(reach(24), "near");
        assert_eq!(reach(151), "far");
    }
}
