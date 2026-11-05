// Static popup text in the browser's language (_locales); English stays
// when a language has no entry.
try {
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const m = chrome.i18n && chrome.i18n.getMessage(el.getAttribute("data-i18n"));
    if (!m) return;
    // Replace only the text: elements inside (the Videos tab's counter)
    // must stay, the list code fills them.
    const text = [...el.childNodes].find((n) => n.nodeType === 3 && n.nodeValue.trim());
    if (text) text.nodeValue = m;
    else if (!el.children.length) el.textContent = m;
  });
} catch (e) { /* keep English */ }

const $ = (id) => document.getElementById(id);

// Promise-style calls (`await api.*` below) work in both engines: Firefox
// exposes the promise-returning `browser` namespace, Chrome only `chrome`
// (promise-capable under MV3). Callback-style calls keep using `chrome`.
const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;
const R = (typeof DCRank !== "undefined") ? DCRank : null;

// ---------- theme: the app's Light / Dark / System choice ----------
function applyTheme(appTheme) {
  let mode = appTheme;
  if (mode !== "light" && mode !== "dark") {
    mode = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  const T = R ? R.panelTokens(mode) : null;
  if (!T) return;
  const css = document.documentElement.style;
  css.setProperty("--bg", T.bg);
  css.setProperty("--card", T.card);
  css.setProperty("--hover", T.hover);
  css.setProperty("--line", T.border);
  css.setProperty("--text", T.text);
  css.setProperty("--dim", T.dim);
  css.setProperty("--muted", T.muted);
  css.setProperty("--accent", T.accent);
  css.setProperty("--accent-soft", T.accentLight);
  css.setProperty("--thumb", T.thumbBg);
  css.setProperty("--ok", mode === "light" ? "#15803d" : "#22c55e");
  css.setProperty("--bad", mode === "light" ? "#b91c1c" : "#ef4444");
  css.setProperty("--warn", mode === "light" ? "#b45309" : "#f59e0b");
  document.documentElement.style.colorScheme = mode;
}
chrome.storage.local.get({ appTheme: "system" }, (r) => applyTheme(r && r.appTheme));
chrome.storage.onChanged.addListener((ch, area) => {
  if (area === "local" && ch.appTheme) applyTheme(ch.appTheme.newValue);
});

function status(text, kind) {
  const el = $("status");
  el.textContent = text || "";
  el.className = kind || "";
}

// Per-viewer conveniences (which tab, which sections are folded).
const prefs = (() => {
  try { return JSON.parse(localStorage.getItem("dcPopup") || "{}"); } catch (e) { return {}; }
})();
const savePrefs = () => { try { localStorage.setItem("dcPopup", JSON.stringify(prefs)); } catch (e) { /* private window */ } };

let hostOk = false;
// Re-renders the Videos tab with what was last found (filters, state changes).
let lastRender = null;
let filterKind = prefs.filter || "all";
// 🗑 Clear: cards hidden for this tab and page until ⟳ Rescan (or the page
// changes). Kept in session storage, so reopening the popup keeps them gone.
let hidden = { key: "", keys: new Set() };
let shownKeys = [];
async function loadHidden(tab) {
  hidden = { key: tab && tab.id !== undefined ? `hidden:${tab.id}` : "", keys: new Set(), url: (tab && tab.url) || "" };
  if (!hidden.key || !chrome.storage.session) return;
  try {
    const got = (await chrome.storage.session.get(hidden.key))[hidden.key];
    if (got && got.url === hidden.url) hidden.keys = new Set(got.keys || []);
  } catch (e) { /* session storage unavailable */ }
}
async function saveHidden() {
  if (!hidden.key || !chrome.storage.session) return;
  try {
    if (hidden.keys.size) await chrome.storage.session.set({ [hidden.key]: { url: hidden.url, keys: [...hidden.keys] } });
    else await chrome.storage.session.remove(hidden.key);
  } catch (e) { /* ignore */ }
}

// ---------- tabs ----------
let pollTimer = 0;
function showTab(which) {
  for (const [t, v] of [["Videos", "viewVideos"], ["Downloads", "viewDownloads"], ["Settings", "viewSettings"]]) {
    $("tab" + t).classList.toggle("on", which === t.toLowerCase());
    $(v).hidden = which !== t.toLowerCase();
  }
  clearInterval(pollTimer);
  if (which === "downloads") {
    renderDownloads();
    pollTimer = setInterval(renderDownloads, 1000);
  }
  prefs.tab = which;
  savePrefs();
}
$("tabVideos").onclick = () => showTab("videos");
$("tabDownloads").onclick = () => showTab("downloads");
$("tabSettings").onclick = () => showTab("settings");

// ---------- settings + toolbar state ----------
function setToolState(auto, detect) {
  $("autoBtn").classList.toggle("on", !!auto);
  $("autoBtn").title = auto ? "Browser downloads go to the app automatically (on)" : "Send the browser's downloads to the app automatically (off)";
  $("detectBtn").classList.toggle("off", detect === false);
  $("detectBtn").title = detect === false ? "Video detection is off — click to turn it on" : "Video detection is on — click to turn it off";
}
chrome.runtime.sendMessage({ type: "getState" }, (res) => {
  if (chrome.runtime.lastError || !res) {
    $("conn").textContent = "Extension error";
    $("conn").className = "conn bad";
    status("Extension background unreachable — reload the extension.", "bad");
    return;
  }
  hostOk = !!res.hostOk;
  $("conn").textContent = res.hostOk ? "● App connected" : "● Browser only";
  $("conn").className = res.hostOk ? "conn ok" : "conn bad";
  $("conn").title = res.hostOk ? "Download Craft is ready" : (res.hostError || "Download Craft is not running — videos can still be downloaded in the browser.");
  $("auto").checked = !!res.autoCapture;
  $("detect").checked = res.detection !== false;
  $("deep").checked = res.deepDetect !== false;
  $("hideUI").checked = !!res.hideBrowserUI;
  $("hideUI").disabled = !(res.downloadUI && res.downloadUI.supported);
  if (res.minSizeKB !== undefined) $("minSize").value = String(res.minSizeKB);
  if (typeof res.blockHosts === "string") $("block").value = res.blockHosts;
  setToolState(res.autoCapture, res.detection);
  if (res.fallbackError) status("Browser re-download failed: " + res.fallbackError, "bad");
  if (lastRender) lastRender();
});

const setAuto = (v) => { $("auto").checked = v; setToolState(v, $("detect").checked); chrome.runtime.sendMessage({ type: "setAuto", value: v }); };
const setDetect = (v) => { $("detect").checked = v; setToolState($("auto").checked, v); chrome.runtime.sendMessage({ type: "setDetection", value: v }); };
$("auto").onchange = () => setAuto($("auto").checked);
$("detect").onchange = () => setDetect($("detect").checked);
$("autoBtn").onclick = () => setAuto(!$("auto").checked);
$("detectBtn").onclick = () => setDetect(!$("detect").checked);
$("deep").onchange = () => chrome.runtime.sendMessage({ type: "setDeepDetect", value: $("deep").checked });
// One click in the last-picked quality (the page's video buttons read it).
try {
  chrome.storage.local.get({ oneClickQuality: true }, (r) => { if (!chrome.runtime.lastError) $("oneClick").checked = !r || r.oneClickQuality !== false; });
} catch (e) { /* stays unchecked */ }
$("oneClick").onchange = () => { try { chrome.storage.local.set({ oneClickQuality: $("oneClick").checked }); } catch (e) {} };
$("hideUI").onchange = () => {
  chrome.runtime.sendMessage({ type: "setBrowserUI", value: $("hideUI").checked }, (res) => {
    status(res && res.ok ? "Browser download UI preference saved" : "This browser could not change its download UI.", res && res.ok ? "ok" : "bad");
  });
};
$("saveFilters").onclick = () => {
  chrome.runtime.sendMessage({ type: "setFilters", minSizeKB: $("minSize").value.trim(), blockHosts: $("block").value.trim() }, (res) => {
    if (chrome.runtime.lastError || !res) status("Extension background unreachable.", "bad");
    else status(res.ok ? "Filters saved ✓" : ("Could not save filters. " + (res.error || "")), res.ok ? "ok" : "bad");
  });
};
$("restoreVideoButtons").onclick = async () => {
  try {
    const [tab] = await api.tabs.query({ active: true, currentWindow: true });
    if (!tab || !tab.id) throw new Error("No active page");
    // Omit frameId so embedded Vimeo and other players receive it too.
    await api.tabs.sendMessage(tab.id, { type: "restoreVideoButtons" });
    status("Video download buttons restored.", "ok");
  } catch (e) { status("Reload this page, then try again.", "bad"); }
};
$("pauseBtn").onclick = () => {
  chrome.runtime.sendMessage({ type: "appControl", action: "pause_all" }, (res) => {
    status(res && res.ok ? "Paused every download in the app" : "The app is not running.", res && res.ok ? "ok" : "bad");
  });
};

// ---------- sending ----------
function send(msg) {
  chrome.runtime.sendMessage(msg, (res) => {
    if (chrome.runtime.lastError || !res) status("App not reachable — open Download Craft, or use Download in browser.", "bad");
    else status(res.ok ? "Sent to Download Craft ✓" : ("Not sent. " + (res.error || "")), res.ok ? "ok" : "bad");
  });
}
function sendBrowser(job) {
  chrome.runtime.sendMessage({ type: "browserDownload", ...job }, (res) => {
    if (chrome.runtime.lastError || !res) status("Extension background unreachable.", "bad");
    else if (res.ok) { status("Downloading in the browser — see the Downloads tab", "ok"); updateDlCount(); }
    else status("Could not start: " + (res.error || "unknown error"), "bad");
  });
}
$("send").onclick = () => {
  const url = $("url").value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url) || url.length > 4000) {
    status("Only http(s) links up to 4000 chars can be sent.", "bad");
    return;
  }
  if (hostOk) send({ type: "download", url });
  else sendBrowser({ url, filename: "" });
};

// ---------- building cards ----------
function fmtSize(n) {
  if (!n || n <= 0) return "";
  const u = ["B", "KB", "MB", "GB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}
const fmtDur = (s) => (R ? R.fmtDur(s) : "");
const safeName = (s, ext = ".mp4") => {
  const base = String(s || "video").replace(/[<>:"/\\|?*]/g, "_").trim().slice(0, 120) || "video";
  return base.toLowerCase().endsWith(ext) ? base : base + ext;
};
// `v.title` comes from content.js, which already prefers authoritative
// player metadata over page chrome, so trust it here: only a missing title
// falls back to the video id.
const youtubeStem = (title, id) => {
  const t = String(title || "").replace(/\s+/g, " ").trim();
  const clean = t || (id ? `youtube_${id}` : "youtube_video");
  return safeName(clean).replace(/\.mp4$/i, "");
};
const contOf = (mime) => String(mime || "").split(";")[0].split("/")[1] || "mp4";
const tagOf = (url, mime) => {
  const m = String(mime || "").toLowerCase();
  if (/dash/.test(m) || /\.mpd(\?|$)/i.test(url)) return "DASH";
  if (/mpegurl/.test(m) || /\.m3u8(\?|$)/i.test(url)) return "HLS";
  if (m.startsWith("audio/")) return "AUDIO";
  const ext = (R ? R.extOf(url) : "") || contOf(m);
  return (ext || "file").toUpperCase().slice(0, 5);
};

const SVG = {
  down: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>',
  dots: '<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><circle cx="12" cy="5" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="12" cy="19" r="1.8"/></svg>',
  play: '<svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>',
  chev: '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linejoin="round"><path d="M3 7h6l2 2h10v10H3z"/></svg>',
  open: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>',
  pause: '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M4 7h16M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>',
};

let openMenu = null;
document.addEventListener("click", () => { if (openMenu) { openMenu.remove(); openMenu = null; } });

// item: { title, thumb, dur, tag, link, page, kind, options: [opt] }
// opt: { label, size, approx, url, pairUrl, msg (to the app), browser (job) }
// mode "app": every option, sent to the app; "browser": only options the
// browser can fetch itself (a page link only the app can resolve is left out).
function card(item, mode) {
  // In the browser section a quality the browser cannot fetch itself stays
  // listed (greyed, "needs app") so it is clear why it cannot be saved here.
  const options = item.options;
  // YouTube cannot be saved by the browser itself: its card stays in the
  // browser section, but hands the video to the app.
  const viaApp = mode === "browser" && item.tag === "YOUTUBE";
  if (mode === "browser" && !viaApp && !options.some((o) => o.browser)) return null;
  const el = document.createElement("div");
  el.className = "card";
  const th = document.createElement("div");
  th.className = "thumb";
  if (item.thumb && /^https?:\/\//i.test(item.thumb)) {
    const im = document.createElement("img");
    im.src = item.thumb; im.loading = "lazy"; im.alt = "";
    im.onerror = () => { im.remove(); };
    th.appendChild(im);
  } else {
    const ph = document.createElement("div");
    ph.className = "ph"; ph.innerHTML = SVG.play;
    th.appendChild(ph);
  }
  const d = fmtDur(item.dur);
  if (d) { const s = document.createElement("span"); s.className = "dur"; s.textContent = d; th.appendChild(s); }

  const body = document.createElement("div");
  body.className = "body";
  const title = document.createElement("div");
  title.className = "title"; title.textContent = item.title || "Video"; title.title = item.title || "";
  const meta = document.createElement("div");
  meta.className = "meta";
  const tag = document.createElement("span");
  tag.className = "tag"; tag.textContent = item.tag || "VIDEO";
  const size = document.createElement("span");
  const queued = document.createElement("span");
  queued.className = "queued";
  meta.append(tag, size, queued);

  const row = document.createElement("div");
  row.className = "row";
  const sel = document.createElement("select");
  options.forEach((o, i) => {
    const op = document.createElement("option");
    op.value = String(i);
    op.textContent = mode === "browser" && !viaApp && !o.browser ? `${o.label} — needs app` : o.label;
    if (mode === "browser" && !viaApp && !o.browser) op.disabled = true;
    sel.appendChild(op);
  });
  if (mode === "browser" && !viaApp) sel.value = String(options.findIndex((o) => o.browser));
  if (options.length < 2) sel.disabled = true;
  const showSize = () => {
    const o = options[Number(sel.value)] || {};
    // A stream (HLS/DASH) has no size until it is downloaded: say how long
    // it is instead, as IDM does.
    size.textContent = o.size > 0 ? (o.approx ? "~" : "") + fmtSize(o.size) : (d ? `⏱ ${d}` : "");
    size.title = o.size > 0 ? "" : (d ? "Length — streams have no size until downloaded" : "");
    queued.textContent = mode === "app" && o.queued ? "✓ Queued" : "";
  };
  sel.onchange = showSize;
  (item.refresh = item.refresh || []).push(showSize);
  const dl = document.createElement("button");
  const inBrowser = mode === "browser" && !viaApp;
  dl.className = inBrowser ? "dl br" : "dl";
  dl.innerHTML = SVG.down + `<span>${inBrowser ? "Save" : viaApp ? "Use app" : "Download"}</span>`;
  dl.title = inBrowser ? "Download in this browser — no app needed"
    : viaApp ? "YouTube only lets the app download it — hands this quality to Download Craft"
    : "Download with Download Craft";
  // DRM files are listed (so it is clear why) but never downloaded: their
  // bytes are encrypted and a saved copy has no picture or sound.
  const drmBlock = () => {
    const o = options[Number(sel.value)] || {};
    dl.disabled = !!o.drm;
    dl.style.opacity = o.drm ? ".45" : "";
    dl.style.cursor = o.drm ? "not-allowed" : "";
    if (o.drm) dl.title = "DRM-protected — this video can't be downloaded (a saved copy would have no picture or sound)";
    tag.textContent = o.drm ? "DRM" : (item.tag || "VIDEO");
  };
  sel.addEventListener("change", drmBlock);
  dl.onclick = () => {
    const o = options[Number(sel.value)];
    if (!o || o.drm) return;
    if (inBrowser) sendBrowser({ ...o.browser, title: item.title, pageUrl: item.page });
    else send(o.msg);
  };
  const more = document.createElement("button");
  more.className = "more"; more.innerHTML = SVG.dots; more.title = "More";
  more.onclick = (ev) => {
    ev.stopPropagation();
    if (openMenu) { openMenu.remove(); openMenu = null; return; }
    const o = options[Number(sel.value)] || {};
    const link = o.url || item.link;
    const m = document.createElement("div");
    m.className = "menu";
    const add = (label, fn) => { const b = document.createElement("button"); b.textContent = label; b.onclick = (e) => { e.stopPropagation(); fn(); m.remove(); openMenu = null; }; m.appendChild(b); };
    if (link) add("Copy link", async () => { try { await navigator.clipboard.writeText(link); status("Link copied", "ok"); } catch (e) { status("Could not copy", "bad"); } });
    if (link) add("Open link in new tab", () => api.tabs.create({ url: link }));
    if (mode === "app" && o.browser) add("Download in browser instead", () => sendBrowser({ ...o.browser, title: item.title, pageUrl: item.page }));
    if (inBrowser) add("Send to Download Craft instead", () => send(o.msg));
    if (item.page && mode === "app") add("Send page link instead", () => send({ type: "download", url: item.page, pageUrl: item.page }));
    more.appendChild(m);
    openMenu = m;
  };
  row.append(sel, dl, more);
  body.append(title, meta, row);
  if (viaApp) {
    const hint = document.createElement("div");
    hint.className = "meta";
    hint.textContent = "YouTube refuses downloads made by the browser itself (its links only work inside YouTube's player). The app downloads it — it starts on its own.";
    body.appendChild(hint);
  }
  el.append(th, body);
  showSize();
  drmBlock();
  return el;
}

// The browser job for a direct URL (plain file, pair or stream).
const browserJob = (url, filename, extra) => (url && /^https?:\/\//i.test(url) ? { url, filename, ...(extra || {}) } : null);

// A YouTube video from the page's player data: every quality in one select.
function youtubeItem(v, pageUrl) {
  const page = v.pageUrl || pageUrl;
  const stem = youtubeStem(v.title, v.videoId);
  const best = (arr) => (arr || []).slice().sort((a, b) => {
    // By bitrate (`br`): YouTube's audio labels are "MEDIUM"/"LOW".
    const bn = (x) => Number(x.br) > 0 ? Number(x.br) : (parseInt((String(x.label || "").match(/(\d+)k/) || [0, 0])[1], 10) * 1000 || 0);
    return bn(b) - bn(a);
  })[0] || null;
  const bestA = best(v.audio);
  const opts = [];
  const height = (l) => parseInt(String(l || "").match(/^(\d+)/) ? String(l).match(/^(\d+)/)[1] : "0", 10) || 0;
  const pushStream = (st, kind) => {
    const pair = kind === "v" && bestA && bestA.url && bestA.url !== st.url ? { url: bestA.url, mime: bestA.mime } : null;
    const ext = /webm/i.test(st.mime || "") ? ".webm" : (kind === "a" ? ".m4a" : ".mp4");
    const name = kind === "a" ? `${stem} [audio]${ext}` : kind === "v" || st.ciphered ? `${stem} [${st.label}]${ext}` : `${stem}${ext}`;
    const label = kind === "a"
      ? `Audio · ${contOf(st.mime)}${st.br ? ` · ${Math.round(st.br / 1000)}k` : ""}`
      : `${st.label} · ${(st.disp || contOf(st.mime)).toUpperCase()}${kind === "v" ? " + audio" : ""}`;
    const exact = st._size > 0 ? st._size + (pair && bestA._size > 0 ? bestA._size : 0) : 0;
    // No browser job: YouTube's links are refused (403) outside its own
    // player, even in the user's session - they need the player's "n"
    // transform and a proof-of-origin token, which is what yt-dlp in the app
    // provides. Checked live: the page's own 360p link answers 403.
    const browser = null;
    opts.push({
      h: kind === "a" ? -1 : height(st.label),
      label,
      size: exact || st._est || 0,
      approx: !exact && st._est > 0,
      url: st.url || null,
      browser,
      msg: st.ciphered && !st.url
        ? { type: "download", url: page, filename: name, pageUrl: page, quality: st.label, size: st._size || null, dur: st._dur || null, pairUrl: pair && pair.url, pairMime: pair && pair.mime }
        : { type: "download", url: st.url, filename: name, pageUrl: page, quality: st.label, size: st._size || null, dur: st._dur || null, pairUrl: pair && pair.url, pairMime: pair && pair.mime },
    });
  };
  (v.streams || []).forEach((st) => pushStream(st, "av"));
  (v.videoOnly || []).forEach((st) => pushStream(st, "v"));
  // The app resolves every standard quality itself (sizes from page JSON).
  const have = new Set(opts.map((o) => o.h));
  const sizes = v.sizes || {};
  const dur = Math.max(0, ...Object.values(sizes).map((m) => (m && m.dur) || 0));
  for (const q of [2160, 1440, 1080, 720, 480, 360, 240, 144]) {
    if (have.has(q)) continue;
    const key = Object.keys(sizes).find((k) => k.startsWith(`${q}p`));
    if (!key && q > 1080) continue;
    const sz = key ? sizes[key].size || 0 : 0;
    opts.push({ h: q, label: `${q}p · MP4`, size: sz, url: null, browser: null,
      msg: { type: "download", url: page, filename: `${stem} [${q}p].mp4`, pageUrl: page, quality: `${q}p`, size: sz || null, dur: dur || null } });
  }
  (v.audio || []).forEach((st) => pushStream(st, "a"));
  opts.sort((a, b) => b.h - a.h);
  const firstDur = (v.streams || []).concat(v.videoOnly || []).map((s) => s._dur).find((x) => x > 0) || dur;
  return { key: `yt:${v.videoId}`, kind: "videos", title: v.title || v.videoId, thumb: v.thumb, dur: firstDur, tag: "YOUTUBE", link: page, page, options: opts };
}

// A Facebook video from its own JSON (content.js fbStoredVideos): every
// quality as one file with the audio track paired in, then the single-file
// SD/HD encodes. Sizes start as bitrate x duration ("~") and are measured.
function facebookItem(e, pageUrl) {
  const stem = safeName(e.title || `facebook_${e.id}`).replace(/\.mp4$/i, "");
  const dur = e.dur || 0;
  const a = (e.audio || [])[0] || null;
  const est = (bw) => (bw > 0 && dur > 0 ? Math.round((bw * dur) / 8) : 0);
  const opts = [];
  const seen = new Set();
  for (const v of (e.video || [])) {
    const q = Math.min(v.w || 0, v.h || 0) || v.h || 0;
    if (!q || seen.has(q)) continue;
    seen.add(q);
    const filename = `${stem} [${q}p].mp4`;
    opts.push({
      label: `${q}p${a ? " (+audio)" : ""}`,
      size: est(v.bw + (a ? a.bw : 0)), approx: true, url: v.url, pairUrl: a && a.url,
      browser: browserJob(v.url, filename, a ? { pairUrl: a.url, quality: `${q}p` } : { quality: `${q}p` }),
      msg: { type: "download", url: v.url, filename, pageUrl, quality: `${q}p`,
        dur: dur || null, pairUrl: a ? a.url : undefined, pairMime: a ? (a.mime || "audio/mp4") : undefined },
    });
  }
  for (const p of (e.progressive || [])) {
    opts.push({
      label: `${p.quality === "HD" ? "HD" : "SD"} · MP4 (single file)`,
      size: est(p.bw), approx: true, url: p.url,
      browser: browserJob(p.url, `${stem}.mp4`),
      msg: { type: "download", url: p.url, filename: `${stem}.mp4`, pageUrl, dur: dur || null },
    });
  }
  if (a) {
    opts.push({
      label: "Audio · M4A", size: est(a.bw), approx: true, url: a.url,
      browser: browserJob(a.url, `${stem} [audio].m4a`),
      msg: { type: "download", url: a.url, filename: `${stem} [audio].m4a`, pageUrl, dur: dur || null },
    });
  }
  return { key: `fb:${e.id}`, kind: "videos", title: e.title || "Facebook video", thumb: e.thumb, dur, tag: "FACEBOOK", link: pageUrl, page: pageUrl, options: opts };
}

// Everything the page offers, as cards: [{kind: "videos"|"streams"|"other", ...}].
function collectItems(peek, pageUrl) {
  const items = [];
  for (const v of (peek.videos || [])) items.push(youtubeItem(v, pageUrl));
  for (const e of (peek.fb || [])) {
    const it = facebookItem(e, pageUrl);
    if (it.options.length) items.push(it);
  }
  const net = peek.net || [];
  const have = new Set((peek.links || []).map((l) => l.url));
  const hasPlaylist = net.some((n) => n && R && R.kindOf(n) === "playlist");
  const seen = new Set();
  const pageTitle = peek.title || "";
  const netItems = net
    .filter((n) => n && /^https?:\/\//i.test(n.url || "") && !have.has(n.url))
    .filter((n) => { const k = R ? R.canonicalUrl(n.url) : n.url; if (seen.has(k)) return false; seen.add(k); return true; })
    .map((n) => ({ ...n, _score: R ? R.scoreMedia(n, { hasPlaylist }) : 0 }))
    .filter((n) => n._score >= 0)
    .sort((a, b) => b._score - a._score);
  // Video files of one page (a player's 1080p, 720p, 480p…) are one card
  // with a quality list, labelled by the resolution read from each file.
  const qualityCards = new Map();
  for (const n of netItems) {
    const kind = R ? R.kindOf(n) : "file";
    const isStream = kind === "playlist";
    const isSub = kind === "subtitle";
    const tag = isSub ? "SUBS" : kind === "segment" ? "PART" : tagOf(n.url, n.mime);
    const name = isStream || isSub ? undefined : safeName(pageTitle || "video", /^audio\//i.test(n.mime || "") ? ".m4a" : ".mp4");
    const streamName = safeName(pageTitle || "video", ".mp4");
    if (kind === "file" && tag !== "AUDIO" && !/^audio\//i.test(n.mime || "")) {
      const key = `${pageUrl}|${pageTitle}`;
      let card = qualityCards.get(key);
      if (!card) {
        card = { key: `net:${n.url}`, kind: "videos", title: pageTitle || "Video", thumb: n.thumb, dur: n.dur, tag, link: n.url, page: pageUrl, options: [] };
        qualityCards.set(key, card);
        items.push(card);
      }
      if (!card.thumb && n.thumb) card.thumb = n.thumb;
      if (!card.dur && n.dur) card.dur = n.dur;
      if (n.drm) card.drm = true;
      const res = n.h > 0 ? `${n.h}p` : "";
      card.options.push({
        h: n.h || 0,
        label: [res, tag].filter(Boolean).join(" · ") + (n.pairUrl ? " + audio" : "") + (n.drm ? " · DRM" : ""),
        size: n.size || 0, url: n.url, pairUrl: n.pairUrl || undefined, drm: !!n.drm,
        browser: browserJob(n.url, res ? name.replace(/\.mp4$/i, ` [${res}].mp4`) : name, { mime: n.mime || "", pairUrl: n.pairUrl || undefined }),
        msg: { type: "download", url: n.url, filename: res ? name.replace(/\.mp4$/i, ` [${res}].mp4`) : name, pageUrl: pageUrl || undefined, pairUrl: n.pairUrl || undefined, pairMime: n.pairMime || undefined, dur: n.dur || undefined },
      });
      card.options.sort((a, b) => (b.h - a.h) || (b.size - a.size));
      continue;
    }
    items.push({
      key: `net:${n.url}`,
      kind: isStream ? "streams" : isSub || kind === "segment" ? "other" : "videos",
      title: pageTitle || "Video",
      thumb: n.thumb, dur: n.dur, tag, link: n.url, page: pageUrl,
      options: [{
        label: isStream ? (n.master ? "Best quality" : "Stream") : isSub ? "Subtitles" : `${tag}${n.pairUrl ? " + audio" : ""}`,
        size: n.size || 0, url: n.url, pairUrl: n.pairUrl || undefined,
        browser: browserJob(n.url, isStream ? streamName : (name || ""), { mime: n.mime || "", pairUrl: n.pairUrl || undefined }),
        msg: { type: "download", url: n.url, filename: name, pageUrl: pageUrl || undefined, pairUrl: n.pairUrl || undefined, pairMime: n.pairMime || undefined, dur: n.dur || undefined },
      }],
    });
  }
  for (const l of (peek.links || [])) {
    if (!/^https?:\/\//i.test(l.url)) continue; // defense-in-depth: never offer odd schemes
    items.push({
      key: `link:${l.url}`,
      kind: "other",
      title: l.text || l.url, thumb: l.thumb, tag: tagOf(l.url, ""), link: l.url, page: pageUrl,
      options: [{ label: "File", url: l.url, browser: browserJob(l.url, ""), msg: { type: "download", url: l.url, pageUrl: pageUrl || undefined } }],
    });
  }
  return items;
}


function group(id, label, sub, count, cls, build) {
  const g = document.createElement("div");
  g.className = "group" + ((prefs.closed || {})[id] ? " closed" : "");
  const h = document.createElement("div");
  h.className = "ghead";
  h.innerHTML = `<span class="chev">${SVG.chev}</span><span class="pillsrc ${cls}"></span><b></b><span class="n"></span><span class="sub"></span>`;
  h.querySelector(".pillsrc").textContent = cls === "br" ? "BROWSER" : "APP";
  h.querySelector("b").textContent = label;
  h.querySelector(".n").textContent = String(count);
  h.querySelector(".sub").textContent = sub;
  h.onclick = () => {
    g.classList.toggle("closed");
    prefs.closed = { ...(prefs.closed || {}), [id]: g.classList.contains("closed") };
    savePrefs();
  };
  const b = document.createElement("div");
  b.className = "gbody";
  build(b);
  g.append(h, b);
  return g;
}

function render(peek, pageUrl) {
  lastRender = () => render(peek, pageUrl);
  const box = $("videos");
  box.textContent = "";
  const all = collectItems(peek, pageUrl).filter((it) => !hidden.keys.has(it.key));
  shownKeys = collectItems(peek, pageUrl).map((it) => it.key);
  const q = $("q").value.trim().toLowerCase();
  const items = all.filter((it) => (filterKind === "all" || it.kind === filterKind)
    && (!q || String(it.title || "").toLowerCase().includes(q) || String(it.tag || "").toLowerCase().includes(q)));
  $("count").textContent = all.filter((it) => it.kind !== "other").length ? String(all.filter((it) => it.kind !== "other").length) : "";
  if (!items.length) {
    const e = document.createElement("div");
    e.className = "empty";
    e.innerHTML = all.length
      ? "<b>Nothing matches</b>Clear the filter or the search to see everything."
      : hidden.keys.size
        ? "<b>List cleared</b>Press ⟳ to look for videos on this page again."
        : "<b>No videos found yet</b>Play the video on this page, then open this again.";
    box.appendChild(e);
    return;
  }
  const order = { videos: 0, streams: 1, other: 2 };
  items.sort((a, b) => order[a.kind] - order[b.kind]);
  const browserCount = items.filter((it) => it.options.some((o) => o.browser)).length;
  // 1) With the app: every quality, the app resolves and merges.
  box.appendChild(group("app", "Download with Download Craft", hostOk ? "" : "app not running", items.length, "", (b) => {
    if (!hostOk) {
      const n = document.createElement("div");
      n.className = "notice";
      n.textContent = "Download Craft is not running. Open it to download here, or use “Download in browser” below — it works without the app.";
      b.appendChild(n);
    }
    for (const it of items) { const c = card(it, "app"); if (c) b.appendChild(c); }
  }));
  // 2) In the browser: no app needed (pairs and streams are joined here).
  box.appendChild(group("browser", "Download in browser", "no app needed", browserCount, "br", (b) => {
    if (!browserCount) {
      const n = document.createElement("div");
      n.className = "empty";
      n.textContent = "These need the app (the page only names them; the app resolves them).";
      b.appendChild(n);
    }
    for (const it of items) { const c = card(it, "browser"); if (c) b.appendChild(c); }
  }));
  enrich(items, pageUrl);
}

$("q").oninput = () => { if (lastRender) lastRender(); };
$("filterBtn").onclick = (ev) => {
  ev.stopPropagation();
  if (openMenu) { openMenu.remove(); openMenu = null; return; }
  const m = document.createElement("div");
  m.className = "menu";
  m.style.left = "0"; m.style.right = "auto";
  for (const [k, label] of [["all", "Everything"], ["videos", "Videos"], ["streams", "Streams (HLS / DASH)"], ["other", "Other files"]]) {
    const b = document.createElement("button");
    b.textContent = label;
    if (k === filterKind) b.className = "sel";
    b.onclick = (e) => {
      e.stopPropagation();
      filterKind = k; prefs.filter = k; savePrefs();
      $("filterBtn").classList.toggle("on", k !== "all");
      m.remove(); openMenu = null;
      if (lastRender) lastRender();
    };
    m.appendChild(b);
  }
  $("filterBtn").appendChild(m);
  openMenu = m;
};
$("filterBtn").classList.toggle("on", filterKind !== "all");

// Sizes for rows that have none (measured in the browser session) and
// "✓ Queued" for rows already in the app.
function enrich(items, pageUrl) {
  const opts = items.flatMap((it) => it.options.map((o) => ({ it, o })));
  const refresh = (it) => (it.refresh || []).forEach((f) => f());
  const urls = [...new Set(opts.map(({ o }) => o.url).filter(Boolean))];
  if (urls.length && hostOk) {
    chrome.runtime.sendMessage({ type: "isQueued", urls: urls.slice(0, 25) }, (res) => {
      if (chrome.runtime.lastError) return;
      const hit = new Set((res && res.queued) || []);
      for (const { it, o } of opts) if (o.url && hit.has(o.url)) { o.queued = true; refresh(it); }
    });
  }
  // Rows without a size, or with only an estimate, get measured; a row that
  // carries its audio track (`pairUrl`) is both files together.
  const want = opts.filter(({ o }) => o.url && (!(o.size > 0) || o.approx) && !/\.(m3u8|mpd)(\?|$)/i.test(o.url)).slice(0, 10);
  const need = [...new Set(want.flatMap(({ o }) => [o.url, o.pairUrl].filter(Boolean)))].slice(0, 16);
  if (need.length) {
    chrome.runtime.sendMessage({ type: "getSizes", urls: need, referer: pageUrl }, (res) => {
      if (chrome.runtime.lastError) return;
      const sizes = (res && res.sizes) || {};
      for (const { it, o } of want) {
        const v = sizes[o.url] || 0;
        const a = o.pairUrl ? sizes[o.pairUrl] || 0 : 0;
        if (v > 0 && (!o.pairUrl || a > 0)) { o.size = v + a; o.approx = false; refresh(it); }
      }
    });
  }
}

// ---------- downloads tab ----------
const ago = (t) => {
  const d = new Date(t);
  return isNaN(d) ? "" : d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
};
function jobCard({ src, name, state, chipText, chipCls, frac, indeterminate, line, error, actions }) {
  const el = document.createElement("div");
  el.className = "job " + state;
  const top = document.createElement("div");
  top.className = "jt";
  const s = document.createElement("span"); s.className = "src"; s.textContent = src;
  const chip = document.createElement("span"); chip.className = "chip " + (chipCls || ""); chip.textContent = chipText;
  top.append(s, chip);
  for (const [svg, title, fn] of actions || []) {
    const b = document.createElement("button");
    b.className = "ja"; b.innerHTML = svg; b.title = title; b.onclick = fn;
    top.appendChild(b);
  }
  const n = document.createElement("div"); n.className = "jn"; n.textContent = name; n.title = name;
  el.append(top, n);
  if (frac !== null && frac !== undefined) {
    const bar = document.createElement("div");
    bar.className = "bar" + (indeterminate ? " ind" : "");
    const i = document.createElement("i"); i.style.width = Math.round(Math.max(0, Math.min(1, frac)) * 100) + "%";
    bar.appendChild(i);
    el.appendChild(bar);
  }
  const b = document.createElement("div"); b.className = "jb"; b.textContent = line; el.appendChild(b);
  if (error) { const e = document.createElement("div"); e.className = "err"; e.textContent = error; el.appendChild(e); }
  return el;
}

function updateDlCount() {
  chrome.runtime.sendMessage({ type: "bdlList" }, (res) => {
    if (chrome.runtime.lastError || !res) return;
    const n = (res.jobs || []).length;
    $("dlCount").textContent = n ? String(n) : "";
  });
}

let renderingDownloads = false;
async function renderDownloads() {
  if (renderingDownloads) return;
  renderingDownloads = true;
  try {
    const [b, app] = await Promise.all([
      new Promise((r) => chrome.runtime.sendMessage({ type: "bdlList" }, (x) => r(chrome.runtime.lastError ? null : x))),
      new Promise((r) => chrome.runtime.sendMessage({ type: "appList" }, (x) => r(chrome.runtime.lastError ? null : x))),
    ]);
    const box = $("downloads");
    const keepScroll = $("viewDownloads").scrollTop;
    box.textContent = "";
    const jobs = (b && b.jobs) || [];
    const hist = (b && b.history) || [];
    $("dlCount").textContent = jobs.length ? String(jobs.length) : "";
    // In the browser.
    const sec = (label) => { const h = document.createElement("div"); h.className = "sec"; h.textContent = label; box.appendChild(h); };
    sec(`In browser · active (${jobs.length})`);
    if (!jobs.length) { const e = document.createElement("div"); e.className = "empty"; e.textContent = "No browser downloads running."; box.appendChild(e); }
    for (const j of jobs) {
      const pieces = j.total > 0 ? `${j.done}/${j.total} pieces · ` : "";
      const frac = j.kind === "file" ? (j.size > 0 ? j.bytes / j.size : 0) : (j.total > 0 ? j.done / j.total : 0);
      box.appendChild(jobCard({
        src: j.page ? new URL(j.page).hostname : "browser", name: j.filename, state: "running",
        chipText: j.phase || "Downloading", frac, indeterminate: !(frac > 0) && j.phase !== "Saving",
        line: `${pieces}${fmtSize(j.bytes) || "0 B"}${j.size > 0 && j.kind === "file" ? " of " + fmtSize(j.size) : ""}`,
        actions: [[SVG.x, "Cancel", () => chrome.runtime.sendMessage({ type: "bdlCancel", id: j.id }, () => renderDownloads())]],
      }));
    }
    if (hist.length) sec(`In browser · history (${hist.length})`);
    for (const h of hist.slice(0, 20)) {
      const ok = h.state === "done";
      const acts = [];
      if (ok && Number.isInteger(h.downloadId)) {
        acts.push([SVG.open, "Open file", () => chrome.runtime.sendMessage({ type: "bdlOpen", downloadId: h.downloadId }, (r) => { if (!r || !r.ok) status("The file was moved or deleted.", "bad"); })]);
        acts.push([SVG.folder, "Show in folder", () => chrome.runtime.sendMessage({ type: "bdlShow", downloadId: h.downloadId })]);
      }
      acts.push([SVG.trash, "Remove from history", () => chrome.runtime.sendMessage({ type: "bdlForget", id: h.id }, () => renderDownloads())]);
      box.appendChild(jobCard({
        src: `${ago(h.endedAt || h.startedAt)} · ${h.page ? new URL(h.page).hostname : "browser"}`, name: h.filename,
        state: h.state, chipText: ok ? "Done" : h.state === "cancelled" ? "Cancelled" : "Failed", chipCls: ok ? "ok" : "bad",
        line: `${(h.kind || "file").toUpperCase()}${h.size > 0 ? " · " + fmtSize(h.size) : ""}${h.note ? " · " + h.note : ""}`,
        error: ok ? "" : h.error, actions: acts,
      }));
    }
    // In the app.
    const items = (app && app.items) || [];
    sec(`In Download Craft (${app && app.ok ? items.length : "app not running"})`);
    if (!(app && app.ok)) {
      const e = document.createElement("div"); e.className = "empty"; e.textContent = "Open Download Craft to see its downloads here.";
      box.appendChild(e);
    }
    for (const it of items) {
      const live = it.state === "downloading";
      const frac = it.total > 0 ? it.done / it.total : (live ? 0 : null);
      const acts = [];
      if (it.state === "downloading" || it.state === "queued") acts.push([SVG.pause, "Pause", () => chrome.runtime.sendMessage({ type: "appControl", action: "pause", id: it.id }, () => renderDownloads())]);
      if (it.state === "paused" || it.state === "failed") acts.push([SVG.open, "Resume", () => chrome.runtime.sendMessage({ type: "appControl", action: "resume", id: it.id }, () => renderDownloads())]);
      if (it.state !== "done") acts.push([SVG.x, "Cancel", () => chrome.runtime.sendMessage({ type: "appControl", action: "cancel", id: it.id }, () => renderDownloads())]);
      let host = "";
      try { host = new URL(it.url).hostname; } catch (e) { host = ""; }
      box.appendChild(jobCard({
        src: `#${it.id} · ${host}`, name: it.name, state: it.state === "downloading" ? "running" : it.state,
        chipText: { downloading: "Downloading", queued: "Queued", paused: "Paused", done: "Done", failed: "Failed" }[it.state] || it.state,
        chipCls: { done: "ok", failed: "bad", paused: "warn", queued: "warn" }[it.state] || "",
        frac: it.state === "done" ? null : frac, indeterminate: live && !(it.total > 0),
        line: `${fmtSize(it.done) || "0 B"}${it.total > 0 ? " of " + fmtSize(it.total) : ""}${live && it.speed > 0 ? " · " + fmtSize(it.speed) + "/s" : ""}`,
        error: it.state === "failed" ? it.error : "", actions: acts,
      }));
    }
    $("viewDownloads").scrollTop = keepScroll;
  } finally {
    renderingDownloads = false;
  }
}

// ---------- videos: ask the page ----------
async function activeTab() {
  try {
    const tabs = await api.tabs.query({ active: true, currentWindow: true });
    return (tabs && tabs[0]) || null;
  } catch (e) {
    return null;
  }
}

async function loadVideos() {
  const tab = await activeTab();
  await loadHidden(tab);
  const tabUrl = (tab && tab.url && /^https?:\/\//i.test(tab.url)) ? tab.url : null;
  if (tab && tab.id !== undefined) {
    try {
      // The top frame only: every iframe also answers (ad/account frames
      // replied first with an empty or foreign video list).
      const peek = await api.tabs.sendMessage(tab.id, { type: "peekState" }, { frameId: 0 });
      if (peek && (peek.videos || peek.links || peek.net || peek.fb)) {
        render(peek, peek.page || tabUrl);
        return;
      }
    } catch (e) {
      /* content unreachable (no script on this page): fall through */
    }
  }
  // Fallback: last stored snapshot, filtered to this tab's page when known.
  chrome.storage.local.get(
    ["downloadCraftVideos", "downloadCraftLinks", "downloadCraftNet", "downloadCraftPage"],
    ({ downloadCraftVideos, downloadCraftLinks, downloadCraftNet, downloadCraftPage }) => {
      const samePage = (p) => !tabUrl || !p || p.split("#")[0] === tabUrl.split("#")[0];
      const ok = samePage(downloadCraftPage);
      render({
        videos: (downloadCraftVideos || []).filter((v) => samePage(v.pageUrl || downloadCraftPage)),
        links: ok ? (downloadCraftLinks || []) : [],
        net: ok ? (downloadCraftNet || []) : [],
        title: tab && tab.title,
      }, downloadCraftPage || tabUrl);
    }
  );
}
// ⟳ Rescan: show everything again, freshly read from the page.
$("refreshBtn").onclick = async () => {
  status("Looking again…");
  hidden.keys.clear();
  await saveHidden();
  await loadVideos();
  status("");
};
// 🗑 Clear: drop what was found (the sniffer's list and the cards the page
// itself reports) until ⟳ Rescan.
$("clearBtn").onclick = async () => {
  const tab = await activeTab();
  for (const k of shownKeys) hidden.keys.add(k);
  await saveHidden();
  if (tab && tab.id !== undefined) chrome.runtime.sendMessage({ type: "clearMedia", tabId: tab.id }, () => loadVideos());
  else if (lastRender) lastRender();
  status("Cleared — press ⟳ to look for videos again", "ok");
};

loadVideos();
updateDlCount();
showTab(prefs.tab === "downloads" || prefs.tab === "settings" ? prefs.tab : "videos");
