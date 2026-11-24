// Series episodes are named "Show S01E02 - Episode name", from the embed
// players' addresses and the page's text.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const R = require('./rank.js');

test('the season and episode in embed players\' addresses', () => {
  const cases = [
    ['https://vidsrc.su/embed/tv/tt4574334/1/2', 1, 2],
    ['https://vidsrc.to/embed/tv/tt4574334/1/2', 1, 2],
    ['https://www.2embed.cc/embedtv/tt4574334&s=1&e=2', 1, 2],
    ['https://multiembed.mov/?video_id=tt4574334&s=3&e=14', 3, 14],
    ['https://vidfast.pro/tv/tt4574334/4/9?autoPlay=true', 4, 9],
    ['https://player.example/embed?imdb=tt4574334&season=2&episode=5', 2, 5],
    ['https://cdn.example/stranger.things.s02e05.1080p.mkv', 2, 5],
    // Every server of the IMDb page's player, as they really are.
    ['https://player.vidzee.wtf/embed/tv/tt4574334/1/2', 1, 2],
    ['https://vidsrc.wtf/api/1/tv/?id=tt4574334&s=1&e=2', 1, 2],
    ['https://moviesapi.to/tv/tt4574334/1/2', 1, 2],
    ['https://multiembed.mov/?video_id=tt4574334&tmdb=0&s=1&e=3', 1, 3],
    ['https://hnembed.cc/embed/tv/tt4574334/1/2', 1, 2],
    ['https://vaplayer.ru/embed/tv/tt4574334/1/2', 1, 2],
    ['https://vidsrc-embed.ru/embed/tt4574334/1-2/', 1, 2],
    ['https://vidsrc-embed.su/embed/tt4574334/1-2/', 1, 2],
  ];
  for (const [u, s, e] of cases) assert.deepEqual(R.episodeFromUrl(u), { s, e }, u);
  // A film's player names no episode.
  assert.equal(R.episodeFromUrl('https://vidsrc.su/embed/movie/tt0111161'), null);
});

test('the episode in a page\'s text', () => {
  assert.deepEqual(
    R.episodeFromText('Stranger Things\nSeason 1 • Episode 2 • Chapter Two: The Weirdo on Maple Street\nLucas, Mike and Dustin…'),
    { s: 1, e: 2, name: 'Chapter Two: The Weirdo on Maple Street' },
  );
  assert.deepEqual(R.episodeFromText('S1 · E2 · Chapter Two: The Weirdo on Maple Street'), { s: 1, e: 2, name: 'Chapter Two: The Weirdo on Maple Street' });
  assert.equal(R.episodeFromText('A film page with no episodes'), null);
});

test('an episode is named after the show, the episode and its name', () => {
  const text = 'playing on VidSrc.su\nStranger Things TV Series\nSeason 1 • Episode 2 • Chapter Two: The Weirdo on Maple Street\nLucas, Mike and Dustin try…';
  assert.equal(
    R.episodeTitle('Stranger Things (TV Series 2016–2025)', ['https://vidsrc.su/embed/tv/tt4574334/1/2'], text),
    'Stranger Things S01E02 - Chapter Two: The Weirdo on Maple Street',
  );
  // Another server for the same episode: the same name.
  assert.equal(
    R.episodeTitle('Stranger Things (TV Series 2016–2025)', ['https://www.2embed.cc/embedtv/tt4574334&s=1&e=2'], text),
    'Stranger Things S01E02 - Chapter Two: The Weirdo on Maple Street',
  );
  // The page's text is about another episode than the one playing: no name.
  assert.equal(
    R.episodeTitle('Stranger Things', ['https://vidsrc.su/embed/tv/tt4574334/1/3'], text),
    'Stranger Things S01E03',
  );
  // The IMDb page with a player script, as it really is: the title carries
  // a rating, the season menu runs into the episode list (S5 / E1 on two
  // lines is not S5E1), and the name comes from the list's own line.
  const imdb = 'playing on VidSrc.su\nS1\nS2\nS3\nS4\nS5\nE1 · Chapter One: The Vanishing of Will Byers\nE2 · Chapter Two: The Weirdo on Maple Street\nE3 · Chapter Three: Holly, Jolly\n✓ 2Embed\n✓ VidSrc.su\nEpisodes\n42\nS1 Episodes\nE1\n8.3\nE2\n8.8';
  assert.equal(
    R.episodeTitle('Stranger Things (TV Series 2016–2025) ⭐ 8.6', ['https://vidsrc.su/embed/tv/tt4574334/1/2', 'https://www.imdb.com/title/tt4574334/'], imdb),
    'Stranger Things S01E02 - Chapter Two: The Weirdo on Maple Street',
  );
  assert.equal(R.episodeFromText(imdb), null, 'no S5E1 across lines');
  // Already named, or a film: left as it is.
  assert.equal(R.episodeTitle('Stranger Things S01E02', ['https://vidsrc.su/embed/tv/tt4574334/1/2'], ''), 'Stranger Things S01E02');
  assert.equal(R.episodeTitle('The Shawshank Redemption', ['https://vidsrc.su/embed/movie/tt0111161'], 'A film'), 'The Shawshank Redemption');
});
