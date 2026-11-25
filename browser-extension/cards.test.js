// The Videos panel's cards for a page's file links: a link whose text
// names nothing ("Download") is shown under the page's title with the
// page's picture, and a stream is sent to the app with the page's name.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function panel() {
  const src = fs.readFileSync(__dirname + '/popup.js', 'utf8');
  const from = src.indexOf('const tagOf = ');
  const tagEnd = src.indexOf('\n};', from) + 3;
  const start = src.indexOf('// Which streams to show');
  const end = src.indexOf('\nfunction group(');
  const context = {
    console, Map, Set, RegExp,
    R: require('./rank.js'),
    safeName: (t, ext) => String(t).replace(/\s*:\s+/g, ' - ').replace(/[\\/:*?"<>|]/g, '_') + ext,
    browserJob: (url, name) => ({ url, name }),
    youtubeItem: () => ({}), facebookItem: () => ({ options: [] }),
  };
  vm.createContext(context);
  vm.runInContext(src.slice(from, tagEnd) + '\n' + src.slice(start, end) + '\nthis.collectItems = collectItems; this.linkTextIsWeak = linkTextIsWeak;', context);
  return context;
}

test('a "Download" link to a movie file gets the page title and picture', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'Spider-Man: Brand New Day',
    thumb: 'https://img.example/poster.jpg',
    links: [
      { url: 'https://files.example/a1b2c3.mp4', text: 'Download' },
      { url: 'https://files.example/setup.exe', text: 'Get the installer' },
    ],
    net: [],
  }, 'https://movies.example/spider-man/');
  const movie = items.find((i) => i.link.endsWith('.mp4'));
  assert.equal(movie.title, 'Spider-Man: Brand New Day');
  assert.equal(movie.thumb, 'https://img.example/poster.jpg');
  // A link with a real name keeps it, and a program gets no movie poster.
  const exe = items.find((i) => i.link.endsWith('.exe'));
  assert.equal(exe.title, 'Get the installer');
  assert.equal(exe.thumb, undefined);
});

test('which link texts name nothing', () => {
  const p = panel();
  for (const t of ['Download', 'download now', 'Click here', 'Direct link', 'Mirror 2', '3.2 GB', '', 'Watch']) {
    assert.equal(p.linkTextIsWeak(t), true, t);
  }
  for (const t of ['Spider-Man 1080p', 'Download Craft setup', 'Season 1 Episode 2']) {
    assert.equal(p.linkTextIsWeak(t), false, t);
  }
});

test('a stream is sent to the app named after the page, not index.m3u8', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'Spider-Man: Brand New Day',
    links: [],
    net: [{ url: 'https://cdn.example/hls/movie.mp4/index.m3u8', mime: 'application/vnd.apple.mpegurl', stream: 'hls', master: true }],
  }, 'https://movies.example/spider-man/');
  const stream = items.find((i) => i.link.endsWith('.m3u8'));
  assert.ok(stream, 'the stream has a card');
  assert.equal(stream.options[0].msg.filename, 'Spider-Man - Brand New Day.mp4');
});

// The IMDb page with an episode player, as it really was: IMDb's own
// 11-second trailer preview, the episode played before (E3) and the one
// playing now (E4), each a master playlist with three qualities whose
// variant playlists were caught too.
test('one card per video, with its qualities; no preview; no earlier episode', () => {
  const p = panel();
  const R = require('./rank.js');
  const master = (base, hs, seenAt) => {
    const body = hs.map((h, i) => `#EXT-X-STREAM-INF:BANDWIDTH=${(i + 1) * 800000},RESOLUTION=${h * 2}x${h}\nv${h}.m3u8`).join('\n');
    const info = R.hlsInfo('#EXTM3U\n' + body, base + 'index.m3u8');
    return { url: base + 'index.m3u8', master: true, variants: info.variants, seenAt, mime: 'application/vnd.apple.mpegurl' };
  };
  const variant = (url, dur, seenAt) => ({ url, dur, seenAt, mime: 'application/vnd.apple.mpegurl' });
  const imdb = 'https://imdb-video.example/previews/';
  const e3 = 'https://cdn.example/e3/';
  const e4 = 'https://cdn.example/e4/';
  const net = [
    master(imdb, [180, 270, 288], 1), variant(imdb + 'v288.m3u8', 11, 2),
    master(e3, [960, 640, 320], 3), variant(e3 + 'v960.m3u8', 2967, 4), variant(e3 + 'v640.m3u8', 2967, 4),
    master(e4, [960, 640, 320], 5), variant(e4 + 'v960.m3u8', 3097, 6), variant(e4 + 'v320.m3u8', 3097, 6),
  ];
  const items = p.collectItems({ title: 'Stranger Things S01E04 - Chapter Four: The Body', links: [], net }, 'https://www.imdb.com/title/tt4574334/');
  const streams = items.filter((i) => i.kind === 'streams');
  assert.equal(streams.length, 1, 'one card for the video playing now: ' + JSON.stringify(items.map((i) => [i.kind, i.link])));
  assert.equal(streams[0].link, e4 + 'index.m3u8');
  assert.equal(JSON.stringify(streams[0].options.map((o) => o.label)), JSON.stringify(['Best quality', '960p', '640p', '320p']));
  assert.equal(streams[0].dur, 3097);
  assert.ok(!items.some((i) => i.link.startsWith(imdb)), 'the 11-second preview is left out');
  assert.equal(items.length, 1, 'nothing else: the earlier episode would carry this name');
  assert.ok(!items.some((i) => /v(960|640|320)\.m3u8$/.test(i.link)), 'no card per quality playlist');
});

test('a master playlist\'s qualities and a media playlist\'s length', () => {
  const R = require('./rank.js');
  const m = R.hlsInfo('#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x320\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x960\nhttps://cdn.example/hi.m3u8', 'https://cdn.example/a/master.m3u8');
  assert.equal(m.master, true);
  assert.deepEqual(m.variants.map((v) => [v.h, v.url]), [[960, 'https://cdn.example/hi.m3u8'], [320, 'https://cdn.example/a/low/index.m3u8']]);
  const media = R.hlsInfo('#EXTM3U\n#EXTINF:5.0,\na.ts\n#EXTINF:5.005,\nb.ts\n#EXTINF:1.2,\nc.ts\n#EXT-X-ENDLIST', 'https://cdn.example/x.m3u8');
  assert.deepEqual([media.master, media.dur], [false, 11]);
});

test('an 11 s preview is left out even when the episode stream could not be read', () => {
  const p = panel();
  const net = [
    { url: 'https://imdb-video.media-imdb.com/previews/v.m3u8', mime: 'application/vnd.apple.mpegurl', dur: 11, seenAt: 1 },
    { url: 'https://cdn.example/a/master.m3u8', mime: 'application/vnd.apple.mpegurl', seenAt: 2 },
  ];
  const items = p.collectItems({ title: 'Stranger Things S01E04', links: [], net }, 'https://www.imdb.com/title/tt4574334/');
  assert.equal(JSON.stringify(items.map((i) => [i.kind, i.link])), JSON.stringify([['streams', 'https://cdn.example/a/master.m3u8']]));
});

test('a short MP4 beside a long video (a preview, a piece of it) is not listed', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'Some long video',
    shot: { x: 0, y: 0, w: 800, h: 450, vw: 1280, vh: 720, dur: 863 },
    links: [],
    net: [
      { url: 'https://cdn.example/preview-720.mp4', mime: 'video/mp4', h: 720, dur: 12, size: 5e6 },
      { url: 'https://cdn.example/full-720.mp4', mime: 'video/mp4', h: 720, dur: 863, size: 3e8 },
    ],
  }, 'https://site.example/watch/1');
  assert.equal(items.some((i) => i.link.includes('preview')), false);
  const video = items.find((i) => i.kind === 'videos');
  assert.deepEqual(JSON.parse(JSON.stringify(video.options.map((o) => o.url))), ['https://cdn.example/full-720.mp4']);
});

test('a short MP4 alone (a real short video) stays a video', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'A reel', shot: { dur: 20 }, links: [],
    net: [{ url: 'https://cdn.example/reel.mp4', mime: 'video/mp4', h: 1280, dur: 20 }],
  }, 'https://site.example/reel/1');
  assert.equal(items.length, 1);
  assert.equal(items[0].kind, 'videos');
});

test('a GIF-path MP4 (a 3 MB hover clip of a 12-minute video) is not the video', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'A long video', shot: { dur: 758 }, links: [
      { url: 'https://site.example/player.php?id=1', text: 'Page media (source)' },
    ],
    net: [{ url: 'https://cdn.example/pics/gifs/055/447/961/55447961a.mp4?validfrom=1', mime: 'video/mp4', size: 3135000 }],
  }, 'https://site.example/view/1');
  assert.equal(items.some((i) => i.link.includes('/gifs/')), false);
  // The .php player script is no download at all.
  assert.equal(items.some((i) => i.link.includes('.php')), false);
});

test('an Instagram reel is one card of qualities sent as the post link, not its pieces', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'Instagram', links: [], net: [],
    social: { post: 'https://www.instagram.com/reel/De7LaHnTe_T/', title: 'NCP explained in 12 seconds', dur: 12, site: 'instagram.com' },
  }, 'https://www.instagram.com/reels/De7LaHnTe_T/');
  assert.equal(items.length, 1);
  const it = items[0];
  assert.equal(it.tag, 'INSTAGRAM');
  assert.equal(it.title, 'NCP explained in 12 seconds');
  assert.deepEqual(JSON.parse(JSON.stringify(it.options.map((o) => o.label))), ['Best quality', '1080p', '720p', '480p', '360p', 'Audio (M4A) · fastest', 'Audio (MP3)']);
  const q720 = it.options.find((o) => o.label === '720p').msg;
  assert.equal(q720.url, 'https://www.instagram.com/reel/De7LaHnTe_T/');
  assert.equal(q720.quality, '720p');
  assert.equal(q720.filename, 'NCP explained in 12 seconds [720p].mp4');
});

test('a stream whose link names no .m3u8 is sent with its type, and shows no playlist size', () => {
  const p = panel();
  const items = p.collectItems({
    title: 'The Cuckold (2009)', links: [],
    net: [{ url: 'https://vercetti.org/seg/9XtqMmahx', mime: 'application/vnd.apple.mpegurl', size: 24, dur: 5707 }],
  }, 'https://www.imdb.com/title/tt1326207/');
  const s = items.find((i) => i.kind === 'streams');
  assert.ok(s, 'listed as a stream');
  assert.equal(s.options[0].msg.mime, 'application/vnd.apple.mpegurl');
  assert.equal(s.options[0].size, 0);
});
