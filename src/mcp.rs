//! AI assistants (Model Context Protocol): `download_craft --mcp`.
//!
//! Claude Desktop, Claude Code, Cursor, VS Code and other MCP clients start
//! this as a program and talk JSON-RPC 2.0 to it, one message per line on
//! stdin and stdout. Its tools list, add, pause, resume and remove downloads
//! through the same shared files as the phone remote (`queue.json`,
//! `wake.json`, `cmd.json`), so they act on the app's own list; adding a
//! download starts the app when it is not running.
//!
//! Settings → AI assistants decides what an assistant may do: nothing (off),
//! look only, standard (add, pause, resume) or full (also remove). Every call
//! is written to `mcp-audit.log` in the settings folder.

use crate::model::{config_dir, DownloadItem, Status};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Off,
    Read,
    Standard,
    Full,
}

/// Settings' access level (`mcp_level` in ui.json; standard by default).
pub fn level() -> Level {
    if !crate::model::ui_pref_bool("mcp_enabled", true) {
        return Level::Off;
    }
    match crate::model::ui_pref_str("mcp_level", "standard").as_str() {
        "read" => Level::Read,
        "full" => Level::Full,
        _ => Level::Standard,
    }
}

fn audit(line: &str) {
    if let Some(p) = config_dir().map(|d| d.join("mcp-audit.log")) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "[{}] {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        }
    }
}

/// (name, description, minimum level, input schema)
fn tools() -> Vec<(&'static str, &'static str, Level, Value)> {
    let id = json!({ "type": "object", "properties": { "id": { "type": "integer", "description": "The download's id (from list_downloads)" } }, "required": ["id"], "additionalProperties": false });
    let none = json!({ "type": "object", "properties": {}, "additionalProperties": false });
    vec![
        ("list_downloads", "List the downloads in Download Craft: id, file name, status, size, progress and speed. Optionally only some statuses.", Level::Read,
            json!({ "type": "object", "properties": { "status": { "type": "string", "enum": ["all", "active", "queued", "paused", "done", "failed"], "description": "Which ones (default all)" } }, "additionalProperties": false })),
        ("get_download", "One download's details: address, folder, status, error.", Level::Read, id.clone()),
        ("add_download", "Add a download and start it: an http(s) or FTP link, a magnet link or a .torrent link.", Level::Standard,
            json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"], "additionalProperties": false })),
        ("add_downloads", "Add several downloads at once (up to 200) and start them.", Level::Standard,
            json!({ "type": "object", "properties": { "urls": { "type": "array", "items": { "type": "string" }, "maxItems": 200 } }, "required": ["urls"], "additionalProperties": false })),
        ("pause_download", "Pause one download (it can be resumed).", Level::Standard, id.clone()),
        ("resume_download", "Resume a paused or failed download.", Level::Standard, id.clone()),
        ("pause_all", "Pause every running download.", Level::Standard, none.clone()),
        ("resume_all", "Start the queue again.", Level::Standard, none),
        ("remove_download", "Take a download off the list (the file on disk stays).", Level::Full, id),
    ]
}

fn status_word(s: &Status) -> &'static str {
    match s {
        Status::Queued => "queued",
        Status::Downloading => "downloading",
        Status::Paused => "paused",
        Status::Completed => "done",
        Status::Failed(_) => "failed",
    }
}

fn summary(d: &DownloadItem) -> Value {
    json!({
        "id": d.id,
        "name": d.filename,
        "status": status_word(&d.status),
        "downloaded_bytes": d.downloaded_bytes,
        "total_bytes": d.total_bytes,
        "progress_percent": (d.progress() * 1000.0).round() / 10.0,
        "speed_bytes_per_s": d.speed_bps.round(),
        "torrent": crate::torrent::is_torrent(&d.url),
    })
}

fn queue() -> Vec<DownloadItem> {
    config_dir()
        .map(|d| d.join("queue.json"))
        .and_then(|p| crate::model::read_queue_items(&p).ok())
        .unwrap_or_default()
}

fn add_one(url: &str) -> Result<u64, String> {
    let url = url.trim();
    if !crate::model::is_download_url(url) {
        return Err(format!("not a download link or magnet: {url}"));
    }
    let prefs = crate::confirm::load_prefs();
    let name = crate::model::filename_from_url(url);
    let mut item = DownloadItem::new(0, url.to_string(), crate::engine::sanitize_filename(&name), prefs.save_dir.clone(), prefs.connections);
    match crate::model::append_to_queue(&mut item) {
        crate::model::AppendResult::Added(id) | crate::model::AppendResult::AlreadyQueued(id) => Ok(id),
        crate::model::AppendResult::Error(e) => Err(e),
    }
}

/// Run one tool. `Ok` is the result to show; `Err` an error result.
fn call(name: &str, args: &Value) -> Result<Value, String> {
    let lvl = level();
    let Some((_, _, need, _)) = tools().into_iter().find(|t| t.0 == name) else {
        return Err(format!("unknown tool: {name}"));
    };
    if lvl == Level::Off {
        return Err("AI assistants are switched off in Download Craft (Settings → AI assistants)".into());
    }
    if lvl < need {
        return Err(format!("not allowed at this access level (Settings → AI assistants allows {lvl:?}; {name} needs {need:?})"));
    }
    let id = || args.get("id").and_then(Value::as_u64).ok_or_else(|| "missing id".to_string());
    match name {
        "list_downloads" => {
            let want = args.get("status").and_then(Value::as_str).unwrap_or("all");
            let rows: Vec<Value> = queue()
                .iter()
                .rev()
                .filter(|d| match want {
                    "active" => matches!(d.status, Status::Downloading),
                    "queued" => matches!(d.status, Status::Queued),
                    "paused" => matches!(d.status, Status::Paused),
                    "done" => matches!(d.status, Status::Completed),
                    "failed" => matches!(d.status, Status::Failed(_)),
                    _ => true,
                })
                .map(summary)
                .collect();
            Ok(json!({ "count": rows.len(), "downloads": rows }))
        }
        "get_download" => {
            let id = id()?;
            let q = queue();
            let d = q.iter().find(|d| d.id == id).ok_or_else(|| format!("no download #{id}"))?;
            let mut v = summary(d);
            v["url"] = json!(d.url);
            v["folder"] = json!(d.save_dir);
            if let Status::Failed(e) = &d.status {
                v["error"] = json!(e);
            }
            Ok(v)
        }
        "add_download" => {
            let url = args.get("url").and_then(Value::as_str).ok_or("missing url")?;
            let id = add_one(url)?;
            crate::confirm::wake_ids(&[id]);
            Ok(json!({ "id": id, "started": true }))
        }
        "add_downloads" => {
            let urls: Vec<&str> = args.get("urls").and_then(Value::as_array).ok_or("missing urls")?.iter().filter_map(Value::as_str).take(200).collect();
            let mut ids = Vec::new();
            let mut errors = Vec::new();
            for u in urls {
                match add_one(u) {
                    Ok(id) => ids.push(id),
                    Err(e) => errors.push(e),
                }
            }
            if !ids.is_empty() {
                crate::confirm::wake_ids(&ids);
            }
            Ok(json!({ "added": ids, "errors": errors }))
        }
        "pause_download" => crate::native_host::app_control(Some(id()?), "pause").map(|_| json!({ "ok": true })),
        "resume_download" => {
            let id = id()?;
            let st = queue().into_iter().find(|d| d.id == id).map(|d| d.status);
            match st {
                Some(Status::Queued) | Some(Status::Failed(_)) => {
                    crate::confirm::wake_ids(&[id]);
                    Ok(json!({ "ok": true }))
                }
                Some(_) => crate::native_host::app_control(Some(id), "resume").map(|_| json!({ "ok": true })),
                None => Err(format!("no download #{id}")),
            }
        }
        "pause_all" => crate::native_host::app_control(None, "pause_all").map(|_| json!({ "ok": true })),
        "resume_all" => crate::native_host::app_control(None, "resume_all").map(|_| json!({ "ok": true })),
        "remove_download" => crate::native_host::app_control(Some(id()?), "remove").map(|_| json!({ "ok": true })),
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// Answer one JSON-RPC message; `None` for a notification.
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let id = id?; // notifications (initialized, cancelled) get no answer
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "download-craft", "title": "Download Craft", "version": crate::model::APP_VERSION },
            "instructions": "Download Craft is a download manager. Use list_downloads to see what is there; add_download for a link, a magnet or a .torrent link."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let lvl = level();
            let list: Vec<Value> = tools()
                .into_iter()
                .filter(|t| lvl != Level::Off && lvl >= t.2)
                .map(|(n, d, _, s)| json!({ "name": n, "description": d, "inputSchema": s }))
                .collect();
            Ok(json!({ "tools": list }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let r = call(name, &args);
            audit(&format!("{name} {} -> {}", args, if r.is_ok() { "ok" } else { "refused/failed" }));
            Ok(match r {
                Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }], "structuredContent": v, "isError": false }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    })
}

/// The stdio loop: one JSON message per line in, one per line out.
pub fn run() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() || line.len() > 1 << 20 {
            continue;
        }
        let line = line.trim_start_matches('\u{feff}');
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(msg) => handle(&msg),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
}

/// The snippet Settings shows for an assistant's configuration file.
pub fn config_snippet() -> String {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "download_craft".into());
    let cfg = json!({ "mcpServers": { "download-craft": { "command": exe, "args": ["--mcp"] } } });
    serde_json::to_string_pretty(&cfg).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list_tools() {
        let r = handle(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).unwrap();
        assert_eq!(r["result"]["serverInfo"]["name"], "download-craft");
        let r = handle(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        assert!(r["result"]["tools"].as_array().is_some());
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        let r = handle(&json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" })).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn levels_order() {
        assert!(Level::Full > Level::Standard && Level::Standard > Level::Read && Level::Read > Level::Off);
    }
}
