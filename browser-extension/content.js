// Content script: collects downloadable links AND page media
// (<video>/<audio>/<source>) so the popup can one-click them.
// On YouTube watch pages it additionally extracts direct progressive
// streams from the page player (real browser session: real cookies/TLS,
// so no bot-wall) and offers an in-page download button with qualities.
(() => {
  // Must match manifest version + background.js EXT_VER. The background
  // rejects messages from any other version with a "reload this tab"
  // error, so a stale script can never fail mysteriously.
  const EXT_VER = "0.9.15";
  // Promise-style calls (the `await api.*` one below) go through this handle
  // so one file serves both engines: Firefox exposes the promise-returning
  // `browser` namespace, Chrome only has `chrome` (promise-capable under MV3).
  // Callback-style calls keep using `chrome`, which Firefox also provides.
  const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;
  // Generation guard: when the worker re-injects this script (after an
  // update), older live instances must go quiet instead of fighting over
  // the button. The newest instance always wins.
  const GEN = (window.__dcGen = (window.__dcGen || 0) + 1);
  // Only the top frame owns the popup's shared storage keys: ad/account
  // iframes wrote empty or foreign video lists over the page's own.
  // Incognito pages never write it either: storage.local is on disk and
  // outlives the private window.
  const IS_TOP = (() => {
    try {
      return window.top === window && !(chrome.extension && chrome.extension.inIncognitoContext);
    } catch (e) { return false; }
  })();
  const alive = () => window.__dcGen === GEN;
  // "Extension context invalidated" happens when Chrome reloads/updates the
  // extension while this page is open: every chrome.* call in the OLD script
  // then throws. Detect it once, shut down timers/observers (no spam), and
  // tell the user the one-click fix (reload the tab).
  let __dcDead = false;
  const __dcTimers = [];
  const __dcObservers = [];
  function ctxInvalidated(e) {
    const m = String((e && e.message) || e || "");
    return /extension context invalidated/i.test(m);
  }
  function markDead() {
    if (__dcDead) return;
    __dcDead = true;
    try { __dcTimers.forEach(clearInterval); } catch (e) {}
    try { __dcTimers.forEach(clearTimeout); } catch (e) {}
    try { __dcObservers.forEach((o) => { try { o.disconnect(); } catch (x) {} }); } catch (e) {}
    toast("Download Craft was updated — reload this tab (Ctrl+R), then click again");
  }
  // `let`: site rules from the app may add extensions (see applySiteRules).
  let DL = /\.(zip|rar|7z|exe|msi|pdf|mp4|mkv|webm|mov|mp3|flac|wav|ogg|m4a|iso|tar|gz|dmg)$/i;

  function collectLinks() {
    try {
      const links = [...document.querySelectorAll("a[href]")]
        .map((a) => ({ url: a.href, text: (a.textContent || "").trim().slice(0, 80) }))
        .filter((l) => /^https?:\/\//i.test(l.url) && DL.test(l.url.split("?")[0]))
        .slice(0, 30);
      const media = [];
      const posterOf = (el) => {
        try {
          const v = el && el.tagName && el.tagName.toLowerCase() === "video"
            ? el
            : (el && el.closest && el.closest("video")) || null;
          const p = v && v.getAttribute && v.getAttribute("poster");
          return /^https?:\/\//i.test(p || "") ? p : "";
        } catch (e) { return ""; }
      };
      document.querySelectorAll("video[src], audio[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.currentSrc || el.src)) {
          media.push({ url: el.currentSrc || el.src, text: `Page media (${el.tagName.toLowerCase()})`, thumb: posterOf(el) });
        }
      });
      document.querySelectorAll("video source[src], audio source[src]").forEach((el) => {
        if (/^https?:\/\//i.test(el.src)) {
          media.push({ url: el.src, text: "Page media (source)", thumb: posterOf(el) });
        }
      });
      const found = [...media, ...links].slice(0, 50);
      if (IS_TOP) chrome.storage.local.set({
        downloadCraftLinks: found,
        downloadCraftPage: location.href,
      });
      return found;
    } catch (e) {
      if (ctxInvalidated(e)) { markDead(); return; }
      /* page CSP may block storage; popup manual URL still works */
    }
  }

  // ---------- YouTube support ----------
  // YouTube's visitor id from the page's config script: yt-dlp can use it to
  // get past a bot check when there are no cookies (the app retries with it).
  function ytVisitorData() {
    try {
      if (!/(^|\.)youtube\.com$/i.test(location.hostname)) return null;
      for (const s of document.scripts) {
        if (s.src || !s.textContent || s.textContent.length > 3000000) continue;
        const m = /"VISITOR_DATA":"([A-Za-z0-9%_=.-]{10,200})"/.exec(s.textContent);
        if (m) return m[1];
      }
    } catch (e) {}
    return null;
  }

  function ytVideoId() {
    try {
      const u = new URL(location.href);
      if (u.hostname.endsWith("youtube.com")) {
        if (u.pathname === "/watch") return u.searchParams.get("v");
        const m = u.pathname.match(/^\/(shorts|embed|live|v)\/([^/?#]+)/);
        if (m) return m[2];
      }
      if (u.hostname === "youtu.be") {
        const id = u.pathname.split("/").filter(Boolean)[0];
        if (id) return id;
      }
    } catch (e) {
      /* ignore */
    }
    return null;
  }

  // Balanced-brace scan: finds EVERY `var ytInitialPlayerResponse = {...};`
  // block without choking on braces inside strings. Returns all parsed
  // responses — YouTube's SPA keeps stale script tags from previous
  // videos, so callers must match by video id (never trust order).
  function playerResponses() {
    const out = [];
    try {
      for (const s of document.scripts) scanPlayerResponses(s.textContent || "", out);
    } catch (e) {
      /* unparseable page variant */
    }
    return out;
  }

  // SPA navigation (home → video, autoplay) adds no new script tag, so the
  // current video's sizes are missing from the page. Fetch its watch page
  // once (same origin, the user's session) and keep the parsed response.
  const fetchedPlayer = new Map(); // videoId -> player response | "pending" | "failed"
  function fetchPlayerResponse(id, onReady) {
    if (fetchedPlayer.has(id)) return;
    fetchedPlayer.set(id, "pending");
    if (fetchedPlayer.size > 20) fetchedPlayer.delete(fetchedPlayer.keys().next().value);
    fetch(`/watch?v=${encodeURIComponent(id)}`, { credentials: "include" })
      .then((r) => (r.ok ? r.text() : ""))
      .then((html) => {
        const found = [];
        scanPlayerResponses(html, found);
        const pr = found.find((p) => p && p.videoDetails && p.videoDetails.videoId === id && p.streamingData);
        fetchedPlayer.set(id, pr || "failed");
        if (pr) onReady();
      })
      .catch(() => fetchedPlayer.set(id, "failed"));
  }

  function scanPlayerResponses(t, out) {
    const marker = "ytInitialPlayerResponse = ";
    let p = t.indexOf(marker);
    while (p >= 0) {
      try {
        let i = p + marker.length;
        while (i < t.length && t[i] !== "{") i++;
        const start = i;
        let depth = 0;
        let inStr = false;
        let esc = false;
        let end = -1;
        for (; i < t.length; i++) {
          const c = t[i];
          if (inStr) {
            if (esc) esc = false;
            else if (c === "\\") esc = true;
            else if (c === '"') inStr = false;
          } else if (c === '"') {
            inStr = true;
          } else if (c === "{") {
            depth++;
          } else if (c === "}") {
            depth--;
            if (depth === 0) { end = i; break; }
          }
        }
        if (end > start) out.push(JSON.parse(t.slice(start, end + 1)));
      } catch (e) {
        /* malformed block: try the next one */
      }
      p = t.indexOf(marker, p + 1);
    }
    return out;
  }

  function mimeExt(mime) {
    const m = String(mime || "").split(";")[0].trim().toLowerCase();
    if (m === "audio/mp4" || m === "audio/m4a" || m === "audio/x-m4a") return ".m4a";
    if (m === "audio/webm") return ".webm";
    if (m === "video/webm") return ".webm";
    return ".mp4";
  }

  // Short container tag for quality rows (IDM shows "MKV file / MP4 file").
  function contOf(mime) {
    const m = String(mime || "").split(";")[0].trim().toLowerCase();
    if (m === "audio/mp4" || m === "audio/m4a" || m === "audio/x-m4a") return "m4a";
    if (m === "audio/webm") return "weba";
    if (m === "video/webm") return "webm";
    if (m === "video/mp4") return "mp4";
    return (m.split("/")[1] || "mp4").slice(0, 5);
  }

  function ytThumb(pr, id) {
    try {
      const thumbs = pr && pr.videoDetails && pr.videoDetails.thumbnail && pr.videoDetails.thumbnail.thumbnails;
      if (Array.isArray(thumbs) && thumbs.length) {
        const valid = thumbs.filter((t) => t && /^https?:\/\//i.test(t.url || ""));
        if (valid.length) {
          valid.sort((a, b) => (b.width || 0) - (a.width || 0));
          return valid[0].url;
        }
      }
    } catch (e) { /* ignore */ }
    return id ? `https://i.ytimg.com/vi/${id}/hqdefault.jpg` : "";
  }

  // The title on screen. YouTube A/B-tests titles, so videoDetails.title in
  // the page JSON can differ from what the user sees (and would name the
  // file after a title they never saw). Only trusted when the watch page's
  // own video-id matches — during SPA navigation the heading is stale.
  function ytTitleText(t) {
    const s = String(t || "").replace(/\s+/g, " ").trim();
    return s && !isMetricTitle(s) ? s : "";
  }

  function ytShownTitle(id) {
    try {
      const flexy = document.querySelector("ytd-watch-flexy[video-id]");
      if (!flexy || flexy.getAttribute("video-id") !== id) return "";
      const h1 = document.querySelector("ytd-watch-metadata h1");
      return ((h1 && h1.textContent) || "").replace(/\s+/g, " ").trim();
    } catch (e) { return ""; }
  }

  function ytStreams() {
    const id = ytVideoId();
    if (!id) return null;
    // Strict id match: stale SPA script tags must never win.
    const prs = playerResponses();
    let pr = prs.find((p) => p && p.videoDetails && p.videoDetails.videoId === id && p.streamingData) || null;
    if (!pr) {
      const fetched = fetchedPlayer.get(id);
      if (fetched && typeof fetched === "object") pr = fetched;
      else fetchPlayerResponse(id, () => { try { if (alive() && ytVideoId() === id) refreshYouTube(); } catch (e) {} });
    }
    if (!pr) pr = prs.find((p) => p && p.videoDetails && p.videoDetails.videoId === id) || null;
    const docTitle = ytTitleText(document.title.replace(/\s*-\s*YouTube\s*$/i, ""));
    if (!pr || !pr.streamingData) {
      return { videoId: id, title: docTitle || `youtube_${id}`, thumb: ytThumb(pr, id), streams: [], videoOnly: [], audio: [], subs: [] };
    }
    const shown = ytShownTitle(id);
    // Player metadata is authoritative: a video really titled "50 Views"
    // keeps that name. Only a counter picked up from the on-screen fallback
    // is rejected in favour of the metadata.
    const metaTitle = String((pr.videoDetails && pr.videoDetails.title) || "").replace(/\s+/g, " ").trim();
    const title = (shown && !isMetricTitle(shown) ? shown : "")
      || metaTitle
      || docTitle
      || `youtube_${id}`;
    // Stream URLs carry `expire=<unix>` and are session-locked: rows older
    // than ~2 min 403 in the app (IDM re-grabs instead of offering dead
    // links), so drop them here and let the sniffer/menu refresh refill.
    const nowS = Date.now() / 1000;
    const expOf = (u) => {
      try {
        const m = String(u || "").match(/[?&]expire=(\d+)/);
        return m ? parseInt(m[1], 10) : 0;
      } catch (e) { return 0; }
    };
    const fresh = (u) => {
      const e = expOf(u);
      return e === 0 || e > nowS + 120;
    };
    const direct = (arr) => (Array.isArray(arr) ? arr : [])
      .filter((f) => f && typeof f.url === "string" && f.url.startsWith("http") && fresh(f.url));
    // Sizes straight from the page JSON (contentLength per format): exact,
    // instant, no HEAD request — googlevideo HEADs 403 off-session, which is
    // why rows historically showed no size. Duration likewise.
    const sizeOf = (f) => {
      try {
        const n = parseInt(String(f.contentLength || "0").replace(/\D/g, ""), 10);
        return n > 0 ? n : 0;
      } catch (e) { return 0; }
    };
    const durOf = (f) => {
      try {
        const ms = parseInt(String(f.approxDurationMs || "0").replace(/\D/g, ""), 10);
        return ms > 0 && ms < 86400000 * 3 ? Math.round(ms / 1000) : 0;
      } catch (e) { return 0; }
    };
    // Display-only estimate when YouTube omits contentLength (typical for the
    // progressive 360p stream, whose HEAD also 403s off-session): bitrate ×
    // duration. Shown as "~"; never sent to the app as the real size.
    const estOf = (f) => {
      if (sizeOf(f) > 0) return 0;
      const br = Number(f.averageBitrate || f.bitrate || 0);
      const d = durOf(f);
      return br > 0 && d > 0 ? Math.round((br * d) / 8) : 0;
    };
    const qlabel = (f) => {
      // qualityLabel is "1080p" but often lacks fps; append it like IDM
      // ("1080p60") when the stream is high-frame-rate.
      const q = f.qualityLabel || f.quality || "unknown";
      if (/^\d+p$/.test(q) && (f.fps || 0) >= 50) return `${q}${f.fps}`;
      return q;
    };
    // Ciphered entries carry signatureCipher instead of a direct URL (the
    // page hides URLs until playback). Offered as "(via app)" rows: the
    // click sends the page URL + quality hint and the desktop app deciphers
    // that exact quality server-side — this is what fills the menu on
    // VEVO/music videos that embed zero direct URLs.
    const ciphered = (arr, wantVideo) => (Array.isArray(arr) ? arr : [])
      .filter((f) => f && typeof f.signatureCipher === "string" && !f.url
        && (wantVideo ? String(f.mimeType || "").startsWith("video/")
                      : String(f.mimeType || "").startsWith("audio/")));
    const cipherRows = (arr, wantVideo, kind) => ciphered(arr, wantVideo).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: null, label: qlabel(f), mime, disp: contOf(mime), kind, itag: f.itag || 0, ciphered: true, _size: sizeOf(f), _est: estOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
    });
    // Progressive = video+audio in one file (ready to play).
    const streams = direct(pr.streamingData.formats).map((f) => {
      const mime = String(f.mimeType || "").split(";")[0];
      return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "av", itag: f.itag || 0, _size: sizeOf(f), _est: estOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
    });
    cipherRows(pr.streamingData.formats, true, "av").forEach((s) => {
      if (!streams.some((x) => x.label === s.label && x.disp === s.disp)) streams.push(s);
    });
    // Adaptive video-only incl. WebM VP9/AV1 — this is where 1080p+/4K live
    // (IDM's "2160p 4K / 1440p HD" rows). Honestly video-only: the app labels
    // them "(no audio)" until the P1.3 ffmpeg merge lands.
    const seenQ = new Set();
    const videoOnly = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("video/"))
      .map((f) => {
        const mime = String(f.mimeType || "").split(";")[0];
        return { url: f.url, label: qlabel(f), mime, disp: contOf(mime), kind: "v", itag: f.itag || 0, _size: sizeOf(f), _est: estOf(f), _dur: durOf(f), w: f.width || 0, h: f.height || 0 };
      })
      .filter((s) => {
        const k = `${s.label}|${s.disp}`;
        if (seenQ.has(k)) return false;
        seenQ.add(k);
        return true;
      })
      .slice(0, 16);
    cipherRows(pr.streamingData.adaptiveFormats, true, "v").forEach((s) => {
      if (!videoOnly.some((x) => x.label === s.label && x.disp === s.disp)) videoOnly.push(s);
    });
    // Size/duration map over EVERY video format (even ones with no URL at
    // all): powers the generic ladder + popup rows with exact page-JSON
    // sizes, no network measuring needed.
    const sizes = {};
    for (const f of [...(pr.streamingData.formats || []), ...(pr.streamingData.adaptiveFormats || [])]) {
      if (!String(f.mimeType || "").startsWith("video/")) continue;
      const q = qlabel(f);
      const sz = sizeOf(f);
      if (sz > 0 && !sizes[q]) sizes[q] = { size: sz, dur: durOf(f) };
    }
    const audio = direct(pr.streamingData.adaptiveFormats)
      .filter((f) => String(f.mimeType || "").startsWith("audio/"))
      .map((f) => {
        const mime = String(f.mimeType || "").split(";")[0];
        return {
          url: f.url,
          label: (f.audioQuality || "").replace("AUDIO_QUALITY_", "") || (f.bitrate ? Math.round(f.bitrate / 1000) + "k" : "audio"),
          mime,
          disp: contOf(mime),
          kind: "a",
          itag: f.itag || 0,
          br: f.bitrate || 0,
          _size: sizeOf(f),
          _dur: durOf(f),
        };
      })
      .slice(0, 6);
    // Subtitle tracks (VTT): shown as "Other Files" chips in the panel and
    // sent to the app as plain file downloads — the timedtext URL is a
    // normal https GET, and `fmt=vtt` yields a straight .vtt file.
    const subs = [];
    try {
      const list = pr.captions && pr.captions.playerCaptionsTracklistRenderer
        && pr.captions.playerCaptionsTracklistRenderer.captionTracks;
      for (const c of (list || []).slice(0, 8)) {
        const u = String((c && c.baseUrl) || "");
        if (!/^https?:\/\//i.test(u)) continue;
        const nm = (c.name && (c.name.simpleText
          || (c.name.runs && c.name.runs[0] && c.name.runs[0].text))) || c.languageCode || "sub";
        // YouTube already says "(auto-generated)" — don't stack "(auto)" on
        // top of it, just normalize the label to a short one.
        const short = String(nm).replace(/\s*\([^)]*\bauto[^)]*\)\s*/i, "").trim() || c.languageCode || "sub";
        subs.push({
          url: /[?&]fmt=/i.test(u) ? u : u + "&fmt=vtt",
          label: `${short}${c.kind === "asr" ? " (auto)" : ""}`,
          lang: c.languageCode || "",
        });
      }
    } catch (e) { /* captions are optional */ }
    // The best M4A sound track's size and length, links or not: what the
    // panel's "♪ M4A" saves (and roughly what an MP3 of it weighs).
    let audioSize = 0;
    let audioDur = 0;
    let audioBr = -1;
    for (const f of (pr.streamingData.adaptiveFormats || [])) {
      if (!String(f.mimeType || "").startsWith("audio/mp4")) continue;
      if (f.audioTrack && f.audioTrack.audioIsDefault === false) continue;
      const br = Number(f.bitrate || 0);
      if (br > audioBr && sizeOf(f) > 0) { audioBr = br; audioSize = sizeOf(f); audioDur = durOf(f); }
    }
    return { videoId: id, title, thumb: ytThumb(pr, id), streams, videoOnly, audio, sizes, subs, audioSize, audioDur };
  }

  function stripFbSlice(url) {
    try {
      const u = new URL(url);
      if (!u.searchParams.has("bytestart") && !u.searchParams.has("byteend")) return url;
      u.searchParams.delete("bytestart");
      u.searchParams.delete("byteend");
      return u.toString();
    } catch (e) { return url; }
  }

  function playerHasAudio(v) {
    return !!(v && (v.mozHasAudio === true || Number(v.webkitAudioDecodedByteCount) > 0
      || (v.audioTracks && v.audioTracks.length > 0)));
  }

  function sendDownload(url, filename, mime, size, pair, quality, dur) {
    // Use only the matching video or an unambiguous sole video. Subtitle,
    // audio-only and ordinary file links do not borrow video expectations.
    const evidence = typeof DCRank !== "undefined"
      ? DCRank.playerEvidence(document.querySelectorAll("video"), url, mime, location.href, dur)
      : {dur: Number.isFinite(Number(dur)) && Number(dur) > 0 ? Math.round(Number(dur)) : null, audioExpected:false};
    dur = evidence.dur;
    const audioExpected = evidence.audioExpected;
    // A tab running a stale script (extension reloaded without a tab
    // reload) has no extension APIs left — fail loudly, not mysteriously.
    try {
      if (!chrome || !chrome.runtime || !chrome.runtime.sendMessage) {
        throw new Error("page runs an older script — reload this tab (Ctrl+R), then click again");
      }
      void chrome.runtime.id; // throws when the context was invalidated
    } catch (e) {
      if (ctxInvalidated(e)) markDead();
      toast("Download Craft: " + (e && e.message ? e.message : e));
      return;
    }
    // blob: / mediasource streams cannot be fetched by the desktop app —
    // fall back to the page URL so the engine can resolve it server-side.
    // The extension keeps audio extensions for audio fallbacks.
    let sendUrl = stripFbSlice(url);
    if (/^blob:/i.test(url)) {
      sendUrl = stripFbSlice(location.href);
      if (!filename) {
        const base = String(document.title || "media").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim().replace(/\.+$/, "") || "media";
        filename = base + mimeExt(mime);
      }
    }
    // A player-wrapper page ("/jwplayer/?source=<encoded media>") is HTML;
    // the real stream is in the query. Unwrap it so we send the media, not
    // the shell, and rename from the real URL (the release folder for HLS).
    const inner = unwrapMediaUrl(sendUrl);
    if (inner) {
      sendUrl = stripFbSlice(inner);
      const base = mediaBase(sendUrl);
      const ext = mimeExt(mime) || outExtFor(sendUrl);
      filename = base.toLowerCase().endsWith(ext.toLowerCase()) ? base : base + ext;
    }
    try {
      chrome.runtime.sendMessage({ type: "download", url: sendUrl, filename, ver: EXT_VER, pageUrl: location.href, referer: location.href, mime: mime || null, size: size || null, pairUrl: (pair && pair.url) || null, pairMime: (pair && pair.mime) || null, quality: quality || null, dur: dur || null, audioExpected, visitorData: ytVisitorData() }, (res) => {
      if (chrome.runtime.lastError) {
        if (ctxInvalidated(chrome.runtime.lastError)) markDead();
        toast("Download Craft: " + chrome.runtime.lastError.message);
        return;
      }
      if (!res || !res.ok) {
        toast("Download Craft: " + ((res && res.error) || "app not reachable"));
      }
      });
    } catch (e) {
      if (ctxInvalidated(e)) markDead();
      toast("Download Craft: " + (e && e.message ? e.message : e));
    }
  }

  // ---------- naming ----------
  // A direct media URL often carries the release name
  // ("...Pooja.Meri.Jaan.2026.1080p.x264.AAC.Hin.BTs.VTL.mp4") while the page
  // title is just "Video", "Watch online" or the site name; on other sites
  // the reverse is true (hashed CDN names). Pick whichever one describes the
  // file, never a bare "video.mp4".
  const URL_DECODE = (s) => { try { return decodeURIComponent(s); } catch (e) { return s; } };
  function urlBasename(u) {
    try {
      const p = new URL(u, location.href).pathname;
      const last = p.split("/").filter(Boolean).pop() || "";
      return URL_DECODE(last);
    } catch (e) { return ""; }
  }
  function pageTitle() {
    const clean = (t) =>
      String(t || "")
        .replace(/\s+[-–—|]\s+[^-–—|]{1,40}$/, "")
        .trim();
    // The player shell is often a same-origin iframe inside the real page
    // (dooplay embeds /jwplayer/): prefer the top document's og:title so the
    // download is named for the movie/release, not the player.
    try {
      if (window.top && window.top !== window && window.top.document) {
        const td = window.top.document;
        const og = td.querySelector('meta[property="og:title"]');
        const t = clean((og && og.getAttribute("content")) || td.title);
        if (t) return t;
      }
    } catch (e) { /* cross-origin parent: fall through */ }
    let t = "";
    try {
      const og = document.querySelector('meta[property="og:title"]');
      t = (og && og.getAttribute("content")) || "";
    } catch (e) {}
    if (!t) t = String(document.title || "");
    t = t.trim();
    // "Title - Site" / "Title | Site": the suffix is chrome, not the name.
    t = clean(t) || String(document.title || "").trim();
    return isMetricTitle(t) ? "" : t;
  }
  const GENERIC_TITLE = /^(video|videos|media|stream|player|watch|download|index|untitled|movie|online\s+video|full\s+video)$/i;
  // A title that is only a compact counter ("50K views", "6.6K likes") is
  // page chrome, not a name. A real numeric title ("50k", "6.6k") is kept.
  const METRIC_TITLE = /^(?:no|[\d.,]+\s*[KMB]?)\s+(?:views?|likes?|subscribers?|comments?|shares?)\s*$/i;
  function isMetricTitle(t) {
    return METRIC_TITLE.test(String(t || "").replace(/\s+/g, " ").trim());
  }
  const BORING_STEM = /^(videoplayback|index|file|media|stream|download|video|play|master|playlist|manifest|chunk|segment|init|frag\d*)$/i;
  // The caller has already resolved the title (authoritative player metadata
  // wins over page chrome), so only make it filesystem-safe here: a real
  // video titled "50 Views" keeps its name.
  function safeFilename(title, id) {
    const fallback = id ? `youtube_${id}` : "youtube_video";
    const s = String(title || fallback).replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
    const base = s.replace(/\.+$/, "") || fallback;
    return base.endsWith(".mp4") ? base : base + ".mp4";
  }
  // Extensions the desktop side supplies from the MIME type: a title that
  // already ends in one must not produce "name.mp4.mp4".
  const MEDIA_EXT_RE = /\.(mp4|mkv|webm|mov|avi|m4v|mp3|m4a|aac|ogg|wav|flac|m3u8|mpd|ts|m4s)$/i;
  // URL -> name stem. When the last segment is a generic playlist/segment
  // name (index.m3u8, master.mpd, seg1.ts) the release name is the folder
  // above it, which is what the site (and IDM) use.
  function urlStem(u) {
    let segs = [];
    try {
      segs = new URL(u, location.href).pathname.split("/").filter(Boolean);
    } catch (e) { return { stem: "", hasExt: false, fromFolder: false }; }
    if (!segs.length) return { stem: "", hasExt: false, fromFolder: false };
    const last = URL_DECODE(segs[segs.length - 1]);
    const m = last.match(/\.([a-z0-9]{2,5})$/i);
    const stem = m ? last.slice(0, -m[0].length) : last;
    if (BORING_STEM.test(stem) && segs.length >= 2) {
      return { stem: URL_DECODE(segs[segs.length - 2]), hasExt: false, fromFolder: true };
    }
    return { stem, hasExt: !!m, fromFolder: false };
  }
  function mediaBase(mediaUrl) {
    const title = pageTitle();
    const info = urlStem(mediaUrl || "");
    const stem = info.stem;
    const generic = !title || GENERIC_TITLE.test(title) || title.length < 4;
    // A descriptive URL name wins over a generic title, and over a title
    // that is clearly shorter than the name itself. Folder names behind a
    // playlist qualify too (the release name, not a hashed CDN segment).
    const descriptive =
      stem.length >= 8 && !BORING_STEM.test(stem) && (info.hasExt || info.fromFolder);
    if (descriptive && (generic || stem.length > title.length + 8)) {
      return stem.slice(0, 80);
    }
    const base = (title || stem || "video")
      .replace(MEDIA_EXT_RE, "")
      .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
      .trim()
      .replace(/\.+$/, "");
    return (base || "video").slice(0, 80);
  }
  // Media extension for a URL, mapped to what the engine will actually write
  // (an HLS/DASH playlist assembles to MP4).
  function outExtFor(u) {
    const m = /\.([a-z0-9]{2,5})(?:\?|#|$)/i.exec(String(u || "").split("#")[0]);
    const e = m ? "." + m[1].toLowerCase() : "";
    if (e === ".m3u8" || e === ".mpd") return ".mp4";
    return e;
  }
  // Player wrapper pages carry the real stream in a query parameter
  // ("/jwplayer/?source=https%3A%2F%2F..."). Decode it so the download is the
  // media, not the HTML shell. Also used by the desktop engine.
  const UNWRAP_KEYS = ["source", "src", "file", "url", "video", "stream", "media", "link", "path"];
  const MEDIA_HINT_RE = /\.(m3u8|mpd|mp4|webm|mkv|mov|m4v|ts|m4s|mp3|m4a|aac|flac|ogg|wav|avi)(\?|#|$)/i;
  const PLAYER_HINT_RE = /(player|jwplayer|embed|iframe|watch|playlist)/i;
  function unwrapMediaUrl(raw) {
    try {
      let cur = String(raw || "");
      if (!cur) return "";
      let changed = false;
      for (let depth = 0; depth < 3; depth++) {
        // Already a media file/playlist: nothing to unwrap.
        if (MEDIA_HINT_RE.test(cur.split("#")[0])) break;
        const u = new URL(cur, location.href);
        let next = "";
        for (const k of UNWRAP_KEYS) {
          let v = u.searchParams.get(k);
          if (!v) continue;
          if (!/^https?:\/\//i.test(v)) v = URL_DECODE(v);
          if (!/^https?:\/\//i.test(v) || v === cur) continue;
          if (MEDIA_HINT_RE.test(v.split("#")[0]) || PLAYER_HINT_RE.test(cur)) {
            next = v;
            break;
          }
        }
        if (!next) break;
        cur = next;
        changed = true;
      }
      return changed ? cur : "";
    } catch (e) { return ""; }
  }

  // ---------- Facebook reels support (IDM-style quality rows) ----------
  // Facebook is an SPA: switching reels swaps /reel/<id> with no reload and
  // the player holds a blob: URL, so the real file URL only exists in
  // network traffic + embedded page JSON. IDM shows per-quality rows with
  // title/duration/quality/size — this rebuilds the same from:
  //   title    <- og:description / og:title (the reel caption)
  //   HD/SD    <- playable_url_quality_hd / playable_url (/hd_src/sd_src,
  //               browser_native_hd/sd_url) in the page JSON
  //   live set <- background-sniffed fbcdn/mp4 traffic for this tab
  //   duration <- duration_in_ms-style keys in the page JSON
  function fbUnescape(s) {
    try {
      return String(s || "")
        .replace(/\\\//g, "/")
        .replace(/\\u([\da-fA-F]{4})/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
        .replace(/&amp;/g, "&");
    } catch (e) { return String(s || ""); }
  }
  function fbMeta(prop) {
    try {
      const el = document.querySelector(`meta[property="${prop}"]`);
      const c = el && el.getAttribute("content");
      return c ? fbUnescape(c) : "";
    } catch (e) { return ""; }
  }
  // Thumbnails: a reel page often has no og:image, and after an SPA
  // navigation the embedded JSON still describes the page that loaded first —
  // that left the sender panel on the gradient placeholder. The live player
  // is the reliable source, so candidates are collected with a source weight
  // and ranked by fbPickThumb (pure, unit-tested).
  function fbJsonUrlNear(html, key) {
    const p = String(html || "").indexOf('"' + key + '"');
    if (p < 0) return "";
    const seg = String(html).slice(p, p + 500);
    const m = seg.match(/https?:\\?\/\\?\/[^"\\\s]+/);
    return m ? fbUnescape(m[0]) : "";
  }
  // Facebook's own UI sprites are never a video thumbnail, and neither are
  // emoji images or SVGs.
  function fbThumbOk(u) {
    if (!/^https?:\/\//i.test(u || "")) return false;
    return !/rsrc\.php|static\.xx\.fbcdn\.net|\/emoji\/|\.svg(\?|$)/i.test(u);
  }
  // c = {url, kind, dist, w, h}: kind is the source weight (poster 1 ...
  // embedded JSON 0.4), dist the pixel distance from the player.
  function fbThumbScore(c) {
    if (!c || !fbThumbOk(c.url)) return -1e9;
    let s = (Number(c.kind) || 0) * 100;
    if (/fbcdn\.net|fbsbx\.com|cdninstagram\.com/i.test(c.url)) s += 25;
    if (/\/(t15|t39|t45)[./]/i.test(c.url)) s += 18; // FB video thumbnails
    const area = (Number(c.w) || 0) * (Number(c.h) || 0);
    if (area > 0) s += Math.min(20, Math.log10(area) * 4);
    if (c.dist != null) s -= Math.min(30, Number(c.dist) * 0.15);
    return s;
  }
  function fbPickThumb(list) {
    let best = "";
    let bs = 0;
    for (const c of (list || [])) {
      const s = fbThumbScore(c);
      if (s > bs) { bs = s; best = c.url; }
    }
    return best;
  }
  function fbRectDist(r, video) {
    try {
      if (!r || !video || !video.getBoundingClientRect) return null;
      const v = video.getBoundingClientRect();
      const dx = Math.max(0, v.left - r.right, r.left - v.right);
      const dy = Math.max(0, v.top - r.bottom, r.top - v.bottom);
      return Math.round(Math.hypot(dx, dy));
    } catch (e) { return null; }
  }
  // Walk up from the player to the smallest box that still holds it, so the
  // backdrop scan stays cheap (never the whole document).
  function fbPlayerHull(video) {
    try {
      let el = video;
      for (let i = 0; i < 8 && el && el.parentElement; i++) {
        el = el.parentElement;
        const r = el.getBoundingClientRect();
        if (r.width > window.innerWidth * 0.92 || r.height > window.innerHeight * 0.98) break;
      }
      return el || document.body;
    } catch (e) { return document.body; }
  }
  function fbThumbCandidates(html, video, current) {
    const out = [];
    const seen = new Set();
    const add = (url, kind, dist, w, h) => {
      if (!fbThumbOk(url) || seen.has(url)) return;
      seen.add(url);
      out.push({ url, kind, dist: dist == null ? null : dist, w: w || 0, h: h || 0 });
    };
    // 1. The player's own poster. Other videos' posters never count: in a
    //    post window the feed reel behind it is still mounted, and its
    //    poster won over the post's own picture.
    try {
      if (video) add(video.getAttribute("poster") || video.poster || "", 1, 0, 0, 0);
    } catch (e) { /* ignore */ }
    // 2. Images and CSS backdrops beside the player (Facebook paints the reel
    //    poster as a background-image on a div when there is no <img>).
    try {
      const hull = fbPlayerHull(video);
      const els = hull.querySelectorAll("img[src], img[data-src], div, span");
      const max = Math.min(els.length, 400);
      for (let i = 0; i < max; i++) {
        const el = els[i];
        const r = el.getBoundingClientRect ? el.getBoundingClientRect() : null;
        if (!r || r.width < 80 || r.height < 60) continue;
        const dist = fbRectDist(r, video);
        // Only what sits on the player itself: avatars and neighbouring
        // posts' pictures beside it are not this video.
        if (dist !== 0) continue;
        if (el.tagName === "IMG") {
          const u = el.currentSrc || el.src || el.getAttribute("data-src") || "";
          add(u, 0.8, dist, el.naturalWidth || r.width, el.naturalHeight || r.height);
          continue;
        }
        const cs = window.getComputedStyle ? window.getComputedStyle(el) : null;
        const m = String((cs && cs.backgroundImage) || (el.getAttribute("style") || ""))
          .match(/url\(["']?(https?:\/\/[^"')]+)/i);
        if (m) add(m[1], 0.8, dist, r.width, r.height);
      }
    } catch (e) { /* ignore */ }
    // 3. Page metas, then 4. whatever the embedded JSON happens to hold. Both
    //    describe the page that loaded first, so only while it is still this
    //    page: after a reel swap they are the previous reel's poster.
    if (!current || fbInOverlay(video)) return out;
    add(fbMeta("og:image:secure_url"), 0.6, null, 0, 0);
    add(fbMeta("og:image"), 0.6, null, 0, 0);
    add(fbMeta("twitter:image"), 0.5, null, 0, 0);
    for (const k of ["preferred_thumbnail", "thumbnailImage", "thumbnail_src", "thumbnail", "image"]) {
      add(fbJsonUrlNear(html, k), 0.4, null, 0, 0);
    }
    return out;
  }
  // The video sits in a post window or other layer opened over the page:
  // the page's metas and JSON then describe the page underneath.
  function fbInOverlay(video) {
    try { return !!(video && video.closest && video.closest('[role="dialog"],[aria-modal="true"]')); } catch (e) { return false; }
  }
  // A still of what the player is showing right now (or its first frame):
  // the one picture that is certainly this video. Null when the browser
  // will not hand the pixels over (a cross-origin, tainted video).
  function fbFrame(video) {
    try {
      if (!video || video.readyState < 2 || !(video.videoWidth > 0)) return "";
      const w = 320, h = Math.max(1, Math.round(320 * video.videoHeight / video.videoWidth));
      const c = document.createElement("canvas");
      c.width = w; c.height = h;
      c.getContext("2d").drawImage(video, 0, 0, w, h);
      return c.toDataURL("image/jpeg", 0.8);
    } catch (e) { return ""; }
  }
  function fbJsonUrls(html) {
    const out = {};
    const grab = (key) => {
      // "key":"https:\/\/..." or "key": "https://..."
      const re = new RegExp(`"${key}"\\s*:\\s*"((?:\\\\.|[^"\\\\])*)"`);
      const m = html.match(re);
      const u = m && fbUnescape(m[1]);
      return u && /^https?:\/\//i.test(u) ? u : "";
    };
    out.hd = grab("playable_url_quality_hd") || grab("browser_native_hd_url") || grab("hd_src");
    out.sd = grab("playable_url") || grab("browser_native_sd_url") || grab("sd_src");
    return out;
  }
  function fbJsonNum(html, keys) {
    for (const k of keys) {
      const m = html.match(new RegExp(`"${k}"\\s*:\\s*"?([\\d.]+)`));
      if (m) {
        const n = parseFloat(m[1]);
        if (n > 0 && n < 10000000) return n;
      }
    }
    return 0;
  }
  // Captions: IDM names the download after the reel's description, so the
  // caption matters as much as the file URL. Same staleness problem as the
  // thumbnail — prefer the live DOM and rank by source weight, so a meta left
  // over from the first page load never beats what is on screen now.
  function fbCleanCaption(s) {
    let t = String(s == null ? "" : s);
    try { t = t.normalize("NFC"); } catch (e) { /* ignore */ }
    t = t.replace(/\r?\n/g, " ").replace(/\s+/g, " ").trim();
    t = t.replace(/^\(\d+\)\s*/, "");
    t = t.replace(/\s*[|·]\s*Facebook\s*$/i, "").replace(/\s+on Facebook\b.*$/i, "").trim();
    // The collapsed caption's "… See more" link is page chrome, not title.
    t = t.replace(/\s*(?:…|\.\.\.)?\s*See more\s*$/i, "").trim();
    return t;
  }
  // Site chrome, audio lines and counters are not a caption.
  function fbCaptionUsable(t) {
    if (!t || t.length < 12) return false;
    const low = t.toLowerCase();
    if (/^(facebook|watch|reels|video|log ?in|sign ?up|see more|more)$/.test(low)) return false;
    if (/^(see posts, photos and more|log in to facebook|content not available|this content isn)/.test(low)) return false;
    if (/^♪/.test(t) || /original audio$/i.test(t)) return false;
    if (/(^|\s)[·•]\s*follow(ed|ing)?$/i.test(t)) return false;
    // A post's byline ("AI content · Name", "2h · Shared with Public").
    if (/^[^·•]{0,40}\s[·•]\s/.test(t) && t.length < 90) return false;
    if (/^\d[\d.,\s]*[km]?$/i.test(t)) return false;
    return true;
  }
  // c = {text, kind}: the strongest source wins, then the longer text.
  function fbPickCaption(list) {
    let best = "";
    let bs = 0;
    for (const c of (list || [])) {
      const t = fbCleanCaption(c && c.text);
      if (!fbCaptionUsable(t)) continue;
      const s = (Number(c && c.kind) || 0) * 100 + Math.min(t.length, 200);
      if (s > bs) { bs = s; best = t; }
    }
    return best.slice(0, 200);
  }
  // Every "message":{"text":"..."} value: the story message is the caption on
  // a fresh load, but comments match too, so keep several and let the ranker
  // choose.
  function fbJsonTexts(html, max) {
    const out = [];
    const s = String(html || "");
    const ws = (k) => { while (k < s.length && /\s/.test(s[k])) k++; return k; };
    let i = 0;
    while (out.length < (max || 6)) {
      const p = s.indexOf('"message"', i);
      if (p < 0) break;
      let j = ws(p + 9);
      if (s[j] === ":") j = ws(j + 1);
      if (s[j] === "{" && s.startsWith('"text"', j + 1)) {
        j = ws(j + 7);
        if (s[j] === ":") j = ws(j + 1);
        if (s[j] === '"') {
          j++;
          let raw = "";
          while (j < s.length) {
            const ch = s[j];
            if (ch === "\\") { raw += ch + (s[j + 1] || ""); j += 2; continue; }
            if (ch === '"') break;
            raw += ch;
            j++;
          }
          out.push(fbUnescape(raw));
          i = j + 1;
          continue;
        }
      }
      i = p + 9;
    }
    return out;
  }
  // The caption Facebook is rendering right now: the longest real sentence
  // near the player (author line, audio line and counters lose).
  function fbDomCaption(video) {
    try {
      // The post (or window) that holds this video, not the whole page:
      // the feed behind a post window and its sidebars name other things.
      const scope = (video && video.closest && video.closest('[role="dialog"],[role="article"],[aria-posinset]')) || document;
      // Facebook marks the post's own text: that is the caption when present.
      const msg = scope.querySelector && scope.querySelector('[data-ad-preview="message"],[data-ad-comet-preview="message"]');
      if (msg) {
        const t = fbCleanCaption(msg.textContent);
        if (fbCaptionUsable(t)) return t;
      }
      const sel = 'div[dir="auto"], span[dir="auto"], h1, h2, h3';
      const nodes = scope.querySelectorAll(sel);
      const v = video && video.getBoundingClientRect ? video.getBoundingClientRect() : null;
      let best = "";
      let bs = 0;
      for (const n of nodes) {
        if (n.querySelector && n.querySelector(sel)) continue; // leaf texts only
        const t = fbCleanCaption(n.textContent);
        if (!fbCaptionUsable(t)) continue;
        const r = n.getBoundingClientRect ? n.getBoundingClientRect() : null;
        if (!r || r.width < 60 || r.height < 10) continue;
        let s = Math.min(t.length, 180);
        if (v) {
          const d = fbRectDist(r, video);
          if (d != null) s -= Math.min(60, d * 0.2);
        }
        if (s > bs) { bs = s; best = t; }
      }
      return best;
    } catch (e) { return ""; }
  }
  function fbCaptionCandidates(html, video, current) {
    const out = [];
    const add = (text, kind) => { if (text) out.push({ text, kind }); };
    add(fbDomCaption(video), 1); // what the user is looking at
    // The metas/title/JSON come from the page that loaded: after a reel swap
    // they would name the new file after the reel the user left.
    if (current) {
      add(fbMeta("og:description"), 0.9);
      add(fbMeta("og:title"), 0.7);
      for (const t of fbJsonTexts(html, 6)) add(t, 0.55);
      add(document.title, 0.3);
    }
    return out;
  }
  // The reel on screen changes with no page load: /reel/<id> swaps through the
  // history API while Facebook keeps the same <video> node, and the page's
  // embedded JSON and the tab's sniffed requests both still describe the reel
  // the user scrolled past. Everything below is therefore keyed on the current
  // reel: a key from the URL (or from the player, for feeds whose URL has no
  // id), a generation counter, and the moment the current reel started. That is
  // what stops the previous reel's playable_url - and its sniffed video/audio
  // legs - from being sent as "the" download.
  let fbReelKey = "";
  let fbReelGen = 0;
  let fbReelAt = 0;
  // The page's own JSON/meta describe the page that loaded FIRST, so they are
  // only trustworthy while the document itself has not navigated since.
  let fbPageUrl = (typeof location !== "undefined" && location && location.href) || "";

  function fbReelKeyOf(href) {
    const s = String(href || "");
    const m = s.match(/\/reel\/(\d+)/i)
      || s.match(/\/videos\/(?:[^/?#]+\/)?(\d+)/i)
      || s.match(/[?&]v=(\d+)/i);
    return m ? m[1] : "";
  }

  // Which <video> is the user actually looking at? Facebook keeps the previous
  // and next reels mounted and preloaded, so document.querySelector("video") is
  // usually NOT the one on screen - that is how a scrolled-past reel's caption,
  // poster and dimensions ended up in the panel.
  function fbPickVideo(list, vw, vh) {
    let best = null;
    let bs = -1;
    const width = Number(vw) > 0 ? Number(vw) : 0;
    const height = Number(vh) > 0 ? Number(vh) : 0;
    for (const v of Array.from(list || [])) {
      let r = null;
      try { r = v && v.getBoundingClientRect ? v.getBoundingClientRect() : null; } catch (e) { r = null; }
      if (!r) continue;
      const visW = Math.min(r.right, width) - Math.max(r.left, 0);
      const visH = Math.min(r.bottom, height) - Math.max(r.top, 0);
      if (!(visW > 40 && visH > 40)) continue;
      let s = visW * visH;
      try { if (v.paused === false) s += 1e7; } catch (e) { /* ignore */ }
      try { if (v.videoWidth > 0) s += 1e5; } catch (e) { /* ignore */ }
      if (s > bs) { bs = s; best = v; }
    }
    return best;
  }

  function fbActiveVideo() {
    try {
      return fbPickVideo(
        document.querySelectorAll("video"),
        (window && window.innerWidth) || 0,
        (window && window.innerHeight) || 0
      );
    } catch (e) { return null; }
  }

  // Preloaded neighbours differ in poster/blob src, so this changes when the
  // reel does even on pages whose URL does not.
  function fbVideoKey(v) {
    try {
      if (!v) return "";
      const p = v.getAttribute("poster") || v.poster || "";
      const s = v.currentSrc || v.src || "";
      return `${p}|${String(s).slice(0, 120)}`;
    } catch (e) { return ""; }
  }

  // Called before anything is read so the generation is current. A no-op when
  // the reel has not changed.
  function fbSyncReel() {
    try {
      const key = fbReelKeyOf(location.href) || fbVideoKey(fbActiveVideo());
      if (key && key !== fbReelKey) {
        fbReelKey = key;
        fbReelGen++;
        fbReelAt = Date.now();
      }
      return fbReelKey;
    } catch (e) { return fbReelKey; }
  }

  // The embedded JSON and the metas describe the first load: once the reel
  // changes they name the reel the user came from.
  function fbJsonIsCurrent() {
    try { return !!fbPageUrl && location.href === fbPageUrl; } catch (e) { return false; }
  }

  // A DASH audio leg arrives as a plain .mp4, so the sniffer's content type is
  // the only reliable signal for it. Getting this wrong offered an audio-only
  // stream as a video row, which saved "with audio and no video".
  function fbAudioItem(m) {
    const mime = String((m && m.mime) || "").toLowerCase();
    if (mime.indexOf("audio/") === 0) return true;
    if (mime.indexOf("video/") === 0) return false;
    return /\.(m4a|mp3|aac|ogg|opus|weba)(\?|#|$)/i.test(String((m && m.url) || ""));
  }

  // What the player fetched for the reel on screen, newest first. The tab's
  // list keeps every reel of the session, so recency is what keeps the current
  // reel ahead of the one before it; `since` additionally drops traffic that
  // predates the current reel when that moment is known. Never returns empty
  // because of `since`: an empty list would send the page URL instead of the
  // reel's own media.
  function fbReelMedia(items, since) {
    const list = (Array.isArray(items) ? items : [])
      .filter((m) => m && /^https?:\/\//i.test(m.url || ""));
    const fresh = Number(since) > 0
      ? list.filter((m) => !m.seenAt || Number(m.seenAt) >= Number(since))
      : [];
    const use = fresh.length ? fresh : list;
    return use.slice().sort((a, b) => (Number(b.seenAt) || 0) - (Number(a.seenAt) || 0));
  }

  // The Facebook video ids a <video> in the feed belongs to, nearest first:
  // its post links to it (/reel/<id>, /videos/<id>, /watch/?v=<id>). The walk
  // stops at the first ancestor that holds another video too, so a
  // neighbouring post's links are never taken. [{ id, href }].
  function fbIdsNear(v, maxUp) {
    const out = [];
    let el = v;
    for (let i = 0; el && i < (maxUp || 25); i++) {
      el = el.parentElement;
      if (!el || !el.querySelectorAll) break;
      let vids = [];
      try { vids = Array.from(el.querySelectorAll("video")); } catch (e) { vids = []; }
      if (vids.some((x) => x !== v)) break;
      let links = [];
      try { links = Array.from(el.querySelectorAll("a[href]")); } catch (e) { links = []; }
      for (const a of links) {
        const href = String((a.getAttribute && a.getAttribute("href")) || a.href || "");
        const id = fbReelKeyOf(href);
        if (id && !out.some((x) => x.id === id)) out.push({ id, href });
      }
      // The first box with a video link is the post: going higher would
      // reach the feed and its other posts.
      if (out.length) break;
    }
    return out;
  }

  // A link yt-dlp (in the app) can download one video from.
  function fbPermalink(id, href) {
    if (/\/reel\//i.test(String(href || ""))) return `https://www.facebook.com/reel/${id}`;
    return `https://www.facebook.com/watch/?v=${id}`;
  }

  // The stored videos reordered for the <video> the user clicked: its own id
  // (from its post's links) first, else the only stored video as long as it.
  // `known` = true when the clicked video was identified at all; when its id
  // is known but its data has not arrived, `missing` names it, so the caller
  // waits for it instead of offering a neighbour.
  function fbOrderFor(list, ids, dur) {
    const all = Array.isArray(list) ? list : [];
    const idList = (ids || []).map((x) => x.id);
    let hit = null;
    for (const id of idList) {
      hit = all.find((e) => e.id === id);
      if (hit) break;
    }
    if (hit) return { list: [hit, ...all.filter((e) => e !== hit)], known: true, missing: "" };
    if (idList.length) return { list: [], known: true, missing: idList[0] };
    const d = Number(dur);
    if (Number.isFinite(d) && d > 0) {
      const same = all.filter((e) => e.dur > 0 && Math.abs(e.dur - d) <= 1.5);
      if (same.length === 1) return { list: [same[0], ...all.filter((e) => e !== same[0])], known: true, missing: "" };
    }
    return { list: all, known: false, missing: "" };
  }

  function fbInfo(clicked) {
    fbSyncReel();
    let html = "";
    try { html = document.documentElement.innerHTML.slice(0, 1500000); } catch (e) {}
    // The video whose button was clicked; else the reel on screen, not the
    // first <video> the document happens to hold: Facebook keeps the
    // neighbours mounted and preloaded.
    const video = clicked || fbActiveVideo() || document.querySelector("video");
    // Only while the page still describes itself (see fbJsonIsCurrent).
    const current = fbJsonIsCurrent();
    const urls = current ? fbJsonUrls(html) : { hd: "", sd: "" };
    // Duration: the live player describes the reel on screen; the embedded
    // JSON is the fallback for a page that has not navigated.
    let durSec = 0;
    try {
      const d = video && video.duration;
      if (Number.isFinite(d) && d > 0 && d < 36000) durSec = Math.round(d);
    } catch (e) { durSec = 0; }
    if (!durSec && current) {
      const durMs = fbJsonNum(html, ["playable_duration_in_ms", "duration_in_ms", "length_in_ms"]);
      durSec = durMs > 0 ? Math.round(durMs / 1000)
        : Math.round(fbJsonNum(html, ["videoDuration", "video_length", "duration", "length_in_seconds"]) || 0);
      if (!(durSec > 0 && durSec < 36000)) durSec = 0;
    }
    // Dimensions: the live player first (it is the reel on screen, and it is
    // what labels the quality), then the metas/JSON while they are still about
    // this page.
    let w = 0;
    let h = 0;
    try { w = Math.round(video && video.videoWidth) || 0; h = Math.round(video && video.videoHeight) || 0; } catch (e) { w = 0; h = 0; }
    if (current) {
      if (!(w > 0)) w = parseInt((document.querySelector('meta[property="og:video:width"]') || {}).content, 10) || 0;
      if (!(h > 0)) h = parseInt((document.querySelector('meta[property="og:video:height"]') || {}).content, 10) || 0;
      if (!(w > 0)) w = Math.round(fbJsonNum(html, ["original_width", "video_width", "width"]) || 0);
      if (!(h > 0)) h = Math.round(fbJsonNum(html, ["original_height", "video_height", "height"]) || 0);
    }
    // Title: the reel description — that is what names the saved file.
    const title = fbPickCaption(fbCaptionCandidates(html, video, current)) || "facebook_video";
    return {
      hd: urls.hd,
      sd: urls.sd,
      durSec,
      w,
      h,
      title,
      current,
      since: fbReelAt,
      key: fbReelKey,
      // The player's own poster, then a frame of the video itself, then
      // pictures on the player; the page's own image last.
      thumb: (() => {
        let own = "";
        try { own = video && (video.getAttribute("poster") || video.poster) || ""; } catch (e) { own = ""; }
        if (fbThumbOk(own)) return own;
        return fbFrame(video) || fbPickThumb(fbThumbCandidates(html, video, current));
      })(),
    };
  }
  function fbQuality(w, h, isHd) {
    // The label counts lines, so a portrait reel (1080x1920) and a landscape
    // one (1920x1080) are both 1080p — take the shorter side.
    const q = w > 0 && h > 0 ? Math.min(w, h) : Math.max(w || 0, h || 0);
    if (q >= 1080) return "1080p HD";
    if (q >= 720) return "720p HD";
    if (q >= 480) return "480p";
    if (q > 0) return `${q}p`;
    return isHd ? "HD" : "SD";
  }
  // The numeric cap the desktop app needs from a panel label: "720p HD" →
  // "720p" (yt-dlp then picks the best stream at or below it). "HD"/"SD"
  // carry no number, so the app falls back to best rather than guessing.
  function fbQualityCap(q) {
    const m = String(q || "").match(/(\d{3,4})p/);
    return m ? `${m[1]}p` : "";
  }
  function fbFilename(title) {
    const s = String(title || "facebook_video").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
    const base = (s.replace(/\.+$/, "") || "facebook_video").slice(0, 80);
    return base.endsWith(".mp4") ? base : base + ".mp4";
  }
  // The rows the reels panel offers. The page's own HD/SD encodes lead while
  // they still describe this page; after a scroll they are dropped and the band
  // captured legs of the reel on screen lead instead. Audio legs become chips
  // and one of them pairs a video-only leg.
  function fbMenuRows(info, items) {
    const seen = new Set();
    const rows = [];
    const audio = [];
    const fname = fbFilename(info.title);
    const dur = info.durSec > 0 ? `, ${info.durSec} sec` : "";
    const push = (url, quality, fromJson, size, thumb) => {
      if (!url || !/^https?:\/\//i.test(url) || seen.has(url)) return;
      seen.add(url);
      rows.push({
        url,
        fname,
        q: fbQualityCap(quality),
        label: `⬇ ${info.title.slice(0, 40)}, MP4 file${dur}, quality ${quality}`,
        _size: size || 0,
        dur: info.durSec || null,
        thumb: thumb || info.thumb || "",
        fromJson: !!fromJson,
      });
    };
    // P4: when a playlist is on the tab, loose segments are its children
    // (ad slivers included) — offer the playlist, never the parts.
    const R = (typeof DCRank !== "undefined") ? DCRank : null;
    const isPlaylist = (u) => R ? R.kindOf({ url: u }) === "playlist" : /m3u8|mpd|mpegurl|dash/i.test(String(u));
    const isSeg = (u) => R ? R.kindOf({ url: u }) === "segment" : /(\.ts|\.m4s|\.cmfv|\.cmfa)(\?|#|$)/i.test(String(u).split("?")[0]);
    const hasPlaylist = (Array.isArray(items) ? items : []).some((m) => m && isPlaylist(m.url));
    // Subtitle files (.srt/.vtt, captured at any size) are never "MP4 file"
    // rows: a caption could become the only row and be sent as the video.
    const live = fbReelMedia(items, info.since).filter((m) => !isPlaylist(m.url)
      && !(R && R.kindOf(m) === "subtitle"));
    const liveVideo = live.filter((m) => !fbAudioItem(m) && !(hasPlaylist && isSeg(m.url)));
    const liveAudio = live.filter((m) => fbAudioItem(m) && !isSeg(m.url));
    const liveQ = fbQuality(info.w, info.h, true);
    // The metas/JSON always describe the reel that was on screen when the page
    // loaded, so after a scroll they name the reel the user came from and must
    // not be offered at all. While the page still describes itself they are the
    // page's own HD/SD encodes, which stay ahead of the captured legs.
    if (info.current) {
      // The HD and SD URL often resolve to the same "1080p HD" text — label the
      // lower encode plainly instead of showing the same resolution twice.
      const hdQ = fbQuality(info.w, info.h, true);
      let sdQ = fbQuality(info.w, info.h, info.hd === info.sd);
      if (info.hd && info.sd && info.hd !== info.sd && sdQ === hdQ) sdQ = "SD";
      if (info.hd && info.hd !== info.sd) push(info.hd, hdQ, true);
      if (info.sd) push(info.sd, sdQ, true);
    }
    // This used to be pushed *after* the page JSON, so a scrolled-past reel's
    // URL took the first row - the "it downloads the previous video" report.
    for (const m of liveVideo.slice(0, 4)) push(m.url, liveQ, false, m.size, m.thumb);
    // Audio-only traffic from the same playback session becomes an "Other
    // Files" chip (same idea as YouTube's audio-only tracks).
    for (const m of liveAudio.slice(0, 3)) {
      audio.push({ url: m.url, mime: m.mime || "audio/mp4", size: m.size || 0 });
    }
    // Facebook serves its higher qualities as a video-only DASH leg plus a
    // separate audio leg: a captured video row would otherwise save silently.
    // Hand the newest audio leg of the same reel along so the app merges them
    // (it already merges the page URL it keeps alongside the row).
    const pair = liveVideo.length && liveAudio.length
      ? { url: liveAudio[0].url, mime: liveAudio[0].mime || "audio/mp4" }
      : null;
    return { rows, audio, pair };
  }

  // Whole videos from Facebook's own JSON (DCRank.fbVideos): every quality
  // as one file, its audio track, duration and thumbnail - instead of the
  // dozens of byte-range pieces the sniffer sees per reel. Filled from the
  // page's script tags on first use and from the GraphQL answers page-hook.js
  // relays as reels load. Newest data last (Map order), capped.
  const fbStore = new Map();
  let fbDocScanned = false;
  function fbAddVideos(text) {
    if (typeof DCRank === "undefined" || !DCRank.fbVideos) return;
    for (const v of DCRank.fbVideos(text, [...fbStore.keys()])) {
      const old = fbStore.get(v.id);
      fbStore.delete(v.id);
      fbStore.set(v.id, old ? {
        ...old,
        dur: v.dur || old.dur,
        thumb: v.thumb || old.thumb,
        video: v.video.length ? v.video : old.video,
        audio: v.audio.length ? v.audio : old.audio,
        progressive: v.progressive.length ? v.progressive : old.progressive,
      } : v);
    }
    while (fbStore.size > 40) fbStore.delete(fbStore.keys().next().value);
  }
  function fbScanDocument() {
    if (fbDocScanned) return;
    fbDocScanned = true;
    // The quality lists and each video's duration/SD-HD/thumbnail sit in
    // different script tags, so the relevant ones are read as one text.
    try {
      const keys = ["all_video_dash_prefetch_representations", '"length_in_second"',
        '"playable_duration_in_ms"', '"progressive_urls"', '"preferred_thumbnail"'];
      const parts = [];
      for (const s of document.scripts) {
        const t = s.textContent || "";
        if (keys.some((k) => t.indexOf(k) >= 0)) parts.push(t);
      }
      if (parts.length) fbAddVideos(parts.join("\n"));
    } catch (e) { /* best effort */ }
  }
  // Every file path a stored video owns (its qualities, audio, SD/HD).
  function fbPathsOf(e) {
    const R = DCRank;
    return [...e.video, ...e.audio, ...e.progressive].map((x) => R.fbPathOf(x.url));
  }
  // The stored videos, the one on screen first: the reel id in the URL, else
  // the video whose file the player fetched last.
  function fbStoredVideos(items) {
    fbScanDocument();
    const list = [...fbStore.values()].reverse();
    if (!list.length) return [];
    const seen = new Map(); // path -> newest seenAt
    for (const m of (items || [])) {
      const p = DCRank.fbPathOf(m && m.url);
      if (p) seen.set(p, Math.max(seen.get(p) || 0, Number(m.seenAt) || 1));
    }
    const lastSeen = (e) => Math.max(0, ...fbPathsOf(e).map((p) => seen.get(p) || 0));
    const urlId = fbReelKeyOf(location.href);
    const score = (e) => (e.id === urlId ? 1e15 : 0) + lastSeen(e);
    return list.map((e, i) => ({ e, s: score(e), i }))
      .sort((a, b) => (b.s - a.s) || (a.i - b.i))
      .map((x) => x.e);
  }
  // Panel rows for one stored video: each quality with its audio paired in
  // and a bitrate x duration size (shown with "~"), then the SD/HD files.
  function fbStoredRows(e, info) {
    const rows = [];
    const fname = fbFilename(info.title);
    const stem = fname.replace(/\.mp4$/i, "");
    const dur = e.dur || info.durSec || 0;
    const a = e.audio[0] || null;
    const est = (bw) => (bw > 0 && dur > 0 ? Math.round((bw * dur) / 8) : 0);
    const seenQ = new Set();
    for (const v of e.video) {
      const quality = fbQuality(v.w, v.h, true);
      if (seenQ.has(quality)) continue;
      seenQ.add(quality);
      const q = fbQualityCap(quality);
      rows.push({
        url: v.url, fname: q ? `${stem} [${q}].mp4` : fname, q,
        label: `⬇ ${info.title.slice(0, 40)}, MP4 file, quality ${quality}`,
        _size: 0, _est: est(v.bw + (a ? a.bw : 0)), dur: dur || null,
        thumb: e.thumb || info.thumb || "",
        pair: a ? { url: a.url, mime: a.mime || "audio/mp4" } : null,
      });
    }
    for (const p of e.progressive) {
      const quality = p.quality === "HD" ? "HD (single file)" : "SD (single file)";
      rows.push({
        url: p.url, fname, q: "", label: `⬇ ${info.title.slice(0, 40)}, MP4 file, quality ${quality}`,
        _size: 0, _est: est(p.bw), dur: dur || null, thumb: e.thumb || info.thumb || "", fromJson: true,
      });
    }
    return rows;
  }

  // ---------- end Facebook reels support ----------
  // Pick the best directly-downloadable candidate the background sniffer
  // caught for this page: prefer real video/mp4 over playlists, biggest
  // known size first. Facebook reels render as blob: players, so this is
  // what makes the pill work without a page reload.
  // `opts.streamPlayer`: the clicked <video> plays a blob:/MediaSource source,
  // so what it plays is a stream; its playlist beats any loose file on the
  // page (an ad or preview .mp4 used to win: files outrank playlists).
  function bestSniffed(items, opts) {
    const R = (typeof DCRank !== "undefined") ? DCRank : null;
    // Subtitle side files are listed in the popup, never picked as the video.
    let list = (items || []).filter((m) => m && /^https?:\/\//i.test(m.url)
      && !(R && R.kindOf(m) === "subtitle"));
    if (!list.length) return null;
    // What the page's players really requested beats URLs a page only
    // mentioned in its JSON (a feed lists every post's .mp4).
    if (list.some((m) => m.via !== "page")) list = list.filter((m) => m.via !== "page");
    if (opts && opts.streamPlayer && R) {
      const streams = list.filter((m) => R.kindOf(m) === "playlist");
      if (streams.length) list = streams;
    }
    // P4: when the tab already holds a playlist, loose segments are its
    // children (the engine assembles them) — never pick a part. Without
    // rank.js, fall back to the legacy inline score below.
    const hasPlaylist = R
      ? list.some((m) => R.kindOf(m) === "playlist")
      : list.some((m) => /m3u8|mpd|mpegurl|dash/i.test(String(m.url || "")));
    const pool = R
      ? list.filter((m) => R.scoreMedia(m, { hasPlaylist }) >= 0)
      : list.slice();
    if (!pool.length) return null;
    const score = (m) => {
      if (R) return R.scoreMedia(m, { hasPlaylist });
      const u = String(m.url || "");
      const mime = String(m.mime || "").toLowerCase();
      if (/m3u8|mpd|mpegurl|dash/i.test(u)) return -1; // engine can't take playlists directly
      let s = 0;
      if (mime.startsWith("video/")) s += 100;
      if (/mp4/i.test(mime) || /\.mp4/i.test(u)) s += 50;
      if (/fbcdn|lookaside|facebook|instagram|cdninstagram/i.test(u)) s += 20;
      s += Math.min(30, Math.log10((m.size || 0) + 1) * 5);
      return s;
    };
    return pool.slice().sort((a, b) => score(b) - score(a))[0];
  }

  // DRM (Widevine / PlayReady / FairPlay through the browser's Encrypted
  // Media Extensions): the page decrypts inside the browser's protected
  // pipeline, so the stream's bytes are encrypted and a saved copy never
  // plays. Such videos are reported, never downloaded — the protection is
  // the rights holder's and is not worked around.
  const drmVideos = new WeakSet();
  try {
    // "encrypted" does not bubble, but a capture listener still sees it.
    document.addEventListener("encrypted", (e) => { if (e.target && e.target.tagName === "VIDEO") drmVideos.add(e.target); }, true);
  } catch (e) { /* no EME in this browser */ }
  function drmProtected(v) {
    try { return !!v && (drmVideos.has(v) || !!v.mediaKeys); } catch (e) { return false; }
  }
  const DRM_NOTE = "This video is DRM-protected, so it can't be downloaded: a saved copy would have no picture or sound. Watch it on the site instead.";

  // On-page toast, bottom center: a card in its own shadow root (the site's
  // CSS cannot reach it) with an icon, a title, the message, an optional
  // action and a bar that runs down until it closes. `toast(text)` keeps
  // working for every caller; a message asking for a tab reload becomes the
  // "updated" card with a Reload tab button.
  const TOAST_ICONS = {
    error: '<path d="M12 8v5M12 16.5v.01"/><circle cx="12" cy="12" r="9"/>',
    update: '<path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 4v7h-7"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5v.01"/>',
  };
  // A tab open across an extension update still runs the old script, whose
  // red bottom-left box (#dc-yt-toast) shows next to this card. A stylesheet
  // rule beats its inline display:block, now and whenever it shows later.
  try {
    if (!document.getElementById("dc-old-toast-off")) {
      const st = document.createElement("style");
      st.id = "dc-old-toast-off";
      st.textContent = "#dc-yt-toast{display:none!important}";
      (document.head || document.documentElement).appendChild(st);
    }
  } catch (e) { /* page without a DOM to style */ }
  function toast(text, opts) {
    opts = opts || {};
    let msg = String(text || "").replace(/^Download Craft:\s*/, "");
    let kind = opts.kind || "error";
    let title = opts.title || "Download Craft";
    let action = opts.action || null;
    if (/reload this tab/i.test(msg) && !opts.kind) {
      kind = "update";
      title = "Download Craft was updated";
      msg = "Reload this tab to keep downloading from it.";
      action = { label: "Reload tab", run: () => location.reload() };
    } else if (/already in queue/i.test(msg) && !opts.kind) {
      kind = "info";
      msg = "This download is already in the queue.";
    }
    let host = document.getElementById("dc-toast-host");
    if (!host) {
      host = document.createElement("div");
      host.id = "dc-toast-host";
      host.style.cssText = "position:fixed;left:0;right:0;bottom:24px;z-index:2147483647;display:flex;justify-content:center;pointer-events:none";
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = `<style>
        .card{pointer-events:auto;box-sizing:border-box;display:flex;align-items:center;gap:12px;
          width:max-content;max-width:min(520px,calc(100vw - 32px));padding:12px 12px 12px 14px;
          border-radius:14px;background:rgba(22,24,34,.92);backdrop-filter:blur(14px) saturate(140%);
          -webkit-backdrop-filter:blur(14px) saturate(140%);border:1px solid rgba(255,255,255,.09);
          box-shadow:0 18px 48px rgba(0,0,0,.45),0 2px 8px rgba(0,0,0,.3);color:#e9eaf2;
          font:13px/1.4 "Segoe UI",system-ui,-apple-system,sans-serif;position:relative;overflow:hidden;
          transform:translateY(16px) scale(.98);opacity:0;transition:transform .28s cubic-bezier(.2,.9,.3,1.2),opacity .2s}
        .card.on{transform:none;opacity:1}
        .ic{flex:none;width:34px;height:34px;border-radius:10px;display:grid;place-items:center}
        .ic svg{width:19px;height:19px;fill:none;stroke:currentColor;stroke-width:2;stroke-linecap:round;stroke-linejoin:round}
        .error .ic{background:rgba(255,92,92,.14);color:#ff8a8a}
        .update .ic{background:rgba(108,124,255,.16);color:#9aa6ff}
        .info .ic{background:rgba(70,200,150,.14);color:#6fe0b0}
        .tx{min-width:0;flex:1}
        .tt{font-weight:600;font-size:13.5px;color:#fff}
        .ms{color:#b9bccb;margin-top:1px;word-break:break-word}
        .act{flex:none;border:0;border-radius:9px;padding:8px 14px;font:600 12.5px "Segoe UI",system-ui,sans-serif;
          color:#fff;background:linear-gradient(135deg,#6c7cff,#8b5cf6);cursor:pointer;box-shadow:0 4px 14px rgba(108,124,255,.35)}
        .act:hover{filter:brightness(1.1)}
        .x{flex:none;width:26px;height:26px;border:0;border-radius:8px;background:transparent;color:#8d90a3;
          cursor:pointer;font-size:16px;line-height:1}
        .x:hover{background:rgba(255,255,255,.08);color:#fff}
        .bar{position:absolute;left:0;bottom:0;height:2px;width:100%;transform-origin:left;
          background:currentColor;opacity:.55}
        .error .bar{color:#ff8a8a}.update .bar{color:#9aa6ff}.info .bar{color:#6fe0b0}
        @media (prefers-color-scheme: light){
          .card{background:rgba(255,255,255,.94);color:#1b1d27;border-color:rgba(20,24,40,.1);
            box-shadow:0 18px 48px rgba(20,24,40,.18),0 2px 8px rgba(20,24,40,.08)}
          .tt{color:#12131a}.ms{color:#565a6e}.x{color:#6b6f82}.x:hover{background:rgba(20,24,40,.06);color:#12131a}
        }
      </style><div class="card" role="status" aria-live="polite"><div class="ic"><svg viewBox="0 0 24 24"></svg></div>
        <div class="tx"><div class="tt"></div><div class="ms"></div></div>
        <button class="act" hidden></button><button class="x" aria-label="Close">✕</button><div class="bar"></div></div>`;
      document.documentElement.appendChild(host);
    }
    const root = host.shadowRoot;
    const card = root.querySelector(".card");
    const close = () => { card.classList.remove("on"); clearTimeout(host.__dcT); };
    card.className = "card " + kind;
    root.querySelector("svg").innerHTML = TOAST_ICONS[kind] || TOAST_ICONS.info;
    root.querySelector(".tt").textContent = title;
    root.querySelector(".ms").textContent = msg;
    const act = root.querySelector(".act");
    act.hidden = !action;
    if (action) {
      act.textContent = action.label;
      act.onclick = () => { close(); action.run(); };
    }
    root.querySelector(".x").onclick = close;
    // An update card waits for the user; the rest leave on their own.
    const ms = kind === "update" ? 20000 : 7000;
    const bar = root.querySelector(".bar");
    bar.style.transition = "none";
    bar.style.transform = "scaleX(1)";
    clearTimeout(host.__dcT);
    requestAnimationFrame(() => {
      card.classList.add("on");
      requestAnimationFrame(() => {
        bar.style.transition = `transform ${ms}ms linear`;
        bar.style.transform = "scaleX(0)";
      });
    });
    host.__dcT = setTimeout(close, ms);
  }

  // A browser-session preference, shared through the worker (not website storage).
  let barPosition = null;
  let positionRevision = 0;
  let activeBarDrag = null;
  function validBarPosition(p) {
    return p && Number.isFinite(p.x) && Number.isFinite(p.y) && p.x >= 0 && p.x <= 1 && p.y >= 0 && p.y <= 1;
  }
  function applySavedBarPosition(host) {
    // The saved spot is a fraction of the window: just right of the player
    // in a normal window is mid-screen in fullscreen, so fullscreen always
    // anchors to the player's top-right corner instead.
    if (!barPosition || document.fullscreenElement) return false;
    const parent=document.documentElement;
    if(host.parentNode!==parent)parent.appendChild(host);
    const width=Math.max(0,Math.min(barWidth(host),window.innerWidth-16));
    // Sized by its content: a fixed width cut the bar off when the drag
    // handle appeared on hover (or a compact bar opened).
    host.style.position="fixed";host.style.width="max-content";
    host.style.right="auto";host.style.bottom="auto";
    host.style.left=(8+barPosition.x*Math.max(0,window.innerWidth-width-16))+"px";
    host.style.top=(8+barPosition.y*Math.max(0,window.innerHeight-44))+"px";
    return true;
  }
  function positionDraggedPanel(host,panel) {
    if(!panel || !barPosition)return;
    placePanelBy(host,panel);
  }
  // The quality menu next to its button, on the page itself (fixed, kept
  // inside the window): inside the player it was cut off by the player's
  // own edges, sometimes hiding the Download row.
  function placePanelBy(host,panel) {
    if(!host || !panel)return;
    const parent=document.fullscreenElement || document.documentElement;
    if(panel.parentNode!==parent)parent.appendChild(panel);
    const r=host.getBoundingClientRect();
    const width=Math.min(300,Math.max(0,window.innerWidth-16));
    panel.style.position="fixed";panel.style.width=width+"px";
    panel.style.left=Math.max(8,Math.min(window.innerWidth-width-8,r.right-width))+"px";
    panel.style.right="auto";panel.style.bottom="auto";
    const above=Math.max(0,r.top-14),below=Math.max(0,window.innerHeight-r.bottom-14);
    const naturalHeight=panel.scrollHeight || panel.offsetHeight || 300;
    const up=naturalHeight>below && above>below;
    const available=up?above:below;
    panel.style.boxSizing="border-box";panel.style.maxHeight=available+"px";
    panel.style.overflowY="auto";panel.style.zIndex="2147483646";
    const height=Math.min(naturalHeight,available);
    panel.style.top=(up?Math.max(8,r.top-height-6):r.bottom+6)+"px";
    const arrow=panel.querySelector('[data-dc-p="arrow"]');if(arrow)arrow.style.display="none";
  }
  function rememberBarPosition() {
    try { chrome.runtime.sendMessage({type:"setVideoBarPosition",position:barPosition},()=>{void chrome.runtime.lastError;}); } catch(e) { /* keep page-local drag usable */ }
  }
  function loadBarPosition() {
    const revision=positionRevision;
    try { chrome.runtime.sendMessage({type:"getVideoBarPosition"},res=>{
      if(chrome.runtime.lastError || !alive() || revision!==positionRevision)return;
      barPosition=validBarPosition(res && res.position) ? res.position : null;
      syncDismissControls();
    }); } catch(e) { /* default placement remains available */ }
  }
  function installBarDrag(host,main,grip) {
    let drag=null,suppressClick=false;
    host.addEventListener("click",ev=>{
      if(suppressClick){suppressClick=false;ev.preventDefault();ev.stopImmediatePropagation();}
    },true);
    const update=(left,top)=>{
      const width=Math.min(barWidth(host),window.innerWidth-16);
      if(document.fullscreenElement){
        // Moves for this fullscreen session only; the saved spot is for
        // the normal window (see applySavedBarPosition).
        host.style.left=Math.max(8,Math.min(window.innerWidth-width-8,left))+"px";
        host.style.top=Math.max(8,Math.min(window.innerHeight-36,top))+"px";
        return;
      }
      barPosition={x:Math.max(0,Math.min(1,(left-8)/Math.max(1,window.innerWidth-width-16))),
        y:Math.max(0,Math.min(1,(top-8)/Math.max(1,window.innerHeight-44)))};
      positionRevision++;applySavedBarPosition(host);
    };
    // The bar is only re-parented once a real drag starts. Moving it on
    // pointerdown detached the pressed element, and Chrome then drops the
    // click: the first click after every menu refresh did nothing.
    const onMove=ev=>{
      if(!drag || ev.pointerId!==drag.id)return;
      const dx=ev.clientX-drag.x,dy=ev.clientY-drag.y;
      if(!drag.moved && Math.hypot(dx,dy)<5)return;
      if(!drag.moved){
        hidePanels();
        const parent=document.fullscreenElement || document.documentElement;
        if(host.parentNode!==parent)parent.appendChild(host);
        host.style.position="fixed";host.style.left=drag.left+"px";host.style.top=drag.top+"px";
        host.style.right="auto";host.style.bottom="auto";
      }
      drag.moved=true;ev.preventDefault();ev.stopPropagation();update(drag.left+dx,drag.top+dy);
    };
    const finish=(ev,cancel)=>{
      if(!drag || ev.pointerId!==drag.id)return;
      const moved=drag.moved,previous=drag.previous;
      drag=null;activeBarDrag=null;
      window.removeEventListener("pointermove",onMove,true);
      window.removeEventListener("pointerup",onUp,true);
      window.removeEventListener("pointercancel",onCancel,true);
      if(cancel){if(moved){barPosition=previous;positionRevision++;syncDismissControls();}}
      else if(moved){
        suppressClick=true;setTimeout(()=>{suppressClick=false;},0);
        if(!document.fullscreenElement)rememberBarPosition();
        ev.preventDefault();ev.stopPropagation();
        if(onShorts() && host.id==="dc-yt-btn"){
          const row=shortsControlRow();
          const q=row && row.getBoundingClientRect();
          const home=q && ev.clientX>=q.left-60 && ev.clientX<=q.right+20 && ev.clientY>=q.top-24 && ev.clientY<=q.bottom+24;
          setShortsBarFree(!home);
          if(home)placeInPlayer(host,document.getElementById("dc-yt-panel"));
        }
      }
    };
    const onUp=ev=>finish(ev,false);
    const onCancel=ev=>finish(ev,true);
    for(const control of [main,grip]) {
      control.style.touchAction="none";
      control.addEventListener("pointerdown",ev=>{
        if(ev.button!==0 || ev.isPrimary===false)return;
        const r=host.getBoundingClientRect();
        drag={id:ev.pointerId,x:ev.clientX,y:ev.clientY,left:r.left,top:r.top,moved:false,previous:barPosition};
        suppressClick=false;activeBarDrag=host;
        // Window listeners follow the pointer off the bar without pointer
        // capture (capturing, then re-parenting, used to cancel the drag).
        window.addEventListener("pointermove",onMove,true);
        window.addEventListener("pointerup",onUp,true);
        window.addEventListener("pointercancel",onCancel,true);
        ev.stopPropagation();
      });
    }
    grip.addEventListener("click",ev=>{ev.preventDefault();ev.stopPropagation();});
    grip.addEventListener("keydown",ev=>{
      const delta={ArrowLeft:[-1,0],ArrowRight:[1,0],ArrowUp:[0,-1],ArrowDown:[0,1]}[ev.key];
      if(!delta)return;
      ev.preventDefault();ev.stopPropagation();hidePanels();
      const r=host.getBoundingClientRect(),step=ev.shiftKey?24:8;
      update(r.left+delta[0]*step,r.top+delta[1]*step);if(!document.fullscreenElement)rememberBarPosition();
    });
  }

  // IDM-style: the circle lives ON the video player (top-right corner),
  // falling back to a fixed viewport corner when no player is found. The
  // bubble's arrow always points back at the circle (up when the panel sits
  // below it, down when it sits above it).
  // The open quality menu follows its button while the page scrolls or the
  // window changes size.
  let openMenu = null;
  const followMenu = () => {
    const m = openMenu;
    if (!m) return;
    if (!m.btn.isConnected || !m.panel.isConnected || m.panel.style.display === "none") { openMenu = null; return; }
    placePanelBy(m.btn, m.panel);
  };
  try {
    window.addEventListener("scroll", followMenu, { capture: true, passive: true });
    window.addEventListener("resize", followMenu, { passive: true });
  } catch (e) { /* best effort */ }

  // The YouTube player on screen: the watch page's #movie_player, or on
  // Shorts #shorts-player (YouTube keeps a hidden 0x0 #movie_player there,
  // and a button inside it is never seen).
  function ytPlayer() {
    let best = null;
    let area = 0;
    for (const p of document.querySelectorAll("#movie_player, #shorts-player, .html5-video-player")) {
      const r = p.getBoundingClientRect();
      const a = r.width * r.height;
      if (a > area) { area = a; best = p; }
    }
    return best;
  }

  // Dragged out of a Short's control row: stays where it was dropped on
  // Shorts too (dropping it back on the row returns it there).
  let shortsBarFree = false;
  try {
    chrome.storage.local.get({ shortsBarFree: false }, (r) => { if (!chrome.runtime.lastError) shortsBarFree = !!(r && r.shortsBarFree); });
  } catch (e) { /* stays in the row */ }
  function setShortsBarFree(v) {
    shortsBarFree = v;
    try { chrome.storage.local.set({ shortsBarFree: v }); } catch (e) { /* this page only */ }
  }

  // The top-right control row (More, Full screen) of the Short on screen.
  function shortsControlRow() {
    let best = null;
    let dist = Infinity;
    const mid = window.innerHeight / 2;
    for (const r of document.querySelectorAll("ytd-shorts-player-controls #right-controls, #shorts-player-controls #right-controls, ytd-reel-video-renderer #right-controls")) {
      const q = r.getBoundingClientRect();
      if (!q.width || q.bottom < 0 || q.top > window.innerHeight) continue;
      const d = Math.abs(q.top - mid);
      if (d < dist) { dist = d; best = r; }
    }
    return best;
  }

  function placeInPlayer(btn, panel) {
    if (!btn) return false;
    // Being dragged: the pointer decides where it goes.
    if (activeBarDrag === btn) return true;
    const setArrow = (side) => {
      if (!panel) return;
      const a = panel.querySelector('[data-dc-p="arrow"]');
      if (!a) return;
      const T = panel._dcT || dcPanelTokens();
      const top = side === "top";
      a.setAttribute("data-dc-arrow", top ? "top" : "bottom");
      a.style.background = T.bg;
      a.style.right = "11px";
      a.style.left = "auto";
      a.style.top = top ? "-7px" : "auto";
      a.style.bottom = top ? "auto" : "-7px";
      a.style.borderLeftColor = top ? T.border : "transparent";
      a.style.borderTopColor = top ? T.border : "transparent";
      a.style.borderRightColor = top ? "transparent" : T.border;
      a.style.borderBottomColor = top ? "transparent" : T.border;
    };
    const player = ytPlayer();
    // Shorts: a compact round button in the Short's own top control row,
    // just left of its More (and Full screen) buttons. It opens on hover.
    if (player && player.id === "shorts-player") {
      btn.setAttribute("data-dc-compact", "");
      const row = shortsBarFree && barPosition ? null : shortsControlRow();
      if (row) {
        // One button per row: a previous Short's button never stays beside it.
        row.querySelectorAll("[data-dc-bar]").forEach((x) => { if (x !== btn) { if (x.__dcClose) x.__dcClose.remove(); x.remove(); } });
        btn.style.position = "relative";
        btn.style.top = btn.style.right = btn.style.left = btn.style.bottom = "auto";
        btn.style.width = "max-content";
        btn.style.alignSelf = "center";
        btn.style.margin = "0 4px";
        if (btn.parentElement !== row || row.firstElementChild !== btn) row.insertBefore(btn, row.firstChild);
        setArrow("top");
        return true;
      }
    } else {
      btn.removeAttribute("data-dc-compact");
      btn.style.alignSelf = btn.style.margin = "";
    }
    if (!player && ytVideoId()) {
      // The page is still loading its player: wait for it (the watcher
      // places the button once it exists) instead of a window corner.
      btn.style.display = "none";
      if (panel) panel.style.display = "none";
      return false;
    }
    if (btn.style.display === "none") btn.style.display = "block";
    if(applySavedBarPosition(btn)){positionDraggedPanel(btn,panel);return true;}
    if (player) {
      btn.style.position = "absolute";
      // Shorts keep their own More / Full screen buttons in the top-right
      // corner, on a layer above the player: sit just below them.
      btn.style.top = player.id === "shorts-player" ? "72px" : "12px";
      btn.style.right = "12px";
      btn.style.bottom = "auto";
      // Clear what a saved (dragged) placement left behind: a stale left or
      // width would pin the bar mid-player once the player grows.
      btn.style.left = "auto";
      btn.style.width = "";
      // The menu is placed on the page when it opens (placePanelBy).
      setArrow("top");
      if (btn.parentElement !== player) player.appendChild(btn);
      return true;
    }
    btn.style.position = "fixed";
    btn.style.right = "18px";
    btn.style.bottom = "18px";
    btn.style.top = "auto";
    if (panel) {
      panel.style.position = "fixed";
      panel.style.right = "18px";
      panel.style.bottom = "62px";
      panel.style.top = "auto";
      if (panel.parentElement !== document.documentElement) document.documentElement.appendChild(panel);
      setArrow("bottom");
    }
    if (btn.parentElement !== document.documentElement) document.documentElement.appendChild(btn);
    return false;
  }

  // The Facebook pill lives inside the player box, which is only as wide as
  // the video: a 300px panel anchored in there gets clipped on the left once
  // the reel column is narrower than the panel (narrow/vertical reels). Anchor
  // the panel to the page instead, clamped into the viewport, and keep the
  // arrow pointing at the pill.
  function placeFbPanel(pill, panel) {
    if (!pill || !panel) return;
    const host = document.body || document.documentElement;
    if (panel.parentElement !== host) host.appendChild(panel);
    let pr;
    try { pr = pill.getBoundingClientRect(); } catch (e) { pr = null; }
    if (!pr) return;
    const de = document.documentElement;
    const vw = window.innerWidth || de.clientWidth || 360;
    const vh = window.innerHeight || de.clientHeight || 640;
    const ox = window.pageXOffset || de.scrollLeft || 0;
    const oy = window.pageYOffset || de.scrollTop || 0;
    // 300px is the design width; shrink on very narrow viewports so both
    // edges stay on screen.
    const w = Math.max(240, Math.min(300, vw - 16));
    let left = Math.round(pr.right - w); // right edges aligned with the pill
    left = Math.min(Math.max(8, left), Math.max(8, vw - w - 8));
    // ~300px tall; if the pill sits low, lift the panel so it still fits.
    const est = 300;
    let top = Math.round(pr.bottom + 6);
    if (top + est > vh - 8) top = Math.max(8, vh - 8 - est);
    panel.style.position = "absolute";
    panel.style.width = w + "px";
    panel.style.left = left + ox + "px";
    panel.style.top = top + oy + "px";
    panel.style.right = "auto";
    panel.style.bottom = "auto";
    panel.style.zIndex = "2147483647";
    const arrow = panel.querySelector('[data-dc-p="arrow"]');
    if (arrow) {
      // Centre the arrow over the pill, then keep it inside the panel's radius.
      const at = pr.left + pr.width / 2 - left - 7;
      arrow.style.left = Math.round(Math.min(Math.max(12, at), w - 26)) + "px";
      arrow.style.right = "auto";
    }
  }

  // Presence pre-check: qualities already in the app queue get a "✓ Queued"
  // tag and a toast instead of re-sending into an "already in queue" error.
  function markQueuedRows(pairs) {
    try {
      const urls = pairs.map((p) => p.st.url).filter(Boolean).slice(0, 25);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "isQueued", urls }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const hit = new Set((res && res.queued) || []);
        if (!hit.size) return;
        for (const p of pairs) {
          if (!hit.has(p.st.url) || !p.item || !p.item.el.isConnected) continue;
          if (p.item.el.getAttribute("data-dc-queued")) continue;
          p.item.el.setAttribute("data-dc-queued", "1");
          p.item.fmtEl.textContent = dcT("queued", "✓ Queued");
          p.item.el.title = "Already in the download queue";
          p.item.el.onclick = (ev) => {
            ev.stopPropagation();
            toast("Download Craft: already in queue");
          };
        }
      });
    } catch (e) { /* best-effort */ }
  }

  function fmtSize(n) {    if (!n || n <= 0) return "";
    const u = ["B", "KB", "MB", "GB"];
    let v = n, i = 0;
    while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
    return i === 0 ? `${n} ${u[i]}` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
  }

  // Seconds → "3:26" for rows with page-JSON durations (shared rank.js
  // helper when loaded, inline fallback otherwise).
  function fmtDurSec(sec) {
    if (typeof DCRank !== "undefined" && DCRank.fmtDur) return DCRank.fmtDur(sec);
    const n = Math.round(Number(sec) || 0);
    if (!(n > 0) || n >= 86400) return "";
    return `${Math.floor(n / 60)}:${String(n % 60).padStart(2, "0")}`;
  }

  // ---------- In-page floating panel (extension_popup.html design) ----------
  // The trigger is a compact download bar; the panel is a message bubble with an
  // arrow, a "send link" row, video info, a 2-column quality grid, "Other
  // Files" chips and a Download / Cancel / X action row. Every rule is set
  // inline (a page's own CSS can never leak in), and each element carries a
  // `data-dc-p` role so the light/dark toggle can repaint it without
  // touching layout.
  const SVG_NS = "http://www.w3.org/2000/svg";
  const ICON_DOWN = "M12 4V16M6 10L12 16L18 10";
  const ICON_SEND = "M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z";
  const ICON_CLOSE = "M6 6l12 12M18 6L6 18";
  const ICON_PLAY = "M8 5.5v13l11-6.5-11-6.5z";

  // Transparent-background inline SVG (no web_accessible_resources, crisp at
  // every UI scale). `currentColor` lets the caller theme it with `color`.
  function svgIcon(px, d, w) {
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("width", String(px));
    svg.setAttribute("height", String(px));
    svg.setAttribute("aria-hidden", "true");
    svg.setAttribute("focusable", "false");
    svg.style.cssText = "display:block;flex:0 0 auto;pointer-events:none;";
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", d);
    if (w === 0) {
      path.setAttribute("fill", "currentColor");
      path.setAttribute("stroke", "none");
    } else {
      path.setAttribute("fill", "none");
      path.setAttribute("stroke", "currentColor");
      path.setAttribute("stroke-width", String(w || 2));
      path.setAttribute("stroke-linecap", "round");
      path.setAttribute("stroke-linejoin", "round");
    }
    svg.appendChild(path);
    return svg;
  }

  // One isolated toolbar: the primary action and dismiss button never overlap.
  // Shadow DOM keeps player/site button and SVG rules out of these controls.
  // Colors come from the panel theme (the app's Light/Dark/System choice), and
  // the bar is see-through until pointed at, IDM-style, so it never hides
  // the video. Width is its content's: icon + label, no padding to spare.
  function paintBar(host) {
    const bar = host && host.shadowRoot && host.shadowRoot.querySelector(".bar");
    if (!bar) return;
    const T = dcPanelTokens();
    const light = T.mode === "light";
    const set = (k, v) => bar.style.setProperty(k, v);
    set("--bg", T.card);
    set("--glass", light ? "rgba(255,255,255,.92)" : "rgba(14,16,24,.86)");
    set("--acc", light ? "#6d5df5" : "#7c6cf6");
    set("--fg", T.text);
    set("--line", T.border);
    set("--hover", T.hover);
    set("--dim", T.dim);
    set("--ok", light ? "#168047" : "#4ade80");
    set("--shadow", light ? "0 4px 14px rgba(15,23,42,.18)" : "0 6px 20px rgba(0,0,0,.45)");
    set("--x-bg", light ? "#ffe9ec" : "rgba(239,68,68,.18)");
    set("--x-fg", light ? "#a91d2b" : "#fca5a5");
    host.style.colorScheme = light ? "light" : "dark";
  }
  // The bar's natural width, measured once it is in the page.
  function barWidth(host) {
    if (!host.__dcW) {
      const b = host.shadowRoot && host.shadowRoot.querySelector(".bar");
      const w = b ? Math.ceil(b.scrollWidth) : 0;
      if (w > 0) host.__dcW = w + 2;
    }
    return host.__dcW || 200;
  }
  function videoDownloadBar(title) {
    const host = document.createElement("div");
    host.title = title;
    host.style.cssText = "all:initial;display:block;height:30px;margin:0;padding:0;border:0;box-sizing:border-box;z-index:2147483647;";
    const root = host.attachShadow({mode:"open"});
    const style = document.createElement("style");
    style.textContent = `
      * { box-sizing:border-box; }
      .bar { display:inline-flex;align-items:center;gap:2px;width:max-content;max-width:100%;height:30px;
        padding:0 3px;overflow:hidden;border:1px solid var(--line);border-radius:15px;background:var(--glass);
        box-shadow:var(--shadow);font:12px/1.2 system-ui,sans-serif;
        /* See-through until pointed at: the video stays visible. */
        opacity:.32;transition:opacity .15s ease; }
      :host(:hover) .bar, .bar:focus-within, :host([data-dc-awake]) .bar { opacity:1; }
      /* Compact (Shorts): just the round arrow; pointing at it opens the
         whole bar. */
      :host([data-dc-compact]:not(:hover)) .bar:not(:focus-within) .label { display:none; }
      :host([data-dc-compact]:not(:hover)) .bar:not(:focus-within) .download { padding:0 4px; }
      :host([data-dc-compact]:not(:hover)) .bar:not(:focus-within) { padding:0 1px; }
      /* Shorts: hidden while the feed scrolls, back for the next Short. */
      :host([data-dc-scrolling]) .bar { opacity:0 !important; pointer-events:none; transition:opacity .1s ease; }
      button { all:unset;box-sizing:border-box;display:flex;align-items:center;justify-content:center;cursor:pointer;
        color:var(--fg);background:transparent;font:600 12.5px/1.2 system-ui,sans-serif; }
      /* Pointing anywhere at the bar lights the whole bar (one control);
         only × keeps a hover of its own. */
      :host(:hover) .bar { box-shadow:var(--shadow), inset 0 0 0 999px var(--hover); }
      button:focus-visible { outline:2px solid var(--acc);outline-offset:-2px; }
      .grip { flex:0 0 0;width:0;opacity:0;height:22px;border-radius:11px;cursor:grab;color:var(--dim);touch-action:none;
        transition:width .15s ease,opacity .15s ease; }
      :host(:hover) .grip, .bar:focus-within .grip { flex-basis:16px;width:16px;opacity:1; }
      .grip:active { cursor:grabbing; }
      .download { touch-action:none;flex:0 0 auto;gap:7px;height:24px;padding:0 11px 0 4px;border-radius:12px; }
      .dot { width:18px;height:18px;border-radius:9px;background:var(--acc);color:#fff;display:flex;align-items:center;justify-content:center;flex:none; }
      .label { white-space:nowrap; }
      .close { flex:0 0 22px;width:22px;height:22px;border-radius:11px;color:var(--dim); }
      .more { flex:0 0 20px;width:20px;height:22px;border-radius:11px;color:var(--dim); }
      :host([data-dc-compact]:not(:hover)) .bar:not(:focus-within) .more { display:none; }
      .close:hover { color:var(--x-fg);background:var(--x-bg); }
      [hidden] { display:none !important; }
      svg { display:block;flex:none;width:14px;height:14px;pointer-events:none; }
    `;
    const bar = document.createElement("div");bar.className="bar";
    const main = document.createElement("button");main.type="button";main.className="download";
    main.title=title;main.setAttribute("aria-label","Download this video");
    const dot=document.createElement("span");dot.className="dot";
    dot.appendChild(svgIcon(11,ICON_DOWN,2.8));
    main.appendChild(dot);
    const label=document.createElement("span");label.className="label";label.textContent=dcT("barDownload","Download");
    main.appendChild(label);
    const close=document.createElement("button");close.type="button";close.className="close";close.hidden=true;
    close.setAttribute("data-dc-dismiss","1");
    close.setAttribute("aria-label","Hide download bar for this video");
    close.title="Hide for this video — restore from the extension toolbar";
    close.appendChild(svgIcon(12,ICON_CLOSE,2));
    const grip=document.createElement("button");grip.type="button";grip.className="grip";
    grip.title="Drag to move · arrow keys move when focused";
    grip.setAttribute("aria-label","Move video download bar");
    grip.appendChild(svgIcon(12,"M8 5h.01M8 12h.01M8 19h.01M16 5h.01M16 12h.01M16 19h.01",3));
    const more=document.createElement("button");more.type="button";more.className="more";more.hidden=true;
    more.title=dcT("barMoreQualities","Other qualities");more.setAttribute("aria-label","Other qualities");
    more.appendChild(svgIcon(12,"M6 9l6 6 6-6",2.4));
    bar.append(grip,main,more,close);root.append(style,bar);
    installBarDrag(host,main,grip);
    host.__dcMain=main;host.__dcClose=close;host.__dcLabel=label;host.__dcMore=more;
    host.setAttribute("data-dc-bar","1");
    paintBar(host);
    wakeBar(host);
    return host;
  }

  // A new button (a new video) and a scroll show the bar fully for a few
  // seconds, so people notice it, before it fades to see-through.
  const BAR_AWAKE_MS = 3000;
  // Shorts: only the first one shows the bar fully; scrolling on to the
  // next ones leaves it see-through until pointed at.
  let shortsWoken = false;
  const onShorts = () => /^\/shorts\//.test(location.pathname);
  function wakeBar(host) {
    if (onShorts()) {
      if (shortsWoken) return;
      shortsWoken = true;
    }
    try {
      host.setAttribute("data-dc-awake", "1");
      clearTimeout(host.__dcAwakeT);
      host.__dcAwakeT = setTimeout(() => { try { host.removeAttribute("data-dc-awake"); } catch (e) { /* gone */ } }, BAR_AWAKE_MS);
    } catch (e) { /* best effort */ }
  }
  let lastScrollWake = 0;
  try {
    window.addEventListener("scroll", () => {
      if (onShorts()) {
        try {
          document.querySelectorAll("[data-dc-bar]").forEach((h) => {
            h.setAttribute("data-dc-scrolling", "");
            clearTimeout(h.__dcScrollT);
            h.__dcScrollT = setTimeout(() => {
              try {
                h.removeAttribute("data-dc-scrolling");
                // The next Short may have its own control row by now.
                if (h.id === "dc-yt-btn" && h.isConnected && document.getElementById("dc-yt-btn") === h) placeInPlayer(h, document.getElementById("dc-yt-panel"));
              } catch (e) { /* gone */ }
            }, 450);
          });
        } catch (e) { /* ignore */ }
        return;
      }
      const now = Date.now();
      if (now - lastScrollWake < 400) return;
      lastScrollWake = now;
      try { document.querySelectorAll("[data-dc-bar]").forEach(wakeBar); } catch (e) { /* ignore */ }
    }, { capture: true, passive: true });
  } catch (e) { /* best effort */ }

  // A click that came before the page had loaded its video data waits for it:
  // the bar says so ("Resolving…") instead of sending a half-known link.
  function barBusy(host, text) {
    try {
      const lab = host && host.__dcLabel;
      if (!lab) return;
      if (host.__dcIdleText === undefined) host.__dcIdleText = lab.textContent;
      lab.textContent = text || host.__dcIdleText;
      host.__dcMain.style.cursor = text ? "progress" : "";
      host.__dcBusy = !!text;
      syncDismissControls();
    } catch (e) { /* best effort */ }
  }
  // Retry `probe` every `every` ms until it returns something truthy or
  // `tries` run out; `done` gets the result (or null).
  function waitFor(probe, done, tries = 16, every = 600) {
    let n = 0;
    const step = () => {
      let v = null;
      try { v = probe(); } catch (e) { v = null; }
      Promise.resolve(v).then((r) => {
        if (r || ++n >= tries || !alive()) done(r || null);
        else setTimeout(step, every);
      }, () => done(null));
    };
    step();
  }
  function sniffedNow() {
    return new Promise((resolve) => {
      try {
        chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
          if (chrome.runtime.lastError) return resolve([]);
          resolve((res && res.items) || []);
        });
      } catch (e) { resolve([]); }
    });
  }

  // Theme follows the app (Light / Dark / System, sent by the native host);
  // System tracks the OS. Repaint every bar and panel when either changes.
  function repaintTheme() {
    try { document.querySelectorAll("[data-dc-bar]").forEach(paintBar); } catch (e) {}
    try { allPanels().forEach(paintDcPanel); } catch (e) {}
  }
  function applyAppTheme(mode) {
    try {
      if (mode === "light" || mode === "dark") window.__dcTheme = mode;
      else delete window.__dcTheme;
      if (typeof globalThis !== "undefined") {
        if (mode === "light" || mode === "dark") globalThis.__dcTheme = mode;
        else delete globalThis.__dcTheme;
      }
    } catch (e) {}
    repaintTheme();
  }
  try {
    chrome.storage.local.get({ appTheme: "system" }, (r) => { if (!chrome.runtime.lastError) applyAppTheme(r && r.appTheme); });
    chrome.storage.onChanged.addListener((ch, area) => {
      if (area === "local" && ch.appTheme && alive()) applyAppTheme(ch.appTheme.newValue);
    });
    window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => { if (alive()) repaintTheme(); });
  } catch (e) { /* theme sync is cosmetic */ }

  // Theme: tokens + painter live in rank.js (node-tested); these wrappers
  // fall back to the dark mock when rank.js failed to load.
  const DCUI = (typeof DCRank !== "undefined" && DCRank.paintDcPanel) ? DCRank : null;
  const DC_FALLBACK = {
    mode: "dark", bg: "#0f172a", card: "#1e293b", hover: "#334155", border: "#334155",
    text: "#f1f5f9", dim: "#94a3b8", muted: "#64748b", accent: "#3b82f6",
    accentLight: "rgba(59,130,246,.16)", shadow: "0 8px 32px rgba(0,0,0,.4)",
    thumbBg: "linear-gradient(135deg,#667eea,#764ba2)",
    toggleBorder: "rgba(241,245,249,.25)", toggle: "☀",
  };
  function dcThemeMode() { return DCUI ? DCUI.dcThemeMode() : "dark"; }
  function dcSetTheme(mode) { if (DCUI) DCUI.dcSetTheme(mode); }
  // Text in the browser's language (_locales), English when there is none.
  function dcT(key, english) {
    try { return (chrome.i18n && chrome.i18n.getMessage(key)) || english; } catch (e) { return english; }
  }
  function dcPanelTokens() { return (DCUI && DCUI.panelTokens(dcThemeMode())) || DC_FALLBACK; }
  function paintDcPanel(root) { if (root && DCUI) DCUI.paintDcPanel(root); }
  function allPanels() { try { return [...document.querySelectorAll("[data-dc-panel]")]; } catch (e) { return []; } }
  function hidePanels() { allPanels().forEach((p) => { p.style.display = "none"; }); }
  // Click-away / Escape closes any open panel. One listener per page (a
  // per-rebuild listener leaked and stale closures fought the new panel);
  // panels stop their own clicks from reaching the document.
  function installDcCloser() {
    if (window.__dcDocClick) return;
    window.__dcDocClick = true;
    document.addEventListener("click", () => { hidePanels(); });
    document.addEventListener("keydown", (e) => {
      if (e && (e.key === "Escape" || e.keyCode === 27)) hidePanels();
    });
  }

  // A plain styled div (role = data-dc-p paint role, may be null).
  function dcText(text, role, css) {
    const el = document.createElement("div");
    if (role) el.setAttribute("data-dc-p", role);
    if (css) el.style.cssText = css;
    el.textContent = text == null ? "" : String(text);
    return el;
  }

  // Light/dark toggle for a panel (same idea as rank.js's menu toggle, but
  // it repaints the panel design and any other open panel).
  function panelThemeToggle(panel) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("data-dc-p", "toggle");
    b.title = "Toggle light / dark";
    b.style.cssText = "flex:0 0 auto;width:24px;height:24px;padding:0;margin:0;border-radius:999px;display:grid;place-items:center;"
      + "border:1px solid;background:transparent;cursor:pointer;font:11px/1 system-ui,sans-serif;";
    b.onclick = (ev) => {
      if (ev && ev.stopPropagation) ev.stopPropagation();
      dcSetTheme(dcThemeMode() === "light" ? "dark" : "light");
      try { allPanels().forEach(paintDcPanel); } catch (e) { /* theme is best-effort */ }
      // The row under the pointer keeps its hover colour inline: give it
      // the new theme's, or it stays dark/light until the pointer leaves.
      try {
        const row = b.closest('[data-dc-p="sendrow"]');
        if (row && row.matches(":hover")) row.style.background = dcPanelTokens().hover;
      } catch (e) { /* cosmetic */ }
    };
    return b;
  }

  // One quality cell: container + resolution on the left, size on the right.
  function dcQualityItem(o) {
    const T = dcPanelTokens();
    const el = document.createElement("div");
    el.setAttribute("data-dc-p", "item");
    el.style.cssText = "display:flex;align-items:center;justify-content:space-between;gap:6px;box-sizing:border-box;"
      + "border:1px solid " + T.border + ";border-radius:6px;background:" + T.card + ";cursor:pointer;padding:6px 8px;";
    if (o.tip) el.title = o.tip;
    const col = document.createElement("div");
    col.style.cssText = "display:flex;flex-direction:column;gap:1px;min-width:0;";
    const fmt = dcText(o.format || "", "text", "font-size:10px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    const res = dcText(o.res || "", "muted", "font-size:9px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    col.appendChild(fmt);
    col.appendChild(res);
    const size = dcText(o.size || "—", "dim", "flex:0 0 auto;font-size:10px;text-align:right;white-space:nowrap;");
    el.appendChild(col);
    el.appendChild(size);
    el.onmouseenter = () => { if (!el.getAttribute("data-dc-sel")) el.style.background = dcPanelTokens().hover; };
    el.onmouseleave = () => { if (!el.getAttribute("data-dc-sel")) el.style.background = dcPanelTokens().card; };
    return { el, fmtEl: fmt, resEl: res, sizeEl: size, setSize: (t) => { size.textContent = t; } };
  }

  // "Other Files" chip (subtitles, audio-only tracks).
  function dcFileChip(f) {
    const T = dcPanelTokens();
    const el = document.createElement("div");
    el.setAttribute("data-dc-p", "chip");
    el.style.cssText = "display:inline-flex;align-items:center;gap:5px;box-sizing:border-box;padding:4px 8px;"
      + "border:1px solid " + T.border + ";border-radius:5px;background:" + T.card + ";cursor:pointer;"
      + "font-size:10px;font-weight:500;";
    if (f.tip) el.title = f.tip;
    el.appendChild(dcText(f.label || "file", null, "white-space:nowrap;"));
    el.onclick = (ev) => {
      if (ev && ev.stopPropagation) ev.stopPropagation();
      hidePanels();
      if (f.send) f.send();
    };
    el.onmouseenter = () => { el.style.borderColor = dcPanelTokens().accent; };
    el.onmouseleave = () => { el.style.borderColor = dcPanelTokens().border; };
    return { el };
  }

  // Build the bubble panel. Spec:
  //   {title, thumb, dur, res, size, format,          // video info block
  //    hint, sendLabel, onSendLink,                   // "send link" row
  //    primaryLabel, onPrimary, onClose}              // action row
  // plus api.addQuality({format,res,size,tip,send,onPick}) / api.addChip(...).
  function dcPanel(spec) {
    const s = spec || {};
    const T = dcPanelTokens();
    const items = [];
    const chips = [];
    let selected = null;
    const panel = document.createElement("div");
    panel.setAttribute("data-dc-panel", "1");
    panel.style.cssText = [
      "display:none", "position:relative", "width:300px", "max-width:96vw",
      "box-sizing:border-box", "padding:0", "margin:0", "text-align:left",
      "border:1px solid " + T.border, "border-radius:12px",
      "background:" + T.bg, "color:" + T.text, "box-shadow:" + T.shadow,
      "font:12px/1.35 system-ui,'Segoe UI',sans-serif", "z-index:2147483647",
    ].join(";");
    // Clicks inside the panel never reach the page (YouTube's own handlers
    // would pause/close the player, and the document closer would fire).
    panel.onclick = (ev) => { if (ev && ev.stopPropagation) ev.stopPropagation(); };

    const arrow = document.createElement("div");
    arrow.setAttribute("data-dc-p", "arrow");
    arrow.setAttribute("data-dc-arrow", "top");
    arrow.style.cssText = "position:absolute;top:-7px;right:11px;left:auto;width:14px;height:14px;"
      + "background:" + T.bg + ";border:1px solid transparent;border-left-color:" + T.border
      + ";border-top-color:" + T.border + ";border-radius:2px 0 0 0;transform:rotate(45deg);pointer-events:none;";
    panel.appendChild(arrow);

    const inner = document.createElement("div");
    inner.style.cssText = "display:flex;flex-direction:column;border-radius:11px;overflow:hidden;";

    // --- "Send video link to app" row
    const sendRow = document.createElement("div");
    sendRow.setAttribute("data-dc-p", "sendrow");
    sendRow.style.cssText = "display:flex;align-items:center;gap:8px;box-sizing:border-box;margin:4px 8px 0;padding:10px 12px;border-radius:6px;";
    const sendIcon = svgIcon(14, ICON_SEND, 2);
    sendIcon.setAttribute("data-dc-p", "icon");
    const sendTxt = document.createElement("div");
    sendTxt.style.cssText = "flex:1;min-width:0;";
    sendTxt.appendChild(dcText(s.sendLabel || dcT("panelSendLink", "Send video link to app"), "text", "font-size:11px;"));
    sendTxt.appendChild(dcText(s.hint || "", "muted", "font-size:9px;margin-top:1px;line-height:1.35;"));
    sendRow.appendChild(sendIcon);
    sendRow.appendChild(sendTxt);
    sendRow.appendChild(panelThemeToggle(panel));
    sendRow.onmouseenter = () => { sendRow.style.background = (panel._dcT || dcPanelTokens()).hover; };
    sendRow.onmouseleave = () => { sendRow.style.background = "transparent"; };
    if (s.onSendLink) {
      sendRow.style.cursor = "pointer";
      sendRow.onclick = (ev) => {
        if (ev && ev.stopPropagation) ev.stopPropagation();
        hidePanels();
        s.onSendLink();
      };
    }
    inner.appendChild(sendRow);

    // --- video info (thumb + title + resolution / size / container)
    const info = document.createElement("div");
    info.style.cssText = "display:flex;gap:10px;align-items:center;box-sizing:border-box;padding:10px 12px;";
    const thumb = document.createElement("div");
    thumb.setAttribute("data-dc-p", "thumb");
    thumb.style.cssText = "position:relative;flex:0 0 80px;width:80px;height:48px;border-radius:6px;overflow:hidden;"
      + "display:flex;align-items:center;justify-content:center;background:" + T.thumbBg + ";color:#fff;";
    const play = svgIcon(14, ICON_PLAY, 0);
    play.style.opacity = ".85";
    // An https picture, or a frame grabbed from the player (data:image/…).
    if (s.thumb && (/^https?:\/\//i.test(s.thumb) || /^data:image\/(jpeg|png|webp);base64,/i.test(s.thumb))) {
      const im = document.createElement("img");
      im.src = s.thumb;
      im.loading = "lazy";
      im.style.cssText = "width:100%;height:100%;object-fit:cover;display:block;";
      im.onload = () => { play.style.display = "none"; };
      im.onerror = () => { im.remove(); };
      thumb.appendChild(im);
    }
    thumb.appendChild(play);
    if (s.dur) {
      thumb.appendChild(dcText(s.dur, null,
        "position:absolute;bottom:2px;right:4px;font-size:7px;color:#fff;background:rgba(0,0,0,.7);padding:1px 3px;border-radius:2px;line-height:1;"));
    }
    const det = document.createElement("div");
    det.style.cssText = "flex:1;min-width:0;";
    const titleEl = dcText(s.title, "text", "font-size:12px;font-weight:600;margin-bottom:4px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;");
    titleEl.title = s.title || "";
    const meta = document.createElement("div");
    meta.style.cssText = "display:flex;align-items:center;gap:8px;flex-wrap:wrap;";
    const resEl = dcText(s.res || "", "accent", "font-size:10px;font-weight:500;");
    const sizeEl = dcText(s.size ? "• " + s.size : "", "muted", "font-size:10px;");
    const fmtEl = dcText(s.format ? String(s.format).toUpperCase() : "", "chipmini",
      "font-size:9px;padding:1px 5px;border-radius:3px;");
    meta.appendChild(resEl);
    meta.appendChild(sizeEl);
    meta.appendChild(fmtEl);
    det.appendChild(titleEl);
    det.appendChild(meta);
    info.appendChild(thumb);
    info.appendChild(det);
    inner.appendChild(info);

    // --- scrollable middle: quality grid + other files
    const scroller = document.createElement("div");
    scroller.style.cssText = "max-height:min(58vh,420px);overflow-y:auto;overflow-x:hidden;";
    const mkDivider = () => {
      const d = document.createElement("div");
      d.setAttribute("data-dc-p", "divider");
      d.style.cssText = "height:1px;margin:0 12px;background:" + T.border + ";";
      return d;
    };
    const gridDivider = mkDivider();
    const gridSec = document.createElement("div");
    gridSec.style.cssText = "padding:8px 12px;";
    const gridHead = document.createElement("div");
    gridHead.style.cssText = "display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;";
    const badge = dcText("", "badge", "font-size:9px;padding:2px 6px;border-radius:8px;");
    gridHead.appendChild(dcText(dcT("panelVideoFiles", "Video Files"), "muted", "font-size:10px;font-weight:600;letter-spacing:.5px;text-transform:uppercase;"));
    gridHead.appendChild(badge);
    const grid = document.createElement("div");
    grid.style.cssText = "display:grid;grid-template-columns:1fr 1fr;gap:5px;";
    gridSec.appendChild(gridHead);
    gridSec.appendChild(grid);
    scroller.appendChild(gridDivider);
    scroller.appendChild(gridSec);

    const chipsDivider = mkDivider();
    const chipsSec = document.createElement("div");
    chipsSec.style.cssText = "padding:8px 12px;";
    const chipsHead = document.createElement("div");
    chipsHead.style.cssText = "display:flex;align-items:center;justify-content:space-between;margin-bottom:6px;";
    chipsHead.appendChild(dcText(dcT("panelOtherFiles", "Other Files"), "muted", "font-size:10px;font-weight:600;letter-spacing:.5px;text-transform:uppercase;"));
    const chipsWrap = document.createElement("div");
    chipsWrap.style.cssText = "display:flex;gap:6px;flex-wrap:wrap;";
    chipsSec.appendChild(chipsHead);
    chipsSec.appendChild(chipsWrap);
    scroller.appendChild(chipsDivider);
    scroller.appendChild(chipsSec);
    inner.appendChild(scroller);

    // --- action row: Download / Cancel / X
    const actions = document.createElement("div");
    actions.setAttribute("data-dc-p", "borders");
    actions.style.cssText = "display:flex;gap:6px;box-sizing:border-box;padding:8px 12px;border-top:1px solid " + T.border + ";";
    const mkBtn = (label, role, on, icon) => {
      const b = document.createElement("button");
      b.type = "button";
      b.setAttribute("data-dc-p", role);
      b.style.cssText = "display:flex;align-items:center;justify-content:center;gap:4px;flex:1;box-sizing:border-box;"
        + "padding:7px 12px;margin:0;border-radius:5px;border:1px solid;cursor:pointer;"
        + "font:500 11px/1.2 system-ui,'Segoe UI',sans-serif;";
      if (label) b.appendChild(dcText(label, null, "white-space:nowrap;"));
      if (icon) b.appendChild(svgIcon(10, icon, 2.5));
      b.onclick = (ev) => {
        if (ev && ev.stopPropagation) ev.stopPropagation();
        if (on) on();
      };
      return b;
    };
    const primary = mkBtn(s.primaryLabel || dcT("panelDownload", "Download"), "btnp", () => {
      hidePanels();
      if (s.onPrimary) s.onPrimary();
    }, ICON_DOWN);
    const cancel = mkBtn(dcT("panelCancel", "Cancel"), "btn", () => {
      hidePanels();
      if (s.onClose) s.onClose();
    }, null);
    // Cancel closes the menu; a second × beside it did the same.
    actions.appendChild(primary);
    actions.appendChild(cancel);
    inner.appendChild(actions);

    panel.appendChild(inner);

    function select(it) {
      if (selected && selected.el) selected.el.removeAttribute("data-dc-sel");
      selected = it || null;
      if (selected && selected.el) selected.el.setAttribute("data-dc-sel", "1");
      paintDcPanel(panel);
      // The bar that opened this menu names the quality now selected.
      if (api.bar && selected && selected.q) barShowQuality(api.bar, selected.q.short || selected.q.res);
    }
    function sync() {
      badge.textContent = items.length === 1 ? "1 quality" : `${items.length} qualities`;
      gridDivider.style.display = items.length ? "block" : "none";
      gridSec.style.display = items.length ? "block" : "none";
      chipsDivider.style.display = chips.length ? "block" : "none";
      chipsSec.style.display = chips.length ? "block" : "none";
      scroller.style.display = (items.length || chips.length) ? "block" : "none";
    }
    const api = {
      panel,
      grid,
      items,
      bar: null, // the button this menu belongs to (relabelled on a pick)
      select,
      selected: () => selected,
      addQuality: (q) => {
        const it = dcQualityItem(q);
        it.q = q;
        it.el.onclick = (ev) => {
          if (ev && ev.stopPropagation) ev.stopPropagation();
          select(it);
          if (q.onPick) q.onPick();
        };
        grid.appendChild(it.el);
        items.push(it);
        // The highest quality is selected up front (like the mockup), so the
        // primary button always has something to download.
        if (!selected) {
          select(it);
          if (q.onPick) q.onPick();
        }
        sync();
        return it;
      },
      addChip: (c) => {
        const chip = dcFileChip(c);
        chipsWrap.appendChild(chip.el);
        chips.push(chip);
        sync();
        return chip;
      },
      setInfo: (o) => {
        if (o.res != null) resEl.textContent = o.res;
        if (o.size != null) sizeEl.textContent = o.size ? "• " + o.size : "";
        if (o.format != null) fmtEl.textContent = String(o.format).toUpperCase();
      },
      setPrimaryLabel: (t) => { primary.firstChild.textContent = t; },
      setHint: (t) => { sendTxt.lastChild.textContent = t; },
      sync,
      show: () => { panel.style.display = "block"; paintDcPanel(panel); },
      hide: () => { panel.style.display = "none"; },
    };
    paintDcPanel(panel);
    sync();
    return api;
  }

  // Size enrichment for grid cells whose page JSON carried no contentLength:
  // ask the background for a HEAD/Range measurement. Never for rows with an
  // exact page-JSON size (they already print it) and never for "via app"
  // rows (a watch-page byte count would be a bogus size).
  function enrichSizes(pairs) {
    try {
      const need = (pairs || []).filter((p) => p && p.sizeEl && p.st
        && !(p.st._size > 0) && !p.st.ciphered && /^https?:\/\//i.test(p.st.url || ""));
      // A row with its own audio track (`st.pair`) is that much bigger once
      // merged, so both files are measured and added.
      const pairOf = (p) => (p.st.pair && /^https?:\/\//i.test(p.st.pair.url || "") ? p.st.pair.url : "");
      const urls = [...new Set(need.flatMap((p) => [p.st.url, pairOf(p)].filter(Boolean)))].slice(0, 16);
      if (!urls.length) return;
      chrome.runtime.sendMessage({ type: "getSizes", urls, referer: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const sizes = (res && res.sizes) || {};
        for (const p of need) {
          const audio = pairOf(p);
          const n = (sizes[p.st.url] || 0) > 0 && (!audio || sizes[audio] > 0)
            ? sizes[p.st.url] + (audio ? sizes[audio] : 0)
            : 0;
          if (n > 0 && p.sizeEl.isConnected) {
            // The app gets the file's own size; the row shows the merge.
            p.st._size = sizes[p.st.url];
            p.sizeEl.textContent = fmtSize(n);
          }
        }
      });
    } catch (e) { /* sizes are best-effort */ }
  }

  // itag → quality/container for labeling sniffed live streams (page JSON
  // hides adaptive URLs until playback, but the player's traffic carries
  // itags — this rebuilds IDM-style "1080p · mp4" rows from them).
  const ITAG_Q = {17:"144p",18:"360p",22:"720p",36:"240p",43:"360p",133:"240p",134:"360p",135:"480p",136:"720p",137:"1080p",160:"144p",242:"240p",243:"360p",244:"480p",247:"720p",248:"1080p",271:"1440p",272:"2160p",278:"144p",298:"720p60",299:"1080p60",300:"1440p60",301:"2160p60",302:"720p60",303:"1080p60",308:"1440p60",313:"2160p",315:"2160p60",394:"144p",395:"240p",396:"360p",397:"480p",398:"720p",399:"1080p"};
  const ITAG_A = {139:"48k",140:"128k",141:"256k",249:"50k",250:"70k",251:"160k",599:"32k",600:"64k",601:"128k"};
  const PROG_ITAG = new Set([17,18,22,36,43]);
  function sniffItag(m) {
    try {
      const im = String(m.url || "").match(/[?&]itag=(\d+)/);
      return im ? im[1] : "";
    } catch (e) { return ""; }
  }
  // Site rules pushed by the app (newer YouTube itags, more file types):
  // they only add to the tables above, never take anything away.
  function applySiteRules(rules) {
    try {
      if (!rules || typeof rules !== "object") return;
      const yt = rules.youtube || {};
      const okLabel = (v) => typeof v === "string" && /^\d{2,4}p(?:\d{2})?$|^\d{1,3}k$/.test(v);
      for (const [k, v] of Object.entries(yt.itagQuality || {})) if (/^\d+$/.test(k) && okLabel(v)) ITAG_Q[k] = v;
      for (const [k, v] of Object.entries(yt.itagAudio || {})) if (/^\d+$/.test(k) && okLabel(v)) ITAG_A[k] = v;
      for (const n of yt.progressiveItags || []) if (Number.isInteger(n)) PROG_ITAG.add(n);
      const ext = ((rules.links || {}).linkListExtensions || []).filter((e) => /^[a-z0-9]{1,8}$/i.test(e));
      if (ext.length) DL = new RegExp(DL.source.replace(")$", "|" + ext.join("|") + ")$"), "i");
      const click = ((rules.links || {}).clickCaptureExtensions || []);
      if (DCRank.addDirectExtensions) DCRank.addDirectExtensions(click);
    } catch (e) { /* bad rules: keep the built-in ones */ }
  }
  try {
    chrome.storage.local.get({ siteRules: null }, (v) => { void chrome.runtime.lastError; applySiteRules(v && v.siteRules); });
  } catch (e) {}

  // Frame size for the panel's resolution line ("1080p" → "1920×1080").
  const RES_WH = {"4320p":"7680×4320","2160p":"3840×2160","1440p":"2560×1440","1080p":"1920×1080","720p":"1280×720","480p":"854×480","360p":"640×360","240p":"426×240","144p":"256×144"};
  function resWH(label, w, h) {
    if (w > 0 && h > 0) return `${w}×${h}`;
    const m = String(label || "").match(/^(\d+)p/);
    return (m && RES_WH[m[1] + "p"]) || String(label || "video");
  }

  // Sniffed live stream → panel cell parts (container + quality). Audio-only
  // traffic becomes an "Other Files" chip instead of a video cell.
  function sniffParts(m) {
    const itag = parseInt(sniffItag(m), 10) || 0;
    const cont = (contOf(m.mime) || "mp4").toUpperCase();
    if (itag && ITAG_Q[itag]) {
      return { video: true, format: cont + (PROG_ITAG.has(itag) ? "" : " + audio"), res: resWH(ITAG_Q[itag], 0, 0) };
    }
    if (itag && ITAG_A[itag]) return { video: false, format: cont, res: ITAG_A[itag] };
    return { video: /video/i.test(m.mime || ""), format: cont, res: "stream" };
  }

  // IDM-style network catch: merge background-sniffed playing streams
  // (googlevideo range requests from the running player) into the panel.
  // Needed because YouTube often embeds ZERO direct URLs for music/VEVO
  // videos — the page JSON has formats but no `url` fields; only the live
  // traffic carries the deciphered stream URLs. Sniffed URLs carry the
  // player's deciphered `n` parameter, so cells whose itag matches a sniffed
  // stream get their URL SWAPPED to the fresh one (stale page URLs 403).
  function appendSniffed(api, known, pairs, bestAudio, baseName) {
    try {
      const clean = String(baseName || "video").replace(/\.mp4$/i, "");
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive() || !api || !api.panel.isConnected) return;
        const all = ((res && res.items) || []).filter((m) => !(m && m.kind === "subtitle"));
        const items = all.filter((m) => m && /^https?:\/\//i.test(m.url) && !known.has(m.url));
        // Video-only traffic gets an audio track paired, so the app can merge
        // it: the page's best audio when it has a direct URL, else the audio
        // the sniffer just saw in the same playback session.
        const sniffAudio = all.find((x) => x && /^audio\//i.test(x.mime || "") && /^https?:\/\//i.test(x.url || ""));
        const pickAudio = (bestAudio && bestAudio.url)
          ? { url: bestAudio.url, mime: bestAudio.mime }
          : (sniffAudio ? { url: sniffAudio.url, mime: sniffAudio.mime } : null);
        const pairOf = (m) => (pickAudio && pickAudio.url && pickAudio.url !== m.url) ? pickAudio : null;
        const byItag = {};
        for (const m of all) {
          try {
            const im = String(m.url || "").match(/[?&]itag=(\d+)/);
            if (im && !byItag[im[1]]) byItag[im[1]] = m;
          } catch (e) {}
        }
        // Swap stale page URLs for fresh player-deciphered ones (same itag):
        // the row stops being a "via app" hint and becomes a real download.
        const swapped = [];
        const swappedItags = new Set();
        for (const p of pairs || []) {
          const tag = p.st && p.st.itag ? String(p.st.itag) : null;
          const fresh = tag && byItag[tag];
          if (fresh && fresh.url !== p.st.url && p.item && p.item.el.isConnected) {
            p.st.url = fresh.url;
            p.st.ciphered = false;
            // The row now says "+ audio": pair the audio this playback really
            // used (on ciphered pages the page lists none, so the merge used
            // to be skipped and the file came out silent).
            if (p.st.kind === "v" && pickAudio && pickAudio.url !== fresh.url) p.st._pair = pickAudio;
            const cont = (contOf(fresh.mime) || p.st.disp || "mp4").toUpperCase();
            const fmt = cont + (p.st.kind === "v" ? " + audio" : "");
            if (p.item.fmtEl) p.item.fmtEl.textContent = fmt;
            if (p.item.q) p.item.q.format = fmt;
            p.item.el.title = `${p.st.label} · ${cont} · fresh URL from the player`;
            known.add(fresh.url);
            swappedItags.add(tag);
            swapped.push(p);
          }
        }
        if (swapped.length) enrichSizes(swapped);
        if (!items.length) return;
        // Late playback can add streams after the first merge: append the
        // new ones as extra grid cells (same look as the page-JSON rows),
        // never duplicating a row that is already listed — neither a cell
        // that was just refreshed in place, nor one with the same label.
        items.slice(0, 8).forEach((m) => {
          known.add(m.url);
          const part = sniffParts(m);
          const tag = sniffItag(m);
          if (tag && swappedItags.has(tag)) return;
          if (!part.video) {
            api.addChip({
              label: `♪ ${part.format} ${part.res}`,
              tip: m.url.slice(0, 120),
              send: () => sendDownload(m.url, `${clean} [audio]${mimeExt(m.mime)}`, m.mime, m.size || null),
            });
            return;
          }
          if (api.items.some((x) => x.q && x.q.format === part.format && x.q.res === part.res)) return;
          if (tag && api.items.some((x) => x.q && x.q.st && String(x.q.st.itag) === tag)) return;
          const qtag = ITAG_Q[parseInt(tag, 10)] || part.res;
          const it = api.addQuality({
            format: part.format,
            res: part.res,
            size: m.size > 0 ? fmtSize(m.size) : "",
            tip: `${m.url.slice(0, 120)} — detected from the player`,
            send: () => sendDownload(m.url, `${clean} [${qtag}].mp4`, m.mime, m.size || null, pairOf(m)),
          });
          enrichSizes([{ item: it, sizeEl: it.sizeEl, st: { url: m.url, _size: m.size || 0 } }]);
        });
        api.sync();
        try { paintDcPanel(api.panel); } catch (e) { /* theme is best-effort */ }
      });
    } catch (e) { /* ignore */ }
  }

  // Dismiss just this video, surviving periodic refreshes and quality menu
  // rebuilds. A different video gets its own control; page reload resets it.
  const dismissedVideos = new Set();
  function dismissalKey(video, id) {
    return DCRank.videoDismissKey(location.href, video && (video.currentSrc || video.src), id);
  }
  function syncDismissControls() {
    // Remove the separate X buttons left by an older injected extension.
    document.querySelectorAll("button[data-dc-dismiss]").forEach(el=>el.remove());
    // Facebook keeps the previous and next reels mounted: one bar, on the
    // reel being watched (a scrolled-away neighbour's bar used to sit at the
    // top edge as a second button).
    let fbActive=null;
    try { if (isFacebookPage()) fbActive=fbActiveVideo(); } catch (e) { fbActive=null; }
    document.querySelectorAll("[data-dc-pill]").forEach(host=>{
      const video=host.__dcVideo;
      if (!video || !video.isConnected) { host.remove(); return; }
      const fullscreen=document.fullscreenElement;
      const parent=fullscreen && fullscreen.contains(video) ? fullscreen : document.documentElement;
      if(host.parentNode!==parent)parent.appendChild(host);
      // The part of the video that is actually visible: Facebook and others
      // draw a video wider than its player and let the player clip it.
      const r=visibleRect(video);
      const vw=window.innerWidth, vh=window.innerHeight;
      // Mostly scrolled away = not the video being watched: its bar would be
      // pinned to the window edge, over the page.
      const visH=Math.min(r.bottom,vh)-Math.max(r.top,0), visW=Math.min(r.right,vw)-Math.max(r.left,0);
      // On the player's top edge, outside the video (IDM's place): nothing
      // of the picture is covered. Only when the player touches the top of
      // the window does it go just inside the corner instead.
      // Anchored by its right edge to the player's: pointing at it grows it
      // to the left, along the player, never out past its edge.
      const right=Math.max(8,vw-r.right);
      // Where it goes: on the player's top edge, outside the video (IDM's
      // place); when a header pinned to the page (Facebook's top bar, a
      // site's sticky menu) sits there, just inside the player's corner;
      // and when that is covered too, down the player until clear of it.
      // It moves out of the header's way instead of hiding behind it.
      const free=(y)=>y>=8 && y<=vh-32 && !coveredByPinned(vw-right-12,y+15,video,host);
      let top=-1;
      const tries=[r.top-34,r.top+8];
      for(let y=r.top+24;y<=Math.min(r.bottom-40,r.top+240);y+=16)tries.push(y);
      for(const y of tries){if(free(y)){top=y;break;}}
      const shown=top>=0 && r.width>=120 && r.height>=70 && visW>0 && visH>=Math.min(r.height*0.35,200)
        && (!fbActive || video===fbActive)
        && !videoOccluded(video,host);
      if(top<0)top=Math.max(8,Math.min(vh-32,r.top+8));
      host.style.display=shown ? "block" : "none";
      if(activeBarDrag===host || applySavedBarPosition(host))return;
      host.style.position="fixed";host.style.left="auto";host.style.bottom="auto";
      host.style.width="max-content";host.style.maxWidth=Math.max(0,r.width-16)+"px";
      host.style.right=right+"px";
      host.style.top=top+"px";
    });
    const yt=document.getElementById && document.getElementById("dc-yt-btn");
    if(yt)placeInPlayer(yt,document.getElementById("dc-yt-panel"));
  }
  // The video's box cut down by every ancestor that clips its content
  // (overflow other than visible), so a bar placed in it stays inside the
  // player the user sees.
  function visibleRect(video) {
    const r = video.getBoundingClientRect();
    let left = r.left, top = r.top, right = r.right, bottom = r.bottom;
    try {
      for (let n = video.parentElement; n && n !== document.body && n !== document.documentElement; n = n.parentElement) {
        const cs = getComputedStyle(n);
        if (cs.overflowX !== "visible" || cs.overflowY !== "visible") {
          const c = n.getBoundingClientRect();
          if (c.width > 0 && c.height > 0) {
            left = Math.max(left, c.left); right = Math.min(right, c.right);
            top = Math.max(top, c.top); bottom = Math.min(bottom, c.bottom);
          }
        }
        if (cs.position === "fixed") break;
      }
    } catch (e) { /* the plain box */ }
    if (right - left < 40 || bottom - top < 40) return r;
    return { left, top, right, bottom, width: right - left, height: bottom - top };
  }
  // Is the point (x, y) covered by something pinned to the page (position
  // fixed or sticky) that is not part of this video's player? Our own bars
  // and panels are skipped; the first other element there decides.
  function coveredByPinned(x, y, video, self) {
    try {
      for (const el of document.elementsFromPoint(x, y)) {
        if (el === self || (el.closest && el.closest("[data-dc-bar],[data-dc-panel]"))) continue;
        if (el === video || el.contains(video) || video.contains(el)) return false;
        for (let n = el; n && n !== document.body && n !== document.documentElement; n = n.parentElement) {
          if (n.contains(video)) return false; // the player's own overlay
          const pos = getComputedStyle(n).position;
          if (pos === "fixed" || pos === "sticky") return true;
        }
        return false;
      }
    } catch (e) { /* best effort */ }
    return false;
  }
  // The video's player: its ancestors while they stay about the video's size
  // (Facebook and most players lay their controls over the video in there).
  function playerHull(video) {
    let hull = video;
    try {
      const vr = video.getBoundingClientRect(), area = Math.max(1, vr.width * vr.height);
      for (let n = video.parentElement; n && n !== document.body; n = n.parentElement) {
        const r = n.getBoundingClientRect();
        if (r.width * r.height > area * 1.6) break;
        hull = n;
      }
    } catch (e) { /* the video alone */ }
    return hull;
  }
  // Is the video itself hidden under something that is not its player: a
  // post dialog opened over the feed, a large modal, a lightbox? Then its bar
  // would float over that layer (a Facebook post's comments). Checked just
  // inside the video's top-right corner, where the bar points.
  function videoOccluded(video, self) {
    try {
      const r = visibleRect(video);
      const vw = window.innerWidth, vh = window.innerHeight;
      const x = Math.min(vw - 2, Math.max(1, r.right - 14)), y = Math.min(vh - 2, Math.max(1, r.top + 14));
      const hull = playerHull(video);
      for (const el of document.elementsFromPoint(x, y)) {
        if (el === self || (el.closest && el.closest("[data-dc-bar],[data-dc-panel]"))) continue;
        if (el === video || el.contains(video) || hull.contains(el)) return false;
        // Only a separate floating layer hides the video: the nearest fixed
        // or dialog ancestor of what is on top, when that layer does not
        // hold this video. Overlays in the video's own layer (Facebook draws
        // wide ones over a post dialog's video) never count.
        for (let n = el; n && n !== document.body && n !== document.documentElement; n = n.parentElement) {
          const role = n.getAttribute && n.getAttribute("role");
          const layer = role === "dialog" || (n.getAttribute && n.getAttribute("aria-modal") === "true")
            || n.tagName === "DIALOG" || getComputedStyle(n).position === "fixed";
          if (layer) return !n.contains(video);
        }
        return false;
      }
    } catch (e) { /* best effort */ }
    return false;
  }
  function addDismissControl(button, key, panel) {
    const close=button.__dcClose;
    if(!close)return;
    close.hidden=false;
    close.onclick = ev => {
      ev.preventDefault();ev.stopPropagation();
      if(dismissedVideos.size>=100)dismissedVideos.clear();
      dismissedVideos.add(typeof key==="function" ? key() : key);
      button.remove();
      if(panel)panel.remove();
      if(button.__dcFbPanel)button.__dcFbPanel.remove();
    };
  }
  document.addEventListener("scroll",syncDismissControls,true);
  window.addEventListener("resize",syncDismissControls);
  document.addEventListener("fullscreenchange",syncDismissControls);

  // The video whose bar was clicked before its qualities were known.
  let ytOpenWhenReady = "";
  function ensureButton(info) {
    const key = dismissalKey(null,info.videoId);
    if (dismissedVideos.has(key)) {
      const old=document.getElementById("dc-yt-btn");
      if(old){if(old.__dcClose)old.__dcClose.remove();old.remove();}
      const panel=document.getElementById("dc-yt-panel");if(panel)panel.remove();
      return;
    }
    syncDismissControls();
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldPanel = document.getElementById("dc-yt-panel");
    if (oldBtn && oldBtn.dataset.gen !== String(GEN)) {
      // Adopted DOM from an older live instance (e.g. re-injected after
      // an update): always rebuild so the handlers belong to this
      // (current) generation.
      oldBtn.remove();
      if (oldPanel) oldPanel.remove();
    }
    // Rebuild whenever the content signature changes — not just the video
    // id. Expiries AND url sets are included so refreshed (re-issued) URLs
    // rebuild the menu instead of serving the stale 403-prone rows forever.
    const expSig = (l) => (l || []).map((s) => {
      try {
        const m = String(s.url || "").match(/[?&]expire=(\d+)/);
        return m ? m[1] : "0";
      } catch (e) { return "0"; }
    }).join(",");
    const urlSig = (l) => {
      // Cheap rotation fingerprint: itags + url-length checksum. Same-second
      // re-issues with identical expiries still rebuild.
      let h = 0;
      for (const s of (l || [])) {
        const u = String(s.url || "");
        h = (h * 31 + (s.itag || 0)) >>> 0;
        h = (h * 31 + u.length) >>> 0;
        for (let i = 0; i < Math.min(u.length, 48); i++) h = (h * 31 + u.charCodeAt(i)) >>> 0;
      }
      return String(h);
    };
    const sigParts = [
      info.videoId, info.title,
      (info.streams || []).length,
      (info.videoOnly || []).length,
      (info.audio || []).length,
      expSig(info.streams), expSig(info.videoOnly), expSig(info.audio),
      urlSig(info.streams), urlSig(info.videoOnly), urlSig(info.audio),
      // Sizes arrive later on SPA navigation (fetched watch page): rebuild.
      Object.keys(info.sizes || {}).length,
    ];
    const sig = sigParts.join("|");
    // NOTE: the old button/panel were already dropped above when they
    // belonged to an older generation, so reaching here means same-generation
    // DOM.
    let keepFaded = false;
    if (oldBtn && oldBtn.isConnected) {
      if (oldBtn.dataset.sig === sig) {
        // Same content: keep everything, just re-anchor (the player
        // element can be recreated by SPA navigation).
        placeInPlayer(oldBtn, oldPanel);
        addDismissControl(oldBtn,key,oldPanel);
        return;
      }
      // Rebuilt for the same video (its qualities arrived): keep it faded
      // if it already faded, instead of flashing it solid again.
      keepFaded = oldBtn.dataset.vid === info.videoId && !oldBtn.hasAttribute("data-dc-awake");
      oldBtn.remove();
      if (oldPanel) oldPanel.remove();
    }
    const btn = videoDownloadBar(dcT("barChooseQuality", "Choose video quality in Download Craft"));
    btn.dataset.vid = info.videoId || "";
    if (keepFaded) {
      clearTimeout(btn.__dcAwakeT);
      btn.removeAttribute("data-dc-awake");
    }
    btn.id = "dc-yt-btn";
    btn.dataset.vid = info.videoId;
    btn.dataset.gen = String(GEN);
    btn.dataset.sig = sigParts.join("|");
    const fname = safeFilename(info.title, info.videoId);
    const tagged = (base, st) => {
      const clean = base.replace(/\.mp4$/i, "");
      // Rows the app resolves itself share the page URL: the quality in the
      // name keeps 1080p and 720p of one video as separate files.
      if (st.kind === "v" || (st.ciphered && st.label)) return `${clean} [${st.label}].mp4`;
      if (st.kind === "a") return `${clean} [audio]${mimeExt(st.mime)}`;
      return base;
    };
    // Video-only cells bundle the best audio track so the app merges them to
    // a single file (IDM's "MKV file" behavior).
    const bestAudio = (info.audio || []).slice().sort((a, b) => (b.br || 0) - (a.br || 0))[0] || null;
    const pairFor = (st) => {
      if (st._pair && st._pair.url && st._pair.url !== st.url) return { url: st._pair.url, mime: st._pair.mime };
      return (st.kind === "v" && bestAudio && bestAudio.url !== st.url)
        ? { url: bestAudio.url, mime: bestAudio.mime }
        : null;
    };
    const sendStream = (st, name) => {
      // Ciphered rows have no URL at build time: hand the page + quality hint
      // over and the app deciphers that exact quality itself.
      if (st.ciphered && !st.url) {
        sendDownload(location.href, name, st.mime, st._size || null, pairFor(st), st.label, st._dur || null);
      } else {
        // A direct row already carries the exact stream, but the quality hint
        // travels with it too: if the app ever has to re-resolve the page (a
        // stale signed URL), it must not silently fall back to "best".
        sendDownload(st.url || location.href, name, st.mime, st._size || null, pairFor(st), st.label, st._dur || null);
      }
    };
    // One cell per quality. The same height + container twice (a progressive
    // row and its video-only twin) keeps the progressive one: it plays as-is.
    const seenQ = new Set();
    const entries = [];
    const addEntry = (st, kind, viaApp) => {
      const hh = parseInt((String(st.label || "").match(/^(\d+)/) || [0, 0])[1], 10) || 0;
      const key = `${hh}|${String(st.disp || "").toLowerCase()}`;
      if (!hh || seenQ.has(key)) return;
      seenQ.add(key);
      const cont = (st.disp || contOf(st.mime) || "mp4").toUpperCase();
      const fps = (String(st.label || "").match(/^(\d+)p(\d+)$/) || [])[2];
      // A video-only cell is downloaded together with the paired audio leg, so
      // its shown size has to include that leg or the row's number never
      // matches the merged file the app reports (the complaint that started
      // this: the panel's size and the progress window's size disagreed).
      const audioSz = (kind === "v" && !viaApp && bestAudio && bestAudio._size > 0)
        ? bestAudio._size
        : 0;
      const exact = st._size > 0;
      entries.push({
        h: hh,
        size: (exact ? st._size : (st._est || 0)) + audioSz,
        approx: !exact && st._est > 0,
        dur: st._dur || 0,
        st,
        // Ciphered/"via app" rows have no container pairing of their own —
        // the app decides, so no "+ audio" marker there.
        format: cont + (viaApp ? " via app" : (kind === "v" ? " + audio" : "")),
        res: resWH(st.label, st.w, st.h),
        tip: `${st.label}${fps ? ` (${fps} fps)` : ""} · ${cont} · ${info.title}`
          + (kind === "v" ? " — merged with the audio track when the download finishes" : "")
          + (viaApp ? " — the app resolves this quality itself" : ""),
        send: () => sendStream(st, tagged(fname, st)),
      });
    };
    for (const st of (info.streams || [])) addEntry(st, "av", !!st.ciphered);
    for (const st of (info.videoOnly || [])) addEntry(st, "v", !!st.ciphered);
    // Full ladder fallback: when the page barely exposes direct rows
    // (ciphered embeds, music videos), still offer every standard quality like
    // IDM does — the app resolves each to the closest stream at or below it.
    // Sizes come from the page-JSON map (contentLength per quality).
    if (entries.length < 6) {
      const sizeByQ = {};
      for (const [q, m] of Object.entries(info.sizes || {})) {
        if (!sizeByQ[q] && m && m.size > 0) sizeByQ[q] = m.size;
      }
      for (const s of [...(info.streams || []), ...(info.videoOnly || [])]) {
        if (s && s.label && s._size > 0 && !sizeByQ[s.label]) sizeByQ[s.label] = s._size;
      }
      const sizeForQ = (q) => {
        if (sizeByQ[q]) return sizeByQ[q];
        const k = Object.keys(sizeByQ).find((x) => x.startsWith(q));
        return k ? sizeByQ[k] : 0;
      };
      const haveH = new Set(entries.map((e) => e.h));
      const ladderDur = Math.max(0, ...Object.values(info.sizes || {}).map((m) => (m && m.dur) || 0));
      // Every height the page's player data lists (1440p, 4K, 8K included,
      // even when it hides their links), filled out with the usual steps
      // below the tallest. With nothing listed, the full ladder up to 4K:
      // the app takes the closest quality at or below the one picked.
      const listed = Object.keys(sizeByQ)
        .map((q) => parseInt((q.match(/^(\d+)p/) || [0, 0])[1], 10) || 0)
        .filter((h) => h >= 144);
      const tallest = listed.length ? Math.max(...listed) : 2160;
      const ladder = [...new Set([...listed, 4320, 2160, 1440, 1080, 720, 480, 360, 240, 144])]
        .filter((h) => h <= tallest)
        .sort((a, b) => b - a);
      for (const q of ladder) {
        if (haveH.has(q)) continue;
        const label = `${q}p`;
        const sz = sizeForQ(label);
        entries.push({
          h: q,
          size: sz,
          dur: ladderDur,
          st: null,
          format: "MP4 via app",
          res: RES_WH[label],
          tip: `${label} · MP4 · the app resolves this quality itself`,
          send: () => sendDownload(location.href, `${fname.replace(/\.mp4$/i, "")} [${label}].mp4`, "video/mp4", sz || null, null, label, ladderDur || null),
        });
      }
    }
    entries.sort((a, b) => (b.h - a.h) || (b.size - a.size));
    // "Direct" means at least one row the browser already holds a real URL
    // for — ciphered pages (VEVO) only carry qualities the app must resolve,
    // so they get the mockup's "press play" hint instead.
    const hasDirect = entries.some((e) => e.st && e.st.url);
    const infoOf = (e) => ({
      res: e ? e.res : "",
      size: e && e.size > 0 ? (e.approx ? "~" : "") + fmtSize(e.size) : "",
      format: e ? e.format.split(" ")[0] : "MP4",
    });
    let api = null;
    api = dcPanel({
      title: info.title,
      thumb: info.thumb,
      dur: entries[0] && entries[0].dur > 0 ? fmtDurSec(entries[0].dur) : "",
      res: infoOf(entries[0]).res,
      size: infoOf(entries[0]).size,
      format: infoOf(entries[0]).format,
      hint: hasDirect
        ? "Pick a quality below, or send the link as-is."
        : "No direct links yet — press play, then reopen for fresh qualities.",
      onSendLink: () => sendDownload(location.href, fname),
      primaryLabel: entries.length ? "Download" : "Send to app",
      onPrimary: () => {
        const it = api.selected();
        if (it && it.q && it.q.send) it.q.send();
        else sendDownload(location.href, fname);
      },
    });
    api.panel.id = "dc-yt-panel";
    const sizePairs = [];
    entries.forEach((e) => {
      const it = api.addQuality({
        format: e.format,
        res: e.res,
        size: e.size > 0 ? (e.approx ? "~" : "") + fmtSize(e.size) : "",
        tip: e.tip,
        short: e.h > 0 ? e.h + "p" : "",
        send: () => { rememberQuality(e.h + "p"); e.send(); },
        onPick: () => api.setInfo(infoOf(e)),
      });
      if (e.st) sizePairs.push({ item: it, sizeEl: it.sizeEl, st: e.st });
    });
    // Opens on the remembered quality (the nearest at or below it), and from
    // here on a pick relabels the button.
    if (oneClickOn && prefQuality && prefQuality !== "audio" && typeof DCRank !== "undefined" && DCRank.pickHeight) {
      const qi = DCRank.pickHeight(entries.map((e) => e.h), prefQuality);
      if (qi >= 0 && api.items[qi]) { api.select(api.items[qi]); api.setInfo(infoOf(entries[qi])); }
    }
    api.bar = btn;
    // "Other Files": subtitle tracks and audio-only streams as chips.
    for (const sb of (info.subs || [])) {
      api.addChip({
        label: `📄 VTT ${sb.label}`,
        tip: sb.url.slice(0, 120),
        send: () => sendDownload(sb.url, `${fname} [${sb.lang || sb.label}].vtt`, "text/vtt", null, null, null, null),
      });
    }
    // The sound alone, always offered (YouTube hides its audio links on
    // most videos now): the app reads the page itself and saves the M4A as
    // served, or an MP3 made from it. Sizes from the page's player data.
    const audioSize = info.audioSize || 0;
    const stem = fname.replace(/\.mp4$/i, "");
    api.addChip({
      label: "♪ MP3",
      tip: `Audio only, as MP3${audioSize ? " · about " + fmtSize(Math.round(audioSize * 1.9)) : ""} — the app saves the sound and converts it`,
      send: () => { rememberQuality("audio"); sendDownload(location.href, `${stem}.mp3`, "audio/mpeg", null, null, "audio", info.audioDur || null); },
    });
    api.addChip({
      label: "♪ M4A",
      tip: `Audio only, as M4A${audioSize ? " · " + fmtSize(audioSize) : ""} — YouTube's own sound track, no conversion`,
      send: () => { rememberQuality("audio"); sendDownload(location.href, `${stem}.m4a`, "audio/mp4", audioSize || null, null, "audio-m4a", info.audioDur || null); },
    });
    api.sync();
    enrichSizes(sizePairs);
    markQueuedRows(sizePairs);
    // One click: the remembered quality among this video's rows (the
    // nearest one at or below it), or its best audio track.
    btn.__dcQuickFor = () => {
      if (!prefQuality) return null;
      if (prefQuality === "audio") {
        return { label: dcT("barAudio", "audio"), run: () => sendDownload(location.href, `${fname.replace(/\.mp4$/i, "")}.mp3`, "audio/mpeg", null, null, "audio", info.audioDur || null) };
      }
      const i = typeof DCRank !== "undefined" && DCRank.pickHeight ? DCRank.pickHeight(entries.map((e) => e.h), prefQuality) : -1;
      return i >= 0 ? { label: entries[i].h + "p", run: entries[i].send } : null;
    };
    applyQuick(btn);
    appendSniffed(api, new Set([
      ...info.streams.map((s) => s.url),
      ...(info.videoOnly || []).map((s) => s.url),
      ...(info.audio || []).map((s) => s.url),
    ]), sizePairs, bestAudio, fname);
    // Ready = the page has handed over this video's qualities. A click before
    // that waits ("Resolving…"): each refresh rebuilds the bar, and the first
    // rebuild that is ready opens its own menu.
    const ready = (info.streams || []).length + (info.videoOnly || []).length > 0
      || Object.keys(info.sizes || {}).length > 0;
    btn.dataset.ready = ready ? "1" : "";
    const open = () => {
      barBusy(btn, null);
      hidePanels();
      api.show();
      placePanelBy(btn, api.panel);
      openMenu = { btn, panel: api.panel };
    };
    btn.__dcOpen = open;
    btn.onclick = (ev) => {
      ev.stopPropagation();
      if (btn.__dcBusy) return;
      if (api.panel.style.display !== "none") { api.hide(); return; }
      if (ready) { open(); return; }
      ytOpenWhenReady = info.videoId;
      barBusy(btn, dcT("busyResolving", "Resolving…"));
      waitFor(() => {
        refreshYouTube();
        const cur = document.getElementById("dc-yt-btn");
        return cur && cur.dataset.ready === "1" ? cur : null;
      }, (cur) => {
        if (ytOpenWhenReady !== info.videoId) return; // opened already
        ytOpenWhenReady = "";
        // Still nothing after the wait: the menu can still hand the page to
        // the app, which resolves the video itself.
        const now = cur || document.getElementById("dc-yt-btn");
        if (now && now.__dcOpen) now.__dcOpen();
      });
    };
    installDcCloser();
    placeInPlayer(btn, api.panel);
    addDismissControl(btn,key,api.panel);
    if (ytOpenWhenReady === info.videoId) {
      if (ready) { ytOpenWhenReady = ""; setTimeout(open, 0); }
      else barBusy(btn, dcT("busyResolving", "Resolving…"));
    }
  }

  // The popup's ⏻ switch: with detection off no bar is shown anywhere.
  let detectionOn = true;
  try {
    chrome.storage.local.get({ detection: true }, (r) => {
      detectionOn = !(r && r.detection === false);
      if (!detectionOn) { try { refreshYouTube(); ensureUniversalButtons(); } catch (e) { /* ignore */ } }
    });
    chrome.storage.onChanged.addListener((ch, area) => {
      if (area !== "local" || !ch.detection) return;
      detectionOn = ch.detection.newValue !== false;
      try { refreshYouTube(); ensureUniversalButtons(); } catch (e) { /* ignore */ }
    });
  } catch (e) { /* storage unavailable: stay on */ }

  // The video whose qualities were read from the page (self-check).
  let ytParsedFor = "";
  function youtubeSelfCheck() {
    const id = ytVideoId();
    if (!id || !detectionOn) return;
    const player = ytPlayer();
    const waiting = document.getElementById("dc-yt-btn");
    if (waiting && waiting.style.display === "none" && player) placeInPlayer(waiting, document.getElementById("dc-yt-panel"));
    layoutCheck("yt-player", !player, "YouTube: no video player element found on a video page (the layout may have changed); the button sits in the window corner");
    const shorts = /^\/shorts\//.test(location.pathname);
    if (shorts) {
      layoutCheck("yt-shorts-row", !document.querySelector("ytd-shorts-player-controls #right-controls, #shorts-player-controls #right-controls, ytd-reel-video-renderer #right-controls"),
        "YouTube Shorts: the top control row (#right-controls) is gone (the layout may have changed); the button sits in the player corner");
    }
    // The qualities come from the page's own player data; without it the
    // app still resolves the video, only slower and without the menu sizes.
    if (!shorts) layoutCheck("yt-data", ytParsedFor !== id, "YouTube: the page's player data could not be read (the page may have changed); the app resolves videos itself", 20000);
  }

  function refreshYouTube() {
    const id = detectionOn ? ytVideoId() : null;
    const oldBtn = document.getElementById("dc-yt-btn");
    const oldPanel = document.getElementById("dc-yt-panel");
    if (!id) {
      if (oldBtn) oldBtn.remove();
      if (oldPanel) oldPanel.remove();
      try {
        if (IS_TOP) chrome.storage.local.set({ downloadCraftVideos: [] });
      } catch (e) {
        /* ignore */
      }
      return false;
    }
    // Direct page streams when YouTube exposes them (fast path with
    // quality choice); otherwise the app resolves the video itself via
    // its player-API fallback — so the button always appears.
    // ytStreams() matches the CURRENT video id, so SPA autoplay can
    // never show the previous video's title/streams.
    const parsed = ytStreams();
    ytParsedFor = parsed ? id : "";
    const info = parsed || { videoId: id, title: ytTitleText(document.title.replace(/\s*-\s*YouTube\s*$/i, "")) || `youtube_${id}`, thumb: `https://i.ytimg.com/vi/${id}/hqdefault.jpg`, streams: [], videoOnly: [], audio: [], subs: [] };
    try {
      if (IS_TOP) chrome.storage.local.set({
        downloadCraftVideos: [{
          videoId: info.videoId, title: info.title, thumb: info.thumb || "",
          streams: info.streams, videoOnly: info.videoOnly || [], audio: info.audio || [],
          sizes: info.sizes || {}, subs: info.subs || [],
          pageUrl: location.href,
        }],
        downloadCraftPage: location.href,
      });
    } catch (e) {
      /* ignore */
    }
    ensureButton(info);
    return true;
  }

  // ---------- Universal video overlay (IDM-style) ----------
  // Floating "Download this video" pill on EVERY playable <video>, not just
  // YouTube. Uses the element's currentSrc when it is http(s), otherwise
  // falls back to network-sniffed candidates from the background worker or
  // the page URL (engine resolves og:video / VideoObject server-side).
  function mediaUrlOf(el) {
    try {
      const u = el.currentSrc || el.src || "";
      if (/^https?:\/\//i.test(u)) return u;
      const s = el.querySelector && el.querySelector("source[src]");
      if (s && /^https?:\/\//i.test(s.src)) return s.src;
    } catch (e) { /* ignore */ }
    return "";
  }

  function ensureUniversalButtons() {
    if (!alive()) return;
    if (!detectionOn) {
      document.querySelectorAll("[data-dc-pill]").forEach((el) => { if (el.__dcClose) el.__dcClose.remove(); el.remove(); });
      return;
    }
    syncDismissControls();
    // YouTube watch/Shorts pages already have the quality menu button —
    // a second generic pill on the same player is the "double button" bug.
    try { if (ytVideoId()) return; } catch (e) { /* ignore */ }
    try {
      // Drop pills left by an older injected generation (their handlers are
      // dead after an extension reload) so only this generation's pills show.
      document.querySelectorAll("[data-dc-pill]").forEach((el) => {
        if (el.dataset.gen !== String(GEN)) el.remove();
      });
      const vids = pageVideos().slice(0, 10);
      for (const v of vids) {
        const key = dismissalKey(v,null);
        if (dismissedVideos.has(key)) continue;
        if (v.__dcBtn) {
          if (!v.__dcBtn.isConnected) v.__dcBtn = null;
          else if (v.__dcKey === key) continue;
          else { if(v.__dcBtn.__dcClose)v.__dcBtn.__dcClose.remove(); v.__dcBtn.remove();v.__dcBtn=null; }
        }
        // Skip anything inside YouTube's player (covered by its own button).
        try { if (v.closest && v.closest(".html5-video-player")) continue; } catch (e) {}
        // Skip tiny preview thumbnails.
        const r = v.getBoundingClientRect ? v.getBoundingClientRect() : null;
        if (r && (r.width < 120 || r.height < 70)) continue;
        const pill = videoDownloadBar(dcT("barDownloadVideo", "Download this video with Download Craft"));
        pill.setAttribute("data-dc-pill", "1");
        pill.dataset.gen = String(GEN);
        pill.style.position = "absolute";
        pill.style.top = "8px";
        pill.style.right = "8px";
        pill.style.zIndex = "2147483646";
        pill.onclick = (ev) => {
          ev.stopPropagation(); ev.preventDefault();
          if (drmProtected(v)) { toast(dcT("drmProtected", DRM_NOTE)); return; }
          // Facebook reels: open the IDM-style quality menu (title + HD/SD
          // rows with sizes) instead of firing one mystery download — the
          // player only holds a blob: URL and every reel needs no reload.
          if (isFacebookPage()) {
            handleFbPill(pill);
            return;
          }
          if (isSocialPage()) {
            const post = socialPostFor(v);
            layoutCheck("social-post-" + location.hostname, !post,
              `${location.hostname}: no post link found next to a video (the site's layout may have changed); the page was sent instead`, 0);
            if (post) {
              openSocialMenu(pill, post, v);
              return;
            }
          }
          const direct = mediaUrlOf(v);
          if (direct) {
            const base = mediaBase(direct);
            const ext = /\.mp3|\.m4a|\.wav|\.ogg/i.test(direct) ? "" : ".mp4";
            let vdur = null;
            try {
              const d = Math.round(v.duration);
              if (Number.isFinite(d) && d > 0 && d < 86400) vdur = d;
            } catch (e) { /* duration unknown */ }
            sendDownload(direct, base + ext, v.getAttribute && v.getAttribute("type"), null, null, null, vdur);
          } else {
            // Ask background for sniffed media URLs for this tab first.
            // (Facebook reels render as blob: players — the real fbcdn/mp4
            // URL only exists in the sniffed network traffic.) Clicked before
            // the player fetched anything: wait for it ("Capturing…") rather
            // than sending the bare page.
            if (pill.__dcBusy) return;
            const src = String((v && (v.currentSrc || v.src)) || "");
            const pick = (items) => bestSniffed(items, { streamPlayer: /^blob:/i.test(src) || !!(v && v.srcObject) });
            barBusy(pill, dcT("busyCapturing", "Capturing…"));
            waitFor(() => sniffedNow().then(pick), (best) => {
              barBusy(pill, null);
              if (best) {
                const base = mediaBase(best.url);
                // Page readers hand over a video track with its audio pair
                // (merged by the app) and the player's duration.
                const pair = best.pairUrl ? { url: best.pairUrl, mime: best.pairMime || "audio/mp4" } : null;
                let vdur = best.dur || null;
                try { const d = Math.round(v.duration); if (!vdur && Number.isFinite(d) && d > 0 && d < 86400) vdur = d; } catch (e) {}
                sendDownload(best.url, base + ".mp4", best.mime, best.size || null, pair, null, vdur);
              } else {
                sendDownload(location.href, undefined, null);
              }
            }, 14, 600);
          }
        };
        if (isSocialPage()) {
          pill.__dcQuickFor = () => { const post = socialPostFor(v); return post ? socialQuick(post, v) : null; };
          applyQuick(pill);
        }
        try {
          pill.__dcVideo=v;
          document.documentElement.appendChild(pill);
          v.__dcBtn=pill;v.__dcKey=key;
          addDismissControl(pill,()=>dismissalKey(v,null),null);
          syncDismissControls();
        } catch (e) { /* overlay is best-effort */ }
      }
    } catch (e) { /* ignore */ }
  }

  // ---------- Remembered quality ----------
  // The quality picked last ("720p", or "audio") is what the button shows
  // and what its menu has selected when it opens; picking another one in the
  // menu relabels the button at once. Popup → Settings switches this off.
  let prefQuality = "";
  let oneClickOn = true;
  function rememberQuality(q) {
    if (!q || q === prefQuality) return;
    prefQuality = q;
    try { chrome.storage.local.set({ dcQuality: q }); } catch (e) { /* this page only */ }
    refreshQuickBars();
  }
  function refreshQuickBars() {
    try { document.querySelectorAll("[data-dc-bar]").forEach(applyQuick); } catch (e) { /* ignore */ }
  }
  try {
    chrome.storage.local.get({ dcQuality: "", oneClickQuality: true }, (r) => {
      if (chrome.runtime.lastError || !r) return;
      prefQuality = String(r.dcQuality || "");
      oneClickOn = r.oneClickQuality !== false;
      refreshQuickBars();
    });
    chrome.storage.onChanged.addListener((ch, area) => {
      if (area !== "local" || !alive()) return;
      if (ch.dcQuality) prefQuality = String(ch.dcQuality.newValue || "");
      if (ch.oneClickQuality) oneClickOn = ch.oneClickQuality.newValue !== false;
      if (ch.dcQuality || ch.oneClickQuality) refreshQuickBars();
    });
  } catch (e) { /* menu only */ }
  // host.__dcQuickFor() -> {label, run} | null decides what one click does.
  function applyQuick(host) {
    try {
      const q = oneClickOn && host.__dcQuickFor ? host.__dcQuickFor() : null;
      host.__dcQuick = q;
      const lab = host.__dcLabel;
      // Always just "Download": the quality the user picks most is what
      // the menu has selected when it opens.
      const text = dcT("barDownload", "Download");
      if (lab && !host.__dcBusy && lab.textContent !== text) {
        lab.textContent = text;
        host.__dcIdleText = undefined;
        host.__dcW = 0;
      }
      // One control: the button opens the menu (pre-set to this quality),
      // so the separate ▾ stays hidden.
      if (host.__dcMore) host.__dcMore.hidden = true;
    } catch (e) { /* cosmetic */ }
  }
  // "Download 720p" on a bar while its menu has 720p selected.
  function barShowQuality(host, q) {
    try {
      const lab = host && host.__dcLabel;
      if (!lab || host.__dcBusy || !q) return;
      // The menu shows the pick; the button keeps its plain label.
      const text = dcT("barDownload", "Download");
      if (lab.textContent !== text) { lab.textContent = text; host.__dcIdleText = undefined; host.__dcW = 0; }
    } catch (e) { /* cosmetic */ }
  }
  // Was the click on the bar's ▾ (open the menu) rather than the button?
  function clickedMore(ev, host) {
    try { return !!host.__dcMore && ev.composedPath().includes(host.__dcMore); } catch (e) { return false; }
  }

  // ---------- Self-check: tell the app when a site's layout changed ----------
  // Something the button relies on (YouTube's control row, its player data,
  // a post's link) missing for a while is reported once per page; the
  // extension's worker keeps one report per day per kind, and the app shows
  // it in its log. Brief gaps (a feed redrawing) are not reported.
  const layoutMissingSince = new Map();
  const layoutReported = new Set();
  function layoutCheck(key, missing, text, graceMs = 8000) {
    try {
      if (!missing) { layoutMissingSince.delete(key); return; }
      const now = Date.now();
      if (!layoutMissingSince.has(key)) { layoutMissingSince.set(key, now); if (graceMs > 0) return; }
      if (now - layoutMissingSince.get(key) < graceMs || layoutReported.has(key)) return;
      layoutReported.add(key);
      chrome.runtime.sendMessage({ type: "layoutReport", key, text, page: location.hostname }, () => void chrome.runtime.lastError);
    } catch (e) { /* best effort */ }
  }

  // ---------- Instagram, TikTok, X, Reddit ----------
  // The post the video under the button belongs to. In a feed many posts
  // play at once: the post's own links (Reddit's permalink attribute,
  // TikTok's player id) name it. The climb stops where a container holds
  // more than one video: past that point the links belong to other posts.
  function socialPostFor(video) {
    const R = typeof DCRank !== "undefined" ? DCRank : null;
    if (!R || !R.socialPostUrl) return "";
    const site = R.socialSite(location.hostname);
    let el = video;
    // Out of a player's shadow DOM (Reddit) into the post around it.
    const up = (n) => n.parentElement || (n.getRootNode && n.getRootNode() !== document ? n.getRootNode().host : null) || null;
    for (let depth = 0; el && depth < 16; depth++, el = up(el)) {
      if (el.getAttribute) {
        const pl = el.getAttribute("permalink") || el.getAttribute("data-permalink") || el.getAttribute("content-href");
        if (pl) {
          const u = R.socialPostUrl(pl, location.origin);
          if (u) return u;
        }
        const tt = site === "tiktok.com" && /(?:^|-)(\d{15,25})$/.exec(el.id || "");
        if (tt) {
          // The post's own link (with the author) when the page has one.
          const a = document.querySelector(`a[href*="/video/${tt[1]}"]`);
          return (a && R.socialPostUrl(a.getAttribute("href"), location.origin)) || `https://www.tiktok.com/@/video/${tt[1]}`;
        }
      }
      if (depth >= 1 && el.querySelectorAll) {
        if (el.querySelectorAll("video").length > 1) break;
        for (const a of el.querySelectorAll("a[href]")) {
          const u = R.socialPostUrl(a.getAttribute("href"), location.origin);
          if (u) return u;
        }
      }
    }
    return R.socialPostUrl(location.href);
  }
  // Every <video> on the page, including those inside a player's open
  // shadow DOM (Reddit's shreddit-player keeps its video there).
  function pageVideos() {
    const out = [...document.querySelectorAll("video")];
    try {
      for (const host of document.querySelectorAll("shreddit-player, shreddit-player-2, [data-dc-shadow-player]")) {
        if (host.shadowRoot) out.push(...host.shadowRoot.querySelectorAll("video"));
      }
    } catch (e) { /* light DOM only */ }
    return out;
  }
  function isSocialPage() {
    try { return !!(typeof DCRank !== "undefined" && DCRank.socialSite && DCRank.socialSite(location.hostname)); } catch (e) { return false; }
  }
  const SOCIAL_QUALITIES = ["best", "1080p", "720p", "480p", "360p"];
  function socialSend(post, q, video) {
    const title = String(document.title || "video").replace(/\s*[|•·]\s*(Instagram|TikTok|X|Reddit).*$/i, "").replace(/ on X: .*$/i, "").trim().slice(0, 90) || "video";
    const stem = title.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/\.+$/, "").trim() || "video";
    const audio = q === "audio";
    const name = audio ? `${stem}.mp3` : q && q !== "best" ? `${stem} [${q}].mp4` : `${stem}.mp4`;
    let dur = null;
    try { const d = Math.round(video.duration); if (Number.isFinite(d) && d > 0 && d < 86400) dur = d; } catch (e) { /* unknown */ }
    sendDownload(post, name, audio ? "audio/mpeg" : "video/mp4", null, null, q === "best" ? null : q, audio ? null : dur);
  }
  function socialQuick(post, video) {
    if (!prefQuality) return null;
    return { label: prefQuality === "audio" ? dcT("barAudio", "audio") : prefQuality, run: () => socialSend(post, prefQuality, video) };
  }
  // Instagram / TikTok / X / Reddit: the post's qualities (the app fetches
  // the one picked with its sound), plus audio only as MP3.
  function openSocialMenu(pill, post, video) {
    try { document.querySelectorAll("#dc-social-panel").forEach((m) => m.remove()); } catch (e) {}
    let api = null;
    const pick = (q) => { rememberQuality(q); socialSend(post, q, video); };
    api = dcPanel({
      title: String(document.title || "Video").slice(0, 120),
      thumb: (video && video.poster) || "",
      dur: (() => { try { const d = video.duration; return Number.isFinite(d) && d > 0 ? fmtDurSec(d) : ""; } catch (e) { return ""; } })(),
      res: "", size: "", format: "MP4",
      hint: dcT("panelSocialHint", "The app fetches this post's video with its sound."),
      onSendLink: () => pick("best"),
      onPrimary: () => { const it = api.selected(); if (it && it.q && it.q.send) it.q.send(); },
    });
    api.panel.id = "dc-social-panel";
    for (const q of SOCIAL_QUALITIES) {
      const res = q === "best" ? dcT("qualityBest", "Best") : q;
      const it = api.addQuality({ format: "MP4", res, size: "", tip: `${res} · MP4`, send: () => pick(q), onPick: () => api.setInfo({ res, size: "", format: "MP4" }) });
      if (q === prefQuality) { api.select(it); api.setInfo({ res, size: "", format: "MP4" }); }
    }
    api.bar = pill;
    api.addChip({ label: "♪ MP3", tip: dcT("chipAudioOnly", "Audio only, as MP3"), send: () => pick("audio") });
    installDcCloser();
    api.show();
    placePanelBy(pill, api.panel);
  }

  function isFacebookPage() {
    try {
      const h = location.hostname.toLowerCase();
      return h === "facebook.com" || h.endsWith(".facebook.com");
    } catch (e) { return false; }
  }

  // Facebook quality panel (IDM-style): the same circle + bubble as YouTube,
  // with the reel's HD/SD rows (and any fresh sniffed stream) as quality
  // cells, so every reel offers a choice instead of one mystery "Facebook.mp4".
  function openFbMenu(pill, rows, title, thumb, audio, pair) {
    try { document.querySelectorAll("#dc-fb-panel").forEach((m) => m.remove()); } catch (e) {}
    const first = (rows && rows[0]) || {};
    const fname = first.fname || fbFilename(title);
    let api = null;
    api = dcPanel({
      title,
      thumb: thumb || first.thumb || "",
      dur: first.dur > 0 ? fmtDurSec(first.dur) : "",
      res: "",
      size: "",
      format: "MP4",
      hint: dcT("panelHint", "Pick a quality below, or send the link as-is."),
      onSendLink: () => sendDownload(location.href, fname, null, null, null, null, first.dur || null),
      onPrimary: () => {
        const it = api.selected();
        if (it && it.q && it.q.send) it.q.send();
      },
    });
    api.panel.id = "dc-fb-panel";
    pill.__dcFbPanel=api.panel;
    const sizePairs = [];
    (rows || []).forEach((st) => {
      const qm = String(st.label || "").match(/quality\s+(.+?)(?:,|$)/i);
      const q = {
        res: (qm && qm[1]) || "video",
        size: st._size > 0 ? fmtSize(st._size) : st._est > 0 ? "~" + fmtSize(st._est) : "",
        format: "MP4",
        tip: `${(qm && qm[1]) || "video"} · MP4 · ${title}`,
        send: () => sendDownload(st.url, st.fname, "video/mp4", st._size || null, st.pair || ((pair && !st.fromJson) ? pair : null), st.q || null, st.dur || null),
      };
      const it = api.addQuality({ ...q, onPick: () => api.setInfo(q) });
      sizePairs.push({ item: it, sizeEl: it.sizeEl, st });
    });
    api.bar = pill;
    // "Other Files": audio-only streams the sniffer saw in this session.
    for (const a of (audio || [])) {
      const base = String(fname).replace(/\.mp4$/i, "");
      api.addChip({
        label: `♪ ${(mimeExt(a.mime) || ".m4a").replace(".", "").toUpperCase()}${a.size > 0 ? " " + fmtSize(a.size) : ""}`,
        tip: `${a.mime || "audio"} — audio-only track`,
        send: () => sendDownload(a.url, `${base} [audio]${mimeExt(a.mime) || ".m4a"}`, a.mime, a.size || null, null, null, first.dur || null),
      });
    }
    api.sync();
    enrichSizes(sizePairs);
    api.panel.style.display = "block";
    // Page-anchored, viewport-clamped: the player box clips a panel that is
    // wider than the reel column (see placeFbPanel).
    placeFbPanel(pill, api.panel);
    paintDcPanel(api.panel);
  }

  function handleFbPill(pill) {
    if (pill.__dcBusy) return;
    // The reel's own qualities from Facebook's JSON beat the pieces the
    // sniffer caught (each piece used to become its own row). Clicked before
    // either exists: wait for them ("Resolving…").
    // The <video> this button belongs to. In the feed many videos play and
    // preload at once, so "the newest one the page fetched" is often a
    // neighbour; the clicked video's own post links name it.
    const video = pill.__dcVideo && pill.__dcVideo.isConnected !== false ? pill.__dcVideo : null;
    const onReelPage = !!fbReelKeyOf(location.href);
    const ids = video && !onReelPage ? fbIdsNear(video) : [];
    let vdur = 0;
    try { vdur = video ? video.duration : 0; } catch (e) { vdur = 0; }
    const gather = async () => {
      const info = fbInfo(video);
      const items = await sniffedNow();
      const data = fbMenuRows(info, items);
      const order = onReelPage ? { list: fbStoredVideos(items), known: true, missing: "" }
        : fbOrderFor(fbStoredVideos(items), ids, vdur);
      const stored = order.list[0];
      const rows = stored ? fbStoredRows(stored, info) : [];
      // The tab's sniffed pieces are only trusted when the clicked video
      // could not be told apart at all (the old behaviour) or on a reel page.
      if (!rows.length && !order.missing) rows.push(...data.rows);
      return rows.length ? { info, data, rows } : null;
    };
    barBusy(pill, dcT("busyResolving", "Resolving…"));
    waitFor(gather, (got) => {
      barBusy(pill, null);
      if (!got) {
        const info = fbInfo(video);
        // The clicked video's own page beats the feed's address: the app's
        // yt-dlp then downloads exactly that video.
        const target = ids.length ? fbPermalink(ids[0].id, ids[0].href) : location.href;
        sendDownload(target, fbFilename(info.title), null, null, null, null, info.durSec || null);
        return;
      }
      const { info, data, rows } = got;
      const audio = data.audio;
      if (rows.length === 1 && !audio.length) {
        const r0 = rows[0];
        sendDownload(r0.url, r0.fname, "video/mp4", r0._size || null, r0.pair || ((data.pair && !r0.fromJson) ? data.pair : null), r0.q || null, r0.dur || info.durSec || null);
      } else {
        openFbMenu(pill, rows, info.title, info.thumb, audio, data.pair);
      }
    }, 14, 600);
  }
  function pullNetworkMedia() {
    try {
      chrome.runtime.sendMessage({ type: "getMedia", pageUrl: location.href }, (res) => {
        if (chrome.runtime.lastError) return;
        if (!alive()) return;
        const items = (res && res.items) || [];
        if (!items.length) return;
        // On Facebook the tab's list spans every reel of the session, so the
        // streams offered for storage are narrowed to the reel on screen -
        // otherwise the popup keeps offering the reel the user scrolled away
        // from.
        let use = items;
        try {
          if (isFacebookPage()) {
            fbSyncReel();
            use = fbReelMedia(items, fbReelAt);
          }
        } catch (e) { use = items; }
        if (!use.length) return;
        try {
          chrome.storage.local.get(["downloadCraftLinks"], (cur) => {
            const have = new Set(((cur && cur.downloadCraftLinks) || []).map((l) => l.url));
            let pageThumb = "";
            try {
              const pv = document.querySelector("video[poster]");
              const pp = pv && pv.getAttribute("poster");
              if (/^https?:\/\//i.test(pp || "")) pageThumb = pp;
            } catch (e) { /* ignore */ }
            const add = use
              .filter((m) => !have.has(m.url))
              .map((m) => ({ url: m.url, text: `Stream (${m.mime || "media"})`, thumb: m.thumb || pageThumb }));
            if (add.length && IS_TOP) {
              chrome.storage.local.get(["downloadCraftLinks", "downloadCraftPage"], (c2) => {
                const merged = [...(((c2 && c2.downloadCraftLinks) || [])), ...add].slice(0, 50);
                chrome.storage.local.set({ downloadCraftLinks: merged, downloadCraftPage: location.href });
              });
            }
          });
        } catch (e) { /* ignore */ }
      });
    } catch (e) { /* background unreachable */ }
  }

  // Right-click → "Download video with Download Craft": the video under the
  // pointer (sites cover it with their own layers, so the click target is
  // rarely the <video> itself), else the biggest one on screen. It does what
  // its button does: YouTube's menu, a post's qualities, Facebook's menu.
  let lastCtxVideo = null;
  try {
    document.addEventListener("contextmenu", (e) => {
      try {
        lastCtxVideo = (document.elementsFromPoint(e.clientX, e.clientY) || []).find((el) => el.tagName === "VIDEO") || null;
      } catch (x) { lastCtxVideo = null; }
    }, true);
  } catch (e) { /* menu falls back to the page */ }
  function biggestVideo() {
    let best = null, area = 0;
    for (const v of document.querySelectorAll("video")) {
      const r = v.getBoundingClientRect();
      const vis = Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0)) * Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
      if (vis > area) { area = vis; best = v; }
    }
    return best;
  }
  function downloadContextVideo() {
    const v = (lastCtxVideo && lastCtxVideo.isConnected) ? lastCtxVideo : biggestVideo();
    lastCtxVideo = null;
    if (drmProtected(v)) { toast(dcT("drmProtected", DRM_NOTE)); return; }
    if (ytVideoId() && (!v || (v.closest && v.closest(".html5-video-player")))) {
      refreshYouTube();
      const b = document.getElementById("dc-yt-btn");
      if (b && b.__dcOpen && b.dataset.ready === "1") { b.__dcOpen(); return; }
      if (b) { b.click(); return; }
      sendDownload(location.href, undefined, null);
      return;
    }
    if (v && v.__dcBtn && v.__dcBtn.isConnected) { v.__dcBtn.click(); return; }
    if (v && isSocialPage()) {
      const post = socialPostFor(v);
      if (post) { openSocialMenuAt(post, v); return; }
    }
    const direct = v ? mediaUrlOf(v) : "";
    sendDownload(direct || location.href, undefined, v && v.getAttribute ? v.getAttribute("type") : null);
  }
  // A post's menu with no bar on screen (its video was too small for one or
  // the bar was hidden): anchored to the video's top-right corner.
  function openSocialMenuAt(post, video) {
    const anchor = document.createElement("div");
    const r = video.getBoundingClientRect();
    anchor.style.cssText = `position:fixed;left:${Math.max(8, r.right - 40)}px;top:${Math.max(8, r.top + 8)}px;width:32px;height:32px;pointer-events:none;`;
    document.documentElement.appendChild(anchor);
    openSocialMenu(anchor, post, video);
    setTimeout(() => anchor.remove(), 0);
  }

  // Popup peek: the popup asks the ACTIVE tab directly so it never shows
  // another tab's stale snapshot (global storage keys are fallback only).
  try {
    chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
      if (msg && msg.type === "ctxDownloadVideo" && alive()) {
        try { downloadContextVideo(); } catch (e) { /* ignore */ }
        sendResponse({ ok: true });
        return false;
      }
      if (msg && msg.type === "restoreVideoButtons" && alive()) {
        dismissedVideos.clear();
        refreshYouTube();ensureUniversalButtons();
        sendResponse({ok:true});return false;
      }
      if (msg && msg.type === "mediaFrameInfo") {
        const videos = [...document.querySelectorAll("video")];
        const d = videos.length === 1 ? videos[0].duration : 0;
        sendResponse({pageUrl:location.href, videoCount:videos.length,
          audioExpected:videos.length === 1 && playerHasAudio(videos[0]),
          durationSecs:Number.isFinite(d) && d > 0 && d < 86400 ? Math.max(1, Math.round(d)) : null});
        return false;
      }
      if (msg && msg.type === "vimeoFramePage") {
        sendResponse({ pageUrl: location.href });
        return false;
      }
      if (!msg || msg.type !== "peekState") return false;
      if (!alive()) return false;
      (async () => {
        // Answer from this page directly (not via storage, which incognito
        // pages and frames never write): its links and its YouTube video.
        let links = [];
        let videos = [];
        try {
          links = collectLinks() || [];
          refreshYouTube();
          const id = ytVideoId();
          if (id) {
            const info = ytStreams();
            if (info) videos = [{ ...info, pageUrl: location.href }];
          }
        } catch (e) { /* best effort */ }
        let net = [];
        try {
          const r = await api.runtime.sendMessage({ type: "getMedia", pageUrl: location.href });
          net = (r && r.items) || [];
        } catch (e) { /* ignore */ }
        // Page thumbnail for network rows (og:image or the player's poster).
        let thumb = "";
        try {
          const og = document.querySelector('meta[property="og:image"]');
          const v = document.querySelector("video[poster]");
          thumb = (og && og.content) || (v && v.poster) || "";
          if (!/^https?:\/\//i.test(thumb)) thumb = "";
        } catch (e) { /* ignore */ }
        // Facebook: whole videos from its own JSON, the reel on screen first.
        // Their byte-range pieces (and every other fbcdn file the sniffer or
        // the page JSON turned up) are what filled the list with a dozen
        // nameless, sizeless copies of one reel, so those are dropped.
        let fb = [];
        try {
          if (isFacebookPage()) {
            const stored = fbStoredVideos(net);
            if (stored.length) {
              const info = fbInfo();
              fb = stored.slice(0, 15).map((e, i) => ({
                id: e.id, dur: e.dur, thumb: e.thumb || (i === 0 ? info.thumb : ""),
                title: i === 0 ? info.title : "",
                video: e.video, audio: e.audio, progressive: e.progressive,
              }));
              net = net.filter((n) => !/(^|\.)fbcdn\.net$/i.test((() => {
                try { return new URL(n.url).hostname; } catch (e) { return ""; }
              })()));
            }
          }
        } catch (e) { fb = []; }
        sendResponse({
          videos,
          links,
          fb,
          net: net.map((n) => (n.thumb ? n : { ...n, thumb })),
          page: location.href,
          title: document.title || "",
        });
      })();
      return true;
    });
  } catch (e) { /* messaging unavailable */ }

  // Ordinary archive/installer links and explicit <a download> links can
  // bypass the browser downloader entirely. Preserve modified clicks and
  // fall back to the original link if the app declines capture.
  const bypassDownloadLinks = new WeakSet();
  // "Keep in the browser" key (Settings -> Capture in the app; Alt by
  // default): a link clicked with it held is left to the browser.
  let skipKey = "alt";
  const readSkipKey = () => {
    try {
      chrome.storage.local.get({ appPrefs: {} }, (v) => {
        void chrome.runtime.lastError;
        const k = v && v.appPrefs && v.appPrefs.skipKey;
        skipKey = /^(alt|shift|off)$/.test(k || "") ? k : "alt";
      });
    } catch (e) {}
  };
  readSkipKey();
  try { chrome.storage.onChanged.addListener((ch) => { if (ch.appPrefs) readSkipKey(); }); } catch (e) {}
  document.addEventListener("click", ev => {
    // Only a real user click: a page script calling a.click() on download
    // links could otherwise make the app fetch any URL with that site's
    // cookies, silently and in a loop.
    if (!ev.isTrusted) return;
    if (alive() && ev.button === 0 && ((skipKey === "alt" && ev.altKey) || (skipKey === "shift" && ev.shiftKey))) {
      const a = ev.target && ev.target.closest && ev.target.closest("a[href]");
      if (a && /^https?:\/\//i.test(a.href)) {
        try { chrome.runtime.sendMessage({ type: "skipLink", url: a.href }, () => void chrome.runtime.lastError); } catch (e) {}
      }
      return;
    }
    if (!alive() || ev.button !== 0 || ev.ctrlKey || ev.metaKey || ev.shiftKey || ev.altKey) return;
    const a = ev.target && ev.target.closest && ev.target.closest("a[href]");
    if (!a) return;
    if (bypassDownloadLinks.has(a)) { bypassDownloadLinks.delete(a); return; }
    if (!DCRank.directDownloadLink(a.href,a.hasAttribute("download"))) return;
    ev.preventDefault(); ev.stopImmediatePropagation();
    const fallback = () => { bypassDownloadLinks.add(a); a.click(); };
    try {
      chrome.runtime.sendMessage({type:"captureLink",url:a.href,
        filename:a.getAttribute("download") || null,pageUrl:location.href}, res => {
        if (chrome.runtime.lastError || !res || !res.ok) fallback();
      });
    } catch (e) { fallback(); }
  },true);

  loadBarPosition();
  window.addEventListener("focus",loadBarPosition);

  // Initial pass + SPA navigation (YouTube AND Facebook rarely reload).
  // Facebook reels swap /reel/<id> via history API: no reload, same video
  // element, new blob: src. Patch pushState/replaceState + popstate so the
  // pill + sniffed candidates refresh instantly instead of on the 2 s poll.
  // The player response can arrive after the content script runs, so
  // retry a few times before giving up.
  collectLinks();
  ensureUniversalButtons();
  pullNetworkMedia();
  installDcCloser();
  function spaRefresh() {
    if (__dcDead || !alive()) return;
    // A reel swap is an SPA navigation inside the same document: mark the reel
    // as current right away so everything captured before it counts as the reel
    // the user scrolled away from.
    try { if (isFacebookPage()) fbSyncReel(); } catch (e) { /* best effort */ }
    try { collectLinks(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { ensureUniversalButtons(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { pullNetworkMedia(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
    try { refreshYouTube(); } catch (e) { if (ctxInvalidated(e)) return markDead(); }
  }
  try {
    const fireSpa = () => { if (alive()) setTimeout(spaRefresh, 300); };
    // The page's own history.pushState/replaceState run in the page world,
    // which a patch here (isolated world) never sees: page-hook.js wraps
    // them there and announces each navigation. A page faking one only
    // triggers a refresh.
    window.addEventListener("message", (ev) => {
      if (ev.source !== window || !ev.data) return;
      if (ev.data.__dc === "nav") fireSpa();
      // Facebook's GraphQL answers that describe videos (page-hook.js). The
      // parser keeps only Facebook CDN files, whatever a page posts here.
      else if (ev.data.__dc === "fb-json" && typeof ev.data.text === "string" && isFacebookPage()) {
        fbAddVideos(ev.data.text.slice(0, 4 * 1024 * 1024));
      }
    });
    window.addEventListener("popstate", fireSpa);
    window.addEventListener("hashchange", fireSpa);
  } catch (e) { /* navigation hooks are best-effort */ }
  try {
    // Debounced + generation-gated: a superseded instance disconnects
    // instead of fighting the new one, and mutation storms (infinite
    // scroll) collapse into one pass per 400 ms.
    let moTimer = 0;
    const mo = new MutationObserver(() => {
      if (!alive()) { try { mo.disconnect(); } catch (e) {} return; }
      clearTimeout(moTimer);
      moTimer = setTimeout(() => {
        if (!alive()) return;
        collectLinks();
        ensureUniversalButtons();
      }, 400);
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
    __dcObservers.push(mo);
  } catch (e) { /* observer optional */ }
  __dcTimers.push(setInterval(() => { if (__dcDead || !alive()) return; try { pullNetworkMedia(); } catch (x) { if (ctxInvalidated(x)) markDead(); } }, 8000));
  if (!refreshYouTube()) {
    let tries = 0;
    const timer = setInterval(() => {
      tries++;
      if (__dcDead || !alive() || refreshYouTube() || tries >= 6) clearInterval(timer);
    }, 2000);
    __dcTimers.push(timer);
  }
  document.addEventListener("yt-navigate-finish", () => {
    if (!alive()) return; // superseded by a newer injected instance
    collectLinks();
    ensureUniversalButtons();
    refreshYouTube();
  });
  let lastUrl = location.href;
  let lastTitle = document.title;
  let lastReel = "";
  try { if (isFacebookPage()) lastReel = fbSyncReel(); } catch (e) { lastReel = ""; }
  const watcher = setInterval(() => {
    if (__dcDead || !alive()) { clearInterval(watcher); return; } // superseded: stop
    // The reels feed also changes the player without changing the URL (the
    // overlay keeps "/"), so the reel key is part of the change test.
    let reel = "";
    try { if (isFacebookPage()) reel = fbSyncReel(); } catch (e) { reel = ""; }
    if (location.href !== lastUrl || document.title !== lastTitle || reel !== lastReel) {
      lastUrl = location.href;
      lastTitle = document.title;
      lastReel = reel;
      // Drop pills bound to the previous reel's <video> node so the new
      // reel gets a fresh pill even when Facebook reuses the DOM node.
      try {
        document.querySelectorAll("video").forEach((v) => {
          if (v.__dcBtn && !v.__dcBtn.isConnected) v.__dcBtn = null;
        });
      } catch (e) {}
      collectLinks();
      ensureUniversalButtons();
      refreshYouTube();
    } else if (onShorts() && detectionOn && !document.getElementById("dc-yt-btn")) {
      // YouTube redraws a Short's control row now and then, which takes the
      // button with it: put it back (unless it was hidden for this video).
      try {
        const id = ytVideoId();
        if (id && !dismissedVideos.has(dismissalKey(null, id))) refreshYouTube();
      } catch (e) { /* ignore */ }
    }
    try { youtubeSelfCheck(); } catch (e) { /* ignore */ }
  }, 1000);
  __dcTimers.push(watcher);
})();
