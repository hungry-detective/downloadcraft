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
  const start = src.indexOf('// A link text that names nothing');
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
