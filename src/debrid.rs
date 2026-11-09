//! Debrid accounts: a file host's link (Rapidgator, 1fichier, Nitroflare…)
//! turned into a fast direct link through the user's own account at
//! Real-Debrid, AllDebrid or Premiumize, with each service's official API.
//!
//! Settings → Debrid holds the service and its API key (in the settings
//! file, like the site logins). Only links of the file hosts below go
//! through it; anything else downloads as usual.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

/// Hosts whose links go through the debrid account.
const HOSTS: &[&str] = &[
    "rapidgator.net", "rg.to", "nitroflare.com", "nitro.download", "1fichier.com", "turbobit.net", "uploaded.net", "ul.to",
    "katfile.com", "ddownload.com", "ddl.to", "filefactory.com", "uptobox.com", "hitfile.net", "keep2share.cc", "k2s.cc",
    "fboom.me", "tezfiles.com", "uploadgig.com", "alfafile.net", "mexa.sh", "wupfile.com", "clicknupload.click",
    "dailyuploads.net", "fikper.com", "hexload.com", "drop.download", "usersdrive.com", "filespace.com", "worldbytez.com",
    "mega.nz", "mediafire.com",
];

/// Services: (key saved in ui.json, name shown).
pub const SERVICES: [(&str, &str); 4] = [("", "None"), ("realdebrid", "Real-Debrid"), ("alldebrid", "AllDebrid"), ("premiumize", "Premiumize")];

fn service() -> (String, String) {
    (
        crate::model::ui_pref_str("debrid_service", "").trim().to_string(),
        crate::model::ui_pref_str("debrid_key", "").trim().to_string(),
    )
}

/// Should `url` go through the debrid account?
pub fn applies(url: &str) -> bool {
    let (svc, key) = service();
    if svc.is_empty() || key.is_empty() {
        return false;
    }
    let Ok(u) = url::Url::parse(url.trim()) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase();
    HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// The direct link (and the file's name when the service gives it).
pub async fn unlock(client: &reqwest::Client, url: &str) -> Result<(String, Option<String>)> {
    let (svc, key) = service();
    let url = url.trim();
    let v: Value = match svc.as_str() {
        "realdebrid" => client
            .post("https://api.real-debrid.com/rest/1.0/unrestrict/link")
            .bearer_auth(&key)
            .form(&[("link", url)])
            .send()
            .await?
            .json()
            .await
            .context("Real-Debrid answered something unexpected")?,
        "alldebrid" => client
            .get("https://api.alldebrid.com/v4/link/unlock")
            .bearer_auth(&key)
            .query(&[("agent", "DownloadCraft"), ("apikey", key.as_str()), ("link", url)])
            .send()
            .await?
            .json()
            .await
            .context("AllDebrid answered something unexpected")?,
        "premiumize" => client
            .post("https://www.premiumize.me/api/transfer/directdl")
            .form(&[("apikey", key.as_str()), ("src", url)])
            .send()
            .await?
            .json()
            .await
            .context("Premiumize answered something unexpected")?,
        _ => return Err(anyhow!("no debrid service is set")),
    };
    parse(&svc, &v)
}

fn parse(svc: &str, v: &Value) -> Result<(String, Option<String>)> {
    let s = |x: &Value| x.as_str().map(str::to_string);
    match svc {
        "realdebrid" => {
            if let Some(link) = v.get("download").and_then(s) {
                return Ok((link, v.get("filename").and_then(s)));
            }
            Err(anyhow!("Real-Debrid: {}", v.get("error").and_then(s).unwrap_or_else(|| "refused the link".into())))
        }
        "alldebrid" => {
            if v.get("status").and_then(|x| x.as_str()) == Some("success") {
                let d = v.get("data").cloned().unwrap_or(Value::Null);
                if let Some(link) = d.get("link").and_then(s).filter(|l| !l.is_empty()) {
                    return Ok((link, d.get("filename").and_then(s)));
                }
            }
            let msg = v.get("error").and_then(|e| e.get("message")).and_then(s).unwrap_or_else(|| "refused the link".into());
            Err(anyhow!("AllDebrid: {msg}"))
        }
        "premiumize" => {
            if v.get("status").and_then(|x| x.as_str()) == Some("success") {
                if let Some(first) = v.get("content").and_then(|c| c.as_array()).and_then(|a| a.first()) {
                    if let Some(link) = first.get("link").and_then(s) {
                        let name = first.get("path").and_then(s).map(|p| p.rsplit('/').next().unwrap_or(&p).to_string());
                        return Ok((link, name));
                    }
                }
            }
            Err(anyhow!("Premiumize: {}", v.get("message").and_then(s).unwrap_or_else(|| "refused the link".into())))
        }
        _ => Err(anyhow!("no debrid service is set")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_each_service() {
        let (l, n) = parse("realdebrid", &json!({"download": "https://x/y.zip", "filename": "y.zip"})).unwrap();
        assert_eq!((l.as_str(), n.as_deref()), ("https://x/y.zip", Some("y.zip")));
        assert!(parse("realdebrid", &json!({"error": "bad_token"})).unwrap_err().to_string().contains("bad_token"));
        let (l, _) = parse("alldebrid", &json!({"status": "success", "data": {"link": "https://a/b", "filename": "b"}})).unwrap();
        assert_eq!(l, "https://a/b");
        let (l, n) = parse("premiumize", &json!({"status": "success", "content": [{"link": "https://p/q", "path": "dir/q.mkv"}]})).unwrap();
        assert_eq!((l.as_str(), n.as_deref()), ("https://p/q", Some("q.mkv")));
    }
}
