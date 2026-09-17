const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

test('reel cover remains previewable without being reported as an image post', () => {
  const coverUrl = 'https://scontent.cdninstagram.com/reel-cover.jpg?token=fixture';
  const image = {
    tagName: 'IMG',
    src: coverUrl,
    currentSrc: coverUrl,
    alt: 'Reel cover',
    closest: () => null,
  };
  const article = {
    querySelectorAll: (selector) => selector === 'video, img[src]' ? [image] : [],
  };
  const main = {
    querySelector: (selector) => selector === 'article' ? article : null,
    querySelectorAll: (selector) => selector === 'article, [role="dialog"]' ? [article] : [],
  };
  const metadata = {
    'og:type': 'article',
    'og:url': 'https://www.instagram.com/reel/Fixture123/',
    'og:image': coverUrl,
    description: '10 likes, 2 comments - author on September 17, 2026: “Fixture caption”.',
  };
  const document = {
    body: { innerText: 'Fixture reel page with hydrated content' },
    readyState: 'complete',
    title: 'Fixture reel',
    querySelector: (selector) => {
      const meta = selector.match(/^meta\[property="([^"]+)"\], meta\[name="\1"\]$/);
      if (meta && metadata[meta[1]]) return { getAttribute: () => metadata[meta[1]] };
      if (selector === 'main') return main;
      if (selector === 'main img[src], main video') return image;
      return null;
    },
    querySelectorAll: () => [],
  };
  const window = { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/p/Fixture123/',
      pathname: '/p/Fixture123/',
    },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const detail = window.SocaiInstagramPageScripts.postDetail();

  assert.equal(detail.ok, true);
  assert.equal(detail.kind, 'reel');
  assert.equal(detail.media.length, 1);
  assert.equal(detail.media[0].type, 'video');
  assert.equal(detail.media[0].url, '');
  assert.equal(detail.media[0].poster_url, coverUrl);
});
