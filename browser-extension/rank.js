// DCRank: pure media-candidate ranking shared by background/content/popup.
// No chrome.* APIs here so it runs under `node --test` too (see rank.test.js).
// Loaded as a classic script everywhere (manifest content_scripts order,
// popup.html script tags, background importScripts) and as a module in tests.
(function (g) {
  "use strict";

  // Playlist manifests the engine assembles itself (offer these, not parts).
  function isPlaylistUrl(u) {
    try {
      const x = new URL(String(u || ""));
      if (/\.(m3u8|mpd)$/i.test(x.pathname)) return true;
      return [...x.searchParams.values()].some(v => /^(?:m3u8|mpd|application\/(?:dash\+xml|(?:vnd\.apple\.)?mpegurl|x-mpegurl))$/i.test(v));
    } catch (e) { return false; }
  }

  // Vimeo's signed range endpoints can end in .mp4 but contain only a
  // playback fragment. Never rewrite their signed paths/query strings.
  function isVimeoFragment(u) {
    try {
      const x = new URL(String(u || ""));
      return (x.hostname === "vimeocdn.com" || x.hostname.endsWith(".vimeocdn.com"))
        && (/\/v2\/range\//i.test(x.pathname) || /^\d+-\d+$/.test(x.searchParams.get("range") || ""));
    } catch (e) { return false; }
  }

  // Manifests and adaptive MP4 legs must resolve together with audio.
  function isVimeoAdaptive(u) {
    try {
      const x = new URL(String(u || ""));
      const cdn = x.hostname === "vimeocdn.com" || x.hostname.endsWith(".vimeocdn.com");
      return cdn && (isVimeoFragment(u) || isPlaylistUrl(u) || /^vod-adaptive[.-]/i.test(x.hostname));
    } catch (e) { return false; }
  }

  function vimeoVideoPage(u) {
    try {
      const x = new URL(String(u || ""));
      if (!/^https?:$/.test(x.protocol)) return null;
      const player = x.hostname === "player.vimeo.com" && /^\/video\/\d+\/?$/.test(x.pathname);
      const watch = (x.hostname === "vimeo.com" || x.hostname === "www.vimeo.com")
        && /^\/\d+(?:\/[a-zA-Z0-9]+)?\/?$/.test(x.pathname);
      return player || watch ? x.href : null;
    } catch (e) { return null; }
  }

  // Stream parts that only make sense inside a playlist assembly.
  function isSegmentUrl(u) {
    try {
      const x = new URL(String(u || ""));
      const base = x.pathname;
      const leaf = base.split("/").pop() || "";
      const range = x.searchParams.get("range") || "";
      const byteSlice = x.searchParams.has("bytestart") || x.searchParams.has("byteend");
      const media = /\.(?:mp4|m4a|webm|mp3|aac|ts)$/i.test(base);
      return isVimeoFragment(u) || (media && (/^\d+-\d+$/.test(range) || byteSlice))
        || /\.(m4s|cmfv|cmfa|mpd\.seg)$/i.test(base)
        || /^(?:(?:seg(?:ment)?|chunk|fragment|s|filesequence|sequence)[-_]?\d+|\d+)(?:[^/]*)\.ts$/i.test(leaf);
    } catch (e) { return false; }
  }

  // Match the requesting frame, never another player's unrelated playlist.
  function samePlayer(a, b) {
    if (a.tabId !== undefined && b.tabId !== undefined && a.tabId !== b.tabId) return false;
    if (a.frameId !== undefined && b.frameId !== undefined && a.frameId !== b.frameId) return false;
    if (a.pageUrl && b.pageUrl && a.pageUrl !== b.pageUrl) return false;
    return (a.frameId !== undefined && b.frameId !== undefined)
      || !!(a.pageUrl && b.pageUrl && a.pageUrl === b.pageUrl);
  }

  function playlistReferences(text, base) {
    const refs = [];
    for (const line of String(text || "").split(/\r?\n/)) {
      const t = line.trim();
      const raw = t && !t.startsWith("#") && !t.startsWith("<") ? [t]
        : [...t.matchAll(/(?:^|[, :])URI="([^"]+)"/g)].map(m => m[1]);
      for (const r of raw) {
        try { const u = new URL(r, base); if (/^https?:$/.test(u.protocol)) refs.push(u.href); } catch (e) {}
      }
    }
    return [...new Set(refs)];
  }

  function completePlaylist(entry, items) {
    const pool = (items || []).filter(m => m && kindOf(m) === "playlist" && samePlayer(entry, m));
    const reaches = (node, target, seen = new Set()) => {
      if (seen.has(node.url)) return false;
      seen.add(node.url);
      return (node.children || []).some(u => u === target
        || pool.some(child => child.url === u && reaches(child, target, seen)));
    };
    const linked = pool.filter(m => m.url !== entry.url && reaches(m, entry.url));
    if (linked.length) return linked.sort((a,b) => Number(!!b.master)-Number(!!a.master))[0];
    const masters = pool.filter(m => m.master);
    // With ambiguous roots, retain a selected complete manifest, or let a
    // fragment fall back to its exact single-player page. Never guess.
    if (kindOf(entry) === "playlist") return entry;
    return masters.length === 1 ? masters[0] : pool.length === 1 ? pool[0] : null;
  }


  function playerEvidence(videos, url, mime, pageUrl, suppliedDuration) {
    const isVideo = String(url) === String(pageUrl) || /^blob:/i.test(String(url))
      || /^video\//i.test(String(mime || "")) || isPlaylistUrl(url)
      || /\.(?:mp4|webm|mkv|mov|m4v|avi|ts|m4s|cmfv)(?:[?#]|$)/i.test(String(url));
    const list = isVideo ? [...(videos || [])] : [];
    const matches = list.filter(v => v.currentSrc === url || v.src === url);
    const player = matches.length === 1 ? matches[0] : list.length === 1 ? list[0] : null;
    let d = Number(suppliedDuration);
    if (!(d > 0) && player) d = player.duration;
    return {
      dur: Number.isFinite(d) && d > 0 && d < 86400 ? Math.max(1, Math.round(d)) : null,
      audioExpected: !!(player && (player.mozHasAudio === true || Number(player.webkitAudioDecodedByteCount) > 0
        || (player.audioTracks && player.audioTracks.length > 0))),
    };
  }

  function hideDownloadUI(settings, hostConnected) {
    return !!(settings && settings.hideBrowserUI && settings.autoCapture && hostConnected);
  }

  // A navigation response explicitly offered as a download, never a player
  // fragment or an XHR used internally by the page. Firefox can intercept it
  // before its file picker starts, using webRequestBlocking.
  function attachmentDownload(details) {
    if (!details || details.method !== "GET" || !/^https?:\/\//i.test(details.url || "")
        || !["main_frame", "sub_frame", "other"].includes(details.type)
        || details.statusCode < 200 || details.statusCode >= 300 || details.statusCode === 206) return null;
    const headers = new Map((details.responseHeaders || []).map(h => [String(h.name).toLowerCase(), String(h.value || "")]));
    const disposition = headers.get("content-disposition") || "";
    if (!/^\s*attachment(?:\s*;|$)/i.test(disposition)) return null;
    const filename = dispositionFilename(disposition);
    return {filename, size:Math.max(0, Number(headers.get("content-length")) || 0), mime:headers.get("content-type") || null};
  }

  function dispositionFilename(disposition) {
    let filename = null;
    const value = String(disposition || "");
    const encoded = value.match(/(?:^|;)\s*filename\*\s*=\s*UTF-8'[^']*'([^;]+)/i);
    const plain = value.match(/(?:^|;)\s*filename\s*=\s*(?:"((?:[^"\\]|\\.)*)"|([^;]+))/i);
    try { if (encoded) filename = decodeURIComponent(encoded[1].trim()); } catch (e) {}
    if (!filename && plain) filename = (plain[1] || plain[2] || "").replace(/\\(.)/g, "$1").trim();
    // A server filename is a basename, never a directory supplied by the server.
    if (filename) filename = filename.split(/[\\/]/).pop().replace(/[\x00-\x1f\x7f]/g, "").slice(0,240) || null;
    return filename;
  }

  function videoDismissKey(page, src, videoId) {
    if (videoId) return `youtube:${videoId}`;
    return `${String(page || "").split("#")[0]}|${String(src || "")}`;
  }
  // Extensions the app's site rules add to the click-capture list.
  const extraDirect = new Set();
  function addDirectExtensions(list) {
    for (const e of list || []) if (/^[a-z0-9]{1,8}$/i.test(String(e))) extraDirect.add(String(e).toLowerCase());
  }
  function directDownloadLink(href, downloadAttribute) {
    try {
      const u = new URL(String(href || ""));
      // Magnet links: the app downloads them through BitTorrent.
      if (u.protocol === "magnet:") return /^magnet:\?/i.test(String(href));
      if (!/^https?:$/.test(u.protocol)) return false;
      if (downloadAttribute || /\.(?:zip|7z|rar|tar|tgz|gz|bz2|xz|exe|msi|dmg|pkg|deb|rpm|appimage|iso|torrent)(?:$)/i.test(u.pathname)) return true;
      const m = /\.([a-z0-9]{1,8})$/i.exec(u.pathname);
      return !!m && extraDirect.has(m[1].toLowerCase());
    } catch (e) { return false; }
  }

  // Canonical identity for duplicate collapse: slice selectors
  // (bytestart/byteend) address the same file, so they collapse.
  function canonicalUrl(u) {
    try {
      const x = new URL(String(u || ""));
      x.searchParams.delete("bytestart");
      x.searchParams.delete("byteend");
      x.hash = "";
      return x.toString();
    } catch (e) {
      return String(u || "");
    }
  }

  // ---- Response classification (deep detection) ----
  // Decides from one response's headers whether it is a stream, a media file,
  // a subtitle, a playlist part, or worth sniffing its first bytes. Opaque
  // CDNs hide manifests behind extensionless URLs and text/plain or
  // octet-stream types, so URL + MIME matching alone misses them.
  const VIDEO_EXT = ["mp4", "webm", "mkv", "mov", "avi", "m4v", "flv", "3gp", "wmv", "ogv"];
  const AUDIO_EXT = ["mp3", "m4a", "aac", "flac", "ogg", "oga", "opus", "wav", "mka"];
  const SUB_EXT = ["vtt", "webvtt", "srt", "ass", "ssa", "ttml", "dfxp"];
  const SUB_MIME = /^(?:text\/vtt|application\/x-webvtt|application\/x-subrip|application\/x-srt|application\/(?:x-)?ttml\+xml|application\/dfxp\+xml|text\/x-ssa|application\/x-ssa)$/;
  const OPAQUE_MIME = new Set(["text/plain", "application/octet-stream", "binary/octet-stream",
    "application/binary", "application/x-binary", "application/force-download",
    "application/x-download", "application/xml", "text/xml"]);
  const HLS_MIME = "application/vnd.apple.mpegurl";
  const DASH_MIME = "application/dash+xml";
  const BIG_OPAQUE_BYTES = 5 * 1024 * 1024;
  const SNIFF_TYPES = new Set(["xmlhttprequest", "media", "other", "object"]);

  function extOf(name) {
    const leaf = String(name || "").split(/[?#]/, 1)[0].split("/").pop() || "";
    const dot = leaf.lastIndexOf(".");
    return dot > 0 && dot < leaf.length - 1 ? leaf.slice(dot + 1).toLowerCase() : "";
  }

  function mediaMimeForExt(ext) {
    if (VIDEO_EXT.includes(ext)) return ext === "mkv" ? "video/x-matroska" : ext === "mov" ? "video/quicktime" : `video/${ext === "m4v" ? "mp4" : ext}`;
    if (AUDIO_EXT.includes(ext)) return ext === "mp3" ? "audio/mpeg" : ext === "m4a" ? "audio/mp4" : `audio/${ext}`;
    return null;
  }

  // r: {url, mime, type, method, status, size, filename, acceptRanges}
  // opts: {minBytes, deep}. Returns {kind, stream?, mime, reason},
  // {sniff:true, reason}, or null (not media).
  function classifyResponse(r, opts) {
    const o = opts || {};
    const minBytes = Math.max(0, Number(o.minBytes) || 0);
    const url = String((r && r.url) || "");
    const mime = String((r && r.mime) || "").toLowerCase().split(";")[0].trim();
    const type = (r && r.type) || "";
    const method = String((r && r.method) || "GET").toUpperCase();
    const status = Number(r && r.status) || 200;
    const size = Math.max(0, Number(r && r.size) || 0);
    if (!/^https?:\/\//i.test(url)) return null;
    if (!["GET", "HEAD", "POST"].includes(method)) return null;
    if (status < 200 || status >= 300 || status === 204) return null;
    let path = url, host = "";
    try { const x = new URL(url); path = x.pathname; host = x.hostname; } catch (e) {}
    const ext = extOf(path);
    const frame = type === "main_frame" || type === "sub_frame";
    if (frame && /^(?:text\/html|application\/xhtml\+xml)$/.test(mime)) return null; // pages
    const tooSmall = size > 0 && size < minBytes;
    const byExt = (x, why) => {
      if (x === "m3u8") return { kind: "playlist", stream: "hls", mime: HLS_MIME, reason: why + "-hls" };
      if (x === "mpd") return { kind: "playlist", stream: "dash", mime: DASH_MIME, reason: why + "-dash" };
      if (SUB_EXT.includes(x)) return { kind: "subtitle", mime: SUB_MIME.test(mime) ? mime : "text/vtt", reason: why + "-subtitle" };
      if (VIDEO_EXT.includes(x) || AUDIO_EXT.includes(x)) {
        if (tooSmall) return null;
        const usable = mime && !OPAQUE_MIME.has(mime) ? mime : mediaMimeForExt(x);
        return { kind: "file", mime: usable, reason: why + "-media" };
      }
      return undefined;
    };
    if (/mpegurl/.test(mime)) return { kind: "playlist", stream: "hls", mime: HLS_MIME, reason: "hls-mime" };
    if (mime === DASH_MIME) return { kind: "playlist", stream: "dash", mime: DASH_MIME, reason: "dash-mime" };
    // Parts stay captured (unchanged behavior): they let a fragment resolve
    // to its playlist or single-player page, and are demoted in menus.
    if (isSegmentUrl(url) || mime === "video/mp2t") return { kind: "segment", mime: mime || "video/mp2t", reason: "segment" };
    const fromPath = byExt(ext, "ext");
    if (fromPath !== undefined) return fromPath;
    // Content-Disposition names the real file behind download.php?id=… URLs.
    const fromName = r && r.filename ? byExt(extOf(r.filename), "disposition") : undefined;
    if (fromName !== undefined) return fromName;
    if (SUB_MIME.test(mime)) return { kind: "subtitle", mime, reason: "subtitle-mime" };
    if (/^(?:video|audio)\//.test(mime) || mime === "application/mp4" || mime === "application/ogg") {
      return tooSmall ? null : { kind: "file", mime, reason: "media-mime" };
    }
    if (method !== "GET" || !SNIFF_TYPES.has(type)) return null;
    const json = mime === "application/json" || /\+json$/.test(mime);
    const hinted = /(?:^|\/)(?:hls|m3u8|dash|mpd)(?:[-_][^/?#]+)?(?:\/|$)/i.test(path)
      || /(?:^|[.-])(?:hls|m3u8|dash|mpd)(?:[.-]|$)/i.test(host);
    if (hinted && !json) return { sniff: true, reason: "stream-path-hint" };
    if (/(?:^|\/)(?:master|manifest|playlist|index|video|audio|variant)(?:[-_.][^/?#]+)?\.txt$/i.test(path)) {
      return { sniff: true, reason: "manifest-name-hint" };
    }
    // An extensionless, range-capable multi-megabyte body is almost always a
    // progressive video behind an obfuscated URL.
    if (!ext && r && r.acceptRanges && size >= Math.max(minBytes, BIG_OPAQUE_BYTES)) {
      return { sniff: true, reason: "large-ranged-body" };
    }
    if (o.deep !== false && OPAQUE_MIME.has(mime)) return { sniff: true, reason: "opaque-mime" };
    return null;
  }

  // Identify the first bytes (≤4 KiB) of a response. Returns
  // {kind, stream?, mime, master?, reason} or null.
  function sniffBytes(bytes) {
    const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes || []);
    if (!b.length) return null;
    const at = (i, s) => [...s].every((c, k) => b[i + k] === c.charCodeAt(0));
    if (at(4, "ftyp")) return { kind: "file", mime: "video/mp4", reason: "sniff-mp4" };
    if (b[0] === 0x1a && b[1] === 0x45 && b[2] === 0xdf && b[3] === 0xa3) return { kind: "file", mime: "video/webm", reason: "sniff-webm" };
    if (at(0, "FLV")) return { kind: "file", mime: "video/x-flv", reason: "sniff-flv" };
    if (b[0] === 0x47 && (b.length <= 188 || b[188] === 0x47)) return { kind: "segment", mime: "video/mp2t", reason: "sniff-ts" };
    if (at(0, "OggS")) return { kind: "file", mime: "application/ogg", reason: "sniff-ogg" };
    if (at(0, "fLaC")) return { kind: "file", mime: "audio/flac", reason: "sniff-flac" };
    if (at(0, "RIFF") && at(8, "WAVE")) return { kind: "file", mime: "audio/wav", reason: "sniff-wav" };
    if (at(0, "ID3")) return { kind: "file", mime: "audio/mpeg", reason: "sniff-mp3" };
    let enc = "utf-8", skip = 0;
    if (b[0] === 0xff && b[1] === 0xfe) { enc = "utf-16le"; skip = 2; }
    else if (b[0] === 0xfe && b[1] === 0xff) { enc = "utf-16be"; skip = 2; }
    else if (b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf) skip = 3;
    let text = "";
    try { text = new TextDecoder(enc).decode(b.subarray(skip)); } catch (e) { return null; }
    const low = text.trimStart().toLowerCase();
    if (low.includes("#extm3u")) {
      return { kind: "playlist", stream: "hls", mime: HLS_MIME, master: low.includes("#ext-x-stream-inf"), reason: "sniff-hls" };
    }
    if (/<mpd[\s>]/.test(low)) return { kind: "playlist", stream: "dash", mime: DASH_MIME, master: true, reason: "sniff-dash" };
    if (low.startsWith("webvtt")) return { kind: "subtitle", mime: "text/vtt", reason: "sniff-vtt" };
    if (/^\d+\s*\r?\n(?:\d{2}:)?\d{2}:\d{2}[:.,]\d{2,3}\s+-->\s+/.test(low)) return { kind: "subtitle", mime: "application/x-subrip", reason: "sniff-srt" };
    if (low.includes('xmlns="http://www.w3.org/ns/ttml"')) return { kind: "subtitle", mime: "application/ttml+xml", reason: "sniff-ttml" };
    return null;
  }

  // Request headers worth replaying from the download manager: the session
  // (cookie), the page relationship (referer/origin), bearer tokens, and the
  // CDN's own x-* tokens. Never hop-by-hop or browser-variation headers.
  function forwardHeaders(requestHeaders) {
    const out = { referer: null, cookie: null, headers: {} };
    let budget = 8192;
    for (const h of requestHeaders || []) {
      const name = String((h && h.name) || "").toLowerCase();
      const value = String((h && h.value) || "");
      if (!name || !value || /[\r\n]/.test(value) || value.length > 4096) continue;
      if (name === "referer") { out.referer = value; continue; }
      if (name === "cookie") { out.cookie = value.length <= 6000 ? value : null; continue; }
      const keep = name === "origin" || name === "authorization"
        || (name.startsWith("x-") && name !== "x-client-data");
      if (!keep || budget - value.length < 0) continue;
      budget -= value.length;
      out.headers[name] = value;
    }
    return out;
  }

  // 'playlist' | 'segment' | 'subtitle' | 'file'
  function kindOf(entry) {
    const u = (entry && entry.url) || "";
    const mime = String(entry && entry.mime || "").toLowerCase();
    if (isPlaylistUrl(u) || /^(?:application\/(?:x-mpegurl|vnd\.apple\.mpegurl|dash\+xml)|audio\/mpegurl)(?:;|$)/i.test(mime)) return "playlist";
    if (entry && entry.whole) return "file"; // a player's full track (+ audio pair)
    if ((entry && entry.segment) || isSegmentUrl(u)) return "segment";
    if ((entry && entry.kind === "subtitle") || SUB_MIME.test(mime.split(";")[0]) || SUB_EXT.includes(extOf(u))) return "subtitle";
    return "file";
  }

  // Rank score (higher = offer first). A playlist on the tab demotes loose
  // segments: they are children the engine fetches itself, and ad-segments
  // hide among them — never offer parts when the whole is available.
  function scoreMedia(entry, ctx) {
    const e = entry || {};
    const u = String(e.url || "");
    const mime = String(e.mime || "").toLowerCase();
    const hasPlaylist = !!(ctx && ctx.hasPlaylist);
    const kind = kindOf(e);
    if (kind === "segment" && hasPlaylist) return -1;
    // Subtitles are offered as files, but never outrank a video or stream.
    if (kind === "subtitle") return 10;
    let s = kind === "file" ? 300 : kind === "playlist" ? 200 : 50;
    if (mime.startsWith("video/")) s += 100;
    else if (mime.startsWith("audio/")) s += 60;
    if (/mp4/i.test(mime) || /\.mp4/i.test(u)) s += 50;
    if (/fbcdn|lookaside|facebook|instagram|cdninstagram/i.test(u)) s += 20;
    // Exact page-JSON sizes print instantly; measured sizes rank bigger
    // files first (skips ad slivers the size gate let through).
    const size = e.size || e._size || 0;
    if (size > 0) s += Math.min(30, Math.log10(size + 1) * 5);
    return s;
  }

  // Collapse same-file duplicates (slice variants, re-sniffs): keep the
  // biggest-known entry, count distinct tabs for the "×N tabs" hint.
  function collapseDupes(items) {
    const byCanon = new Map();
    for (const m of (items || [])) {
      if (!m || !/^https?:\/\//i.test(m.url || "")) continue;
      const k = canonicalUrl(m.url);
      const size = m.size || m._size || 0;
      const hit = byCanon.get(k);
      if (!hit) {
        byCanon.set(k, { item: m, size, tabs: new Set(m.tabId !== undefined ? [m.tabId] : []) });
      } else {
        if (size > hit.size) { hit.item = m; hit.size = size; }
        if (m.tabId !== undefined) hit.tabs.add(m.tabId);
      }
    }
    return [...byCanon.values()].map((h) => ({ ...h.item, tabs: h.tabs.size || undefined }));
  }

  // Seconds → "3:26" for quality rows with known durations.
  function fmtDur(sec) {
    const n = Math.round(Number(sec) || 0);
    if (!(n > 0) || n >= 86400) return "";
    const h = Math.floor(n / 3600);
    const m = Math.floor((n % 3600) / 60);
    const s = String(n % 60).padStart(2, "0");
    // A movie reads "1:49:02", not "109:02".
    return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
  }

  // Floating panel tokens: the "extension_popup.html" design — a circle
  // trigger opening a message-bubble panel with a 2-column quality grid,
  // other-file chips and a Download / Cancel / X action row. Same keys in
  // both modes so the paint code never branches on missing fields.
  function panelTokens(mode) {
    if (mode === "light") {
      return {
        mode: "light",
        bg: "#ffffff",
        card: "#f8fafc",
        hover: "#eef2f7",
        border: "#e2e8f0",
        text: "#0f172a",
        dim: "#64748b",
        muted: "#94a3b8",
        accent: "#2563eb",
        accentLight: "rgba(37,99,235,.10)",
        shadow: "0 12px 32px rgba(15,23,42,.18)",
        thumbBg: "linear-gradient(135deg,#c7d2fe,#a5b4fc)",
        toggleBorder: "rgba(15,23,42,.2)",
        toggle: "☾",
      };
    }
    return {
      mode: "dark",
      bg: "#0f172a",
      card: "#1e293b",
      hover: "#334155",
      border: "#334155",
      text: "#f1f5f9",
      dim: "#94a3b8",
      muted: "#64748b",
      accent: "#3b82f6",
      accentLight: "rgba(59,130,246,.16)",
      shadow: "0 8px 32px rgba(0,0,0,.4)",
      thumbBg: "linear-gradient(135deg,#667eea,#764ba2)",
      toggleBorder: "rgba(241,245,249,.25)",
      toggle: "☀",
    };
  }

  // Repaint a built panel in the current theme. Role tags live in
  // `data-dc-p` (layout is inline-styled at build time and never touched
  // here), and the bubble arrow reads `data-dc-arrow` for its side.
  function paintDcPanel(root) {
    if (!root) return;
    const T = panelTokens(dcThemeMode());
    root._dcT = T;
    root.style.background = T.bg;
    root.style.borderColor = T.border;
    root.style.color = T.text;
    root.style.boxShadow = T.shadow;
    root.querySelectorAll("[data-dc-p]").forEach((el) => {
      const k = el.getAttribute("data-dc-p");
      if (k === "text") el.style.color = T.text;
      else if (k === "dim") el.style.color = T.dim;
      else if (k === "muted") el.style.color = T.muted;
      else if (k === "accent") el.style.color = T.accent;
      else if (k === "icon") el.style.color = T.accent;
      else if (k === "thumb") el.style.background = T.thumbBg;
      else if (k === "divider") el.style.background = T.border;
      else if (k === "borders") el.style.borderColor = T.border;
      else if (k === "card") { el.style.background = T.card; el.style.borderColor = T.border; }
      else if (k === "chip") { el.style.background = T.card; el.style.borderColor = T.border; el.style.color = T.text; }
      else if (k === "chipmini") { el.style.background = T.hover; el.style.color = T.dim; }
      else if (k === "btn") { el.style.background = T.card; el.style.borderColor = T.border; el.style.color = T.text; }
      else if (k === "btnp") { el.style.background = T.accent; el.style.borderColor = T.accent; el.style.color = "#fff"; }
      else if (k === "badge") { el.style.background = T.accentLight; el.style.color = T.accent; }
      else if (k === "item") {
        const sel = !!el.getAttribute("data-dc-sel");
        el.style.background = sel ? T.accentLight : T.card;
        el.style.borderColor = sel ? T.accent : T.border;
        el.style.color = T.text;
      } else if (k === "arrow") {
        const side = el.getAttribute("data-dc-arrow") === "bottom" ? "bottom" : "top";
        el.style.background = T.bg;
        el.style.borderLeftColor = side === "top" ? T.border : "transparent";
        el.style.borderTopColor = side === "top" ? T.border : "transparent";
        el.style.borderRightColor = side === "top" ? "transparent" : T.border;
        el.style.borderBottomColor = side === "top" ? "transparent" : T.border;
      } else if (k === "toggle") {
        el.style.color = T.text;
        el.style.borderColor = T.toggleBorder;
        if (!toggleIcon(el, T.toggle === "☀" ? "sun" : "moon")) el.textContent = T.toggle;
      }
    });
  }

  // The light/dark switch as a drawn icon (a sun in dark mode, a moon in
  // light mode). False where no DOM can draw it (the caller writes a glyph).
  const TOGGLE_PATHS = {
    // A round core and eight short rays, all on the centre (12, 12).
    sun: ["M12 8.4a3.6 3.6 0 1 0 0 7.2a3.6 3.6 0 1 0 0-7.2z",
      "M12 3.6v1.8M12 18.6v1.8M3.6 12h1.8M18.6 12h1.8M6.06 6.06l1.27 1.27M16.67 16.67l1.27 1.27M6.06 17.94l1.27-1.27M16.67 7.33l1.27-1.27"],
    moon: ["M20.5 14.5A8.5 8.5 0 0 1 9.5 3.5a8.5 8.5 0 1 0 11 11z"],
  };
  function toggleIcon(el, kind) {
    try {
      const doc = el.ownerDocument;
      if (!doc || !doc.createElementNS) return false;
      if (el.getAttribute("data-dc-icon") === kind) return true;
      const NS = "http://www.w3.org/2000/svg";
      const svg = doc.createElementNS(NS, "svg");
      svg.setAttribute("viewBox", "0 0 24 24");
      svg.setAttribute("width", "14");
      svg.setAttribute("height", "14");
      svg.setAttribute("fill", "none");
      svg.setAttribute("stroke", "currentColor");
      svg.setAttribute("stroke-width", "2");
      svg.setAttribute("stroke-linecap", "round");
      svg.setAttribute("stroke-linejoin", "round");
      svg.style.cssText = "display:block;margin:auto;pointer-events:none;";
      for (const d of TOGGLE_PATHS[kind]) {
        const path = doc.createElementNS(NS, "path");
        path.setAttribute("d", d);
        svg.appendChild(path);
      }
      el.textContent = "";
      el.appendChild(svg);
      // Centred whatever the page's button styles say.
      el.style.display = "grid";
      el.style.placeItems = "center";
      el.style.padding = "0";
      el.style.lineHeight = "0";
      el.setAttribute("data-dc-icon", kind);
      el.title = kind === "sun" ? "Light theme" : "Dark theme";
      return true;
    } catch (e) { return false; }
  }

  // In-page theme: auto from prefers-color-scheme, overridable per page via
  // the panel's toggle. Runs against the real DOM in content scripts AND
  // under node with stubbed globals (see rank.test.js "panel paint").
  function dcThemeMode() {
    try {
      const g = (typeof globalThis !== "undefined") ? globalThis : {};
      if (g.__dcTheme === "light" || g.__dcTheme === "dark") return g.__dcTheme;
      if (typeof window !== "undefined" && (window.__dcTheme === "light" || window.__dcTheme === "dark")) {
        return window.__dcTheme;
      }
      const mm = (typeof window !== "undefined" && window.matchMedia) || g.matchMedia;
      if (mm && mm("(prefers-color-scheme: light)").matches) return "light";
    } catch (e) {}
    return "dark";
  }
  function dcSetTheme(mode) {
    try {
      const g = (typeof globalThis !== "undefined") ? globalThis : {};
      if (mode === "light" || mode === "dark") {
        g.__dcTheme = mode;
        if (typeof window !== "undefined") window.__dcTheme = mode;
      }
    } catch (e) {}
  }

  // ---- Facebook: one entry per video from the page's own JSON ----
  // Facebook preloads several reels and plays each as DASH byte ranges, so
  // the sniffer sees dozens of .mp4 pieces per video. The page JSON (initial
  // HTML and /api/graphql answers) describes each video whole instead:
  //   all_video_dash_prefetch_representations: [{video_id, representations:
  //     [{mime_type, base_url, bandwidth, width, height}]}]   (whole files)
  //   progressive_urls: [{progressive_url, metadata:{quality:"SD"|"HD"}}]
  //   length_in_second / playable_duration_in_ms, preferred_thumbnail
  // The last three sit just after their video's "id", so each is tied to
  // the nearest preceding known video id.
  function jsonAt(text, start) {
    // Balanced {...} / [...] from `start`, strings respected.
    let depth = 0, inStr = false, esc = false;
    for (let i = start; i < text.length; i++) {
      const c = text[i];
      if (inStr) {
        if (esc) esc = false;
        else if (c === "\\") esc = true;
        else if (c === '"') inStr = false;
      } else if (c === '"') inStr = true;
      else if (c === "[" || c === "{") depth++;
      else if (c === "]" || c === "}") {
        if (--depth === 0) {
          try { return JSON.parse(text.slice(start, i + 1)); } catch (e) { return null; }
        }
      }
    }
    return null;
  }
  // `known`: ids of videos found earlier. A later answer often carries only
  // a video's metadata (duration, SD/HD, thumbnail) without its DASH list,
  // and those fields still belong to it.
  function fbVideos(text, known) {
    const t = String(text || "");
    const byId = new Map();
    const entry = (id) => {
      if (!byId.has(id)) byId.set(id, { id, dur: 0, thumb: "", video: [], audio: [], progressive: [] });
      return byId.get(id);
    };
    const knownIds = new Set((known || []).map(String));
    for (const id of knownIds) entry(id);
    const httpUrl = (u) => typeof u === "string" && /^https:\/\/[^/]+\.(fbcdn\.net|cdninstagram\.com)\//i.test(u);
    const KEY = '"all_video_dash_prefetch_representations":';
    for (let p = t.indexOf(KEY); p >= 0; p = t.indexOf(KEY, p + KEY.length)) {
      const arr = jsonAt(t, p + KEY.length);
      if (!Array.isArray(arr)) continue;
      for (const v of arr) {
        const id = v && /^\d{6,}$/.test(String(v.video_id)) ? String(v.video_id) : "";
        if (!id || !Array.isArray(v.representations)) continue;
        const e = entry(id);
        for (const r of v.representations) {
          if (!r || !httpUrl(r.base_url)) continue;
          const mime = String(r.mime_type || "");
          const list = mime.startsWith("audio/") ? e.audio : mime.startsWith("video/") ? e.video : null;
          if (!list || list.some((x) => x.url === r.base_url)) continue;
          list.push({ url: r.base_url, mime, bw: Number(r.bandwidth) || 0,
            w: Number(r.width) || 0, h: Number(r.height) || 0, codecs: String(r.codecs || "") });
        }
      }
    }
    if (!byId.size) return [];
    // Nearest known id before `p` (within a window), for the fields that
    // follow a video's "id".
    const idRe = /"(?:id|video_id)":"(\d{6,})"/g;
    const ownerAt = (p) => {
      const from = Math.max(0, p - 6000);
      idRe.lastIndex = from;
      let owner = "";
      for (let m; (m = idRe.exec(t)) && m.index < p;) if (byId.has(m[1])) owner = m[1];
      return owner;
    };
    const each = (key, fn) => {
      for (let p = t.indexOf(key); p >= 0; p = t.indexOf(key, p + key.length)) {
        const id = ownerAt(p);
        if (id) fn(byId.get(id), p + key.length);
      }
    };
    each('"length_in_second":', (e, at) => {
      const n = parseFloat(t.slice(at, at + 20));
      if (!e.dur && n > 0 && n < 86400) e.dur = Math.round(n);
    });
    each('"playable_duration_in_ms":', (e, at) => {
      const n = parseInt(t.slice(at, at + 20), 10);
      if (!e.dur && n > 0 && n < 86400000) e.dur = Math.round(n / 1000);
    });
    each('"progressive_urls":', (e, at) => {
      const arr = jsonAt(t, at);
      for (const x of Array.isArray(arr) ? arr : []) {
        const u = x && x.progressive_url;
        if (!httpUrl(u) || e.progressive.some((y) => y.url === u)) continue;
        const br = parseInt((u.match(/[?&]bitrate=(\d+)/) || [])[1], 10) || 0;
        e.progressive.push({ url: u, quality: String((x.metadata && x.metadata.quality) || ""), bw: br });
      }
    });
    each('"preferred_thumbnail":', (e, at) => {
      const o = jsonAt(t, at);
      const u = o && o.image && o.image.uri;
      if (!e.thumb && typeof u === "string" && /^https:\/\//i.test(u)) e.thumb = u;
    });
    // Per quality, H.264 first: Facebook also serves AV1/VP9 copies, which
    // many players (Windows' own) cannot open - the joined file then looked
    // like it had no picture or no sound.
    const h264 = (c) => (/^(avc1|avc3|h264)/i.test(c || "") ? 1 : 0);
    for (const e of byId.values()) {
      e.video.sort((a, b) => (b.h * b.w - a.h * a.w) || (h264(b.codecs) - h264(a.codecs)) || (b.bw - a.bw));
      e.audio.sort((a, b) => b.bw - a.bw);
    }
    return [...byId.values()].filter((e) => e.video.length || e.progressive.length
      || (knownIds.has(e.id) && (e.dur || e.thumb)));
  }
  // The file path identifies a representation across its byte-range pieces
  // and signature refreshes (the query string changes, the path does not).
  function fbPathOf(u) {
    try { return new URL(String(u)).pathname; } catch (e) { return ""; }
  }

  // ---- Instagram, TikTok, X, Reddit ----
  // The canonical link of a post holding one video (never a profile or a
  // feed), or "". Mirrors the app's social_video_page, which hands these
  // links to yt-dlp (the video, its separate audio, the real qualities).
  const SOCIAL_HOSTS = ["instagram.com", "tiktok.com", "x.com", "twitter.com", "reddit.com"];
  const bareHost = (h) => String(h || "").toLowerCase().replace(/^(?:www|m|old|mobile)\./, "");
  function socialSite(hostname) {
    const h = bareHost(hostname);
    return SOCIAL_HOSTS.includes(h) ? h : "";
  }
  function socialPostUrl(href, base) {
    let u;
    try { u = new URL(String(href || ""), base || undefined); } catch (e) { return ""; }
    if (!/^https?:$/.test(u.protocol)) return "";
    const host = bareHost(u.hostname);
    const p = u.pathname.split("/").filter(Boolean);
    const id = (s) => /^[A-Za-z0-9_-]+$/.test(s || "");
    const num = (s) => /^\d+$/.test(s || "");
    if (host === "instagram.com") {
      for (let i = 0; i + 1 < p.length; i++) {
        if (/^(?:reel|reels|p|tv)$/.test(p[i]) && id(p[i + 1])) {
          return `https://www.instagram.com/${p[i] === "reels" ? "reel" : p[i]}/${p[i + 1]}/`;
        }
      }
      return "";
    }
    if (host === "tiktok.com") {
      for (let i = 0; i + 1 < p.length; i++) {
        if (p[i] === "video" && num(p[i + 1])) {
          const user = i > 0 && p[i - 1].startsWith("@") ? p[i - 1] : "@";
          return `https://www.tiktok.com/${user}/video/${p[i + 1]}`;
        }
      }
      return "";
    }
    if (host === "x.com" || host === "twitter.com") {
      return p.length >= 3 && p[1] === "status" && num(p[2]) && p[0] !== "i" ? `https://x.com/${p[0]}/status/${p[2]}` : "";
    }
    if (host === "reddit.com") {
      return p.length >= 4 && p[0] === "r" && p[2] === "comments" && id(p[3]) ? `https://www.reddit.com/r/${p[1]}/comments/${p[3]}/` : "";
    }
    return "";
  }

  // The remembered quality ("720p") among the heights on offer: that one,
  // else the best below it, else the lowest above it. -1 when none.
  function pickHeight(heights, pref) {
    const want = parseInt(pref, 10);
    if (!(want > 0)) return -1;
    let best = -1;
    heights.forEach((h, i) => {
      if (!(h > 0)) return;
      const cur = best < 0 ? null : heights[best];
      if (cur === null) { best = i; return; }
      const okH = h <= want, okC = cur <= want;
      if (okH && (!okC || h > cur)) best = i;
      else if (!okH && !okC && h < cur) best = i;
    });
    return best;
  }

  // How many videos the popup's Videos tab lists for a page snapshot (the
  // toolbar badge shows the same number). Facebook sends each video as many
  // audio/video pieces; they count once, as the video they belong to.
  function countVideos(peek) {
    if (!peek) return 0;
    let n = (peek.videos || []).length;
    for (const e of (peek.fb || [])) {
      const q = (e.video || []).some((v) => (Math.min(v.w || 0, v.h || 0) || v.h || 0) > 0);
      if (q || (e.progressive || []).length) n++;
    }
    const net = peek.net || [];
    const have = new Set((peek.links || []).map((l) => l.url));
    const hasPlaylist = net.some((x) => x && kindOf(x) === "playlist");
    const seen = new Set();
    for (const x of net) {
      if (!x || !/^https?:\/\//i.test(x.url || "") || have.has(x.url)) continue;
      const k = canonicalUrl(x.url);
      if (seen.has(k)) continue;
      seen.add(k);
      if (scoreMedia(x, { hasPlaylist }) < 0) continue;
      const kind = kindOf(x);
      if (kind !== "subtitle" && kind !== "segment") n++;
    }
    return n;
  }

  // The first bytes of an MP4 (ISO-BMFF) file: its picture size and whether
  // its tracks are DRM-encrypted (CENC "encv"/"enca" sample entries).
  // Returns { w, h, drm } once `moov` is in `bytes`; { moovAt } when the
  // file keeps `moov` after the media data (fetch from there and call
  // again with `base` = that offset); null when it is not an MP4 or more
  // bytes are needed.
  function mp4Info(bytes, base) {
    const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes || []);
    const u32 = (i) => ((b[i] << 24) >>> 0) + (b[i + 1] << 16) + (b[i + 2] << 8) + b[i + 3];
    const type = (i) => String.fromCharCode(b[i], b[i + 1], b[i + 2], b[i + 3]);
    const off0 = base || 0;
    let i = 0;
    let first = true;
    while (i + 8 <= b.length) {
      let size = u32(i);
      const t = type(i + 4);
      let head = 8;
      if (size === 1) {
        if (i + 16 > b.length) return null;
        size = u32(i + 8) * 4294967296 + u32(i + 12);
        head = 16;
      }
      if (first && !off0 && !/^(ftyp|styp|moov|free|skip|wide)$/.test(t)) return null;
      first = false;
      if (size === 0 || size < head) return null;
      if (t === "moov") {
        if (i + size > b.length) return null; // need more bytes
        const out = { w: 0, h: 0, drm: false };
        walk(i + 8, i + size, out, 0);
        return out;
      }
      if (i + size > b.length) return { moovAt: off0 + i + size };
      i += size;
    }
    return null;

    function walk(from, to, out, depth) {
      let j = from;
      let track = null;
      while (j + 8 <= to && depth < 9) {
        const size = u32(j);
        const t = type(j + 4);
        if (size < 8 || j + size > to) return track;
        const body = j + 8;
        if (t === "encv" || t === "enca") out.drm = true;
        else if (t === "trak") {
          const tr = walk(body, j + size, out, depth + 1) || {};
          if (tr.video && tr.h > out.h) { out.w = tr.w; out.h = tr.h; }
        } else if (t === "tkhd" && size >= 92) {
          // Width and height are the box's last 8 bytes (16.16 fixed).
          track = track || {};
          track.w = u32(j + size - 8) >>> 16;
          track.h = u32(j + size - 4) >>> 16;
        } else if (t === "hdlr" && size >= 20) {
          track = track || {};
          if (type(body + 8) === "vide") track.video = true;
        } else if (t === "mdia" || t === "minf" || t === "stbl") {
          const inner = walk(body, j + size, out, depth + 1);
          if (inner) track = Object.assign(track || {}, inner, { w: (track && track.w) || inner.w, h: (track && track.h) || inner.h });
        } else if (t === "stsd" && size > 16) {
          walk(body + 8, j + size, out, depth + 1);
        }
        j += size;
      }
      return track;
    }
  }

  const DCRank = { mp4Info, fbVideos, fbPathOf, countVideos, socialSite, socialPostUrl, pickHeight, attachmentDownload, videoDismissKey, directDownloadLink, addDirectExtensions, hideDownloadUI, playerEvidence, playlistReferences, samePlayer, completePlaylist, isVimeoAdaptive, isVimeoFragment, vimeoVideoPage, isPlaylistUrl, isSegmentUrl, canonicalUrl, kindOf, scoreMedia, classifyResponse, sniffBytes, forwardHeaders, extOf, dispositionFilename, collapseDupes, fmtDur, panelTokens, dcThemeMode, dcSetTheme, paintDcPanel };
  g.DCRank = DCRank;
  if (typeof module !== "undefined" && module.exports) module.exports = DCRank;
})(typeof globalThis !== "undefined" ? globalThis : this);
