//! YouTube read by the app itself, with yt-dlp as the fallback.
//!
//! The same requests yt-dlp makes for a watch page, done here so a video
//! starts in seconds on the app's own multi-connection engine:
//! 1. the watch page (as a browser) for the visitor id, the player script's
//!    address and its signature timestamp;
//! 2. the player API as a smart-TV client (`TVHTML5`), which answers with
//!    plain `https` stream links for every quality, 4K included, that serve
//!    the whole file to any client (the WEB/ANDROID links stop after about a
//!    minute above 360p);
//! 3. the player script, and one run of yt-dlp's own challenge solver
//!    (`yt-dlp/ejs`, public domain, embedded at a pinned release) in Deno
//!    with no permissions, which turns each link's `n` value (and, for some
//!    videos, its ciphered signature) into the one the server accepts.
//!
//! Videos with a pre-roll ad answer 403 until the ad's length has passed,
//! as a TV would play it: the reading waits that long before handing the
//! links out (what yt-dlp calls `available_at`).
//!
//! Anything this cannot read (a sign-in, an age gate, a live stream, a
//! changed client) returns an error and yt-dlp reads the page as before.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::engine::{EngineOpts, ResolvedMedia, SharedState};

/// yt-dlp's `tv_samsung` client (yt-dlp 2026.10.26).
const TV_CLIENT_NAME: &str = "TVHTML5";
const TV_CLIENT_VERSION: &str = "5.20260707";
const TV_CLIENT_ID: &str = "7";
const TV_AGENT: &str = "Mozilla/5.0 (SMART-TV; Linux; Tizen 2.4.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/2.4.0 TV Safari/538.1";
const PAGE_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
/// The solver, yt-dlp/ejs 0.8.0 (SHA-256 c55987fe… and 18da6ce0…).
const SOLVER_LIB: &str = include_str!("../assets/ejs/yt.solver.lib.min.js");
const SOLVER_CORE: &str = include_str!("../assets/ejs/yt.solver.core.min.js");
/// The longest pre-roll wait honoured; yt-dlp takes over past it.
const MOST_AD_WAIT: u64 = 60;
const PAGE_MOST: usize = 8 << 20;

/// Whether a stream link came from this reader (the TV client): those serve
/// bigger ranges than the browser's, so the engine asks in bigger pieces.
pub fn tv_link(url: &str) -> bool {
    url.contains("googlevideo.com") && url.contains("c=TVHTML5")
}

/// One stream of the player's answer.
#[derive(Debug, Clone)]
struct Format {
    itag: u64,
    url: Option<String>,
    cipher: Option<String>,
    container: String,
    video: bool,
    width: u64,
    height: u64,
    fps: u64,
    bitrate: u64,
    /// A dub or a loudness variant: listed, never chosen.
    secondary: bool,
}

impl Format {
    fn of(f: &Value) -> Option<Format> {
        if f.get("drmFamilies").is_some()
            || f.get("type").and_then(Value::as_str) == Some("FORMAT_STREAM_TYPE_OTF")
            || f.get("targetDurationSec").is_some()
        {
            return None;
        }
        let mime = f.get("mimeType")?.as_str()?;
        let (kind, rest) = mime.split_once('/')?;
        let container = rest.split(';').next()?.trim().to_ascii_lowercase();
        let video = match kind {
            "video" => true,
            "audio" => false,
            _ => return None,
        };
        let number = |key: &str| f.get(key).and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()));
        let url = f.get("url").and_then(Value::as_str).map(str::to_string);
        let cipher = f
            .get("signatureCipher")
            .or_else(|| f.get("cipher"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if url.is_none() && cipher.is_none() {
            return None;
        }
        let dubbed = f.pointer("/audioTrack/audioIsDefault").and_then(Value::as_bool) == Some(false);
        let drc = f.get("isDrc").and_then(Value::as_bool).unwrap_or(false);
        Some(Format {
            itag: number("itag")?,
            url,
            cipher,
            container,
            video,
            width: number("width").unwrap_or(0),
            height: number("height").unwrap_or(0),
            fps: number("fps").unwrap_or(0),
            bitrate: number("averageBitrate").or_else(|| number("bitrate")).unwrap_or(0),
            secondary: dubbed || drc,
        })
    }

    /// The side a quality names: the shorter one (a Short's 720p is 720x1280).
    fn side(&self) -> u64 {
        if self.width > 0 && self.width < self.height {
            self.width
        } else {
            self.height
        }
    }
}

/// The picture at or under `cap` (the closest above when every one is
/// bigger), MP4 before WebM at the same size so the result stays an .mp4,
/// and the best sound in the picture's own container.
fn choose(formats: &[Format], cap: Option<u64>) -> Option<(Format, Option<Format>)> {
    let usable: Vec<&Format> = formats.iter().filter(|f| !f.secondary).collect();
    let pictures: Vec<&&Format> = usable.iter().filter(|f| f.video && f.height > 0).collect();
    let fits = |f: &Format| cap.map(|c| f.side() <= c).unwrap_or(true);
    let side = if pictures.iter().any(|f| fits(f)) {
        pictures.iter().filter(|f| fits(f)).map(|f| f.side()).max()?
    } else {
        pictures.iter().map(|f| f.side()).min()?
    };
    let picture = pictures
        .iter()
        .filter(|f| f.side() == side)
        .max_by_key(|f| (f.container == "mp4", f.fps, f.bitrate))?;
    let sound_in = |container: &str| {
        usable
            .iter()
            .filter(|f| !f.video && f.container == container)
            .max_by_key(|f| f.bitrate)
            .map(|f| (*f).clone())
    };
    let sound = match picture.container.as_str() {
        "webm" => sound_in("webm").or_else(|| sound_in("mp4")),
        _ => sound_in("mp4").or_else(|| sound_in("webm")),
    };
    Some(((**picture).clone(), sound))
}

/// The best sound track on its own: M4A first (it plays everywhere and
/// converts to MP3), the best WebM/Opus where the video has no M4A.
fn choose_sound(formats: &[Format]) -> Option<Format> {
    let best_in = |container: &str| {
        formats
            .iter()
            .filter(|f| !f.video && !f.secondary && f.container == container)
            .max_by_key(|f| f.bitrate)
            .cloned()
    };
    best_in("mp4").or_else(|| best_in("webm"))
}

/// What the watch page tells the player API.
#[derive(Debug, Default, PartialEq)]
struct PageFacts {
    visitor: Option<String>,
    player_path: Option<String>,
    sts: Option<u64>,
    title: Option<String>,
}

fn page_facts(html: &str) -> PageFacts {
    let text = |key: &str| {
        let mark = format!("\"{key}\":\"");
        let at = html.find(&mark)? + mark.len();
        let end = html[at..].find('"')?;
        Some(html[at..at + end].to_string()).filter(|s| !s.is_empty())
    };
    let sts = html.find("\"STS\":").and_then(|at| {
        let digits: String = html[at + 6..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    });
    let player_path = text("PLAYER_JS_URL")
        .map(|u| u.strip_prefix("https://www.youtube.com").map(str::to_string).unwrap_or(u))
        .filter(|p| player_path_ok(p));
    PageFacts {
        visitor: text("VISITOR_DATA").or_else(|| text("visitorData")),
        player_path,
        sts,
        title: page_title(html),
    }
}

/// Only YouTube's own player script is ever run: `/s/player/<id>/….js`.
fn player_path_ok(path: &str) -> bool {
    path.starts_with("/s/player/")
        && path.ends_with(".js")
        && !path.contains("..")
        && !path.contains("//")
        && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
}

/// Decode the JSON string literal that starts just after its opening quote
/// (`s` begins inside the literal). Done through the JSON parser so `\uXXXX`
/// escapes decode properly, surrogate pairs (emoji) included — the hand
/// rolled version turned those into nothing.
fn json_string_at(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'"' => {
                let literal = format!("\"{}\"", &s[..i]);
                return serde_json::from_str::<String>(&literal).ok();
            }
            _ => {}
        }
    }
    None
}

/// The title from the page's player data, else its `<title>`.
fn page_title(html: &str) -> Option<String> {
    const MARK: &str = "\"videoDetails\":{";
    if let Some(at) = html.find(MARK) {
        let rest = &html[at..html.len().min(at + 4096)];
        if let Some(t) = rest.find("\"title\":\"") {
            if let Some(out) = json_string_at(&rest[t + 9..]) {
                if !out.trim().is_empty() {
                    return Some(out);
                }
            }
        }
    }
    let start = html.find("<title>")? + 7;
    let end = html[start..].find("</title>")? + start;
    let title = html[start..end].trim();
    let title = title.strip_suffix(" - YouTube").unwrap_or(title);
    let title = title
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    (!title.is_empty() && title != "YouTube").then_some(title)
}

/// A title that is only a compact counter ("50K views", "6.6K likes") is
/// page chrome, not a name. A real numeric title ("50k", "6.6k") is kept.
fn is_metric_title(t: &str) -> bool {
    let mut parts = t.split_whitespace();
    let Some(first) = parts.next() else { return false };
    let Some(unit) = parts.next() else { return false };
    if parts.next().is_some() {
        return false;
    }
    let unit = unit.to_ascii_lowercase();
    if !matches!(
        unit.as_str(),
        "view" | "views" | "like" | "likes" | "subscriber" | "subscribers"
            | "comment" | "comments" | "share" | "shares"
    ) {
        return false;
    }
    if first.eq_ignore_ascii_case("no") {
        return true;
    }
    let count = first.trim_end_matches(['K', 'k', 'M', 'm', 'B', 'b']);
    !count.is_empty()
        && count
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// Player metadata is authoritative: a video really titled "50 Views" keeps
/// that name. The page-derived fallback is checked for counters, because
/// there the title can be YouTube's "50K views" chrome.
fn video_title(player: &serde_json::Value, facts: &PageFacts, id: &str) -> String {
    player
        .pointer("/videoDetails/title")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            facts
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !is_metric_title(s))
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("youtube_{id}"))
}

/// Seconds of pre-roll ads the player must play before the streams are
/// served: each start ad's length, or its skip offset when it has one
/// (yt-dlp's `_get_available_at_timestamp`).
fn ad_wait(player: &Value) -> u64 {
    fn ad_seconds(renderer: &Value) -> Option<u64> {
        if let Some(skip) = renderer.get("skipOffsetMilliseconds").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())) {
            return Some(skip.div_ceil(1000));
        }
        let vars = renderer.get("playerVars")?.as_str()?;
        url::form_urlencoded::parse(vars.as_bytes())
            .filter(|(k, _)| k == "length_seconds")
            .last()
            .and_then(|(_, v)| v.parse().ok())
    }
    let mut renderers = Vec::new();
    for p in player.get("adPlacements").and_then(Value::as_array).into_iter().flatten() {
        if p.pointer("/adPlacementRenderer/config/adPlacementConfig/kind").and_then(Value::as_str)
            == Some("AD_PLACEMENT_KIND_START")
        {
            if let Some(r) = p.pointer("/adPlacementRenderer/renderer/instreamVideoAdRenderer") {
                renderers.push(r);
            }
        }
    }
    for s in player.get("adSlots").and_then(Value::as_array).into_iter().flatten() {
        if s.pointer("/adSlotRenderer/adSlotMetadata/triggerEvent").and_then(Value::as_str)
            != Some("SLOT_TRIGGER_EVENT_BEFORE_CONTENT")
        {
            continue;
        }
        let Some(content) = s.pointer(
            "/adSlotRenderer/fulfillmentContent/fulfilledLayout/playerBytesAdLayoutRenderer/renderingContent",
        ) else {
            continue;
        };
        if let Some(r) = content.get("instreamVideoAdRenderer") {
            renderers.push(r);
        }
        for layout in content
            .pointer("/playerBytesSequentialLayoutRenderer/sequentialLayouts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(r) = layout.pointer("/playerBytesAdLayoutRenderer/renderingContent/instreamVideoAdRenderer") {
                renderers.push(r);
            }
        }
    }
    renderers.into_iter().filter_map(ad_seconds).sum()
}

/// The player script (2-3 MB) and the solver's answers, kept for the
/// session: a re-resolve of an expired link, or the next video on the same
/// player, costs neither the fetch nor the JavaScript run.
#[derive(Default)]
struct Cache {
    player: Option<(String, Arc<String>)>,
    solved: HashMap<(String, char, String), String>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: std::sync::OnceLock<Mutex<Cache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

async fn text_capped(mut resp: reqwest::Response, what: &str) -> Result<String> {
    if !resp.status().is_success() {
        return Err(anyhow!("{what} answered HTTP {}", resp.status().as_u16()));
    }
    if resp.content_length().is_some_and(|n| n as usize > PAGE_MOST) {
        return Err(anyhow!("{what} is too large"));
    }
    // Stream into a capped buffer: a server that sends no Content-Length must
    // not be able to put the whole body in memory before the limit is checked.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > PAGE_MOST {
            return Err(anyhow!("{what} is too large"));
        }
        buf.extend_from_slice(&chunk);
    }
    String::from_utf8(buf).map_err(|_| anyhow!("{what} is not valid text"))
}

async fn player_script(client: &reqwest::Client, path: &str) -> Result<Arc<String>> {
    if let Some((p, js)) = crate::engine::mutex_guard(cache()).player.as_ref() {
        if p == path {
            return Ok(js.clone());
        }
    }
    let resp = client
        .get(format!("https://www.youtube.com{path}"))
        .header(reqwest::header::USER_AGENT, PAGE_AGENT)
        .send()
        .await?;
    let js = Arc::new(text_capped(resp, "the player script").await?);
    crate::engine::mutex_guard(cache()).player = Some((path.to_string(), js.clone()));
    Ok(js)
}

/// What the solver turned each challenge into: `n` values and signatures.
#[derive(Default)]
struct Solved {
    n: HashMap<String, String>,
    sig: HashMap<String, String>,
}

/// Runs the solver over the player script for the given challenges.
async fn solve(player_path: &str, js: &str, ns: Vec<String>, sigs: Vec<String>) -> Result<Solved> {
    let mut solved = Solved::default();
    let mut ask_n = Vec::new();
    let mut ask_sig = Vec::new();
    {
        let c = crate::engine::mutex_guard(cache());
        for n in ns {
            match c.solved.get(&(player_path.to_string(), 'n', n.clone())) {
                Some(v) => drop(solved.n.insert(n, v.clone())),
                None => ask_n.push(n),
            }
        }
        for s in sigs {
            match c.solved.get(&(player_path.to_string(), 's', s.clone())) {
                Some(v) => drop(solved.sig.insert(s, v.clone())),
                None => ask_sig.push(s),
            }
        }
    }
    if ask_n.is_empty() && ask_sig.is_empty() {
        return Ok(solved);
    }
    let mut requests = Vec::new();
    if !ask_n.is_empty() {
        requests.push(json!({"type": "n", "challenges": ask_n}));
    }
    if !ask_sig.is_empty() {
        requests.push(json!({"type": "sig", "challenges": ask_sig}));
    }
    let input = json!({"type": "player", "player": js, "requests": requests});
    let program = format!(
        "{SOLVER_LIB}\nObject.assign(globalThis, lib);\n{SOLVER_CORE}\nconsole.log(JSON.stringify(jsc({input})));\n"
    );
    let output = run_js(program).await?;
    let answer: Value = serde_json::from_str(
        output.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or(""),
    )
    .map_err(|_| anyhow!("the challenge solver printed no answer ({})", output.trim().chars().take(200).collect::<String>()))?;
    if answer.get("type").and_then(Value::as_str) == Some("error") {
        return Err(anyhow!("the challenge solver failed: {}", answer.get("error").and_then(Value::as_str).unwrap_or("")));
    }
    let responses = answer
        .get("responses")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("the challenge solver answered nothing"))?;
    let mut c = crate::engine::mutex_guard(cache());
    if c.solved.len() > 512 {
        c.solved.clear();
    }
    for (request, response) in requests.iter().zip(responses) {
        let kind = request.get("type").and_then(Value::as_str).unwrap_or("");
        if response.get("type").and_then(Value::as_str) != Some("result") {
            let why = response.get("error").and_then(Value::as_str).unwrap_or("");
            return Err(anyhow!("the challenge solver could not solve {kind}: {why}"));
        }
        for (challenge, result) in response.get("data").and_then(Value::as_object).into_iter().flatten() {
            if let Some(result) = result.as_str() {
                let (map, tag) = if kind == "n" { (&mut solved.n, 'n') } else { (&mut solved.sig, 's') };
                map.insert(challenge.clone(), result.to_string());
                c.solved.insert((player_path.to_string(), tag, challenge.clone()), result.to_string());
            }
        }
    }
    Ok(solved)
}

/// Runs `program` (fed on stdin) in Deno with no permissions at all, so the
/// player's code can reach neither files, the network nor programs; Node
/// where that is the only runtime. The app's own Deno is fetched when there
/// is neither (yt-dlp needs it for YouTube too).
async fn run_js(program: String) -> Result<String> {
    let runtime = match crate::engine::js_runtime_for_solver() {
        Some(found) => found,
        None => {
            tokio::task::spawn_blocking(crate::engine::deno_install_blocking)
                .await
                .map_err(|e| anyhow!("{e}"))??;
            crate::engine::js_runtime_for_solver()
                .ok_or_else(|| anyhow!("no JavaScript runtime for YouTube's player challenge"))?
        }
    };
    let (name, path) = runtime;
    let mut cmd = tokio::process::Command::new(&path);
    if name == "deno" {
        cmd.args([
            "run", "--ext=js", "--no-code-cache", "--no-prompt", "--no-remote", "--no-lock",
            "--node-modules-dir=none", "--no-config", "--no-npm", "-",
        ]);
    } else {
        cmd.arg("-");
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| anyhow!("{name} would not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(program.as_bytes()).await?;
        drop(stdin);
    }
    let out = tokio::time::timeout(Duration::from_secs(45), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("the challenge solver took too long"))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("the challenge solver stopped: {}", err.lines().last().unwrap_or("").trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A stream's link with its `n` solved and its signature (if ciphered) added.
fn unlocked(format: &Format, solved: &Solved) -> Result<String> {
    let (raw, signature) = match (&format.url, &format.cipher) {
        (Some(u), _) => (u.clone(), None),
        (None, Some(cipher)) => {
            let pairs: HashMap<String, String> =
                url::form_urlencoded::parse(cipher.as_bytes()).into_owned().collect();
            let u = pairs.get("url").ok_or_else(|| anyhow!("a ciphered stream with no link"))?;
            let s = pairs.get("s").ok_or_else(|| anyhow!("a ciphered stream with no signature"))?;
            let sig = solved.sig.get(s).ok_or_else(|| anyhow!("the stream's signature was not solved"))?;
            let name = pairs.get("sp").cloned().unwrap_or_else(|| "signature".into());
            (u.clone(), Some((name, sig.clone())))
        }
        (None, None) => return Err(anyhow!("a stream with no link")),
    };
    let mut url = url::Url::parse(&raw)?;
    let mut pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    for (k, v) in &mut pairs {
        if k == "n" {
            *v = solved.n.get(v.as_str()).cloned().ok_or_else(|| anyhow!("the stream's n value was not solved"))?;
        }
    }
    if let Some(sig) = signature {
        pairs.push(sig);
    }
    url.query_pairs_mut().clear().extend_pairs(pairs);
    Ok(url.to_string())
}

fn challenges(formats: &[Format]) -> (Vec<String>, Vec<String>) {
    let mut ns = std::collections::BTreeSet::new();
    let mut sigs = std::collections::BTreeSet::new();
    let mut n_of = |u: &str| {
        if let Ok(u) = url::Url::parse(u) {
            ns.extend(u.query_pairs().filter(|(k, _)| k == "n").map(|(_, v)| v.into_owned()));
        }
    };
    for f in formats {
        if let Some(u) = &f.url {
            n_of(u);
        }
        if let Some(cipher) = &f.cipher {
            for (k, v) in url::form_urlencoded::parse(cipher.as_bytes()) {
                match &*k {
                    "s" => drop(sigs.insert(v.into_owned())),
                    "url" => n_of(&v),
                    _ => {}
                }
            }
        }
    }
    (ns.into_iter().collect(), sigs.into_iter().collect())
}

/// Reads one YouTube video for the quality `opts` asks (`quality_pref`,
/// empty = best): the picture and its sound as an adaptive pair, or one
/// file where the video has nothing apart. Waits out a pre-roll ad before
/// returning (a Cancel ends the wait).
pub async fn resolve(
    client: &reqwest::Client,
    opts: &EngineOpts,
    page_url: &str,
    shared: &Arc<SharedState>,
) -> Result<ResolvedMedia> {
    let id = crate::engine::youtube_id(page_url)
        .filter(|id| id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .ok_or_else(|| anyhow!("not a YouTube video link"))?;
    let started = Instant::now();
    shared.set_phase("Reading YouTube");
    let read = tokio::time::timeout(Duration::from_secs(60), read(client, opts, &id))
        .await
        .map_err(|_| anyhow!("reading the page took too long"))??;
    let (hit, wait) = read;
    if wait > MOST_AD_WAIT {
        return Err(anyhow!("the video plays a {wait}s ad first"));
    }
    let left = Duration::from_secs(wait).saturating_sub(started.elapsed());
    if !left.is_zero() {
        // Counted down second by second in the progress window.
        let until = Instant::now() + left;
        while Instant::now() < until {
            if shared.cancelled.load(Ordering::SeqCst) {
                return Err(anyhow!("cancelled"));
            }
            let secs = until.saturating_duration_since(Instant::now()).as_secs_f64().ceil().max(1.0);
            shared.set_phase(format!("Waiting {secs}s for YouTube"));
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    shared.set_phase("Getting info");
    Ok(hit)
}

/// The size of what [`resolve`] would fetch (the streams' own `clen`, no
/// request to them) and the file name, for the Download File Info window:
/// no ad wait.
pub async fn peek(client: &reqwest::Client, opts: &EngineOpts, page_url: &str) -> Result<(Option<u64>, String)> {
    let id = crate::engine::youtube_id(page_url).ok_or_else(|| anyhow!("not a YouTube video link"))?;
    let (hit, _) = tokio::time::timeout(Duration::from_secs(30), read(client, opts, &id))
        .await
        .map_err(|_| anyhow!("reading the page took too long"))??;
    let clen = |u: &str| {
        url::Url::parse(u).ok()?.query_pairs().find(|(k, _)| k == "clen").and_then(|(_, v)| v.parse::<u64>().ok())
    };
    let size = match (clen(&hit.url), hit.pair_url.as_deref().map(clen)) {
        (Some(v), Some(Some(a))) => Some(v + a),
        (Some(v), None) => Some(v),
        _ => None,
    };
    Ok((size, hit.title))
}

/// Whether the app's own reader runs for these options (see the engine).
pub fn own_reader_allowed(opts: &EngineOpts) -> bool {
    crate::engine::youtube_own_reader_allowed(opts)
}

async fn read(client: &reqwest::Client, opts: &EngineOpts, id: &str) -> Result<(ResolvedMedia, u64)> {
    let watch = format!("https://www.youtube.com/watch?v={id}&bpctr=9999999999&has_verified=1");
    // The consent answer some countries' pages ask for first.
    let page = client
        .get(&watch)
        .header(reqwest::header::USER_AGENT, PAGE_AGENT)
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .header(reqwest::header::COOKIE, "PREF=hl=en&tz=UTC; SOCS=CAI")
        .send()
        .await?;
    let html = text_capped(page, "the watch page").await?;
    let facts = page_facts(&html);
    let player_path = facts.player_path.clone().ok_or_else(|| anyhow!("the page names no player script"))?;
    let mut sts = facts.sts;
    if sts.is_none() {
        let js = player_script(client, &player_path).await?;
        sts = sts_in_player(&js);
    }
    let body = json!({
        "context": {"client": {
            "clientName": TV_CLIENT_NAME, "clientVersion": TV_CLIENT_VERSION,
            "deviceMake": "Samsung", "deviceModel": "UKS9800", "userAgent": TV_AGENT,
            "osName": "Tizen", "osVersion": "2.4.0",
            "hl": "en", "timeZone": "UTC", "utcOffsetMinutes": 0,
        }},
        "videoId": id,
        "playbackContext": {"contentPlaybackContext": {
            "html5Preference": "HTML5_PREF_WANTS",
            "signatureTimestamp": sts,
        }},
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let mut ask = client
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header(reqwest::header::USER_AGENT, TV_AGENT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("X-Youtube-Client-Name", TV_CLIENT_ID)
        .header("X-Youtube-Client-Version", TV_CLIENT_VERSION)
        .header(reqwest::header::ORIGIN, "https://www.youtube.com");
    if let Some(v) = &facts.visitor {
        ask = ask.header("X-Goog-Visitor-Id", v);
    }
    let answer = ask.body(body.to_string()).send().await?;
    let player: Value = serde_json::from_str(&text_capped(answer, "the player API").await?)
        .map_err(|e| anyhow!("the player API answered no JSON ({e})"))?;
    let status = player.pointer("/playabilityStatus/status").and_then(Value::as_str).unwrap_or("");
    if status != "OK" {
        let reason = player.pointer("/playabilityStatus/reason").and_then(Value::as_str).unwrap_or(status);
        return Err(anyhow!("YouTube says: {reason}"));
    }
    if player.pointer("/videoDetails/isLive").and_then(Value::as_bool).unwrap_or(false) {
        return Err(anyhow!("a live stream"));
    }
    let streams_at = |at: &str| -> Vec<Format> {
        player.pointer(at).and_then(Value::as_array).into_iter().flatten().filter_map(Format::of).collect()
    };
    let mut formats = streams_at("/streamingData/adaptiveFormats");
    let muxed = formats.is_empty();
    if muxed {
        formats = streams_at("/streamingData/formats");
    }
    let cap = crate::engine::quality_cap(&opts.quality_pref);
    // The sound alone: its best M4A track (WebM/Opus where there is none),
    // fetched as one file.
    if crate::engine::wants_audio_only(&opts.quality_pref) {
        let sound = choose_sound(&formats).ok_or_else(|| anyhow!("the player lists no sound stream"))?;
        let (ns, sigs) = challenges(std::slice::from_ref(&sound));
        let solved = if ns.is_empty() && sigs.is_empty() {
            Solved::default()
        } else {
            let js = player_script(client, &player_path).await?;
            solve(&player_path, &js, ns, sigs).await?
        };
        let title = video_title(&player, &facts, id);
        let ext = if sound.container == "webm" { "webm" } else { "m4a" };
        if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
            eprintln!("[dbg] youtube own reader: {id} sound alone, itag {} {}, ad wait {}s", sound.itag, sound.container, ad_wait(&player));
        }
        return Ok((
            ResolvedMedia { url: unlocked(&sound, &solved)?, title: format!("{title}.{ext}"), pair_url: None },
            ad_wait(&player),
        ));
    }
    let (picture, sound) = choose(&formats, cap).ok_or_else(|| anyhow!("the player lists no stream the app can fetch"))?;
    let sound = if muxed { None } else { Some(sound.ok_or_else(|| anyhow!("the player lists no sound stream"))?) };
    let wanted: Vec<Format> = std::iter::once(picture.clone()).chain(sound.clone()).collect();
    let (ns, sigs) = challenges(&wanted);
    let solved = if ns.is_empty() && sigs.is_empty() {
        Solved::default()
    } else {
        let js = player_script(client, &player_path).await?;
        solve(&player_path, &js, ns, sigs).await?
    };
    let video_url = unlocked(&picture, &solved)?;
    let audio_url = sound.as_ref().map(|s| unlocked(s, &solved)).transpose()?;
    let title = video_title(&player, &facts, id);
    let ext = match (&picture.container[..], sound.as_ref().map(|s| &s.container[..])) {
        ("mp4", None | Some("mp4")) => "mp4",
        ("webm", None) => "webm",
        _ => "mkv",
    };
    if std::env::var("DOWNLOAD_CRAFT_DEBUG").is_ok() {
        eprintln!(
            "[dbg] youtube own reader: {id} itag {} {}x{} {} + {:?}, ad wait {}s",
            picture.itag,
            picture.width,
            picture.height,
            picture.container,
            sound.as_ref().map(|s| s.itag),
            ad_wait(&player)
        );
    }
    Ok((
        ResolvedMedia { url: video_url, title: format!("{title}.{ext}"), pair_url: audio_url },
        ad_wait(&player),
    ))
}

/// The signature timestamp in the player script, where the page gave none.
fn sts_in_player(js: &str) -> Option<u64> {
    for key in ["signatureTimestamp:", "sts:"] {
        let mut rest = js;
        while let Some(at) = rest.find(key) {
            let after = &rest[at + key.len()..];
            let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
            if digits.len() == 5 {
                return digits.parse().ok();
            }
            rest = after;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(itag: u64, mime: &str, w: u64, h: u64, br: u64) -> Value {
        json!({"itag": itag, "mimeType": mime, "width": w, "height": h, "bitrate": br, "url": format!("https://r.googlevideo.com/videoplayback?itag={itag}&n=abc")})
    }

    fn formats() -> Vec<Format> {
        [
            fmt(137, "video/mp4; codecs=\"avc1\"", 1920, 1080, 4_000_000),
            fmt(248, "video/webm; codecs=\"vp9\"", 1920, 1080, 3_000_000),
            fmt(136, "video/mp4; codecs=\"avc1\"", 1280, 720, 2_000_000),
            fmt(313, "video/webm; codecs=\"vp9\"", 3840, 2160, 18_000_000),
            fmt(140, "audio/mp4; codecs=\"mp4a\"", 0, 0, 130_000),
            fmt(251, "audio/webm; codecs=\"opus\"", 0, 0, 150_000),
        ]
        .iter()
        .filter_map(Format::of)
        .collect()
    }

    #[test]
    fn picks_mp4_at_the_cap_with_m4a() {
        let (v, a) = choose(&formats(), Some(1080)).unwrap();
        assert_eq!((v.itag, a.unwrap().itag), (137, 140));
        let (v, a) = choose(&formats(), Some(720)).unwrap();
        assert_eq!((v.itag, a.unwrap().itag), (136, 140));
    }

    #[test]
    fn best_takes_webm_4k_with_opus() {
        let (v, a) = choose(&formats(), None).unwrap();
        assert_eq!((v.itag, a.unwrap().itag), (313, 251));
    }

    #[test]
    fn sound_alone_prefers_m4a() {
        assert_eq!(choose_sound(&formats()).unwrap().itag, 140);
        let only_opus: Vec<Format> = formats().into_iter().filter(|f| f.itag != 140).collect();
        assert_eq!(choose_sound(&only_opus).unwrap().itag, 251);
    }

    #[test]
    fn cap_below_everything_takes_the_smallest() {
        let (v, _) = choose(&formats(), Some(144)).unwrap();
        assert_eq!(v.itag, 136);
    }

    #[test]
    fn shorts_are_measured_by_their_width() {
        let f: Vec<Format> = [fmt(136, "video/mp4", 720, 1280, 1), fmt(137, "video/mp4", 1080, 1920, 1), fmt(140, "audio/mp4", 0, 0, 1)]
            .iter()
            .filter_map(Format::of)
            .collect();
        assert_eq!(choose(&f, Some(720)).unwrap().0.itag, 136);
    }

    #[test]
    fn ad_wait_sums_start_ads_and_honours_skip() {
        let p = json!({"adPlacements": [
            {"adPlacementRenderer": {"config": {"adPlacementConfig": {"kind": "AD_PLACEMENT_KIND_START"}},
              "renderer": {"instreamVideoAdRenderer": {"playerVars": "length_seconds=6&x=1"}}}},
            {"adPlacementRenderer": {"config": {"adPlacementConfig": {"kind": "AD_PLACEMENT_KIND_MILLISECONDS"}},
              "renderer": {"instreamVideoAdRenderer": {"playerVars": "length_seconds=30"}}}},
            {"adPlacementRenderer": {"config": {"adPlacementConfig": {"kind": "AD_PLACEMENT_KIND_START"}},
              "renderer": {"instreamVideoAdRenderer": {"playerVars": "length_seconds=30", "skipOffsetMilliseconds": 5000}}}}
        ]});
        assert_eq!(ad_wait(&p), 11);
        assert_eq!(ad_wait(&json!({})), 0);
    }

    #[test]
    fn n_and_signature_are_replaced() {
        let mut solved = Solved::default();
        solved.n.insert("abc".into(), "XYZ".into());
        solved.sig.insert("SIG".into(), "OK".into());
        let f = Format::of(&json!({"itag": 140, "mimeType": "audio/mp4",
            "signatureCipher": "s=SIG&sp=sig&url=https%3A%2F%2Fr.googlevideo.com%2Fvideoplayback%3Fitag%3D140%26n%3Dabc"})).unwrap();
        let u = unlocked(&f, &solved).unwrap();
        assert!(u.contains("n=XYZ") && u.contains("sig=OK") && !u.contains("n=abc"), "{u}");
        assert_eq!(challenges(&[f]), (vec!["abc".to_string()], vec!["SIG".to_string()]));
    }

    #[test]
    fn page_facts_read_ytcfg() {
        let html = r#"<title>A &amp; B - YouTube</title><script>ytcfg.set({"VISITOR_DATA":"Cgt2","PLAYER_JS_URL":"/s/player/11a45ffb/player_es6.vflset/en_US/base.js","STS":20391});</script>"#;
        let f = page_facts(html);
        assert_eq!(f.visitor.as_deref(), Some("Cgt2"));
        assert_eq!(f.player_path.as_deref(), Some("/s/player/11a45ffb/player_es6.vflset/en_US/base.js"));
        assert_eq!(f.sts, Some(20391));
        assert_eq!(f.title.as_deref(), Some("A & B"));
        assert!(page_facts(r#""PLAYER_JS_URL":"https://evil.example/x.js""#).player_path.is_none());
    }

    #[test]
    fn title_from_player_data_unescapes() {
        let html = r#"var x = {"videoDetails":{"videoId":"a","title":"Café \"Live\""}}"#;
        assert_eq!(page_title(html).as_deref(), Some("Café \"Live\""));
    }

    #[test]
    fn page_title_decodes_json_escapes_and_surrogates() {
        let html = r#"var x = {"videoDetails":{"videoId":"a","title":"Caf\u00e9 \ud83d\ude00 \/ live"}}"#;
        assert_eq!(page_title(html).as_deref(), Some("Café 😀 / live"));
    }

    #[test]
    fn metric_only_titles_are_page_chrome() {
        for t in ["50K views", "6.6K likes", "1,234 subscribers", "no views", "50K VIEWS"] {
            assert!(is_metric_title(t), "{t}");
        }
        for t in ["50k", "6.6k", "50K views of Tokyo", "Song 50K", ""] {
            assert!(!is_metric_title(t), "{t}");
        }
    }

    #[test]
    fn metric_player_title_is_kept_but_fallback_chrome_is_not() {
        let player = json!({"videoDetails": {"title": "50K views"}});
        let facts = PageFacts { title: Some("6.6K likes".into()), ..Default::default() };
        // Player metadata is authoritative: a real video may be titled so.
        assert_eq!(video_title(&player, &facts, "abc123"), "50K views");
        let player = json!({"videoDetails": {"title": "50k"}});
        assert_eq!(video_title(&player, &facts, "abc123"), "50k");
        // With no player title only the fallback is checked for chrome.
        let player = json!({"videoDetails": {}});
        assert_eq!(video_title(&player, &facts, "abc123"), "youtube_abc123");
        let facts = PageFacts { title: Some("A real title".into()), ..Default::default() };
        assert_eq!(video_title(&player, &facts, "abc123"), "A real title");
    }
}
