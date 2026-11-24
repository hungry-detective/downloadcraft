// streams.js: HLS playlists and DASH manifests -> lists of files to fetch.
// Run: `node --test streams.test.js`
const { it } = require("node:test");
const assert = require("node:assert/strict");
const S = require("./streams.js");

it("reads an HLS master: variants and separate audio renditions", () => {
  const pl = S.parseM3u8([
    "#EXTM3U",
    '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="English",LANGUAGE="en",DEFAULT=YES,URI="audio/en.m3u8"',
    '#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,AUDIO="aud",CODECS="avc1.4d401e,mp4a.40.2"',
    "v360/index.m3u8",
    "#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,AUDIO=\"aud\"",
    "v720/index.m3u8",
  ].join("\n"), "https://cdn.example/movie/master.m3u8");
  assert.equal(pl.master, true);
  assert.deepEqual(pl.variants.map((v) => [v.h, v.url]), [
    [360, "https://cdn.example/movie/v360/index.m3u8"],
    [720, "https://cdn.example/movie/v720/index.m3u8"],
  ]);
  assert.equal(pl.audios[0].url, "https://cdn.example/movie/audio/en.m3u8");
  assert.equal(pl.audios[0].default, true);
  // Quality cap: best at or under it; no cap = best overall.
  assert.equal(S.pickVariant(pl.variants, "480p").h, 360);
  assert.equal(S.pickVariant(pl.variants, "").h, 720);
  assert.equal(S.pickVariant(pl.variants, "1080p").h, 720);
  // Portrait video: the short side is the quality.
  assert.equal(S.pickVariant([{ w: 1080, h: 1920, bw: 1 }, { w: 720, h: 1280, bw: 1 }], "720p").w, 720);
});

it("reads an HLS media playlist: AES-128 keys, IVs, init map and byte ranges", () => {
  const pl = S.parseM3u8([
    "#EXTM3U",
    "#EXT-X-MEDIA-SEQUENCE:7",
    '#EXT-X-MAP:URI="init.mp4"',
    '#EXT-X-KEY:METHOD=AES-128,URI="https://keys.example/k1"',
    "#EXTINF:4.0,",
    "s7.m4s",
    '#EXT-X-KEY:METHOD=AES-128,URI="k2",IV=0x000102030405060708090a0b0c0d0e0f',
    "#EXTINF:4.0,",
    "#EXT-X-BYTERANGE:1000@0",
    "big.m4s",
    "#EXTINF:2.5,",
    "#EXT-X-BYTERANGE:500",
    "big.m4s",
    "#EXT-X-ENDLIST",
  ].join("\n"), "https://cdn.example/v/index.m3u8");
  assert.equal(pl.master, false);
  assert.equal(pl.live, false);
  assert.equal(pl.encrypted, true);
  assert.equal(pl.protected, false, "plain AES-128 is not DRM");
  assert.equal(pl.duration, 10.5);
  assert.equal(pl.segments.length, 3);
  assert.equal(pl.segments[0].map.url, "https://cdn.example/v/init.mp4");
  assert.equal(pl.segments[0].key.url, "https://keys.example/k1");
  // No IV given: the media sequence number, big-endian.
  assert.deepEqual([...S.ivFor(pl.segments[0])].slice(12), [0, 0, 0, 7]);
  assert.deepEqual([...S.ivFor(pl.segments[1])], [...Array(16).keys()]);
  assert.deepEqual(pl.segments[1].range, { start: 0, end: 999 });
  assert.deepEqual(pl.segments[2].range, { start: 1000, end: 1499 }, "continues after the previous range");
});

it("marks SAMPLE-AES and key systems as protected, and live lists as live", () => {
  const drm = S.parseM3u8('#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI="skd://x",KEYFORMAT="com.apple.streamingkeydelivery"\n#EXTINF:4,\na.ts\n', "https://h/");
  assert.equal(drm.protected, true);
  const live = S.parseM3u8("#EXTM3U\n#EXTINF:4,\na.ts\n", "https://h/");
  assert.equal(live.live, true);
  assert.throws(() => S.parseM3u8("<html>", "https://h/"), /not an HLS playlist/);
});

it("reads ISO durations", () => {
  assert.equal(S.isoDur("PT1H2M3.5S"), 3723.5);
  assert.equal(S.isoDur("PT0.5S"), 0.5);
  assert.equal(S.isoDur(""), 0);
});

// Minimal XML -> DOM-like tree for parseMpd (elements, attributes, text).
function xml(text) {
  const root = { nodeName: "#document", childNodes: [], getElementsByTagName(n) { return find(this, n); } };
  const stack = [root];
  const re = /<\?[^>]*\?>|<!--[\s\S]*?-->|<\/([\w:.-]+)\s*>|<([\w:.-]+)((?:\s+[\w:.-]+="[^"]*")*)\s*(\/?)>|([^<]+)/g;
  for (let m; (m = re.exec(text));) {
    const top = stack[stack.length - 1];
    if (m[1]) stack.pop();
    else if (m[2]) {
      const attrs = {};
      for (const a of m[3].matchAll(/([\w:.-]+)="([^"]*)"/g)) attrs[a[1]] = a[2].replace(/&amp;/g, "&");
      const el = {
        nodeName: m[2], localName: m[2].split(":").pop(), childNodes: [], attrs,
        getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; },
        get textContent() { return this.childNodes.map((c) => c.textContent || "").join(""); },
      };
      top.childNodes.push(el);
      if (!m[4]) stack.push(el);
    } else if (m[5] && m[5].trim()) top.childNodes.push({ nodeName: "#text", textContent: m[5] });
  }
  return root;
}
function find(node, name) {
  const out = [];
  for (const c of node.childNodes || []) {
    if (c.nodeName === name) out.push(c);
    out.push(...find(c, name));
  }
  return out;
}
const parser = { parseFromString: (t) => xml(t) };

it("reads DASH: SegmentTemplate with $Number$ and $Time$, and single-file representations", () => {
  const mpd = `<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT10S">
  <BaseURL>https://cdn.example/dash/</BaseURL>
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate timescale="1000" duration="4000" startNumber="1" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/seg-$Number%03d$.m4s"/>
      <Representation id="v720" bandwidth="2000000" width="1280" height="720"/>
      <Representation id="v360" bandwidth="600000" width="640" height="360"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="a1" bandwidth="128000">
        <SegmentTemplate timescale="48000" initialization="a/init.mp4" media="a/$Time$.m4s">
          <SegmentTimeline><S t="0" d="96000" r="2"/><S d="48000"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="whole" bandwidth="900000" width="854" height="480"><BaseURL>whole_480.mp4</BaseURL><SegmentBase indexRange="0-100"/></Representation>
    </AdaptationSet>
  </Period>
</MPD>`;
  const m = S.parseMpd(mpd, "https://cdn.example/dash/manifest.mpd", parser);
  assert.equal(m.duration, 10);
  assert.equal(m.protected, false);
  const v720 = m.video.find((r) => r.id === "v720");
  assert.equal(v720.init, "https://cdn.example/dash/v720/init.mp4");
  assert.deepEqual(v720.segments, [1, 2, 3].map((n) => `https://cdn.example/dash/v720/seg-00${n}.m4s`));
  const a = m.audio[0];
  assert.deepEqual(a.segments.map((u) => u.split("/").pop()), ["0.m4s", "96000.m4s", "192000.m4s", "288000.m4s"]);
  const whole = m.video.find((r) => r.id === "whole");
  assert.deepEqual(whole.segments, ["https://cdn.example/dash/whole_480.mp4"]);
  assert.equal(whole.init, "");
});

it("flags DRM (ContentProtection) and refuses live DASH", () => {
  const drm = `<MPD type="static" mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="video/mp4"><ContentProtection schemeIdUri="urn:uuid:edef8ba9"/><Representation id="v" bandwidth="1"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>`;
  assert.equal(S.parseMpd(drm, "https://h/m.mpd", parser).protected, true);
  assert.throws(() => S.parseMpd('<MPD type="dynamic"><Period/></MPD>', "https://h/m.mpd", parser), /live/);
});

// The player's download button picks the episode, not IMDb's 10 s trailer
// preview playing in the page above the embedded player.
{
  const { test } = require('node:test');
  const assert = require('node:assert/strict');
  const R = require('./rank.js');
  test('a player picks the stream of its own length, from its own frame', () => {
    const embed = 'https://vidsrc.su/embed/tv/66732/1/4';
    const preview = { url: 'https://imdb-video.media-imdb.com/previews/v.m3u8', pageUrl: 'https://www.imdb.com/title/tt4574334/', dur: 10 };
    const master = { url: 'https://cdn.example/m.m3u8', pageUrl: embed, master: true, variants: [{ url: 'https://cdn.example/720.m3u8' }] };
    const child = { url: 'https://cdn.example/720.m3u8', pageUrl: embed, dur: 2967 };
    const urls = (l) => l.map((m) => m.url);
    // Known player length: the preview goes, the episode's streams stay.
    assert.deepEqual(urls(R.fitPlayer([preview, master, child], 2967, embed)), [master.url, child.url]);
    // Length unknown, preview's length unknown too: this frame's media wins.
    const noLen = { ...preview, dur: undefined };
    assert.deepEqual(urls(R.fitPlayer([noLen, master, child], 0, embed)), [master.url, child.url]);
    // Nothing fits: everything is kept rather than nothing.
    assert.deepEqual(urls(R.fitPlayer([preview], 2967, embed)), [preview.url]);
  });
}
