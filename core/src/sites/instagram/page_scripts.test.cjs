const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

test('reel cover remains an image when no playable video URL is available', () => {
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
  const openState = window.SocaiInstagramPageScripts.postOpenState({ shortcode: 'Fixture123' });

  assert.equal(detail.ok, true);
  assert.equal(detail.kind, 'reel');
  assert.equal(detail.media.length, 1);
  assert.equal(detail.media[0].type, 'image');
  assert.equal(detail.media[0].url, coverUrl);
  assert.equal(detail.media[0].poster_url, '');
  assert.equal(typeof window.SocaiInstagramPageScripts.scrollComments, 'function');
  assert.equal(openState.ok, false);
  assert.equal(openState.status, 'full_page_navigation');
});

test('post detail exposes the playable Instagram video URL', () => {
  const videoUrl = 'https://scontent.cdninstagram.com/o1/v/t16/fixture.mp4?token=signed';
  const metadata = {
    'og:type': 'article',
    'og:url': 'https://www.instagram.com/reel/Video123/',
    'og:video': videoUrl,
    description: '12 likes, 3 comments - creator on September 18, 2026: “Video caption”.',
  };
  const main = {
    querySelector: () => null,
    querySelectorAll: () => [],
  };
  const document = {
    body: { innerText: 'Hydrated Instagram reel' },
    readyState: 'complete',
    title: 'Video fixture',
    querySelector: (selector) => {
      const meta = selector.match(/^meta\[property="([^"]+)"\], meta\[name="\1"\]$/);
      if (meta && metadata[meta[1]]) return { getAttribute: () => metadata[meta[1]] };
      if (selector === 'main') return main;
      return null;
    },
    querySelectorAll: () => [],
  };
  const window = { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/reel/Video123/',
      pathname: '/reel/Video123/',
    },
    performance: { getEntriesByType: () => [] },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const detail = window.SocaiInstagramPageScripts.postDetail();

  assert.equal(detail.ok, true);
  assert.equal(detail.media.length, 1);
  assert.equal(detail.media[0].type, 'video');
  assert.equal(detail.media[0].url, videoUrl);
});

test('comments retain visible replies as a nested tree', () => {
  const body = { parentElement: null };
  const list = {
    parentElement: body,
    matches: (selector) => selector.includes('ul'),
  };
  function fixtureComment(id, author, text, relative, left) {
    const authorLink = { href: `https://www.instagram.com/${author}/` };
    const commentLink = { href: `https://www.instagram.com/p/Post123/c/${id}/` };
    const row = {
      innerText: `${author}\n${text}\n${relative}`,
      parentElement: list,
      querySelectorAll: (selector) => selector === 'a[href]' ? [authorLink] : [],
      getBoundingClientRect: () => ({ left }),
      matches: () => false,
    };
    const time = {
      innerText: relative,
      dateTime: `2026-09-18T0${id}:00:00Z`,
      parentElement: row,
      closest: (selector) => selector === 'a[href*="/c/"]' ? commentLink : null,
      getAttribute: () => '',
    };
    return time;
  }
  const times = [
    fixtureComment('1', 'parent_user', 'Parent comment', '1h', 120),
    fixtureComment('2', 'reply_user', 'Nested reply', '30m', 156),
  ];
  const document = {
    body,
    querySelectorAll: (selector) => selector === 'a[href*="/c/"] time[datetime]' ? times : [],
    querySelector: () => null,
  };
  const window = { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/p/Post123/',
      pathname: '/p/Post123/',
    },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const comments = window.SocaiInstagramPageScripts.comments({ limit: 10 });

  assert.equal(comments.length, 1);
  assert.equal(comments[0].id, '1');
  assert.equal(comments[0].replies.length, 1);
  assert.equal(comments[0].replies[0].id, '2');
  assert.equal(comments[0].replies[0].text, 'Nested reply');
});

test('post card targeting uses the document fallback without invoking the anchor click', () => {
  let anchorClicked = false;
  const cover = {
    tagName: 'IMG',
    src: 'https://scontent.cdninstagram.com/cover.jpg',
    currentSrc: 'https://scontent.cdninstagram.com/cover.jpg',
    alt: 'Fixture post cover',
    getBoundingClientRect: () => ({ left: 40, top: 100, width: 240, height: 300 }),
  };
  const link = {
    href: 'https://www.instagram.com/p/Click123/',
    getAttribute: (name) => name === 'href' ? '/p/Click123/' : '',
    querySelector: (selector) => selector.includes('img[src]') ? cover : null,
    getBoundingClientRect: () => ({ left: 30, top: 90, width: 260, height: 320 }),
    scrollIntoView: () => {},
    click: () => { anchorClicked = true; },
  };
  cover.closest = () => link;
  const document = {
    body: { innerText: 'Instagram profile grid' },
    readyState: 'complete',
    title: 'Fixture profile',
    querySelector: () => null,
    querySelectorAll: (selector) => selector.includes('a[href*="/p/"]') ? [link] : [],
    elementFromPoint: () => cover,
  };
  const window = { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/example/',
      pathname: '/example/',
    },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const target = window.SocaiInstagramPageScripts.postCardTarget({ shortcode: 'Click123' });

  assert.equal(target.ok, true);
  assert.equal(target.shortcode, 'Click123');
  assert.equal(target.target, 'cover');
  assert.equal(target.x, 160);
  assert.equal(target.y, 250);
  assert.equal(target.hit_owned, true);
  assert.equal(anchorClicked, false);
});

test('post card targeting rejects external lookalike URLs', () => {
  const link = {
    href: 'https://example.test/p/External123/',
    getAttribute: () => 'https://example.test/p/External123/',
    querySelector: () => null,
    scrollIntoView: () => {},
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
  };
  const main = { querySelectorAll: () => [link] };
  const document = {
    body: { innerText: 'Instagram profile grid' },
    readyState: 'complete',
    title: 'Fixture profile',
    querySelector: (selector) => selector === 'main' ? main : null,
    querySelectorAll: () => [],
    elementFromPoint: () => link,
  };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/example/',
      pathname: '/example/',
    },
    window: { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) },
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const target = context.window.SocaiInstagramPageScripts.postCardTarget({ shortcode: 'External123' });

  assert.equal(target.ok, false);
  assert.equal(target.status, 'post_card_not_found');
});

test('post close targeting supports the document-level modal close control', () => {
  const postLink = {
    href: 'https://www.instagram.com/p/Close123/',
    getAttribute: () => '/p/Close123/',
  };
  const article = {
    querySelector: () => ({ tagName: 'IMG' }),
  };
  const dialog = {
    getBoundingClientRect: () => ({ left: 300, top: 40, width: 800, height: 720, right: 1100, bottom: 760 }),
    querySelector: (selector) => selector === 'article' ? article : null,
    querySelectorAll: (selector) => {
      if (selector.includes('a[href*="/p/"]')) return [postLink];
      return [];
    },
  };
  const closeButton = {
    innerText: '',
    getAttribute: () => 'Close',
    getBoundingClientRect: () => ({ left: 1180, top: 20, width: 44, height: 44, right: 1224, bottom: 64 }),
    contains: () => false,
  };
  const closeIcon = {
    getBoundingClientRect: () => ({ left: 1190, top: 30, width: 24, height: 24 }),
    closest: () => closeButton,
  };
  const document = {
    body: { innerText: 'Hydrated Instagram post dialog' },
    readyState: 'complete',
    title: 'Instagram fixture',
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === '[role="dialog"]') return [dialog];
      if (selector === 'svg[aria-label="Close" i]') return [closeIcon];
      return [];
    },
    elementFromPoint: () => closeIcon,
  };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/p/Close123/',
      pathname: '/p/Close123/',
    },
    window: {
      innerHeight: 900,
      innerWidth: 1440,
      getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    },
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const target = context.window.SocaiInstagramPageScripts.closePostTarget();

  assert.equal(target.ok, true);
  assert.equal(target.x, 1202);
  assert.equal(target.y, 42);
});

test('SPA post dialog is readable without page-level Open Graph metadata', () => {
  const postLink = {
    href: 'https://www.instagram.com/p/Spa123/',
    getAttribute: () => '/p/Spa123/',
  };
  const authorLink = {
    href: 'https://www.instagram.com/agentbuilder/',
    getAttribute: () => '/agentbuilder/',
  };
  const caption = {
    innerText: 'AI Agents need explicit recovery paths.',
    textContent: 'AI Agents need explicit recovery paths.',
    getBoundingClientRect: () => ({ left: 600, top: 200, width: 360, height: 80 }),
  };
  const article = {
    querySelector: () => caption,
  };
  const dialog = {
    innerText: 'agentbuilder\nAI Agents need explicit recovery paths.\n12 likes',
    getBoundingClientRect: () => ({ left: 300, top: 40, width: 800, height: 720 }),
    querySelector: (selector) => selector === 'article' ? article : null,
    querySelectorAll: (selector) => {
      if (selector.includes('a[href*="/p/"]')) return [postLink];
      if (selector === 'a[href]') return [authorLink];
      if (selector === 'h1') return [caption];
      return [];
    },
  };
  const document = {
    body: { innerText: 'Hydrated Instagram SPA post dialog' },
    readyState: 'complete',
    title: 'Instagram fixture',
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === '[role="dialog"]') return [dialog];
      if (selector === 'article, [role="dialog"]') return [dialog];
      return [];
    },
  };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/p/Spa123/',
      pathname: '/p/Spa123/',
    },
    performance: { getEntriesByType: () => [] },
    window: { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) },
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);

  const detail = context.window.SocaiInstagramPageScripts.postDetail();
  const open = context.window.SocaiInstagramPageScripts.postOpenState({ shortcode: 'Spa123' });

  assert.equal(detail.ok, true);
  assert.equal(detail.caption, 'AI Agents need explicit recovery paths.');
  assert.equal(detail.author.username, 'agentbuilder');
  assert.equal(open.ok, true);
  assert.equal(open.status, 'post_open');
});

test('comment write helpers expose geometry and exact read-back without clicking', () => {
  let clicked = false;
  const postLink = {
    href: 'https://www.instagram.com/p/Write123/',
    getAttribute: () => '/p/Write123/',
  };
  const editor = {
    value: 'A contextual AI Agent comment',
    placeholder: 'Add a comment…',
    getAttribute: (name) => name === 'aria-label' ? 'Add a comment' : '',
    getBoundingClientRect: () => ({ left: 620, top: 680, width: 360, height: 40, right: 980, bottom: 720 }),
    contains: () => false,
  };
  const submit = {
    innerText: 'Post',
    textContent: 'Post',
    disabled: false,
    className: 'submit',
    getAttribute: () => '',
    getBoundingClientRect: () => ({ left: 990, top: 680, width: 64, height: 40, right: 1054, bottom: 720 }),
    contains: () => false,
    click: () => { clicked = true; },
  };
  const rendered = {
    innerText: 'A contextual AI Agent comment',
    textContent: 'A contextual AI Agent comment',
    getBoundingClientRect: () => ({ left: 700, top: 400, width: 280, height: 40, right: 980, bottom: 440 }),
    closest: () => null,
    querySelectorAll: () => [],
    scrollIntoView: () => {},
  };
  const dialog = {
    getBoundingClientRect: () => ({ left: 400, top: 40, width: 700, height: 720, right: 1100, bottom: 760 }),
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector.includes('a[href*="/p/"]')) return [postLink];
      if (selector.includes('textarea')) return [editor];
      if (selector === 'button, [role="button"]') return [submit];
      if (selector === 'span, div, p') return [rendered];
      return [];
    },
  };
  const document = {
    body: { innerText: 'Hydrated Instagram post' },
    readyState: 'complete',
    title: 'Instagram fixture',
    activeElement: editor,
    querySelector: (selector) => selector === 'main' ? dialog : null,
    querySelectorAll: () => [],
    elementFromPoint: (x) => x < 985 ? editor : submit,
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
  };
  const context = {
    URL,
    document,
    location: {
      href: 'https://www.instagram.com/p/Write123/',
      pathname: '/p/Write123/',
    },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  const scripts = window.SocaiInstagramPageScripts;

  const args = { shortcode: 'Write123' };
  assert.equal(scripts.commentEditorTarget(args).ok, true);
  assert.equal(scripts.commentDraftState(args).focused, true);
  assert.equal(scripts.commentDraftState(args).value, 'A contextual AI Agent comment');
  assert.equal(scripts.commentSubmitTarget(args).status, 'comment_submit_ready');
  const renderedState = scripts.renderedCommentState({
    shortcode: 'Write123',
    text: 'A contextual AI Agent comment',
  });
  assert.equal(renderedState.visible, true);
  assert.equal(renderedState.in_viewport, true);
  assert.equal(renderedState.count, 1);
  assert.equal(scripts.commentEditorTarget({ shortcode: 'Other123' }).status, 'comment_editor_not_found');
  assert.equal(scripts.renderedCommentState({
    shortcode: 'Other123',
    text: 'A contextual AI Agent comment',
  }).status, 'wrong_post');
  assert.equal(clicked, false);
});

test('account suggestions keep homepage dropdown order and skip the nav profile', () => {
  const rect = () => ({ width: 180, height: 40, top: 20, left: 80, bottom: 60, right: 260 });
  const account = (username, name, subtitle) => ({
    href: `https://www.instagram.com/${username}/`,
    innerText: [username, name, subtitle].filter(Boolean).join('\n'),
    getAttribute: (attr) => (attr === 'href' ? `/${username}/` : ''),
    querySelector: () => ({ currentSrc: `https://cdn.example/${username}.jpg`, src: '' }),
    getBoundingClientRect: rect,
  });
  const nike = account('nike', 'Nike', 'Followed by ada');
  const nikeRunning = account('nikerunning', 'Nike Running', '');
  const panel = {
    parentElement: {
      querySelector: () => ({ href: '/direct/inbox/' }),
      querySelectorAll: () => [],
    },
    querySelector: () => null,
    querySelectorAll: (selector) => (
      selector === 'a[href], [role="link"][href]' ? [nike, nikeRunning] : []
    ),
  };
  const input = {
    placeholder: 'Search',
    value: 'nike',
    getAttribute: () => '',
    getBoundingClientRect: rect,
    parentElement: panel,
  };
  const document = {
    body: { innerText: 'Instagram home' },
    readyState: 'complete',
    querySelector: () => null,
    querySelectorAll: (selector) => (
      selector.startsWith('input') ? [input] : []
    ),
  };
  const window = { getComputedStyle: () => ({ visibility: 'visible', display: 'block' }) };
  const context = {
    URL,
    document,
    location: { href: 'https://www.instagram.com/', pathname: '/' },
    window,
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), context);
  context.window.__socaiIgSearchWatch = { key: 'accounts:nike\nnike\nnikerunning', count: 2, at: Date.now() - 1000 };

  const result = window.SocaiInstagramPageScripts.accountSuggestions({ query: 'nike' });

  assert.equal(result.ok, true);
  assert.equal(result.status, 'results');
  assert.equal(result.accounts.map((item) => item.username).join(','), 'nike,nikerunning');
  assert.equal(result.accounts[0].position, 1);
  assert.equal(result.accounts[0].name, 'Nike');
  assert.equal(result.accounts[0].subtitle, 'Followed by ada');
  assert.equal(result.accounts[1].position, 2);
  assert.equal(result.accounts[1].name, 'Nike Running');
});
