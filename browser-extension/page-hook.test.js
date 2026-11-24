// page-hook.js: media URLs from page API answers and player state.
// Run: `node --test page-hook.test.js`
const { it } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

const source = fs.readFileSync(__dirname + "/page-hook.js", "utf8");

function page(globals = {}) {
  const posted = [];
  const raw = [];
  const timers = [];
  const answers = new Map();
  const window = {
    location: { origin: "https://site.example" },
    postMessage: (msg) => { raw.push(msg); if (msg.items) posted.push(...msg.items); },
    // A Response as the page sees it: the hook patches its prototype.
    Response: class Response {
      constructor(a) { this.a = a; this.bodyUsed = false; this.headers = { get: (k) => (k === "content-type" ? a.type : null) }; }
      clone() { return new window.Response(this.a); }
      async text() { this.bodyUsed = true; return this.a.body; }
      async json() { this.bodyUsed = true; return JSON.parse(this.a.body); }
    },
    // The page fetches and reads the answer, as pages do.
    fetch: async (url) => {
      const res = new window.Response(answers.get(url));
      try { await (/json/.test(res.a.type) ? res.json() : res.text()); } catch (e) { /* not JSON */ }
      return res;
    },
    ...globals,
  };
  const context = {
    window, history: {}, location: window.location,
    setTimeout: (fn) => { timers.push(fn); return timers.length; },
  };
  vm.createContext(context);
  vm.runInContext(source, context);
  const flush = async () => {
    for (let i = 0; i < 5; i++) {
      await new Promise((r) => setImmediate(r));
      const run = timers.splice(0);
      run.forEach((fn) => fn());
    }
  };
  return { window, posted, raw, answers, flush };
}

it("finds stream URLs in JSON API answers, including escaped slashes", async () => {
  const p = page();
  // As servers send it: JSON with "\/"-escaped slashes in one field.
  p.answers.set("/api/play", { type: "application/json",
    body: '{"data":{"hls":"https://cdn.example/v/master.m3u8?t=1&x=2","file":"https:\\/\\/cdn.example\\/v\\/clip.mp4"}}' });
  await p.window.fetch("/api/play");
  await p.flush();
  const urls = p.posted.map((x) => x.url).sort();
  assert.deepEqual(urls, ["https://cdn.example/v/clip.mp4", "https://cdn.example/v/master.m3u8?t=1&x=2"]);
  assert.equal(p.posted.find((x) => /m3u8/.test(x.url)).mime, "application/vnd.apple.mpegurl");
});

it("skips manifests and answers without media", async () => {
  const p = page();
  p.answers.set("/pl", { type: "application/vnd.apple.mpegurl", body: "#EXTM3U\nhttps://cdn.example/s1.mp4\n" });
  p.answers.set("/api/user", { type: "application/json", body: '{"name":"x"}' });
  await p.window.fetch("/pl");
  await p.window.fetch("/api/user");
  await p.flush();
  assert.deepEqual(p.posted, []);
});

it("reads Bilibili player state as best video plus its audio pair", async () => {
  const p = page({ __playinfo__: { data: { dash: { duration: 300,
    video: [{ baseUrl: "https://upos.example/v-480.m4s", bandwidth: 500, mimeType: "video/mp4" },
            { baseUrl: "https://upos.example/v-1080.m4s", bandwidth: 3000, mimeType: "video/mp4" }],
    audio: [{ baseUrl: "https://upos.example/a-64.m4s", bandwidth: 64, mimeType: "audio/mp4" },
            { baseUrl: "https://upos.example/a-192.m4s", bandwidth: 192, mimeType: "audio/mp4" }] } } } });
  await p.flush();
  assert.equal(p.posted.length, 1);
  assert.equal(p.posted[0].url, "https://upos.example/v-1080.m4s");
  assert.equal(p.posted[0].pairUrl, "https://upos.example/a-192.m4s");
  assert.equal(p.posted[0].dur, 300);
});

it("on Facebook, hands video JSON to content.js instead of posting each file", async () => {
  const p = page({ location: { origin: "https://www.facebook.com", hostname: "www.facebook.com" } });
  const body = '{"extensions":{"all_video_dash_prefetch_representations":[{"video_id":"1","representations":[{"mime_type":"video/mp4","base_url":"https:\/\/x.fbcdn.net\/v\/a.mp4"}]}]}}';
  p.answers.set("/api/graphql/", { type: "text/html; charset=utf-8", body });
  p.answers.set("/api/other", { type: "application/json", body: '{"u":"https://x.fbcdn.net/v/b.mp4"}' });
  await p.window.fetch("/api/graphql/");
  await p.window.fetch("/api/other");
  await p.flush();
  assert.deepEqual(p.posted, [], "no loose fbcdn files");
  assert.equal(p.raw.filter((m) => m.__dc === "fb-json").length, 1);
  assert.equal(p.raw.find((m) => m.__dc === "fb-json").text, body);
});
