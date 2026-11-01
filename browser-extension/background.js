// Background worker: every browser download is captured automatically -
// paused in the browser, handed to the app, and resumed in the browser when
// the app cannot take it. No clicks, no pasting - zero config.

const HOST = "com.download_craft";
const GRACE_MS = 15_000; // ignore our own fallback re-downloads
const seen = new Map(); // url -> timestamp (loop guard)
let hostOk = true;
let lastHostError = "";
let lastFallbackError = "";
// Must match manifest version + content.js EXT_VER. Tabs running an older
// content script are rejected with a "reload the tab" error instead of
// failing mysteriously (their extension APIs are gone after a reload).
const EXT_VER = "0.9.13";

// Promise-style calls (the `await api.*` ones below) go through this handle so
// one file serves both engines: Firefox exposes the promise-returning
// `browser` namespace, while recent Chrome versions expose it as well. Use
// Firefox's runtime.getBrowserInfo API (not the namespace name) when enabling
// Firefox-only blocking webRequest behavior.
const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;

// Shared ranking (P4): playlist/segment/file kinds, scores, duplicate
// collapse. Service workers support importScripts; if it ever fails the
// guarded fallbacks below keep the old behavior instead of breaking.
try {
  if (typeof importScripts === "function") importScripts("rank.js");
} catch (e) { /* RANK stays null -> legacy paths */ }
// "Download in browser": stream readers, MP4 joiner and the runner (the
// runner itself works in the offscreen document on Chrome, here on Firefox).
try {
  if (typeof importScripts === "function") importScripts("streams.js", "mp4merge.js", "browserdl.js");
} catch (e) { /* browser downloads of plain files still work */ }
// Live connection to the app (persistent port, pushes, request context).
try {
  if (typeof importScripts === "function") importScripts("bridge.js");
} catch (e) { console.error("[download-craft] bridge.js failed to load", e); }
// Talk to the app: the live port when there is one, else a one-shot host.
function hostSend(msg) {
  if (typeof DCBridge !== "undefined") return DCBridge.send(msg);
  return api.runtime.sendNativeMessage(HOST, msg);
}
// Start the live link now: the start-up ping below already goes through it.
if (typeof DCBridge !== "undefined") DCBridge.start(HOST);

function markSeen(url, filename) {
  seen.set(url + "|" + (filename || ""), Date.now());
  if (seen.size > 200) {
    const cut = Date.now() - GRACE_MS;
    for (const [u, t] of seen) if (t < cut) seen.delete(u);
  }
}
function freshSeen(url, filename) {
  const t = seen.get(url + "|" + (filename || "")) ?? seen.get(url + "|");
  return t !== undefined && Date.now() - t < GRACE_MS;
}

async function settings() {
  const s = await api.storage.local.get({
    autoCapture: true,
    hideBrowserUI: false,
    minSizeKB: 100,
    blockHosts: "",
    // Sniff the first bytes of opaque responses (text/plain, octet-stream…)
    // to find manifests and videos hidden behind generic types.
    deepDetect: true,
    appPrefs: {},
    siteRules: null,
  });
  // Sites the project's rules say never to capture (sign-in or streaming
  // services whose downloads only work in the browser).
  const never = ((s.siteRules || {}).capture || {}).neverSites;
  delete s.siteRules;
  const extraBlocked = Array.isArray(never) ? never.filter((h) => typeof h === "string" && /^[a-z0-9.*-]{1,100}$/i.test(h)) : [];
  // Capture settings set in the app (Settings -> Browser Integration ->
  // Capture) win: the app is where they are edited, the popup mirrors them.
  const a = s.appPrefs || {};
  if (typeof a.autoCapture === "boolean") s.autoCapture = a.autoCapture;
  if (Number.isFinite(a.minSizeKB)) s.minSizeKB = a.minSizeKB;
  if (typeof a.blockHosts === "string") s.blockHosts = a.blockHosts;
  if (extraBlocked.length) s.blockHosts = [s.blockHosts, ...extraBlocked].filter(Boolean).join(", ");
  s.captureTypes = typeof a.captureTypes === "string" ? a.captureTypes : "";
  s.notify = a.notify !== false;
  s.browserFallback = a.browserFallback !== false;
  s.skipKey = /^(alt|shift|off)$/.test(a.skipKey || "") ? a.skipKey : "alt";
  delete s.appPrefs;
  return s;
}

// The app's "File types" list: empty = every type. Matched on the file name,
// else on the URL path.
function typeAllowed(cfg, name, url) {
  const list = String(cfg.captureTypes || "").toLowerCase().split(/[\s,;]+/).filter(Boolean);
  if (!list.length) return true;
  const ext = (s) => { const m = /\.([a-z0-9]{1,12})$/i.exec(String(s || "").split(/[?#]/)[0]); return m ? m[1].toLowerCase() : ""; };
  let e = ext(String(name || "").split(/[\\/]/).pop());
  if (!e) { try { e = ext(new URL(url).pathname); } catch (x) {} }
  return !!e && list.includes(e);
}

// Links clicked with the "keep in browser" key held (Alt by default).
const skipOnce = new Map(); // url -> until
function skipped(url) {
  const t = skipOnce.get(url);
  if (t === undefined) return false;
  if (Date.now() > t) { skipOnce.delete(url); return false; }
  return true;
}

// Chromium's UI switch affects the whole profile, so it is opt-in and
// explained in the popup. Release it when capture is disabled or unavailable.
async function syncBrowserDownloadUI() {
  if (!api.downloads || typeof api.downloads.setUiOptions !== "function") return {supported:false};
  const cfg = await settings();
  const hidden = typeof DCRank !== "undefined" && DCRank.hideDownloadUI(cfg, hostOk);
  try {
    await api.downloads.setUiOptions({enabled:!hidden});
    return {supported:true, hidden};
  } catch (e) { return {supported:true, error:(e && e.message) || String(e)}; }
}

function hostBlocked(url, blockHosts) {
  try {
    const h = new URL(url).hostname.toLowerCase();
    const blocked = String(blockHosts || "")
      .split(/[\s,;]+/)
      .map((x) => x.trim().toLowerCase())
      .filter(Boolean);
    return blocked.some((b) => h === b || h.endsWith("." + b));
  } catch (e) {
    return false;
  }
}

// Curated cookies for ONE url only (same-host). Never logged. Whole
// name=value pairs only — a byte cut could split a pair and corrupt auth.
async function cookiesFor(url) {
  try {
    if (!chrome.cookies || !chrome.cookies.getAll) return null;
    const list = await api.cookies.getAll({ url });
    if (!list || !list.length) return null;
    const pairs = list
      .filter((c) => c && c.name)
      .slice(0, 40)
      .map((c) => `${c.name}=${c.value || ""}`);
    let s = "";
    for (const p of pairs) {
      const add = (s ? "; " : "") + p;
      if ((s + add).length > 6000) break;
      s += add;
    }
    return s || null;
  } catch (e) {
    return null;
  }
}

function stripFbSlice(url) {
  try {
    const u = new URL(url);
    const h = u.hostname.toLowerCase();
    const social = h.includes("fbcdn") || h.includes("scontent") || h.includes("facebook")
      || h.includes("fbsbx") || h.includes("lookaside") || h.includes("cdninstagram") || h.includes("instagram");
    if (!social) return url;
    if (!u.searchParams.has("bytestart") && !u.searchParams.has("byteend")) return url;
    u.searchParams.delete("bytestart");
    u.searchParams.delete("byteend");
    return u.toString();
  } catch (e) { return url; }
}

async function sendToHost(url, filename, extra) {
  try {
    url = stripFbSlice(url);
    if (typeof DCRank !== "undefined" && DCRank.isVimeoAdaptive(url)) {
      // The captured document is the exact iframe which requested this
      // asset, unlike the popup's top-level page (possibly several videos).
      let capture = null;
      let captureTab = null;
      for (const [tabId, items] of tabMedia) {
        const hit = items.find((m) => m.url === url);
        if (hit) { capture = hit; captureTab = tabId; break; }
      }
      let framePage = capture && capture.pageUrl;
      // Chromium omits documentUrl on webRequest events. Ask the content
      // script in the requesting frame instead; do not guess from a feed.
      if (!DCRank.vimeoVideoPage(framePage) && capture && capture.frameId >= 0) {
        try {
          const reply = await api.tabs.sendMessage(captureTab,
            { type: "vimeoFramePage" }, { frameId: capture.frameId });
          framePage = reply && reply.pageUrl;
        } catch (e) { /* frame may have navigated or been removed */ }
      }
      const player = DCRank.vimeoVideoPage(framePage)
        || DCRank.vimeoVideoPage(extra && extra.pageUrl)
        || DCRank.vimeoVideoPage(extra && extra.referer);
      if (!player && !DCRank.isPlaylistUrl(url)) throw new Error("Vimeo playback fragment: reload the video page and capture its full stream or Vimeo player link.");
      // A full manifest can be resolved by yt-dlp without a player URL.
      // Never feed it to the built-in video-only HLS concatenator.
      url = player || url;
      // Fragment byte counts/pairs do not describe the resolved full video.
      extra = { ...extra, size: null, pairUrl: null, pairMime: null,
        cookie: null, mime: null };
    }
    const observedPart = [...tabMedia.values()].some(items => items.some(m => m.url === url && m.segment));
    const wholeCapture = [...tabMedia.values()].some(items => items.some(m => m.url === url && m.whole));
    if (typeof DCRank !== "undefined" && !wholeCapture && (DCRank.isSegmentUrl(url) || observedPart) && !DCRank.isPlaylistUrl(url)) {
      let capture = null, items = [], tab = null;
      for (const [tabId, list] of tabMedia) {
        const hit = list.find(m => m.url === url);
        if (hit) { capture = hit; items = list; tab = tabId; break; }
      }
      let frameInfo = null;
      if (capture && capture.frameId >= 0) {
        try { frameInfo = await api.tabs.sendMessage(tab,
          {type:"mediaFrameInfo"}, {frameId:capture.frameId}); } catch (e) {}
      }
      const whole = capture && DCRank.completePlaylist(capture, items);
      if (whole) {
        url = whole.url;
      } else if (frameInfo && frameInfo.videoCount === 1 && /^https?:\/\//i.test(frameInfo.pageUrl || "")) {
        // Keep the fragment URL as evidence. The engine resolves its exact
        // single-player frame page with yt-dlp; it never saves the fragment.
        extra = {...extra, referer:frameInfo.pageUrl, pageUrl:frameInfo.pageUrl};
      } else {
        throw new Error("This capture is a playback fragment. Reload the video and capture its full playlist or a page containing one player.");
      }
      extra = {...extra, size:null, pairUrl:null, pairMime:null,
        dur:(frameInfo && frameInfo.durationSecs) || (extra && extra.dur) || null,
        audioExpected:!!((frameInfo && frameInfo.audioExpected) || (extra && extra.audioExpected))};
    }
    if (typeof DCRank !== "undefined") {
      for (const items of tabMedia.values()) {
        const selected = items.find(m => m.url === url);
        if (!selected || DCRank.kindOf(selected) !== "playlist") continue;
        const root = DCRank.completePlaylist(selected, items);
        if (root && root.url !== url) {
          url = root.url;
          extra = {...extra, size:null, pairUrl:null, pairMime:null};
        }
        break;
      }
    }
    // What the player itself sent for this exact URL when it was captured:
    // the referer/origin the CDN checks, its session cookie and any bearer
    // or x-* token. Truer than the page URL, so it wins when present.
    const seenAuth = mediaAuth.get(url) || null;
    let cookie = (extra && extra.cookie) || (seenAuth && seenAuth.cookie) || null;
    // A URL known only from a page script's report (page-hook) gets no
    // cookies attached: any page can post such reports, and the app must
    // not be steered into fetching other sites with the user's session.
    let pageOnly = false;
    {
      let fromPage = false, fromNet = false;
      for (const items of tabMedia.values()) {
        for (const m of items) {
          if (m.url !== url) continue;
          if (m.via === "page") fromPage = true; else fromNet = true;
        }
      }
      pageOnly = fromPage && !fromNet;
    }
    if (!cookie && /^https?:\/\//i.test(url) && !pageOnly) {
      cookie = await cookiesFor(url); // best-effort, same-host only
      // CDN hosts (fbcdn/lookaside) carry no cookies of their own — fall
      // back to the page's session cookies so expiring signed URLs keep
      // working without a page reload.
      const pageUrl = extra && (extra.pageUrl || extra.referer);
      const vimeoTarget = typeof DCRank !== "undefined"
        && (DCRank.isVimeoAdaptive(url) || DCRank.vimeoVideoPage(url));
      if (!cookie && pageUrl && /^https?:\/\//i.test(pageUrl)
          && (!vimeoTarget || DCRank.vimeoVideoPage(pageUrl))) {
        try {
          const uh = new URL(url).hostname.toLowerCase();
          const ph = new URL(pageUrl).hostname.toLowerCase();
          const socialPage = /(?:^|\.)(?:facebook\.com|instagram\.com)$/.test(ph);
          const socialTarget = /(?:^|\.)(?:fbcdn\.net|facebook\.com|instagram\.com|cdninstagram\.com|fbsbx\.com)$/.test(uh)
            || uh.includes("scontent") || uh.includes("lookaside");
          const social = socialPage && socialTarget;
          // Cross-host session cookies are limited to known platform delivery hosts.
          const youtubeBridge = /(^|\.)youtube\.com$/.test(ph) && /(^|\.)googlevideo\.com$/.test(uh);
          if (uh !== ph && (social || youtubeBridge || (vimeoTarget && DCRank.vimeoVideoPage(pageUrl)))) cookie = await cookiesFor(pageUrl);
        } catch (e) { /* host parse failed: no cookies */ }
      }
    }
    const ctx = typeof DCBridge !== "undefined" ? DCBridge.browserContext(url, extra) : { fromExtension: true };
    // The exact Cookie the browser sent with this request beats a cookie-jar
    // lookup (and never leaves its own host: the request was to that URL).
    if (ctx._cookie && !pageOnly && (!cookie || (extra && extra.fromRequest))) cookie = ctx._cookie;
    const sentReferer = ctx._referer;
    delete ctx._cookie; delete ctx._referer; delete ctx._post;
    // The browser's own download item names its full save path
    // ("/home/me/Downloads/x.zip"): only the name travels, or the app's
    // filename cleaning turned the folders into "_home_me_Downloads_x.zip".
    if (filename) filename = String(filename).split(/[\\/]/).pop() || null;
    rememberCapture(url, filename, extra);
    const res = await hostSend({
      type: "download",
      browser: ctx,
      url,
      filename: filename || null,
      pageUrl: (extra && extra.pageUrl) || null,
      referer: (seenAuth && seenAuth.referer) || (extra && extra.fromRequest && sentReferer) || (extra && extra.referer) || (extra && extra.pageUrl) || null,
      cookie,
      headers: (seenAuth && Object.keys(seenAuth.headers).length) ? seenAuth.headers : null,
      mime: (extra && extra.mime) || null,
      size: (extra && extra.size) || null,
      pairUrl: (extra && extra.pairUrl) || null,
      pairMime: (extra && extra.pairMime) || null,
      quality: (extra && extra.quality) || null,
      dur: (extra && extra.dur) || null,
      audioExpected: !!(extra && extra.audioExpected),
    });
    return !!res && (res.type === "ok" || res.type === "pong");
  } catch (e) {
    // Console-visible: this is the first thing to look at when clicks seem to
    // do nothing (browser console: "No such native application", etc.).
    lastHostError = (e && e.message) || String(e);
    console.error("[download-craft] native host '" + HOST + "' failed:", lastHostError);
    return false;
  }
}

// What each capture was, so a failure the app reports later can be handed to
// the extension's own browser download (the last fallback).
const sentCaptures = new Map(); // url -> {url, filename, pairUrl, mime, quality, pageUrl}
function rememberCapture(url, filename, extra) {
  const e = extra || {};
  sentCaptures.delete(url);
  sentCaptures.set(url, { url, filename: filename || null, pairUrl: e.pairUrl || null, mime: e.mime || null,
    quality: e.quality || null, pageUrl: e.pageUrl || null, title: e.title || null, at: Date.now() });
  if (sentCaptures.size > 100) sentCaptures.delete(sentCaptures.keys().next().value);
}

// ---- IDM-style network sniffer ----
// Watches response headers for media payloads (direct mp4/webm/mkv, audio,
// HLS playlists, DASH manifests). Candidates are stored per-tab so the
// popup + content overlay can offer them even when the DOM only holds a
// blob: URL (un-downloadable directly). Clean-room behavior, no IDM code.
const tabMedia = new Map(); // tabId -> [{url, mime, size, pageUrl, seenAt}]
const tabPlaylist = new Map(); // tabId -> playlist url (P4: demotes loose segments)
const seenTabs = new Map(); // canonical url -> Set(tabId), for the "×N tabs" hint
const sizeCache = new Map(); // url -> bytes (quality-menu sizes)

function rankKind(url, mime, entry) {
  if (typeof DCRank !== "undefined") return DCRank.kindOf({...(entry || {}), url, mime});
  if (typeof DCRank !== "undefined" && DCRank.isPlaylistUrl(url)) return "playlist";
  if (typeof DCRank !== "undefined" && DCRank.isSegmentUrl(url)) return "segment";
  // Legacy fallback when rank.js failed to load (same shapes, inline).
  if (/(\.m3u8|\.mpd)(\?|#|$)/i.test(String(url).split("?")[0]) || /m3u8|mpegurl|dash/i.test(String(url))) return "playlist";
  if (/(\.ts|\.m4s|\.cmfv|\.cmfa)(\?|#|$)/i.test(String(url).split("?")[0])) return "segment";
  return "file";
}

function canonKey(url) {
  if (typeof DCRank !== "undefined") return DCRank.canonicalUrl(url);
  return String(url || "").split("#")[0];
}

// Popup's ⏻: video detection on/off (kept in storage, mirrored here).
let detectionOn = true;
try {
  api.storage.local.get({ detection: true }).then((r) => { detectionOn = r.detection !== false; }, () => {});
  chrome.storage.onChanged.addListener((ch, area) => {
    if (area === "local" && ch.detection) detectionOn = ch.detection.newValue !== false;
  });
} catch (e) { /* storage unavailable: stay on */ }

// ---- what a sniffed video file is: its resolution, and whether it is DRM ----
// The first 512 KB of an MP4 hold its header (or say where it is): enough
// for "1080p" in the menus, and for flagging encrypted (DRM) files, which
// never play once saved. Read once per file, two at a time, and the read
// stops at 512 KB even when a server ignores the Range header.
const probedFiles = new Map(); // url -> {w,h,drm} | null
const probeQueue = [];
let probesRunning = 0;
function probeVideoFile(url, mime) {
  const m = String(mime || "").toLowerCase();
  const looksMp4 = /^video\/(mp4|quicktime|x-m4v)/.test(m) || (!m.startsWith("audio/") && /\.(mp4|m4v|mov)(\?|#|$)/i.test(url));
  if (!looksMp4 || probedFiles.has(url) || typeof DCRank === "undefined" || !DCRank.mp4Info) return;
  probedFiles.set(url, null);
  if (probedFiles.size > 400) probedFiles.delete(probedFiles.keys().next().value);
  probeQueue.push(url);
  pumpProbes();
}
function pumpProbes() {
  while (probesRunning < 2 && probeQueue.length) {
    const url = probeQueue.shift();
    probesRunning++;
    probeMp4(url).then((info) => {
      if (!info) return;
      probedFiles.set(url, info);
      for (const list of tabMedia.values()) {
        for (const e of list) if (e.url === url) Object.assign(e, { w: info.w || undefined, h: info.h || undefined, drm: info.drm || undefined });
      }
    }, () => {}).finally(() => { probesRunning--; pumpProbes(); });
  }
}
async function readBytes(url, start, max) {
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), 12000);
  try {
    const r = await fetch(url, { headers: { Range: `bytes=${start}-${start + max - 1}` }, credentials: "include", cache: "no-store", signal: ctl.signal });
    if (!(r.status === 206 || (r.status === 200 && start === 0)) || !r.body) return null;
    const reader = r.body.getReader();
    const out = new Uint8Array(max);
    let n = 0;
    while (n < max) {
      const { done, value } = await reader.read();
      if (done) break;
      const take = Math.min(value.length, max - n);
      out.set(value.subarray(0, take), n);
      n += take;
    }
    try { await reader.cancel(); } catch (e) { /* done */ }
    return out.subarray(0, n);
  } finally { clearTimeout(t); ctl.abort(); }
}
async function probeMp4(url) {
  const head = await readBytes(url, 0, 512 * 1024);
  if (!head) return null;
  let info = DCRank.mp4Info(head);
  if (info && info.moovAt) {
    const tail = await readBytes(url, info.moovAt, 4 * 1024 * 1024);
    info = tail ? DCRank.mp4Info(tail, info.moovAt) : null;
  }
  return info && (info.h > 0 || info.drm) ? info : null;
}

function noteMedia(tabId, entry) {
  if (!detectionOn) return;
  if (tabId === undefined || tabId < 0) return;
  if (!entry || !entry.url || !/^https?:\/\//i.test(entry.url)) return;
  const whole = stripFbSlice(entry.url);
  if (whole !== entry.url) {
    // A Facebook byte range with its range removed is the whole quality
    // file, not a fragment: flagged as one, the reel's own download was
    // refused as "a playback fragment".
    entry.segment = undefined;
    entry.whole = true;
  }
  entry.url = whole;
  if (entry.url.length > 4000) return;
  // Sniffed verdicts (a .ts behind an opaque URL, a subtitle by content)
  // outrank URL-shape guesses.
  const kind = entry.whole ? "file" : entry.segment ? "segment"
    : entry.subtitle ? "subtitle" : rankKind(entry.url, entry.mime, entry);
  if (kind === "subtitle") entry.kind = "subtitle";
  // P4: a playlist on the tab demotes loose segments — they are children
  // the engine fetches itself (ad slivers hide among them), so menus must
  // offer the whole, and the badge must not count parts as downloads.
  if (kind === "playlist") tabPlaylist.set(tabId, entry.url);
  if (kind === "segment" && tabPlaylist.get(tabId)) entry.child = true;
  let list = tabMedia.get(tabId);
  if (!list) { list = []; tabMedia.set(tabId, list); }
  if (list.some((e) => e.url === entry.url)) return;
  list.push({ ...entry, kind, seenAt: Date.now() });
  if (kind === "file") probeVideoFile(entry.url, entry.mime);
  // An observed playlist reference is stronger evidence than a filename:
  // some sites name every individual segment .mp4.
  if (typeof DCRank !== "undefined") {
    for (const item of list) {
      if (DCRank.kindOf(item) === "playlist") continue;
      if (list.some(parent => DCRank.kindOf(parent) === "playlist"
          && DCRank.samePlayer(parent, item) && (parent.children || []).includes(item.url))) {
        item.segment = true; item.kind = "segment"; item.child = true;
      }
    }
  }
  if (list.length > 25) {
    while (list.length > 25) {
      const part = list.findIndex(m => rankKind(m.url, m.mime) !== "playlist");
      const [d] = list.splice(part < 0 ? 0 : part, 1);
      pruneSeen(d.url, tabId);
    }
  }
  // Cross-tab duplicate collapse (P4): one canonical entry, tab count.
  const ck = canonKey(entry.url);
  let tabs = seenTabs.get(ck);
  if (!tabs) { tabs = new Set(); seenTabs.set(ck, tabs); }
  tabs.add(tabId);
  if (seenTabs.size > 500) {
    seenTabs.clear(); // bounded best-effort; rebuilt by live traffic
  }
  // Legacy global snapshot: fallback only (popup prefers per-tab peek).
  // Never from an incognito tab: storage.local is on disk and outlives the
  // private window (its page and signed media URLs would persist).
  try {
    api.tabs.get(tabId).then((t) => {
      if (t && !t.incognito) chrome.storage.local.set({ downloadCraftNet: list.slice(-25) });
    }).catch(() => {});
  } catch (e) { /* ignore */ }
  // Badge counts downloadable videos/streams, never playlist children or
  // subtitle side files: first from this list, then (a moment later) the
  // number the popup itself lists for the page.
  const badgeN = list.filter((e) => !e.child && e.kind !== "subtitle").length;
  setTabBadge(tabId, badgeN);
  scheduleBadge(tabId);
}

// A per-tab badge overrides the global one, so while the app is down the
// tab keeps the red "!" instead of a count that hides the failure.
function setTabBadge(tabId, n) {
  try {
    api.action.setBadgeText({ text: hostOk ? (n > 0 ? String(n) : "") : "!", tabId }).catch(() => {});
    api.action.setBadgeBackgroundColor({ color: hostOk ? "#1F6FEB" : "#C42B1C", tabId }).catch(() => {});
  } catch (e) { /* ignore */ }
}

// The badge shows what the popup's Videos tab will list: ask the page for
// the same snapshot the popup reads and count it the same way. Media
// responses arrive in bursts, so one count per tab per moment.
const badgeTimers = new Map();
function scheduleBadge(tabId) {
  if (!(tabId >= 0) || typeof DCRank === "undefined" || !DCRank.countVideos) return;
  clearTimeout(badgeTimers.get(tabId));
  badgeTimers.set(tabId, setTimeout(async () => {
    badgeTimers.delete(tabId);
    try {
      const peek = await api.tabs.sendMessage(tabId, { type: "peekState" }, { frameId: 0 });
      if (peek && (peek.videos || peek.links || peek.net || peek.fb)) setTabBadge(tabId, DCRank.countVideos(peek));
    } catch (e) { /* no content script on this page: keep the list count */ }
  }, 700));
}

function pruneSeen(url, tabId) {
  try {
    const ck = canonKey(url);
    const tabs = seenTabs.get(ck);
    if (!tabs) return;
    tabs.delete(tabId);
    if (!tabs.size) seenTabs.delete(ck);
  } catch (e) { /* ignore */ }
}

function forgetTab(tabId, newUrl) {
  tabMedia.delete(tabId);
  tabPlaylist.delete(tabId);
  tabNavSeq.set(tabId, navSeqOf(tabId) + 1);
  // The new page's origin was recorded by its main_frame response, which
  // arrives before this tab update: keep it, drop the previous page's frames.
  tabOrigins.delete(tabId);
  if (newUrl) noteTabOrigin(tabId, newUrl);
  try {
    for (const [ck, tabs] of seenTabs) {
      tabs.delete(tabId);
      if (!tabs.size) seenTabs.delete(ck);
    }
  } catch (e) { /* ignore */ }
  try { api.action.setBadgeText({ text: hostOk ? "" : "!", tabId }).catch(() => {}); } catch (e) {}
}

// Request types that can carry media. Frames are included so a video opened
// directly in a tab or iframe is caught; classifyResponse skips HTML pages.
const SNIFF_REQUEST_TYPES = new Set(["media", "xmlhttprequest", "other", "main_frame", "sub_frame", "object"]);

// Exact request headers per in-flight request (session cookie, referer,
// origin, bearer and CDN x-* tokens), so the app replays what the player
// sent instead of guessing. Background-only: never stored, never sent to
// page scripts.
const requestAuth = new Map(); // requestId -> {referer, cookie, headers}
// Same data, kept per captured media URL for the hand-off to the app.
const mediaAuth = new Map(); // url -> {referer, cookie, headers}
function rememberAuth(map, key, value, cap) {
  map.delete(key);
  map.set(key, value);
  while (map.size > cap) map.delete(map.keys().next().value);
}

// Loopback, private-network and link-local hosts (what a hostile page must
// not steer credentialed extension/app requests at).
function privateHost(u) {
  let h = "";
  try { h = new URL(u).hostname.toLowerCase().replace(/^\[|\]$/g, ""); } catch (e) { return false; }
  return h === "localhost" || h.endsWith(".localhost") || h.endsWith(".local") || h === "::1"
    || /^127\./.test(h) || /^10\./.test(h) || /^192\.168\./.test(h) || /^169\.254\./.test(h)
    || /^172\.(1[6-9]|2\d|3[01])\./.test(h) || /^0\./.test(h) || /^f[cd][0-9a-f]{2}:/.test(h) || /^fe80:/.test(h);
}

// True when `url` is known only from a page's own report (page-hook), never
// from observed network traffic.
function pageReportedOnly(url) {
  let page = false, net = false;
  for (const items of tabMedia.values()) {
    for (const m of items) {
      if (m.url !== url) continue;
      if (m.via === "page") page = true; else net = true;
    }
  }
  return page && !net;
}

// Origins each tab has loaded (top page + frames). Workers and service
// workers fetch with tabId -1; their initiator origin maps them back to the
// tab that owns them.
const tabOrigins = new Map(); // tabId -> Map(origin -> lastSeen)
function originOf(u) {
  try { const x = new URL(u); return /^https?:$/.test(x.protocol) ? x.origin : null; } catch (e) { return null; }
}
function noteTabOrigin(tabId, url) {
  const origin = originOf(url);
  if (tabId === undefined || tabId < 0 || !origin) return;
  let seen = tabOrigins.get(tabId);
  if (!seen) { seen = new Map(); tabOrigins.set(tabId, seen); }
  seen.set(origin, Date.now());
  if (seen.size > 50) seen.delete(seen.keys().next().value);
}
async function resolveTabId(details) {
  if (details.tabId !== undefined && details.tabId >= 0) return details.tabId;
  const origin = originOf(details.initiator || details.documentUrl || details.originUrl || "");
  if (!origin) return null;
  let best = null, bestAt = -1;
  for (const [tabId, seen] of tabOrigins) {
    const at = seen.get(origin);
    if (at !== undefined && at > bestAt) { best = tabId; bestAt = at; }
  }
  if (best !== null) return best;
  try {
    const tabs = await api.tabs.query({});
    const owners = tabs.filter(t => t.id !== undefined && originOf(t.url || "") === origin);
    owners.sort((a, b) => (b.active - a.active) || ((b.lastAccessed || 0) - (a.lastAccessed || 0)));
    return owners.length ? owners[0].id : null;
  } catch (e) { return null; }
}

// Bumped on every navigation: results of a slow sniff that finish after the
// tab moved on belong to the previous page and are dropped.
const tabNavSeq = new Map(); // tabId -> number
const navSeqOf = (tabId) => tabNavSeq.get(tabId) || 0;

// First-bytes sniffing for responses whose URL and type hide what they are.
// Bounded so busy pages never double their traffic.
const SNIFF_BYTES = 4096;
const SNIFF_MAX_INFLIGHT = 4;
const sniffedUrls = new Map(); // url -> result (null = not media)
let sniffInflight = 0;
// Token headers the player sent (Authorization, x-*) that fetch may set;
// cookies ride along via credentials, Origin/Referer are the browser's.
function replayHeaders(auth) {
  const out = {};
  for (const [k, v] of Object.entries((auth && auth.headers) || {})) {
    if (k !== "origin") out[k] = v;
  }
  return out;
}
// Throws when the server refuses (so the caller does not cache a miss).
async function readHead(url, maxBytes, timeoutMs, headers) {
  const res = await fetch(url, {
    credentials: "include",
    headers: { ...(headers || {}), Range: `bytes=0-${maxBytes - 1}` },
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!res.ok || !res.body) throw new Error(`HTTP ${res.status}`);
  // Servers may ignore Range and stream the whole file: stop at maxBytes.
  const reader = res.body.getReader();
  const parts = [];
  let got = 0;
  try {
    while (got < maxBytes) {
      const { value, done } = await reader.read();
      if (done) break;
      parts.push(value);
      got += value.length;
    }
  } finally { try { await reader.cancel(); } catch (e) {} }
  const out = new Uint8Array(Math.min(got, maxBytes));
  let at = 0;
  for (const p of parts) {
    const take = Math.min(p.length, out.length - at);
    out.set(p.subarray(0, take), at);
    at += take;
    if (at >= out.length) break;
  }
  return out;
}
async function sniffUrl(url, headers) {
  if (sniffedUrls.has(url)) return sniffedUrls.get(url);
  if (sniffInflight >= SNIFF_MAX_INFLIGHT) return null; // busy: skip, not cached
  sniffInflight++;
  try {
    const head = await readHead(url, SNIFF_BYTES, 6000, headers);
    const hit = typeof DCRank !== "undefined" ? DCRank.sniffBytes(head) : null;
    rememberAuth(sniffedUrls, url, hit, 500); // a definitive answer only
    return hit;
  } catch (e) {
    return null; // refused or timed out: the player's next request may retry
  } finally { sniffInflight--; }
}

// Master flag + child references of a playlist, for parent/part linking.
async function playlistShape(url, headers) {
  let master = false, children = [];
  try {
    const res = await fetch(url, {credentials:"include", headers:headers || {}, signal:AbortSignal.timeout(8000)});
    if (res.ok && (!res.headers.get("content-length") || Number(res.headers.get("content-length")) <= 1024 * 1024)) {
      const reader = res.body.getReader();
      let body = "", size = 0;
      const decoder = new TextDecoder();
      try {
        while (size <= 1024 * 1024) {
          const {value, done} = await reader.read();
          if (done) break;
          size += value.length; body += decoder.decode(value, {stream:true});
          if (/#EXT-X-STREAM-INF|<MPD[\s>]/i.test(body)) master = true;
        }
        if (typeof DCRank !== "undefined") children = DCRank.playlistReferences(body, url);
      } finally { await reader.cancel(); }
    }
  } catch (e) { /* metadata optional; keep the original signed URL */ }
  return { master, children };
}

function responseMeta(details) {
  const meta = { url: details.url, type: details.type, method: details.method,
    status: details.statusCode, mime: "", size: 0, filename: null, acceptRanges: false };
  let clen = 0, total = 0;
  for (const h of details.responseHeaders || []) {
    const n = String(h.name || "").toLowerCase();
    const v = String(h.value || "");
    if (n === "content-type") meta.mime = v.split(";")[0].trim().toLowerCase();
    else if (n === "content-length") clen = parseInt(v, 10) || 0;
    // Range chunks (206) report the CHUNK size in content-length, not the
    // file size — judge the size filter by the Content-Range total
    // (`bytes 0-0/12345`), or a throttled CDN's drips get skipped and
    // Facebook reels never produce a pill without a reload.
    else if (n === "content-range") { const m = v.match(/\/(\d+)\s*$/); if (m) total = parseInt(m[1], 10) || 0; }
    else if (n === "accept-ranges") meta.acceptRanges = /bytes/i.test(v);
    else if (n === "content-disposition" && typeof DCRank !== "undefined") meta.filename = DCRank.dispositionFilename(v);
  }
  meta.size = total || clen;
  if (total) meta.acceptRanges = true;
  return meta;
}

async function onMediaResponse(details) {
  if (!SNIFF_REQUEST_TYPES.has(details.type) || typeof DCRank === "undefined") return;
  if (details.initiator && /^(?:chrome|moz)-extension:/i.test(details.initiator)) return; // our own sniffs
  const auth = requestAuth.get(details.requestId) || null;
  const tabId = await resolveTabId(details);
  if (tabId === null || tabId < 0) return;
  const seq = navSeqOf(tabId);
  const cfg = await settings().catch(() => ({ blockHosts: "", minSizeKB: 100, deepDetect: true }));
  if (hostBlocked(details.url, cfg.blockHosts)) return;
  const meta = responseMeta(details);
  const minBytes = Math.max(0, (parseInt(cfg.minSizeKB, 10) || 100)) * 1024;
  let hit = DCRank.classifyResponse(meta, { minBytes, deep: cfg.deepDetect !== false });
  if (hit && hit.sniff) {
    const found = await sniffUrl(details.url, replayHeaders(auth));
    if (!found) return;
    if (found.kind === "file" && meta.size > 0 && meta.size < minBytes) return;
    hit = found;
  }
  if (!hit) return;
  const shape = hit.kind === "playlist" ? await playlistShape(details.url, replayHeaders(auth)) : { master: false, children: [] };
  if (navSeqOf(tabId) !== seq) return; // the tab navigated while we looked
  if (auth) rememberAuth(mediaAuth, stripFbSlice(details.url), auth, 300);
  noteMedia(tabId, {
    master: shape.master, children: shape.children,
    url: details.url,
    mime: hit.mime || meta.mime,
    // A playlist's byte length is its text, not the video.
    size: hit.kind === "playlist" ? 0 : (meta.size || 0),
    pageUrl: details.documentUrl || null,
    frameId: details.tabId >= 0 ? details.frameId : undefined,
    segment: hit.kind === "segment" || undefined,
    subtitle: hit.kind === "subtitle" || undefined,
    via: hit.reason,
  });
}

try {
  if (chrome.webRequest && chrome.webRequest.onSendHeaders) {
    // Chrome hides Cookie/Referer/Origin from this event unless asked with
    // extraHeaders; Firefox exposes them already and rejects the option.
    const spec = ["requestHeaders"];
    if (chrome.webRequest.OnSendHeadersOptions
        && Object.prototype.hasOwnProperty.call(chrome.webRequest.OnSendHeadersOptions, "EXTRA_HEADERS")) {
      spec.push("extraHeaders");
    }
    chrome.webRequest.onSendHeaders.addListener((details) => {
      try {
        if (!SNIFF_REQUEST_TYPES.has(details.type) || typeof DCRank === "undefined") return;
        if (details.initiator && /^(?:chrome|moz)-extension:/i.test(details.initiator)) return;
        const picked = DCRank.forwardHeaders(details.requestHeaders);
        if (picked.referer || picked.cookie || Object.keys(picked.headers).length) {
          rememberAuth(requestAuth, details.requestId, picked, 1000);
        }
      } catch (e) { /* header capture is best-effort */ }
    }, { urls: ["http://*/*", "https://*/*"] }, spec);
  }
  if (chrome.webRequest && chrome.webRequest.onHeadersReceived) {
    chrome.webRequest.onHeadersReceived.addListener(
      (details) => {
        if (details.tabId >= 0) {
          if (details.type === "main_frame" || details.type === "sub_frame") noteTabOrigin(details.tabId, details.url);
          if (details.initiator) noteTabOrigin(details.tabId, details.initiator);
        }
        onMediaResponse(details).catch(() => { /* never break navigation */ });
      },
      { urls: ["http://*/*", "https://*/*"] },
      ["responseHeaders"]
    );
  }
  if (chrome.webRequest && chrome.webRequest.onCompleted) {
    const done = (details) => { requestAuth.delete(details.requestId); };
    chrome.webRequest.onCompleted.addListener(done, { urls: ["http://*/*", "https://*/*"] });
    chrome.webRequest.onErrorOccurred.addListener(done, { urls: ["http://*/*", "https://*/*"] });
  }
} catch (e) { /* webRequest unavailable (Firefox MV3 w/o permission) */ }

try {
  if (chrome.tabs && chrome.tabs.onRemoved) {
    chrome.tabs.onRemoved.addListener((tabId) => {
      forgetTab(tabId);
      tabNavSeq.delete(tabId);
    });
  }
  // New navigation (same tab, new video): drop the previous video's URLs so
  // itag-swaps and menus can never graft stale streams. SPA history swaps
  // (Facebook reels, YouTube shorts) fire onUpdated with a new url but no
  // "loading" status — clear on any url change too, so the next reel can't
  // offer the previous reel's fbcdn link.
  if (chrome.tabs && chrome.tabs.onUpdated) {
    chrome.tabs.onUpdated.addListener((tabId, info) => {
      if ((info.status === "loading" && info.url !== undefined) || info.url !== undefined) {
        forgetTab(tabId, info.url);
      }
    });
  }
} catch (e) { /* ignore */ }

function setBadge() {
  api.action
    .setBadgeText({ text: hostOk ? "" : "!" })
    .catch(() => {});
  if (!hostOk) {
    api.action
      .setBadgeBackgroundColor({ color: "#C42B1C" })
      .catch(() => {});
  }
}

// Ping once at startup so the badge reflects reality.
sendToHostPing();
async function sendToHostPing() {
  try {
    const res = await hostSend({ type: "ping" });
    hostOk = !!res && res.type === "pong";
    // The app's Light/Dark/System choice, for the in-page bar and panels.
    if (res && /^(light|dark|system)$/.test(res.theme || "")) {
      const cur = await api.storage.local.get({ appTheme: "system" });
      if (cur.appTheme !== res.theme) await api.storage.local.set({ appTheme: res.theme });
    }
    if (!hostOk) lastHostError = "ping answered, but no pong (old host version?)";
    console.log(
      "[download-craft] native host '" + HOST + "': " +
        (hostOk ? "connected" : "unexpected reply: " + lastHostError)
    );
  } catch (e) {
    hostOk = false;
    lastHostError = (e && e.message) || String(e);
    console.error(
      "[download-craft] native host '" + HOST + "' unreachable:", lastHostError
    );
    // Say how to fix it (at most once a day). The extension keeps working
    // on its own meanwhile: "Download in browser" needs no app.
    if (/native|host|not found|not installed/i.test(lastHostError)) showWelcome("no-app");
  }
  setBadge();
  await syncBrowserDownloadUI();
}

// Pages where "Download video with Download Craft" is also on the page menu.
const VIDEO_SITE_PATTERNS = [
  "*://*.youtube.com/*", "*://*.facebook.com/*", "*://*.instagram.com/*", "*://*.tiktok.com/*",
  "*://*.x.com/*", "*://x.com/*", "*://*.twitter.com/*", "*://*.reddit.com/*", "*://*.vimeo.com/*", "*://vimeo.com/*",
];

// A page's self-check found something the video button relies on missing
// (the site's layout changed): one line in the app's log, at most once a
// day per kind of problem.
async function layoutReport(key, text) {
  try {
    const day = 24 * 3600 * 1000;
    const { dcReports = {} } = await api.storage.local.get({ dcReports: {} });
    const now = Date.now();
    if (dcReports[key] && now - dcReports[key] < day) return;
    dcReports[key] = now;
    for (const k of Object.keys(dcReports)) if (now - dcReports[k] > 7 * day) delete dcReports[k];
    await api.storage.local.set({ dcReports });
    await hostSend({ type: "report", note: `${String(text).slice(0, 260)} (extension ${EXT_VER})` });
  } catch (e) { /* the app is not running: next time */ }
}

// Text from _locales in the browser's language, English when there is none.
function msg(key, english) {
  try { return (chrome.i18n && chrome.i18n.getMessage(key)) || english; } catch (e) { return english; }
}

chrome.runtime.onInstalled.addListener((details) => {
  if (details && details.reason === "install") showWelcome("install");
  // removeAll first: re-installs otherwise fail with duplicate-id.
  api.contextMenus.removeAll().then(() => {
    chrome.contextMenus.create({
      id: "send-to-download-craft",
      title: msg("menuDownload", "Download with Download Craft"),
      contexts: ["link", "audio"],
    });
    // The video under the pointer, as its button would download it. Video
    // sites cover the player with their own layers (the click target is not
    // the <video>), so on those the item is on the page menu too.
    chrome.contextMenus.create({
      id: "download-video",
      title: msg("menuDownloadVideo", "Download video with Download Craft"),
      contexts: ["video"],
    });
    chrome.contextMenus.create({
      id: "download-video-page",
      title: msg("menuDownloadVideo", "Download video with Download Craft"),
      contexts: ["page", "frame"],
      documentUrlPatterns: VIDEO_SITE_PATTERNS,
    });
    // IDM's "Download all links": every link on the page (or in the
    // selection) into the app's pick-and-queue list.
    chrome.contextMenus.create({
      id: "download-all-links",
      title: msg("menuDownloadAll", "Download all links with Download Craft"),
      contexts: ["page", "selection"],
    });
    // IDM's "Download with IDM" on the browser's own downloads page: hand an
    // existing (or failed) browser download to the app. Chromium only.
    if (!(typeof browser !== "undefined" && browser.runtime && browser.runtime.getBrowserInfo)) {
      try {
        chrome.contextMenus.create({
          id: "download-item",
          title: msg("menuDownload", "Download with Download Craft"),
          contexts: ["link"],
          documentUrlPatterns: ["chrome://downloads/*"],
        }, () => void chrome.runtime.lastError);
      } catch (e) { /* not supported here */ }
    }
  });
  sendToHostPing();
  // Push the current page script into every open tab. Content scripts in
  // already-open tabs would otherwise keep running the pre-update code
  // (and fail once their APIs are gone) until each tab is reloaded by hand.
  refreshOpenTabs();
});

async function refreshOpenTabs() {
  try {
    const tabs = await api.tabs.query({});
    for (const t of tabs) {
      if (t.id === undefined) continue;
      try {
        await api.scripting.executeScript({
          target: { tabId: t.id },
          files: ["rank.js", "content.js"],
        });
        // Page reader: catches what the open tab loads from now on.
        await api.scripting.executeScript({ target: { tabId: t.id, allFrames: true }, files: ["page-relay.js"] });
        await api.scripting.executeScript({ target: { tabId: t.id, allFrames: true }, files: ["page-hook.js"], world: "MAIN" });
      } catch (e) {
        /* chrome:// pages, web store, etc. — not scriptable, skip */
      }
    }
  } catch (e) {
    /* tabs API unavailable */
  }
}

// Runs in the page (scripting.executeScript), so it must be self-contained.
// Returns [{url, text}] for every http(s) link and media source, fragments
// dropped and duplicates merged. selectionOnly = links the selection touches.
function pageLinks(selectionOnly) {
  const sel = selectionOnly ? window.getSelection() : null;
  const inSel = (el) => !sel || sel.isCollapsed || sel.containsNode(el, true);
  const out = [];
  const seen = new Map();
  const add = (raw, text) => {
    let u;
    try { u = new URL(raw, document.baseURI); } catch (e) { return; }
    if (u.protocol !== "http:" && u.protocol !== "https:") return;
    u.hash = "";
    const url = u.href;
    if (url.length > 4096) return;
    const t = String(text || "").replace(/\s+/g, " ").trim().slice(0, 200);
    if (seen.has(url)) {
      const row = seen.get(url);
      if (!row.text && t) row.text = t;
      return;
    }
    if (out.length >= 5000) return;
    const row = { url, text: t };
    seen.set(url, row);
    out.push(row);
  };
  // innerText, not textContent: an inline <style>/<script> (SVG logos) is
  // not link text.
  const textOf = (a) => {
    const img = a.querySelector && a.querySelector("img[alt]");
    return (a.innerText || "").trim() || a.getAttribute("title") || a.getAttribute("aria-label")
      || a.getAttribute("alt") || (img && img.getAttribute("alt")) || "";
  };
  for (const a of document.querySelectorAll("a[href], area[href]")) {
    if (inSel(a)) add(a.getAttribute("href"), textOf(a));
  }
  for (const m of document.querySelectorAll("video[src], audio[src], source[src]")) {
    if (inSel(m)) add(m.getAttribute("src"), m.getAttribute("title") || "");
  }
  for (const img of document.querySelectorAll("img[src]")) {
    if (inSel(img)) add(img.currentSrc || img.getAttribute("src"), img.getAttribute("alt") || "");
  }
  // Plain-text URLs: in the selection, in text boxes, and (whole page only)
  // in frames and scripts.
  const URL_RE = /\bhttps?:\/\/[^\s"'<>()\[\]{}]+/gi;
  const fromText = (text) => { for (const m of String(text || "").matchAll(URL_RE)) add(m[0].replace(/[.,;:!?]+$/, ""), ""); };
  if (sel && !sel.isCollapsed) fromText(sel.toString());
  for (const ta of document.querySelectorAll("textarea")) {
    if (!sel || sel.isCollapsed) fromText(ta.value);
    else if (sel.containsNode(ta, true)) fromText(ta.value);
  }
  if (!selectionOnly) {
    for (const f of document.querySelectorAll("iframe[src]")) add(f.getAttribute("src"), "");
    for (const sc of document.querySelectorAll("script:not([src])")) {
      if (sc.textContent && sc.textContent.length < 2000000) fromText(sc.textContent);
    }
  }
  return out;
}

async function sendAllLinks(tab, selectionOnly) {
  if (!tab || tab.id === undefined || !/^https?:\/\//i.test(tab.url || "")) return;
  let links = [];
  try {
    const results = await api.scripting.executeScript({
      target: { tabId: tab.id },
      func: pageLinks,
      args: [!!selectionOnly],
    });
    links = (results && results[0] && results[0].result) || [];
  } catch (e) {
    lastHostError = "this page cannot be read (" + ((e && e.message) || e) + ")";
    return;
  }
  if (!links.length) return;
  try {
    // The page's own cookies only: the app applies them to same-host links.
    const cookie = await cookiesFor(tab.url);
    const res = await hostSend({
      type: "links", pageUrl: tab.url, title: tab.title || "", cookie, links,
    });
    hostOk = !!res && res.type === "ok";
  } catch (e) {
    lastHostError = (e && e.message) || String(e);
    hostOk = false;
  }
  setBadge();
}

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "download-video" || info.menuItemId === "download-video-page") {
    const fallback = () => {
      const src = /^https?:\/\//i.test(info.srcUrl || "") ? info.srcUrl : null;
      const url = src || info.frameUrl || info.pageUrl || (tab && tab.url);
      if (!url || !/^https?:\/\//i.test(url)) return;
      markSeen(url, null);
      sendToHost(url, null, { pageUrl: info.pageUrl || (tab && tab.url) || null }).then((ok) => { hostOk = ok; setBadge(); });
    };
    if (!tab || !(tab.id >= 0)) { fallback(); return; }
    api.tabs.sendMessage(tab.id, { type: "ctxDownloadVideo" }, { frameId: info.frameId || 0 })
      .then((r) => { if (!r || !r.ok) fallback(); })
      .catch(fallback);
    return;
  }
  if (info.menuItemId === "download-all-links") {
    sendAllLinks(tab, !!info.selectionText);
    return;
  }
  if (info.menuItemId === "download-item" && /^https?:\/\//i.test(info.linkUrl || "")) {
    (async () => {
      let referrer = null;
      try {
        const hits = await api.downloads.search({ url: info.linkUrl });
        const hit = hits && hits.find((d) => d.referrer);
        referrer = hit ? hit.referrer : null;
      } catch (e) {}
      markSeen(info.linkUrl, null);
      hostOk = await sendToHost(info.linkUrl, null, { pageUrl: referrer, referer: referrer });
      setBadge();
    })();
    return;
  }
  const url = info.linkUrl || info.srcUrl;
  if (url && /^https?:\/\//i.test(url)) {
    markSeen(url, null);
    const pageUrl = (tab && tab.url) || null;
    sendToHost(url, null, { pageUrl }).then((ok) => {
      hostOk = ok;
      setBadge();
    });
  }
});

// ---- automatic capture ----
async function performBrowserCapture(item) {
  const cfg = await settings();
  const url = item.finalUrl || item.url;
  if (!url || !/^https?:\/\//i.test(url)) return; // blobs/data stay in browser
  if (!cfg.autoCapture) return;
  // Only a download that just started. When the browser opens it can report
  // downloads from the last session (one paused while the app was asked, an
  // interrupted one it resumes); capturing those popped the app's window on
  // every browser start, however often it was cancelled.
  const started = Date.parse(item.startTime || "");
  if (Number.isFinite(started) && Date.now() - started > 30000) return;
  // Brave also replays history entries with no start time, or as finished /
  // interrupted items: only a live transfer is a fresh click.
  if (!Number.isFinite(started) && item.state && item.state !== "in_progress") return;
  if (item.state === "complete" || item.endTime || item.paused) return;
  if (captureBurst()) return;
  // The user chose "Download in browser" for this one: never hand it back.
  if (item.byExtensionId && item.byExtensionId === chrome.runtime.id) return;
  // Clicked with the "keep in browser" key held.
  if (skipped(item.url) || skipped(url)) return;
  // A form (POST) download is handed over with its body when the browser
  // showed it to us; without the body it stays in the browser.
  const ctx = typeof DCBridge !== "undefined" ? DCBridge.browserContext(url) : {};
  const post = ctx._post || nonReplayableDownload(item.url) || nonReplayableDownload(url);
  if (post && ctx.method !== "POST") return;
  if (hostBlocked(url, cfg.blockHosts)) return; // P0.1 site filter
  if (!typeAllowed(cfg, item.filename, url)) return; // the app's file types
  if (freshSeen(url, item.filename)) return; // our own fallback re-download
  // Honor the size filter here too (sniffer-only before): tiny files stay
  // in the browser. Unknown sizes (0/undefined) still capture.
  const minB = Math.max(0, (parseInt(cfg.minSizeKB, 10) || 0)) * 1024;
  const known = item.fileSize || item.totalBytes || 0;
  if (minB > 0 && known > 0 && known < minB) return;
  // IDM's way: PAUSE the browser's download while the app is asked, so a
  // "no" (or no app) simply resumes it - same request, same session, no
  // bytes lost. Only a download that cannot pause is cancelled up front.
  let paused = false;
  try {
    await api.downloads.pause(item.id);
    paused = true;
  } catch (e) {
    try {
      await api.downloads.cancel(item.id);
    } catch (e2) {
      // Already finished (fast download): cancelling is impossible, and
      // re-sending would deliver a SECOND copy — leave the browser's file.
      return;
    }
  }
  const ok = await sendToHost(url, item.filename, {pageUrl:item.referrer || null,referer:item.referrer || null, fromRequest:true});
  hostOk = ok;
  setBadge();
  if (ok) {
    // The app has it: drop the browser's copy and its entry.
    if (paused) { try { await api.downloads.cancel(item.id); } catch (e) { /* finished meanwhile */ } }
    try { await api.downloads.erase({ id: item.id }); } catch (e) { /* already gone */ }
    return;
  }
  if (paused) {
    // The app declined or is not there: the browser carries on.
    try { await api.downloads.resume(item.id); } catch (e) { /* the user acted on it */ }
    await syncBrowserDownloadUI();
    return;
  }
  if (!ok) {
    await syncBrowserDownloadUI();
    // Native host missing: hand the file back to the browser so nothing is lost.
    markSeen(url, null);
    chrome.downloads.download({ url, saveAs: false }, () => {
      if (chrome.runtime.lastError) {
        lastFallbackError = chrome.runtime.lastError.message || "browser re-download failed";
        setBadge();
      } else {
        lastFallbackError = "";
      }
    });
  }
}

const nonGetRequests = new Map();
function rememberDownloadRequest(details) {
  if (!details.url || !details.method || String(details.method).toUpperCase() === "GET") return;
  nonGetRequests.set(details.url, Date.now());
  if (nonGetRequests.size > 500) nonGetRequests.delete(nonGetRequests.keys().next().value);
}
function nonReplayableDownload(url) {
  const time = nonGetRequests.get(url);
  if (time === undefined) return false;
  if (Date.now() - time > 60000) { nonGetRequests.delete(url); return false; }
  return true;
}
try {
  if (chrome.webRequest && chrome.webRequest.onBeforeRequest) {
    chrome.webRequest.onBeforeRequest.addListener(rememberDownloadRequest,
      {urls:["http://*/*", "https://*/*"]});
  }
} catch (e) { /* unsupported API; normal direct-link capture still works */ }

// Flood breaker: a person starts a few downloads at a time, never dozens in
// seconds. When the browser replays its whole download history (seen in
// Brave), every entry would pop the app and the browser had to be killed;
// past the limit automatic capture rests for a minute and the downloads stay
// in the browser.
const BURST_WINDOW_MS = 10000;
const BURST_MAX = 6;
const BURST_REST_MS = 60000;
let burstTimes = [];
let burstUntil = 0;
function captureBurst() {
  const now = Date.now();
  if (now < burstUntil) return true;
  burstTimes = burstTimes.filter((t) => now - t < BURST_WINDOW_MS);
  burstTimes.push(now);
  if (burstTimes.length <= BURST_MAX) return false;
  burstUntil = now + BURST_REST_MS;
  burstTimes = [];
  console.warn("[download-craft] too many browser downloads at once; capture paused for a minute");
  return true;
}

const browserCaptures = new Map();
function captureBrowserDownload(item) {
  const previous = browserCaptures.get(item.id);
  if (previous) { Object.assign(previous.item,item); return previous.promise; }
  const state = {item:{...item},promise:null};
  state.promise = performBrowserCapture(state.item).catch(e => {
    console.error("[download-craft] capture failed",e);
  });
  browserCaptures.set(item.id,state);
  if (browserCaptures.size > 200) browserCaptures.delete(browserCaptures.keys().next().value);
  return state.promise;
}
chrome.downloads.onCreated.addListener(item => { void captureBrowserDownload(item); });
if (chrome.downloads.onDeterminingFilename) {
  chrome.downloads.onDeterminingFilename.addListener((item,suggest) => {
    // Our own "Download in browser" files: while any extension listens here,
    // Chrome drops the name given to downloads.download() unless it is
    // suggested again - which is how joined videos were saved as the blob's
    // random id ("b26cd39c-….mp4") in the bare Downloads folder.
    const ours = item.byExtensionId === chrome.runtime.id
      && (bdlNames.get(item.url) || bdlNames.get(item.finalUrl));
    if (ours) {
      try { suggest({ filename: ours, conflictAction: "uniquify" }); } catch (e) {}
      return false;
    }
    captureBrowserDownload(item).finally(() => { try { suggest(); } catch (e) {} });
    return true;
  });
}


// Firefox supports asynchronous blocking at response headers. Chrome MV3
// uses the shared early-link path plus downloads.onDeterminingFilename above.
async function captureAttachmentResponse(details) {
  try {
    const file = DCRank.attachmentDownload(details);
    if (!file || freshSeen(details.url, file.filename)) return {};
    const cfg = await settings();
    const minBytes = Math.max(0, Number(cfg.minSizeKB) || 0) * 1024;
    if (!cfg.autoCapture || hostBlocked(details.url, cfg.blockHosts)
        || (file.size > 0 && file.size < minBytes)) return {};
    const pageUrl = details.originUrl || details.documentUrl || null;
    const ok = await sendToHost(details.url, file.filename,
      {pageUrl, referer:pageUrl, mime:file.mime, size:file.size || null});
    // If the app declines, continue the original request, including its
    // authentication/session. Do not cancel and recreate this request.
    markSeen(details.url, null);
    hostOk = ok; setBadge();
    return ok ? {cancel:true} : {};
  } catch (e) { return {}; }
}
try {
  if (typeof browser !== "undefined" && browser.runtime
      && typeof browser.runtime.getBrowserInfo === "function"
      && browser.webRequest && browser.webRequest.onHeadersReceived) {
    browser.webRequest.onHeadersReceived.addListener(captureAttachmentResponse,
      {urls:["http://*/*", "https://*/*"], types:["main_frame", "sub_frame", "other"]},
      ["blocking", "responseHeaders"]);
  }
} catch (e) { /* downloads.onCreated remains the fallback */ }

// ---- "Download in browser" (works without the app, like MAX) ----
// A plain file goes to the browser's own downloader. A video + audio pair,
// an HLS or a DASH stream is fetched and joined by DCBrowserDL - in Chrome's
// offscreen document (a service worker can neither hold a long download nor
// make a Blob URL), in Firefox's background page directly - and the joined
// file is then saved through chrome.downloads as well.
const bdlJobs = new Map(); // id -> job record (this session)
const bdlCtl = new Map(); // id -> AbortController (Firefox in-page runs)
const bdlNames = new Map(); // download URL -> relative path to save it under
let bdlSeq = 0;
const BDL_HISTORY_MAX = 50;

// The app's category folders (model::category_of), inside the browser's
// Downloads folder - the browser cannot save anywhere else - so a file
// saved in the browser lands where the app would put it by default.
function bdlCategory(name) {
  const ext = String(name || "").split(".").pop().toLowerCase();
  if (["mp4", "mkv", "avi", "mov", "webm", "ts", "m4s"].includes(ext)) return "Video";
  if (["mp3", "wav", "flac", "ogg", "m4a"].includes(ext)) return "Music";
  if (["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico"].includes(ext)) return "Image";
  if (["pdf", "doc", "docx", "txt", "epub"].includes(ext)) return "Document";
  if (["zip", "rar", "7z", "tar", "gz"].includes(ext)) return "Compressed";
  if (["exe", "msi"].includes(ext)) return "Apps";
  return "Other";
}
// downloads.download() with the name (and category folder) Chrome must use.
async function bdlDownload(url, name) {
  const path = `${bdlCategory(name)}/${name}`;
  bdlNames.set(url, path);
  try {
    return await api.downloads.download({ url, filename: path, conflictAction: "uniquify" });
  } finally {
    setTimeout(() => bdlNames.delete(url), 60000);
  }
}

function bdlSafeName(name, fallbackUrl) {
  let n = String(name || "").trim();
  if (!n) {
    try { n = decodeURIComponent(new URL(fallbackUrl).pathname.split("/").pop() || ""); } catch (e) { n = ""; }
  }
  n = n.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/^\.+/, "").trim().slice(0, 150);
  return n || "video.mp4";
}
function bdlPublic(j) {
  const { id, title, filename, kind, state, bytes, done, total, phase, error, note, startedAt, endedAt, downloadId, size, page } = j;
  return { id, title, filename, kind, state, bytes, done, total, phase, error, note, startedAt, endedAt, downloadId, size, page };
}
async function bdlHistory() {
  try { return (await api.storage.local.get({ bdlHistory: [] })).bdlHistory || []; } catch (e) { return []; }
}
// The page's own request headers for a job fetched here (Tentek/VDH's way):
// a fetch from the extension carries neither the page's Referer nor its
// Origin, and those are exactly what protected CDNs check. Session rules
// set them, plus the Authorization / x-* headers the player sent, on this
// extension's own requests while the job runs.
async function bdlHeaderRules(j, url) {
  const dnr = chrome.declarativeNetRequest;
  if (!dnr || typeof dnr.updateSessionRules !== "function") return;
  const auth = mediaAuth.get(url) || {};
  const page = auth.referer || j.page || "";
  const set = [];
  const add = (header, value) => {
    if (typeof value === "string" && value && value.length < 4096 && !/[\r\n]/.test(value)) {
      set.push({ header, operation: "set", value });
    }
  };
  if (/^https?:\/\//i.test(page)) {
    add("Referer", page);
    try { add("Origin", (auth.headers && auth.headers.origin) || new URL(page).origin); } catch (e) {}
  }
  for (const [k, v] of Object.entries(auth.headers || {})) {
    if (/^(authorization|x-[a-z0-9-]+)$/i.test(k) && k.toLowerCase() !== "x-client-data") add(k, v);
  }
  if (!set.length) return;
  const id = 900000 + (Math.floor(Math.random() * 90000));
  try {
    await dnr.updateSessionRules({ addRules: [{
      id, priority: 1,
      action: { type: "modifyHeaders", requestHeaders: set },
      condition: { initiatorDomains: [new URL(chrome.runtime.getURL("")).host], resourceTypes: ["xmlhttprequest", "media", "other"] },
    }] });
    j.ruleId = id;
  } catch (e) { /* the job still runs, with the browser's own headers */ }
}
async function bdlDropRules(j) {
  if (!j.ruleId || !chrome.declarativeNetRequest) return;
  try { await chrome.declarativeNetRequest.updateSessionRules({ removeRuleIds: [j.ruleId] }); } catch (e) {}
  j.ruleId = 0;
}

async function bdlFinish(j, state, extra) {
  await bdlDropRules(j);
  Object.assign(j, extra || {}, { state, endedAt: Date.now() });
  const hist = await bdlHistory();
  hist.unshift(bdlPublic(j));
  await api.storage.local.set({ bdlHistory: hist.slice(0, BDL_HISTORY_MAX) });
  bdlJobs.delete(j.id);
}
async function ensureOffscreen() {
  if (!chrome.offscreen) return false;
  try {
    if (chrome.offscreen.hasDocument && await chrome.offscreen.hasDocument()) return true;
    await chrome.offscreen.createDocument({
      url: "offscreen.html",
      reasons: ["BLOBS"],
      justification: "Assemble videos downloaded in the browser into one file",
    });
  } catch (e) {
    // "Only a single offscreen document" = it already exists.
    if (!/single offscreen/i.test(String(e && e.message))) throw e;
  }
  return true;
}
// Save an assembled Blob URL under the job's name (extension fixed to what
// was actually built: .mp4, .ts or .m4a).
async function bdlSaveBlob(j, blobUrl, ext, note, bytes) {
  const name = j.filename.replace(/\.(mp4|ts|m4a|webm|mkv)$/i, "") + (ext || ".mp4");
  j.phase = "Saving";
  j.size = bytes || 0;
  j.blobUrl = blobUrl;
  if (note) j.note = note;
  try {
    j.downloadId = await bdlDownload(blobUrl, name);
    j.filename = name;
  } catch (e) {
    await bdlRelease(j);
    await bdlFinish(j, "failed", { error: String((e && e.message) || e) });
  }
}
async function bdlRelease(j) {
  if (!j.blobUrl) return;
  if (chrome.offscreen) {
    try { await api.runtime.sendMessage({ target: "offscreen", type: "bdl-revoke", id: j.id }); } catch (e) {}
  } else {
    try { URL.revokeObjectURL(j.blobUrl); } catch (e) {}
  }
  j.blobUrl = "";
}
async function startBrowserDownload(msg) {
  const url = String(msg.url || "");
  const pairUrl = msg.pairUrl ? String(msg.pairUrl) : "";
  if (!/^https?:\/\//i.test(url) || url.length > 4000 || (pairUrl && !/^https?:\/\//i.test(pairUrl))) {
    return { ok: false, error: "only http(s) links can be downloaded" };
  }
  const kind = typeof DCBrowserDL !== "undefined" ? DCBrowserDL.kindFor(url, msg.mime, pairUrl) : (pairUrl ? "pair" : "file");
  const id = `b${Date.now().toString(36)}${++bdlSeq}`;
  const j = {
    id, kind, title: String(msg.title || "").slice(0, 200), filename: bdlSafeName(msg.filename, url),
    state: "running", bytes: 0, done: 0, total: 0, phase: "Starting", error: "", note: "",
    startedAt: Date.now(), page: msg.pageUrl || "", size: 0, src: url,
  };
  bdlJobs.set(id, j);
  if (kind === "file") {
    // The browser's own downloader: resumable, in its download list. Marked
    // seen so automatic capture does not hand it to the app instead.
    markSeen(url, null);
    try {
      j.downloadId = await bdlDownload(url, j.filename);
      j.phase = "Downloading";
      return { ok: true, id };
    } catch (e) {
      await bdlFinish(j, "failed", { error: String((e && e.message) || e) });
      return { ok: false, error: j.error };
    }
  }
  const job = { id, kind, url, pairUrl, quality: msg.quality || "" };
  await bdlHeaderRules(j, url);
  try {
    if (await ensureOffscreen()) {
      await api.runtime.sendMessage({ target: "offscreen", type: "bdl-start", job });
    } else if (typeof DCBrowserDL !== "undefined" && typeof URL.createObjectURL === "function") {
      const ctl = new AbortController();
      bdlCtl.set(id, ctl);
      DCBrowserDL.run(job, (p) => Object.assign(j, p), ctl.signal)
        .then((r) => bdlSaveBlob(j, URL.createObjectURL(r.blob), r.ext, r.note, r.bytes))
        .catch((e) => bdlFinish(j, ctl.signal.aborted ? "cancelled" : "failed", { error: String((e && e.message) || e) }))
        .finally(() => bdlCtl.delete(id));
    } else {
      throw new Error("this browser cannot assemble videos here");
    }
    return { ok: true, id };
  } catch (e) {
    await bdlFinish(j, "failed", { error: String((e && e.message) || e) });
    return { ok: false, error: j.error };
  }
}
// Live bytes for plain-file jobs come from the browser's download list.
async function bdlRefresh() {
  for (const j of bdlJobs.values()) {
    if (j.downloadId === undefined || j.kind !== "file") continue;
    try {
      const [d] = await api.downloads.search({ id: j.downloadId });
      if (d) { j.bytes = d.bytesReceived || 0; j.size = d.totalBytes > 0 ? d.totalBytes : j.size; }
    } catch (e) { /* ignore */ }
  }
}
try {
  chrome.downloads.onChanged.addListener(async (delta) => {
    const j = [...bdlJobs.values()].find((x) => x.downloadId === delta.id);
    if (!j || !delta.state) return;
    if (delta.state.current === "complete") {
      let size = j.size;
      let path = "";
      try {
        const [d] = await api.downloads.search({ id: delta.id });
        if (d) { size = d.fileSize || d.totalBytes || size; path = d.filename || ""; }
      } catch (e) {}
      await bdlRelease(j);
      await bdlFinish(j, "done", { size, phase: "Done", path });
      // List it in the app too (finished, with Open file / folder).
      if (path) {
        try { await hostSend({ type: "record", url: j.src, filename: path, pageUrl: j.page || null }); } catch (e) { /* app not installed */ }
      }
    } else if (delta.state.current === "interrupted") {
      await bdlRelease(j);
      const why = (delta.error && delta.error.current) || "interrupted";
      await bdlFinish(j, why === "USER_CANCELED" ? "cancelled" : "failed", { error: why });
    }
  });
} catch (e) { /* downloads API unavailable */ }

// ---- popup / content-script messages ----
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg && msg.type === "layoutReport" && sender && sender.id === chrome.runtime.id && typeof msg.key === "string") {
    layoutReport(msg.key.slice(0, 80), msg.text || msg.key);
    sendResponse({ ok: true });
    return false;
  }
  // The offscreen runner reports here (fire-and-forget, no reply needed).
  if (msg && msg.from === "offscreen" && sender && sender.id === chrome.runtime.id) {
    const j = bdlJobs.get(msg.id);
    if (j) {
      if (msg.type === "bdl-progress") Object.assign(j, msg.progress || {});
      else if (msg.type === "bdl-ready") bdlSaveBlob(j, msg.blobUrl, msg.ext, msg.note, msg.bytes);
      else if (msg.type === "bdl-failed") bdlFinish(j, msg.error === "cancelled" ? "cancelled" : "failed", { error: msg.error });
    }
    return false;
  }
  if (msg && msg.target === "offscreen") return false;
  (async () => {
    if (!msg || typeof msg !== "object") return { ok: false };
    if (msg.type === "getVideoBarPosition") {
      const data=await api.storage.session.get("videoBarPosition");
      return {ok:true,position:data.videoBarPosition || null};
    }
    if (msg.type === "setVideoBarPosition") {
      const p=msg.position;
      if(!p || !Number.isFinite(p.x) || !Number.isFinite(p.y) || p.x<0 || p.x>1 || p.y<0 || p.y>1)return {ok:false};
      await api.storage.session.set({videoBarPosition:{x:p.x,y:p.y}});
      return {ok:true};
    }
    if (msg.type === "download" && msg.url) {
      if (!/^https?:\/\//i.test(msg.url) || msg.url.length > 4000) {
        return { ok: false, error: "only http(s) links up to 4000 chars can be sent" };
      }
      if (msg.ver && msg.ver !== EXT_VER) {
        // Stale tab script (extension reloaded without a tab refresh):
        // push the current script into the tab so the *next* click works.
        try {
          if (sender && sender.tab && sender.tab.id !== undefined &&
              chrome.scripting && chrome.scripting.executeScript) {
            await api.scripting.executeScript({
              target: { tabId: sender.tab.id },
              files: ["rank.js", "content.js"],
            });
            return {
              ok: false,
              error: "page script refreshed itself — click once more",
            };
          }
        } catch (e) {
          /* fall through to the reload hint */
        }
        return {
          ok: false,
          error: `page runs script v${msg.ver}, app expects v${EXT_VER} — reload this tab (Ctrl+R), then click again`,
        };
      }
      markSeen(msg.url, msg.filename);
      try {
        const pageUrl = msg.pageUrl || (sender && sender.tab && sender.tab.url) || null;
        const ok = await sendToHost(msg.url, msg.filename, {
          pageUrl,
          referer: msg.referer || pageUrl,
          mime: msg.mime || null,
          size: msg.size || null,
          pairUrl: msg.pairUrl || null,
          pairMime: msg.pairMime || null,
          quality: msg.quality || null,
          dur: msg.dur || null,
          audioExpected: !!msg.audioExpected,
        });
        hostOk = ok;
        setBadge();
        if (ok) return { ok: true };
        return {
          ok: false,
          error: lastHostError ||
            "native host unreachable — reinstall it from Download Craft → Settings → Browser integration, then restart Chrome",
        };
      } catch (e) {
        return { ok: false, error: (e && e.message) || String(e) };
      }
    }
    // Presence pre-check: which urls are already queued (no side effects).
    // Lets menus mark "✓ Queued" rows instead of sending dupes that bounce
    // back as "already in queue" errors.
    if (msg.type === "isQueued" && Array.isArray(msg.urls)) {
      try {
        const urls = msg.urls
          .filter((u) => typeof u === "string" && /^https?:\/\//i.test(u))
          .slice(0, 25);
        if (!urls.length) return { queued: [] };
        const res = await hostSend({ type: "query", urls });
        if (res && res.type === "queued" && Array.isArray(res.urls)) {
          return { queued: res.urls };
        }
        return { queued: [] };
      } catch (e) {
        return { queued: [] };
      }
    }
    // Media URLs read from the page's own API answers and player state
    // (page-hook.js via page-relay.js). Page-supplied, so validated hard.
    if (msg.type === "pageMedia" && Array.isArray(msg.items)) {
      const tabId = sender && sender.tab && sender.tab.id;
      if (tabId === undefined || tabId < 0 || typeof DCRank === "undefined") return { ok: false };
      const cfg = await settings();
      if (cfg.deepDetect === false) return { ok: false };
      // Any page script can post these, so they are held to what the page
      // reader legitimately finds: media-shaped URLs on public hosts (a
      // page on a local host may name local media). Other URLs were a way
      // to make the extension and app send credentialed requests anywhere.
      const pageHost = (() => { try { return new URL(sender.url).hostname; } catch (e) { return ""; } })();
      const http = (u) => typeof u === "string" && u.length <= 4000 && /^https?:\/\//i.test(u)
        && /\.(?:m3u8|mpd|mp4|webm|m4s)(?:[?#]|$)/i.test(u.split(/[?#]/)[0] + "?")
        && (!privateHost(u) || privateHost(sender.url || "") || (() => { try { return new URL(u).hostname === pageHost; } catch (e) { return false; } })());
      for (const it of msg.items.slice(0, 30)) {
        if (!it || !http(it.url) || hostBlocked(it.url, cfg.blockHosts)) continue;
        const mime = typeof it.mime === "string" && it.mime.length < 100 ? it.mime : "";
        const dur = Number(it.dur);
        noteMedia(tabId, {
          master: false, children: [],
          url: it.url,
          mime,
          size: 0,
          pageUrl: (sender && sender.url) || null,
          frameId: sender && sender.frameId,
          pairUrl: http(it.pairUrl) ? it.pairUrl : undefined,
          // A player's full video track + its audio pair is a whole file
          // even when the CDN names it like a fragment (Bilibili .m4s).
          whole: http(it.pairUrl) || undefined,
          pairMime: http(it.pairUrl) && typeof it.pairMime === "string" ? it.pairMime.slice(0, 100) : undefined,
          dur: dur > 0 && dur < 86400 ? Math.round(dur) : undefined,
          via: "page",
        });
      }
      return { ok: true };
    }
    if (msg.type === "getMedia") {      // Popup passes its tab; content scripts arrive with sender.tab.
      const tabId = (msg.tabId !== undefined)
        ? msg.tabId
        : (sender && sender.tab && sender.tab.id);
      const list = (tabId !== undefined && tabMedia.get(tabId)) || [];
      const page = msg.pageUrl || null;
      // SPA pages report a fresh pageUrl while the sniffer's documentUrl may
      // lag one navigation behind — prefer exact matches but fall back to
      // the tab's recent items instead of answering empty (empty = the pill
      // silently sends the unresolvable page URL and the user must reload).
      const items = list.slice(-25).filter((m) => !page || !m.pageUrl || m.pageUrl === page);
      const full = items.length ? items : list.slice(-25);
      // P4 cross-tab hint: how many tabs have seen the same file.
      const annotated = full.map((m) => {
        let tabs = 1;
        try {
          const hit = seenTabs.get(canonKey(m.url));
          if (hit) tabs = Math.max(1, hit.size);
        } catch (e) { /* ignore */ }
        return { ...m, tabs };
      });
      return { items: annotated };
    }
    if (msg.type === "getSizes" && Array.isArray(msg.urls)) {
      // IDM-style file sizes for the quality menu. HEAD first, then a
      // bytes=0-0 Range fallback (media servers often hide length on HEAD).
      // credentials:include sends the browser's own cookies (googlevideo
      // answers 403 to anonymous HEADs but serves logged-in sessions), with
      // the page as referrer. (fetch still cannot set Cookie manually —
      // forbidden header — so off-session hosts may stay unknown; sizes
      // stay best-effort.)
      const seenUrls = new Set();
      // Never a credentialed request to a URL only a page script reported,
      // or to a private-network host from a public page.
      const fromPrivate = !!(sender && sender.url && privateHost(sender.url));
      const urls = msg.urls
        .filter((u) => typeof u === "string" && /^https?:\/\//i.test(u) && !seenUrls.has(u) && (seenUrls.add(u), true))
        .filter((u) => !pageReportedOnly(u) && (!privateHost(u) || fromPrivate))
        .slice(0, 12);
      const referrer = (typeof msg.referer === "string" && /^https?:\/\//i.test(msg.referer))
        ? msg.referer
        : undefined;
      const out = {};
      await Promise.all(urls.map(async (u) => {
        try {
          if (sizeCache.has(u)) { out[u] = sizeCache.get(u); return; }
          const ctl = new AbortController();
          const timer = setTimeout(() => ctl.abort(), 8000);
          let len = 0;
          try {
            const r = await fetch(u, { method: "HEAD", signal: ctl.signal, credentials: "include", redirect: "follow", ...(referrer ? { referrer, referrerPolicy: "unsafe-url" } : {}) });
            len = parseInt(r.headers.get("content-length") || "0", 10) || 0;
          } finally {
            clearTimeout(timer);
          }
          if (!len) {
            const ctl2 = new AbortController();
            const t2 = setTimeout(() => ctl2.abort(), 8000);
            try {
              const r2 = await fetch(u, { method: "GET", headers: { Range: "bytes=0-0" }, signal: ctl2.signal, credentials: "include", redirect: "follow", ...(referrer ? { referrer, referrerPolicy: "unsafe-url" } : {}) });
              const cr = r2.headers.get("content-range") || "";
              const m = cr.match(/\/(\d+)\s*$/);
              len = m ? parseInt(m[1], 10) || 0 : (parseInt(r2.headers.get("content-length") || "0", 10) || 0);
            } finally {
              clearTimeout(t2);
            }
          }
          out[u] = len;
          sizeCache.set(u, len);
          // Drop the oldest half instead of wiping the cache outright.
          if (sizeCache.size > 200) {
            let n = 0;
            for (const k of sizeCache.keys()) {
              sizeCache.delete(k);
              if (++n >= 100) break;
            }
          }
        } catch (e) {
          out[u] = 0;
        }
      }));
      return { sizes: out };
    }
    if (msg.type === "browserDownload") return startBrowserDownload(msg);
    if (msg.type === "bdlList") {
      await bdlRefresh();
      return { jobs: [...bdlJobs.values()].map(bdlPublic), history: await bdlHistory() };
    }
    if (msg.type === "bdlCancel" && typeof msg.id === "string") {
      const j = bdlJobs.get(msg.id);
      if (!j) return { ok: false };
      if (j.downloadId !== undefined) {
        try { await api.downloads.cancel(j.downloadId); } catch (e) {}
      } else if (bdlCtl.has(j.id)) {
        bdlCtl.get(j.id).abort();
      } else {
        try { await api.runtime.sendMessage({ target: "offscreen", type: "bdl-cancel", id: j.id }); } catch (e) {}
      }
      return { ok: true };
    }
    if (msg.type === "bdlShow" && Number.isInteger(msg.downloadId)) {
      try { api.downloads.show(msg.downloadId); return { ok: true }; } catch (e) { return { ok: false }; }
    }
    if (msg.type === "bdlOpen" && Number.isInteger(msg.downloadId)) {
      try { await api.downloads.open(msg.downloadId); return { ok: true }; } catch (e) { return { ok: false, error: String(e && e.message) }; }
    }
    if (msg.type === "bdlForget" && typeof msg.id === "string") {
      const hist = (await bdlHistory()).filter((h) => h.id !== msg.id);
      await api.storage.local.set({ bdlHistory: hist });
      return { ok: true };
    }
    if (msg.type === "appList") {
      try {
        const res = await hostSend({ type: "list" });
        hostOk = !!res;
        return { ok: true, items: (res && res.items) || [] };
      } catch (e) {
        hostOk = false;
        return { ok: false, items: [], error: String((e && e.message) || e) };
      }
    }
    if (msg.type === "appControl" && typeof msg.action === "string") {
      try {
        const res = await hostSend({ type: "control", action: msg.action, id: Number.isInteger(msg.id) ? msg.id : undefined });
        return { ok: !!res && res.type === "ok", error: res && res.error };
      } catch (e) {
        return { ok: false, error: String((e && e.message) || e) };
      }
    }
    if (msg.type === "clearMedia") {
      const tabId = msg.tabId;
      if (Number.isInteger(tabId)) { tabMedia.delete(tabId); tabPlaylist.delete(tabId); setBadge(); }
      return { ok: true };
    }
    if (msg.type === "skipLink" && typeof msg.url === "string" && /^https?:\/\//i.test(msg.url)) {
      skipOnce.set(msg.url, Date.now() + 15000);
      if (skipOnce.size > 50) skipOnce.delete(skipOnce.keys().next().value);
      return { ok: true };
    }
    if (msg.type === "pageFetchChunk" && typeof msg.id === "string" && sender && sender.tab) {
      return typeof DCBridge !== "undefined" ? DCBridge.relayChunk(msg) : { ok: false };
    }
    if (msg.type === "captureLink") {
      const cfg = await settings();
      if (!cfg.autoCapture || !/^https?:\/\//i.test(msg.url || "") || hostBlocked(msg.url,cfg.blockHosts)
          || !typeAllowed(cfg, msg.filename, msg.url)) return {ok:false};
      const ok = await sendToHost(msg.url,msg.filename,{pageUrl:msg.pageUrl,referer:msg.pageUrl});
      // A failed direct handoff must replay in Brave without recapture.
      markSeen(msg.url,null);
      return {ok};
    }
    if (msg.type === "getState") {
      // Fresh status on every popup open (the worker only pings at
      // startup, which can predate the app's first run).
      await sendToHostPing();
      const cfg = await settings();
      const downloadUI = await syncBrowserDownloadUI();
      return { hideBrowserUI:!!cfg.hideBrowserUI, downloadUI, autoCapture: !!cfg.autoCapture, detection: detectionOn, deepDetect: cfg.deepDetect !== false, minSizeKB: cfg.minSizeKB, blockHosts: cfg.blockHosts, hostOk, hostError: lastHostError || null, fallbackError: lastFallbackError || null,
        live: typeof DCBridge !== "undefined" && DCBridge.live(), captureTypes: cfg.captureTypes || "" };
    }
    if (msg.type === "setAuto" && typeof msg.value === "boolean") {
      await api.storage.local.set({ autoCapture: msg.value });
      await mirrorToApp({ autoCapture: msg.value });
      await syncBrowserDownloadUI();
      return { ok: true };
    }
    if (msg.type === "setDetection" && typeof msg.value === "boolean") {
      detectionOn = msg.value;
      await api.storage.local.set({ detection: msg.value });
      if (!msg.value) { tabMedia.clear(); tabPlaylist.clear(); setBadge(); }
      return { ok: true };
    }
    if (msg.type === "setDeepDetect" && typeof msg.value === "boolean") {
      await api.storage.local.set({ deepDetect: msg.value });
      return { ok: true };
    }
    if (msg.type === "setBrowserUI" && typeof msg.value === "boolean") {
      await api.storage.local.set({hideBrowserUI:msg.value});
      await sendToHostPing();
      const state = await syncBrowserDownloadUI();
      return {ok:state.supported && !state.error, ...state};
    }
    if (msg.type === "setFilters") {
      const patch = {};
      let ok = false;
      if (msg.minSizeKB !== undefined) {
        const n = parseInt(msg.minSizeKB, 10);
        if (!Number.isFinite(n) || n < 0 || n > 100000) {
          return { ok: false, error: "min size must be 0–100000 KB" };
        }
        patch.minSizeKB = n;
        ok = true;
      }
      if (typeof msg.blockHosts === "string" && msg.blockHosts.length < 4000) {
        patch.blockHosts = msg.blockHosts;
        ok = true;
      }
      if (!ok) return { ok: false, error: "nothing valid to save" };
      await api.storage.local.set(patch);
      await mirrorToApp(patch);
      return { ok: true };
    }
    return { ok: false };
  })().then(sendResponse,()=>sendResponse({ok:false}));
  return true; // async response
});

// ---- live connection: what the app pushes ----------------------------------
async function mirrorToApp(prefs) {
  try { await hostSend({ type: "setPrefs", prefs }); } catch (e) { /* app not installed: popup settings stay local */ }
}

function notifyUser(id, title, message) {
  try {
    if (!api.notifications || !api.notifications.create) return;
    api.notifications.create(String(id), { type: "basic", iconUrl: "icons/icon.png", title, message: String(message || "").slice(0, 300) });
  } catch (e) { /* notifications not allowed */ }
}

// A media URL the extension can download by itself (not a web page).
function browserCanTake(c) {
  if (!c || !/^https?:\/\//i.test(c.url)) return false;
  if (c.pairUrl || c.mime) return true;
  return typeof DCRank !== "undefined" && (DCRank.isPlaylistUrl(c.url) || /\.(mp4|webm|mkv|mov|m4v|mp3|m4a|aac|ogg|opus|flac|wav)(\?|#|$)/i.test(c.url.split("?")[0]) || /googlevideo\.com\//i.test(c.url));
}

if (typeof DCBridge !== "undefined") {
  DCBridge.on("state", (m) => {
    hostOk = hostOk || !!m.live;
    if (m.live) { lastHostError = ""; setBadge(); }
  });
  DCBridge.on("settings", async (m) => {
    const prefs = (m.settings && typeof m.settings === "object") ? m.settings : {};
    const rules = (m.rules && typeof m.rules === "object") ? m.rules : null;
    await api.storage.local.set({ appPrefs: prefs, siteRules: rules });
    await syncBrowserDownloadUI();
  });
  DCBridge.on("event", async (m) => {
    // Only downloads this browser sent (another browser may share the app).
    if (!sentCaptures.has(m.url)) return;
    const cfg = await settings();
    const name = m.name || m.url || "download";
    if (m.event === "done") {
      if (cfg.notify) notifyUser(`dc-${m.id}`, "Download complete", name);
      return;
    }
    if (m.event !== "failed") return;
    const c = sentCaptures.get(m.url);
    if (cfg.browserFallback && browserCanTake(c) && !String(m.error || "").includes("cancelled")) {
      const r = await startBrowserDownload({ url: c.url, pairUrl: c.pairUrl, mime: c.mime, quality: c.quality,
        filename: c.filename || name, title: c.title, pageUrl: c.pageUrl });
      if (cfg.notify) notifyUser(`dc-${m.id}`, r && r.ok ? "Downloading in the browser instead" : "Download failed",
        r && r.ok ? `The app could not download ${name}: the browser is downloading it now.` : `${name}: ${m.error || "failed"}`);
      return;
    }
    if (cfg.notify) notifyUser(`dc-${m.id}`, "Download failed", `${name}: ${m.error || "failed"}`);
  });
}

// First run, and whenever the app cannot be reached: one page that says what
// is wrong and how to fix it (IDM's welcome / error page).
async function showWelcome(reason) {
  try {
    const { welcomeShownAt = 0 } = await api.storage.local.get({ welcomeShownAt: 0 });
    if (reason !== "install" && Date.now() - welcomeShownAt < 24 * 3600 * 1000) return;
    await api.storage.local.set({ welcomeShownAt: Date.now() });
    await api.tabs.create({ url: api.runtime.getURL(`welcome.html?reason=${encodeURIComponent(reason)}`) });
  } catch (e) { /* no tabs here */ }
}
