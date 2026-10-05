const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const VIDEO_ID = '7691179096160470705';
const PAGE_URL = `https://www.douyin.com/video/${VIDEO_ID}`;
const AUTHOR_URL = 'https://www.douyin.com/user/MS4wLjABAAAAfixture';
const COVER_URL = 'https://p3-pc-sign.douyinpic.com/tos-cn-p-0015c000-ce/fixture~tplv-dy-resize-origshort-autoq-75:330.jpeg?biz_tag=pcweb_cover&sc=cover';
const AVATAR_URL = 'https://p3-pc.douyinpic.com/aweme/100x100/aweme-avatar/fixture.jpeg';
const NEXT_COVER_URL = 'https://p3-pc-sign.douyinpic.com/image-cut-tos-priv/next~tplv-dy-resize-origshort-autoq-75:330.jpeg?biz_tag=pcweb_cover';

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

// Just enough DOM for the detail-page extractors: tag, class, id and attribute
// selectors joined by descendant combinators. `hidden` stands in for a
// stylesheet `display: none`.
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
  get poster() { return this.attrs.poster || ''; }
  get src() { return this.attrs.src || ''; }
  get currentSrc() { return this.src; }
  get href() { return this.attrs.href ? new URL(this.attrs.href, PAGE_URL).href : ''; }
  get textContent() {
    return this.childNodes.map((child) => (typeof child === 'string' ? child : child.textContent)).join('\n');
  }
  get innerText() { return this.textContent; }

  getAttribute(name) { return name in this.attrs ? String(this.attrs[name]) : null; }
  getBoundingClientRect() {
    let rendered = true;
    for (let el = this; el; el = el.parentElement) rendered = rendered && !el.attrs.hidden;
    const size = rendered ? 100 : 0;
    return { left: 0, top: 0, right: size, bottom: size, width: size, height: size };
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

// Trimmed from a logged-out www.douyin.com/video/<id> snapshot (2026-10-05).
function detailPage({ publishTime, videoId = VIDEO_ID, frameId = videoId }) {
  const url = `https://www.douyin.com/video/${videoId}`;
  const breadcrumb = JSON.stringify({
    '@type': 'BreadcrumbList',
    itemListElement: [
      { '@type': 'ListItem', position: 1, name: '抖音', item: 'https://www.douyin.com' },
      { '@type': 'ListItem', position: 2, name: '一粒小尘', item: AUTHOR_URL },
      { '@type': 'ListItem', position: 3, name: '视频作品', item: url },
    ],
  });
  const head = el('head', {},
    el('meta', { name: 'lark:url:video_cover_image_url', content: COVER_URL }),
    el('meta', { name: 'lark:url:video_iframe_url', content: `https://www.douyin.com/light/${frameId}` }),
    el('link', { rel: 'canonical', href: url }),
    el('script', { type: 'application/ld+json' }, breadcrumb));
  const player = el('div', { 'data-e2e': 'player-container', class: `wgoQOERl video_${videoId} video-detail-container` },
    el('xg-video-container', { class: 'xg-video-container' },
      el('video', { src: 'blob:https://www.douyin.com/18ca7cc4' }),
      el('div', { class: 'xgplayer-autoplay-tips', hidden: true },
        el('img', { src: NEXT_COVER_URL }))),
    el('div', { class: 'rDq18SJg faZvEPAT', hidden: true },
      el('div', { 'data-e2e': 'video-player-digg' }, '12'),
      el('div', { 'data-e2e': 'feed-comment-icon' }, '2'),
      el('div', { 'data-e2e': 'video-player-collect' }, '收藏'),
      el('div', { 'data-e2e': 'video-player-share' }, '分享')));
  const info = el('div', { 'data-e2e': 'detail-video-info', class: 'mhhhuwS8' },
    el('h1', { class: 'p0KxhPuQ' },
      el('span', {}, '青藤 9 月真的很友好'),
      el('a', { href: '//www.douyin.com/search/%E6%94%80%E5%B2%A9?enter_from=video_detail' }, '#攀岩')),
    el('div', { class: 'NoBOOMd6' },
      el('span', { class: 'hIpNkUXt' }, '12'),
      el('span', { class: 'hIpNkUXt' }, '2'),
      el('span', { class: 'hIpNkUXt' }, '收藏'),
      el('span', { class: 'sB3y0d3B' }, '分享')),
    el('div', { class: 'o9W_EAcI' },
      el('span', { class: 'WK_QYBms' }, '举报'),
      el('span', { class: 'AxYDNgtW', 'data-e2e': 'detail-video-publish-time' }, `发布时间：${publishTime}`)));
  const sidebar = el('div', { class: 'detailPage detailPageSmallScreen' },
    el('div', { 'data-e2e': 'user-info' },
      el('div', { 'data-click-from': 'click_icon' },
        el('a', { href: AUTHOR_URL },
          el('img', { src: AVATAR_URL, alt: '一粒小尘' })))));
  const body = el('body', {},
    el('div', { 'data-e2e': 'video-detail', class: 'xmrXqloh playerControlHeight' },
      el('div', { class: 'leftContainer' }, player, info),
      sidebar));
  return { html: el('html', {}, head, body), body, url };
}

function loadScripts(page) {
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    getComputedStyle: (node) => ({ visibility: 'visible', display: node.attrs.hidden ? 'none' : 'block' }),
  };
  const context = {
    URL,
    window,
    location: { href: page.url },
    performance: { getEntriesByType: () => [] },
    document: {
      body: page.body,
      title: '青藤 9 月真的很友好 - 抖音',
      readyState: 'complete',
      querySelector: (selector) => page.html.querySelector(selector),
      querySelectorAll: (selector) => page.html.querySelectorAll(selector),
    },
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  return window.SocaiDouyinPageScripts;
}

function inTimeZone(zone, run) {
  const previous = process.env.TZ;
  process.env.TZ = zone;
  try {
    return run();
  } finally {
    if (previous === undefined) delete process.env.TZ;
    else process.env.TZ = previous;
  }
}

test('video detail reads publish time, counts and cover from the current markup', () => {
  const detail = inTimeZone('America/Los_Angeles', () =>
    loadScripts(detailPage({ publishTime: '2026-09-29 21:23' })).videoDetail());

  assert.equal(detail.video_id, VIDEO_ID);
  assert.equal(detail.author_url, AUTHOR_URL);
  assert.equal(detail.created_at, '2026-09-30T12:23:00+08:00');
  assert.equal(detail.likes, '12');
  assert.equal(detail.comments_count, '2');
  // 收藏 and 分享 show their bare label; the neighbouring "2" is not theirs.
  assert.equal(detail.favorites, '');
  assert.equal(detail.shares, '');
  assert.equal(detail.views, '');
  assert.equal(detail.cover_url, COVER_URL);
  assert.equal(detail.video.poster_url, COVER_URL);
});

test('video detail reports one publish instant for every browser time zone', () => {
  const detail = inTimeZone('Asia/Shanghai', () =>
    loadScripts(detailPage({ publishTime: '2026-09-30 12:23' })).videoDetail());

  assert.equal(detail.created_at, '2026-09-30T12:23:00+08:00');
});

test('video detail resolves a repeated fall-back hour with the work id', () => {
  // 01:30 comes twice in Los Angeles on 2026-11-01: 08:30Z, then 09:30Z.
  const read = (createdAt) => inTimeZone('America/Los_Angeles', () => {
    const videoId = String(BigInt(Date.parse(createdAt) / 1000) << 32n);
    return loadScripts(detailPage({ publishTime: '2026-11-01 01:30', videoId })).videoDetail().created_at;
  });

  assert.equal(read('2026-11-01T08:29:30Z'), '2026-11-01T16:30:00+08:00');
  assert.equal(read('2026-11-01T09:29:30Z'), '2026-11-01T17:30:00+08:00');
  // An id later than both instants is not a creation time; claim no instant.
  assert.equal(read('2026-11-01T12:00:00Z'), '2026-11-01 01:30');
});

test('video detail leaves the cover empty when the share card names another work', () => {
  const detail = loadScripts(detailPage({
    publishTime: '2026-09-29 21:23',
    frameId: '7000000000000000000',
  })).videoDetail();

  assert.equal(detail.cover_url, '');
  assert.equal(detail.video.poster_url, '');
});
