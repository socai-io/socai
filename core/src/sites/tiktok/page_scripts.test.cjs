const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// Synthetic markup: fake ids and handles, shaped after the selectors
// page_scripts.js reads. It is not a capture of real TikTok pages.
const HANDLE = 'example_creator';
const VIDEO_ID = '1000000000000000001';
const PHOTO_ID = '1000000000000000002';
const VIDEO_URL = `https://www.tiktok.com/@${HANDLE}/video/${VIDEO_ID}`;
const PHOTO_URL = `https://www.tiktok.com/@${HANDLE}/photo/${PHOTO_ID}`;
const VIDEO_COVER = 'https://p16-sign.tiktokcdn.com/obj/example-video-cover.jpeg';
const PHOTO_COVER = 'https://p16-sign.tiktokcdn.com/obj/example-photo-cover.jpeg';

function matchesCompound(el, compound) {
  const attrs = [];
  const rest = compound.replace(/\[([\w-]+)(?:([*^]?)="([^"]*)")?\]/g, (_, name, op, value) => {
    attrs.push({ name, op, value });
    return '';
  });
  const [, tag = '', simple = ''] = rest.match(/^([a-zA-Z][\w-]*)?(.*)$/);
  if (tag && el.tagName !== tag.toUpperCase()) return false;
  for (const [, kind, name] of simple.matchAll(/([.#])([\w-]+)/g)) {
    const hit = kind === '#' ? el.id === name : el.className.split(/\s+/).includes(name);
    if (!hit) return false;
  }
  return attrs.every(({ name, op, value }) => {
    const actual = el.getAttribute(name);
    if (actual === null) return false;
    if (value === undefined) return true;
    if (op === '*') return actual.includes(value);
    if (op === '^') return actual.startsWith(value);
    return actual === value;
  });
}

// Just enough DOM for the card extractors: tag, class, id and attribute
// selectors joined by descendant combinators.
class FakeElement {
  constructor(tag, attrs = {}, children = []) {
    this.tagName = tag.toUpperCase();
    this.attrs = attrs;
    this.childNodes = children;
    this.parentElement = null;
    for (const child of children) {
      if (child instanceof FakeElement) child.parentElement = this;
    }
  }

  get id() { return this.attrs.id || ''; }
  get className() { return this.attrs.class || ''; }
  get alt() { return this.attrs.alt || ''; }
  get src() { return this.attrs.src || ''; }
  get currentSrc() { return this.src; }
  get href() { return this.attrs.href ? new URL(this.attrs.href, 'https://www.tiktok.com/').href : ''; }
  get textContent() {
    return this.childNodes.map((child) => (typeof child === 'string' ? child : child.textContent)).join('\n');
  }
  get innerText() { return this.tagName === 'SCRIPT' ? '' : this.textContent; }

  getAttribute(name) { return name in this.attrs ? String(this.attrs[name]) : null; }
  getBoundingClientRect() {
    return { left: 0, top: 0, right: 100, bottom: 100, width: 100, height: 100 };
  }
  matches(selector) {
    return selector.split(',').some((alternative) => {
      const compounds = alternative.trim().split(/\s+/);
      if (!matchesCompound(this, compounds.pop())) return false;
      for (let el = this.parentElement; el && compounds.length; el = el.parentElement) {
        if (matchesCompound(el, compounds[compounds.length - 1])) compounds.pop();
      }
      return compounds.length === 0;
    });
  }
  closest(selector) {
    for (let el = this; el; el = el.parentElement) {
      if (el.matches(selector)) return el;
    }
    return null;
  }
  querySelectorAll(selector) {
    const found = [];
    const visit = (node) => {
      for (const child of node.childNodes) {
        if (!(child instanceof FakeElement)) continue;
        if (child.matches(selector)) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
}

const el = (tag, attrs, ...children) => new FakeElement(tag, attrs, children);

function videoCard(wrapper) {
  return el('div', { 'data-e2e': wrapper },
    el('a', { href: `/@${HANDLE}/video/${VIDEO_ID}` },
      el('img', { src: VIDEO_COVER, alt: 'A synthetic video' }),
      el('div', { 'data-e2e': 'video-duration' }, '00:15'),
      el('strong', { 'data-e2e': 'video-views' }, '12.3K')),
    el('div', { 'data-e2e': 'search-card-desc' }, 'A synthetic video'),
    el('a', { href: `/@${HANDLE}` }, HANDLE),
    el('span', { 'data-e2e': 'like-count' }, '1.2K'));
}

// The photo card has no <img>, so its cover has to come from the embedded
// item's imagePost. It shows views but no like count.
function photoCard(wrapper) {
  return el('div', { 'data-e2e': wrapper },
    el('a', { href: `/@${HANDLE}/photo/${PHOTO_ID}` },
      el('strong', { 'data-e2e': 'video-views' }, '7,890')),
    el('div', { 'data-e2e': 'search-card-desc' }, 'A synthetic photo carousel'),
    el('a', { href: `/@${HANDLE}` }, HANDLE));
}

function rehydration(extraScope = {}) {
  return el('script', { id: '__UNIVERSAL_DATA_FOR_REHYDRATION__', type: 'application/json' }, JSON.stringify({
    __DEFAULT_SCOPE__: {
      'webapp.search': {
        itemList: [{
          id: PHOTO_ID,
          imagePost: {
            cover: { imageURL: { urlList: [PHOTO_COVER] } },
            images: [{ imageURL: { urlList: [PHOTO_COVER] } }],
          },
        }],
      },
      ...extraScope,
    },
  }));
}

function loadScripts({ url, body, html }) {
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
  };
  const context = {
    URL,
    window,
    location: { href: url },
    performance: { getEntriesByType: () => [] },
    document: {
      body,
      title: 'TikTok',
      readyState: 'complete',
      querySelector: (selector) => html.querySelector(selector),
      querySelectorAll: (selector) => html.querySelectorAll(selector),
    },
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  return window.SocaiTikTokPageScripts;
}

function searchPage() {
  const body = el('body', {},
    el('main', {}, videoCard('search-card-video'), photoCard('search-card-video')),
    rehydration());
  return { url: 'https://www.tiktok.com/search?q=example', body, html: el('html', {}, body) };
}

function authorPage() {
  const user = {
    userInfo: {
      user: { id: '2000000000000000001', uniqueId: HANDLE, nickname: 'Example Creator' },
      stats: { followerCount: 10, followingCount: 2, heartCount: 30, videoCount: 2 },
    },
  };
  const body = el('body', {},
    el('main', {},
      el('h1', { 'data-e2e': 'user-title' }, 'Example Creator'),
      el('h2', { 'data-e2e': 'user-subtitle' }, HANDLE),
      el('div', { 'data-e2e': 'user-post-item-list' },
        videoCard('user-post-item'),
        photoCard('user-post-item'))),
    rehydration({ 'webapp.user-detail': user }));
  return { url: `https://www.tiktok.com/@${HANDLE}`, body, html: el('html', {}, body) };
}

function assertCards(cards) {
  assert.equal(cards.length, 2);
  const [video, photo] = cards;

  assert.equal(video.video_id, VIDEO_ID);
  assert.equal(video.kind, 'video');
  assert.equal(video.url, VIDEO_URL);
  assert.equal(video.author_id, HANDLE);
  assert.equal(video.likes, '1.2K');
  assert.equal(video.views, '12.3K');
  assert.equal(video.comments, null);
  assert.equal(video.shares, null);
  assert.equal(video.duration_seconds, 15);
  assert.equal(video.cover_url, VIDEO_COVER);
  assert.equal(video.position, 0);

  assert.equal(photo.video_id, PHOTO_ID);
  assert.equal(photo.kind, 'photo');
  assert.equal(photo.url, PHOTO_URL);
  assert.equal(photo.author_id, HANDLE);
  assert.equal(photo.author, HANDLE);
  assert.equal(photo.title, 'A synthetic photo carousel');
  assert.equal(photo.views, '7,890');
  // Not shown on the card: unknown, not zero.
  assert.equal(photo.likes, null);
  assert.equal(photo.comments, null);
  assert.equal(photo.shares, null);
  assert.equal(photo.duration_seconds, null);
  assert.equal(photo.video_url, '');
  assert.equal(photo.cover_url, PHOTO_COVER);
  assert.equal(photo.position, 1);
}

test('search cards keep photo carousels next to videos', () => {
  const scripts = loadScripts(searchPage());
  const cards = scripts.videoCards({ limit: 10 });
  assertCards(cards);

  // Ids stay stable across reads (the Rust side dedupes by them).
  const again = scripts.videoCards({ limit: 10 });
  assert.deepEqual([...again].map((card) => card.video_id), [VIDEO_ID, PHOTO_ID]);
});

test('author grid keeps photo carousels next to videos', () => {
  const profile = loadScripts(authorPage()).authorProfile({ limit: 10 });
  assert.equal(profile.entity_type, 'author');
  assert.equal(profile.handle, HANDLE);
  assertCards(profile.video_cards);
});

test('page state counts photo cards', () => {
  const state = loadScripts(searchPage()).pageState();
  assert.equal(state.card_count, 2);
});
