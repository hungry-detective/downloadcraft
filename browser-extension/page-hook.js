// Page-level media reader (runs in the page's own JS world, MAIN, at
// document_start). Many players never request a recognizable media URL
// until playback, but their site API already returned it as JSON (stream
// lists, player configs). This reads fetch/XHR text answers and known
// player-state globals, and posts the media URLs it finds to page-relay.js,
// which forwards them to the background sniffer. No chrome.* APIs here.
(() => {
  "use strict";
  if (window.__dcPageHook) return;
  window.__dcPageHook = true;
  // Anti-bot frames (Cloudflare Turnstile, hCaptcha, reCAPTCHA, Arkose) hold
  // no media, forbid most connections by CSP (their own `fetch("data:,")`
  // probe was blamed on this wrapper) and may count a wrapped fetch as
  // tampering. The manifest skips them; this covers the on-install injection.
  if (/(^|\.)(challenges\.cloudflare\.com|hcaptcha\.com|recaptcha\.net|arkoselabs\.com)$/i.test(location.hostname)
      || (/(^|\.)google\.com$/i.test(location.hostname) && location.pathname.startsWith("/recaptcha/"))) return;

  const MAX_BODY = 2 * 1024 * 1024;
  const MAX_PER_BODY = 30;
  const sent = new Set();
  let queue = [];
  let timer = 0;

  function post(items) {
    for (const it of items) {
      if (!it || typeof it.url !== "string" || it.url.length > 4000 || sent.has(it.url)) continue;
      sent.add(it.url);
      queue.push(it);
    }
    if (sent.size > 500) sent.clear();
    if (!queue.length || timer) return;
    timer = setTimeout(() => {
      timer = 0;
      const batch = queue.splice(0, MAX_PER_BODY);
      try { window.postMessage({ __dc: "page-media", items: batch }, location.origin); } catch (e) {}
      if (queue.length) post([]);
    }, 300);
  }

  // Media URLs inside JSON/text, including JSON-escaped slashes.
  const MEDIA_RE = /https?:\/\/[^\s"'<>\\]+?\.(m3u8|mpd|mp4|webm)(\?[^\s"'<>\\]*)?(?=[\s"'<>\\,)\]}]|$)/gi;
  const MIME = { m3u8: "application/vnd.apple.mpegurl", mpd: "application/dash+xml", mp4: "video/mp4", webm: "video/webm" };
  // Facebook: its GraphQL answers describe each video whole (every quality,
  // audio, duration), which content.js turns into one entry per video. The
  // generic scan below would instead post every quality of every preloaded
  // reel as a separate nameless file, so it is skipped there.
  const FB = /(^|\.)facebook\.com$/i.test(location.hostname);
  const FB_MAX = 8 * 1024 * 1024;
  function scanFacebook(text) {
    if (text.length > FB_MAX) return;
    if (text.indexOf("all_video_dash_prefetch_representations") < 0 && text.indexOf('"progressive_urls"') < 0) return;
    try { window.postMessage({ __dc: "fb-json", text }, location.origin); } catch (e) {}
  }
  function scanText(text) {
    if (typeof text !== "string" || !text) return;
    if (FB) return scanFacebook(text);
    if (text.length > MAX_BODY) return;
    if (!/\.(?:m3u8|mpd|mp4|webm)/i.test(text)) return;
    const plain = text.replace(/\\u002[fF]/g, "/").replace(/\\\//g, "/");
    const out = [];
    for (const m of plain.matchAll(MEDIA_RE)) {
      out.push({ url: m[0].replace(/&amp;/g, "&"), mime: MIME[m[1].toLowerCase()] });
      if (out.length >= MAX_PER_BODY) break;
    }
    if (out.length) post(out);
  }

  // Answers worth reading: JSON / text / JS. Manifests themselves are already
  // caught by the network sniffer (reading one would list its segments).
  const READABLE = /json|text\/plain|javascript|text\/html|xml/i;
  const SKIP = /mpegurl|dash\+xml|text\/css/i;

  const origFetch = window.fetch;
  if (typeof origFetch === "function") {
    window.fetch = function (...args) {
      const a0 = args[0];
      const target = typeof a0 === "string" ? a0 : (a0 && (a0.url || a0.href)) || "";
      // data:/blob: answers are the page's own bytes, never a media list.
      if (/^(data|blob):/i.test(String(target))) return origFetch.apply(this, args);
      const p = origFetch.apply(this, args);
      p.then((res) => {
        try {
          const ct = res.headers.get("content-type") || "";
          const len = Number(res.headers.get("content-length") || 0);
          if (!READABLE.test(ct) || SKIP.test(ct) || len > (FB ? FB_MAX : MAX_BODY)) return;
          res.clone().text().then(scanText, () => {});
        } catch (e) { /* never disturb the page */ }
      }, () => {});
      return p;
    };
  }

  const XHR = window.XMLHttpRequest && window.XMLHttpRequest.prototype;
  if (XHR && typeof XHR.send === "function") {
    const origSend = XHR.send;
    XHR.send = function (...args) {
      try {
        this.addEventListener("load", function () {
          try {
            const ct = this.getResponseHeader("content-type") || "";
            if (SKIP.test(ct)) return;
            if (this.responseType === "" || this.responseType === "text") scanText(this.responseText);
            else if (this.responseType === "json" && this.response) scanText(JSON.stringify(this.response));
          } catch (e) { /* opaque or binary response */ }
        });
      } catch (e) {}
      return origSend.apply(this, args);
    };
  }

  // Player state that sites keep in globals instead of a request.
  function bilibili() {
    const d = window.__playinfo__ && window.__playinfo__.data;
    if (!d) return [];
    const urlOf = (x) => x && (x.baseUrl || x.base_url || (Array.isArray(x.backupUrl) && x.backupUrl[0]));
    if (d.dash && Array.isArray(d.dash.video) && d.dash.video.length) {
      const best = (list) => list.slice().sort((a, b) => (b.bandwidth || 0) - (a.bandwidth || 0))[0];
      const v = best(d.dash.video);
      const a = Array.isArray(d.dash.audio) && d.dash.audio.length ? best(d.dash.audio) : null;
      const vu = urlOf(v);
      if (!vu) return [];
      return [{ url: vu, mime: v.mimeType || v.mime_type || "video/mp4",
        pairUrl: urlOf(a) || null, pairMime: a ? (a.mimeType || a.mime_type || "audio/mp4") : null,
        dur: d.dash.duration || null }];
    }
    if (Array.isArray(d.durl) && d.durl[0] && d.durl[0].url) return [{ url: d.durl[0].url, mime: "video/mp4" }];
    return [];
  }
  function iqiyi() {
    const data = window.__playerdata__ && window.__playerdata__.__dash && window.__playerdata__.__dash.data;
    const videos = data && data.program && data.program.video;
    if (!Array.isArray(videos)) return [];
    // Only real URLs: iQiyi often inlines the playlist text instead.
    return videos.filter((v) => v && typeof v.m3u8 === "string" && /^https?:\/\//i.test(v.m3u8))
      .map((v) => ({ url: v.m3u8, mime: MIME.m3u8 }));
  }
  const READERS = [bilibili, iqiyi];
  let polls = 0;
  function pollGlobals() {
    for (const r of READERS) {
      try { post(r()); } catch (e) { /* site changed shape */ }
    }
    if (++polls < 30) setTimeout(pollGlobals, 500);
  }
  pollGlobals();
  // SPA navigation: read the next video's state too.
  for (const k of ["pushState", "replaceState"]) {
    const orig = history[k];
    if (typeof orig !== "function") continue;
    history[k] = function (...args) {
      const r = orig.apply(this, args);
      polls = 0;
      setTimeout(pollGlobals, 800);
      // Tell content.js (isolated world, which cannot see these calls).
      try { window.postMessage({ __dc: "nav" }, location.origin); } catch (e) {}
      return r;
    };
  }
})();
