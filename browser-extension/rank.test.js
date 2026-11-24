// Unit tests for rank.js (P4). Run: `node --test rank.test.js`
const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const DCRank = require("./rank.js");

describe("kinds", () => {
  it("classifies playlists, segments, files", () => {
    assert.equal(DCRank.kindOf({ url: "https://h/v/pl.m3u8?x=1" }), "playlist");
    assert.equal(DCRank.kindOf({ url: "https://h/v/manifest.mpd" }), "playlist");
    assert.equal(DCRank.kindOf({ url: "https://h/v/seg-1.m4s" }), "segment");
    assert.equal(DCRank.kindOf({ url: "https://h/v/s0.ts" }), "segment");
    assert.equal(DCRank.kindOf({ url: "https://h/v/clip.mp4?x=1" }), "file");
    assert.equal(DCRank.kindOf({ url: "https://h/v/clip.mp4" }), "file");
  });
});

describe("canonical", () => {
  it("collapses slice variants", () => {
    const a = "https://h/v.mp4?oh=1&bytestart=0&byteend=100";
    const b = "https://h/v.mp4?oh=1&bytestart=101&byteend=200";
    assert.equal(DCRank.canonicalUrl(a), DCRank.canonicalUrl(b));
    assert.ok(!DCRank.canonicalUrl(a).includes("bytestart"));
  });
});

describe("scoring", () => {
  const file = { url: "https://h/v.mp4", mime: "video/mp4", size: 9e6 };
  const list = { url: "https://h/pl.m3u8", mime: "application/vnd.apple.mpegurl" };
  const seg = { url: "https://h/s0.ts", mime: "video/mp2t", size: 2e5 };
  it("orders file > playlist > segment", () => {
    const ctx = { hasPlaylist: false };
    assert.ok(DCRank.scoreMedia(file, ctx) > DCRank.scoreMedia(list, ctx));
    assert.ok(DCRank.scoreMedia(list, ctx) > DCRank.scoreMedia(seg, ctx));
  });
  it("demotes segments when a playlist is known (ad-skip)", () => {
    assert.equal(DCRank.scoreMedia(seg, { hasPlaylist: true }), -1);
    assert.ok(DCRank.scoreMedia(file, { hasPlaylist: true }) > 0);
  });
  it("prefers bigger sizes", () => {
    const small = { url: "https://h/a.mp4", mime: "video/mp4", size: 50e3 };
    const big = { url: "https://h/b.mp4", mime: "video/mp4", size: 50e6 };
    assert.ok(DCRank.scoreMedia(big, {}) > DCRank.scoreMedia(small, {}));
  });
});

describe("collapseDupes", () => {
  it("keeps the biggest, counts tabs", () => {
    const out = DCRank.collapseDupes([
      { url: "https://h/v.mp4?bytestart=0&byteend=9", size: 10, tabId: 1 },
      { url: "https://h/v.mp4?bytestart=10&byteend=99", size: 90, tabId: 2 },
      { url: "https://h/other.mp4", size: 5, tabId: 1 },
    ]);
    assert.equal(out.length, 2);
    const v = out.find((m) => m.url.includes("v.mp4"));
    assert.equal(v.size, 90);
    assert.equal(v.tabs, 2);
  });
});

describe("fmtDur", () => {
  it("formats m:ss", () => {
    assert.equal(DCRank.fmtDur(206), "3:26");
    assert.equal(DCRank.fmtDur(0), "");
    assert.equal(DCRank.fmtDur(-5), "");
  });
  it("adds hours for long videos", () => {
    assert.equal(DCRank.fmtDur(6542), "1:49:02");
    assert.equal(DCRank.fmtDur(3600), "1:00:00");
  });
});

describe("panelTokens", () => {
  it("covers dark and light with the same keys", () => {
    const dark = DCRank.panelTokens("dark");
    const light = DCRank.panelTokens("light");
    assert.deepEqual(Object.keys(dark).sort(), Object.keys(light).sort());
    assert.equal(light.mode, "light");
    assert.equal(dark.mode, "dark");
    assert.equal(light.bg, "#ffffff");
    assert.equal(dark.bg, "#0f172a");
    assert.notEqual(dark.accent, light.accent);
    assert.ok(dark.toggle !== light.toggle);
    // unknown mode falls back to dark (the mockup's palette)
    assert.equal(DCRank.panelTokens().mode, "dark");
  });
});

describe("panel paint", () => {
  // Minimal fake DOM: style bag + role-tagged children addressed by
  // `data-dc-p` (the same query the real painter issues).
  function fakeEl(role) {
    const attrs = {};
    if (role) attrs["data-dc-p"] = role;
    return {
      style: {},
      textContent: "",
      attrs,
      setAttribute(k, v) { this.attrs[k] = v; },
      getAttribute(k) { return this.attrs[k] === undefined ? null : this.attrs[k]; },
      querySelectorAll() { return []; },
    };
  }
  function fakePanel(kids) {
    return {
      _dcT: null,
      style: {},
      kids,
      querySelectorAll(sel) {
        if (sel === "[data-dc-p]") return this.kids.filter((k) => k.attrs["data-dc-p"]);
        return [];
      },
    };
  }
  function withGlobals(t, fn) {
    const g = globalThis;
    const keepW = g.window;
    const keepD = g.document;
    g.window = {
      __dcTheme: t.theme,
      matchMedia: () => ({ matches: t.theme !== "dark" && t.systemLight }),
    };
    g.document = { createElement: () => fakeEl(null) };
    try {
      delete g.__dcTheme;
      if (t.theme === "light" || t.theme === "dark") g.__dcTheme = t.theme;
      return fn();
    } finally {
      delete g.__dcTheme;
      if (keepW === undefined) delete g.window; else g.window = keepW;
      if (keepD === undefined) delete g.document; else g.document = keepD;
    }
  }

  it("defaults to dark without an override or light OS", () => {
    withGlobals({ systemLight: false }, () => {
      assert.equal(DCRank.dcThemeMode(), "dark");
    });
  });

  it("follows prefers-color-scheme and honors the override", () => {
    withGlobals({ systemLight: true }, () => {
      assert.equal(DCRank.dcThemeMode(), "light");
    });
    withGlobals({ theme: "dark", systemLight: true }, () => {
      assert.equal(DCRank.dcThemeMode(), "dark");
    });
  });

  it("paints a panel light without touching layout", () => {
    withGlobals({ theme: "light" }, () => {
      const txt = fakeEl("text");
      const div = fakeEl("divider");
      const item = fakeEl("item");
      const panel = fakePanel([txt, div, item]);
      panel.style.display = "none";
      DCRank.paintDcPanel(panel);
      assert.equal(panel.style.background, "#ffffff");
      assert.equal(panel.style.display, "none"); // layout untouched
      assert.equal(panel._dcT.mode, "light");
      assert.equal(txt.style.color, "#0f172a");
      assert.equal(div.style.background, "#e2e8f0");
      assert.equal(item.style.background, "#f8fafc");
      assert.equal(item.style.borderColor, "#e2e8f0");
    });
  });

  it("highlights the selected cell and flips the bubble arrow", () => {
    withGlobals({ theme: "dark" }, () => {
      const sel = fakeEl("item");
      sel.attrs["data-dc-sel"] = "1";
      const above = fakeEl("arrow");
      above.attrs["data-dc-arrow"] = "bottom";
      DCRank.paintDcPanel(fakePanel([sel, above]));
      assert.equal(sel.style.borderColor, "#3b82f6");
      assert.equal(sel.style.background, "rgba(59,130,246,.16)");
      // Arrow below the panel: the two bottom borders draw the point.
      assert.equal(above.style.borderRightColor, "#334155");
      assert.equal(above.style.borderBottomColor, "#334155");
      assert.equal(above.style.borderLeftColor, "transparent");
    });
  });

  it("writes the toggle glyph for the active theme", () => {
    withGlobals({ theme: "dark" }, () => {
      const tog = fakeEl("toggle");
      DCRank.paintDcPanel(fakePanel([tog]));
      assert.equal(tog.textContent, "☀");
    });
  });
});

const VIMEO_FRAGMENT = "https://vod-adaptive-ak.vimeocdn.com/exp=1~hmac=example/v2/range/prot/cmFuZ2U9MzkzODcxMi00OTgyMzgx/avf/video.mp4?pathsig=example&range=3938712-4982381";
describe("Vimeo range captures", () => {
  it("classifies signed MP4 range URLs as fragments without rewriting them", () => {
    assert.equal(DCRank.kindOf({ url: VIMEO_FRAGMENT }), "segment");
    assert.equal(DCRank.canonicalUrl(VIMEO_FRAGMENT), VIMEO_FRAGMENT);
    assert.equal(DCRank.isVimeoFragment(VIMEO_FRAGMENT.split("?")[0]), true);
    assert.equal(DCRank.isVimeoFragment("https://vod.vimeocdn.com/video.mp4?range=0-99"), true);
  });
  it("chooses a manifest over a Vimeo MP4 fragment", () => {
    const fragment = { url: VIMEO_FRAGMENT, mime: "video/mp4", size: 1043670 };
    const manifest = { url: "https://vod.vimeocdn.com/master.mpd" };
    const pool = [fragment, manifest].filter(m => DCRank.scoreMedia(m, {hasPlaylist:true}) >= 0);
    assert.deepEqual(pool, [manifest]);
    assert.equal(DCRank.kindOf({ url: "https://vod.vimeocdn.com/full.mp4" }), "file");
    assert.equal(DCRank.isVimeoFragment("https://vimeocdn.com.evil.test/v2/range/a.mp4"), false);
  });
  it("accepts exact video pages and preserves unlisted access parameters", () => {
    for (const page of ["https://player.vimeo.com/video/123?h=secret&app_id=1", "https://vimeo.com/123/secret"]) {
      assert.equal(DCRank.vimeoVideoPage(page), page);
    }
    for (const page of ["https://vimeo.com/", "https://vimeo.com/user123", "https://player.vimeo.com.evil.test/video/123", "ftp://vimeo.com/123"]) {
      assert.equal(DCRank.vimeoVideoPage(page), null);
    }
  });
});

describe("Facebook videos from the page JSON", () => {
  // Shape of a reel page / GraphQL answer: the DASH list sits in
  // "extensions", the video node (duration, SD/HD, thumbnail) right after
  // its id. Two reels, as Facebook preloads the next one.
  const cdn = (p) => `https:\/\/scontent.example.fna.fbcdn.net\/o1\/v\/${p}.mp4?_nc_cat=1&oh=x&oe=1`;
  const rep = (id, mime, w, h, bw) => `{"representation_id":"${id}","mime_type":"${mime}","base_url":"${cdn(id)}","bandwidth":${bw},"width":${w},"height":${h},"segments":[{"start":0,"end":826}]}`;
  const page = [
    `{"video":{"id":"111111111","preferred_thumbnail":{"image":{"uri":"https:\/\/scontent.example.fbcdn.net\/t15\/a.jpg"}},`,
    `"length_in_second":19.122,"progressive_urls":[{"progressive_url":"${cdn("sd1")}&bitrate=138807&tag=sve_sd","metadata":{"quality":"SD"}},`,
    `{"progressive_url":"${cdn("hd1")}&bitrate=359978","metadata":{"quality":"HD"}}],"id":"111111111"},`,
    `{"id":"222222222","playable_duration_in_ms":77651},`,
    `"extensions":{"all_video_dash_prefetch_representations":[`,
    `{"video_id":"111111111","representations":[${rep("v720", "video/mp4", 720, 720, 146181)},${rep("v1080", "video/mp4", 1080, 1080, 388664)},${rep("a1", "audio/mp4", 0, 0, 58671)}]},`,
    `{"video_id":"222222222","representations":[${rep("w1080", "video/mp4", 1080, 1920, 642641)},${rep("evil", "video/mp4", 1, 1, 1).replace("scontent.example.fna.fbcdn.net", "evil.example")}]}]}}`,
  ].join("");

  it("groups every quality, audio, duration and thumbnail per video", () => {
    const v = DCRank.fbVideos(page);
    assert.equal(v.length, 2);
    const [a, b] = v;
    assert.equal(a.id, "111111111");
    assert.equal(a.dur, 19);
    assert.match(a.thumb, /a\.jpg$/);
    assert.deepEqual(a.video.map((x) => x.h), [1080, 720], "best first");
    assert.equal(a.audio.length, 1);
    assert.deepEqual(a.progressive.map((x) => [x.quality, x.bw]), [["SD", 138807], ["HD", 359978]]);
    assert.equal(b.id, "222222222");
    assert.equal(b.dur, 78);
    // Only Facebook CDN files are kept, whatever the JSON names.
    assert.deepEqual(b.video.map((x) => x.h), [1920]);
  });

  it("is empty for pages without video data and survives broken JSON", () => {
    assert.deepEqual(DCRank.fbVideos('{"x":1}'), []);
    assert.deepEqual(DCRank.fbVideos('"all_video_dash_prefetch_representations":[{"video_id":"1"'), []);
  });

  it("names a file by its path, so byte-range pieces map to their video", () => {
    assert.equal(DCRank.fbPathOf("https://h.fbcdn.net/o1/v/x.mp4?bytestart=0&byteend=9"), "/o1/v/x.mp4");
    assert.equal(DCRank.fbPathOf("not a url"), "");
  });
});

describe("Facebook quality order", () => {
  it("puts the H.264 copy of a quality before AV1/VP9", () => {
    const rep = (id, codecs) => `{"mime_type":"video/mp4","codecs":"${codecs}","base_url":"https:\/\/s.fbcdn.net\/v\/${id}.mp4","bandwidth":1,"width":720,"height":1280}`;
    const t = `"all_video_dash_prefetch_representations":[{"video_id":"9999999","representations":[${rep("av1", "av01.0.05M.08")},${rep("avc", "avc1.64001F")},${rep("vp9", "vp09.00.31.08")}]}]`;
    const [v] = DCRank.fbVideos(t);
    assert.match(v.video[0].url, /avc\.mp4$/);
    assert.equal(v.video[0].codecs, "avc1.64001F");
  });
});

describe("mp4Info", () => {
  const box = (t, body) => {
    const out = new Uint8Array(8 + body.length);
    const n = out.length;
    out.set([n >>> 24, (n >>> 16) & 255, (n >>> 8) & 255, n & 255], 0);
    out.set([...t].map((c) => c.charCodeAt(0)), 4);
    out.set(body, 8);
    return out;
  };
  const cat = (...a) => { const o = new Uint8Array(a.reduce((n, x) => n + x.length, 0)); let i = 0; for (const x of a) { o.set(x, i); i += x.length; } return o; };
  const tkhd = (w, h) => { const b = new Uint8Array(84); b.set([w >>> 8, w & 255, 0, 0, h >>> 8, h & 255, 0, 0], 76); return box("tkhd", b); };
  const hdlr = (k) => { const b = new Uint8Array(24); b.set([...k].map((c) => c.charCodeAt(0)), 8); return box("hdlr", b); };
  const stsd = (entry) => box("stsd", cat(new Uint8Array([0, 0, 0, 0, 0, 0, 0, 1]), box(entry, new Uint8Array(8))));
  const trak = (w, h, kind, entry) => box("trak", cat(tkhd(w, h), box("mdia", cat(hdlr(kind), box("minf", box("stbl", stsd(entry)))))));
  const file = (moov, mdatFirst) => {
    const ftyp = box("ftyp", new Uint8Array([105, 115, 111, 109, 0, 0, 0, 0]));
    const mdat = box("mdat", new Uint8Array(32));
    return mdatFirst ? cat(ftyp, mdat, moov) : cat(ftyp, moov, mdat);
  };
  it("reads the video track's size, not the audio track's", () => {
    const moov = box("moov", cat(trak(0, 0, "soun", "mp4a"), trak(1920, 1080, "vide", "avc1")));
    assert.deepEqual(DCRank.mp4Info(file(moov)), { w: 1920, h: 1080, drm: false });
  });
  it("flags encrypted (DRM) tracks", () => {
    const moov = box("moov", trak(1280, 720, "vide", "encv"));
    assert.deepEqual(DCRank.mp4Info(file(moov)), { w: 1280, h: 720, drm: true });
  });
  it("points at a moov stored after the media data", () => {
    const moov = box("moov", trak(640, 360, "vide", "avc1"));
    const whole = file(moov, true);
    const head = whole.slice(0, 16 + 8); // ftyp + the mdat header only
    const r = DCRank.mp4Info(head);
    assert.equal(r.moovAt, 16 + 40);
    assert.deepEqual(DCRank.mp4Info(whole.slice(r.moovAt), r.moovAt), { w: 640, h: 360, drm: false });
  });
  it("reads the movie's length from mvhd (a hover preview is told apart)", () => {
    // version 0: flags(4) created(4) modified(4) timescale(4) duration(4) …
    const body = new Uint8Array(100);
    body.set([0, 0, 3, 232], 12); // timescale 1000
    body.set([0, 13, 22, 220], 16); // 857 820 ms = 858 s (14:18)
    const moov = box("moov", cat(box("mvhd", body), trak(1280, 720, "vide", "avc1")));
    assert.deepEqual(DCRank.mp4Info(file(moov)), { w: 1280, h: 720, drm: false, dur: 858 });
  });
  it("ignores what is not an MP4", () => {
    assert.equal(DCRank.mp4Info(new TextEncoder().encode("<html>encv</html>")), null);
  });
});
