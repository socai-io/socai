const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

class FakeNode {
  constructor({ text = '', href = '', src = '', datetime = '', attributes = {}, selectors = {} } = {}) {
    this.innerText = text;
    this.textContent = text;
    this.href = href;
    this.src = src;
    this.currentSrc = src;
    this.dateTime = datetime;
    this.attributes = new Map(Object.entries(attributes));
    this.selectors = selectors;
    this.parentElement = null;
    if (href) this.attributes.set('href', href);
    if (src) this.attributes.set('src', src);
    if (datetime) this.attributes.set('datetime', datetime);
  }

  getAttribute(name) { return this.attributes.get(name) || ''; }
  querySelector(selector) { return (this.selectors[selector] || [])[0] || null; }
  querySelectorAll(selector) { return this.selectors[selector] || []; }
  contains(node) {
    for (let current = node; current; current = current.parentElement) {
      if (current === this) return true;
    }
    return false;
  }
  getBoundingClientRect() { return { left: 10, top: 10, width: 300, height: 180, right: 310, bottom: 190 }; }
  closest(selector) {
    if (selector.includes('article') && this.getAttribute('data-testid') === 'tweet') return this;
    if (selector.includes('[role="region"]') && this.getAttribute('role') === 'region') return this;
    return this.parentElement && this.parentElement.closest ? this.parentElement.closest(selector) : null;
  }
}

function conversationFixture(articles) {
  const region = new FakeNode({
    attributes: { role: 'region', 'aria-label': 'Timeline: Conversation' },
    selectors: { 'article[data-testid="tweet"], article': articles },
  });
  for (const article of articles) article.parentElement = region;
  return region;
}

function tweetFixture(id, {
  username = 'openai',
  text = 'Fixture post',
  reply = false,
  replyTo = 'openai',
  replyLabel = 'Replying to',
} = {}) {
  const status = new FakeNode({ href: `https://x.com/${username}/status/${id}` });
  const time = new FakeNode({ datetime: '2026-09-23T02:00:00.000Z' });
  time.parentElement = status;
  status.querySelector = (selector) => selector === 'time[datetime]' ? time : null;
  const authorLink = new FakeNode({ href: `https://x.com/${username}` });
  const userName = new FakeNode({
    text: `OpenAI\n@${username}`,
    selectors: { 'a[href]': [authorLink] },
  });
  const replyContext = reply ? new FakeNode({ text: `${replyLabel} @${replyTo}` }) : null;
  const body = new FakeNode({ text });
  const image = new FakeNode({ src: 'https://pbs.twimg.com/media/fixture.jpg', attributes: { alt: 'Fixture image' } });
  const photo = new FakeNode({ selectors: { 'img[src]': [image] } });
  const replyButton = new FakeNode({ attributes: { 'aria-label': '12 Replies' } });
  const repostButton = new FakeNode({ attributes: { 'aria-label': '34 reposts' } });
  const likeButton = new FakeNode({ attributes: { 'aria-label': '56 Likes' } });
  const article = new FakeNode({
    text: `${reply ? `${replyLabel} @${replyTo}\n` : ''}OpenAI\n@${username}\n${text}\n12\n34\n56`,
    attributes: { 'data-testid': 'tweet' },
    selectors: {
      '[data-testid="User-Name"]': [userName],
      '[data-testid="tweetText"]': [body],
      'a[href*="/status/"] time[datetime]': [time],
      'a[href*="/status/"]': [status],
      '[data-testid="tweetPhoto"]': [photo],
      '[data-testid="tweetPhoto"] img[src]': [image],
      'video': [],
      '[data-testid="reply"]': [replyButton],
      '[data-testid="retweet"], [data-testid="unretweet"]': [repostButton],
      '[data-testid="like"], [data-testid="unlike"]': [likeButton],
      '[data-testid="bookmark"], [data-testid="removeBookmark"]': [],
      '[data-testid="quoteTweet"]': [],
      'span, div[dir="ltr"]': replyContext ? [replyContext] : [],
    },
  });
  for (const child of [status, time, userName, body, photo, image, replyButton, repostButton, likeButton, replyContext].filter(Boolean)) {
    child.parentElement = article;
  }
  return article;
}

function loadScripts({ href, pathname, articles = [], inputs = [], accountUsername = '' }) {
  const profileLink = accountUsername ? new FakeNode({ href: `https://x.com/${accountUsername}` }) : null;
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    title: 'X fixture',
    readyState: 'complete',
    querySelector: (selector) => {
      if (selector.includes('input')) return inputs[0] || null;
      return null;
    },
    querySelectorAll: (selector) => {
      if (selector === 'article[data-testid="tweet"], article') return articles;
      if (selector.includes('input')) return inputs;
      if (selector.includes('AppTabBar_Profile_Link')) return profileLink ? [profileLink] : [];
      return [];
    },
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  const context = { URL, document, location: { href, pathname }, window, setTimeout };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  return window.SocaiXPageScripts;
}

test('page state reports the observed X login flow as login_required', () => {
  const input = new FakeNode({ attributes: { autocomplete: 'username' } });
  const scripts = loadScripts({
    href: 'https://x.com/i/flow/login?redirect_after_login=%2Fsearch',
    pathname: '/i/flow/login',
    inputs: [input],
  });

  const state = scripts.pageState();

  assert.equal(state.ok, false);
  assert.equal(state.login_required, true);
  assert.equal(state.status, 'login_required');
});

test('post detail returns canonical identity, content, media, and metrics', () => {
  const article = tweetFixture('1234567890');
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/1234567890',
    pathname: '/openai/status/1234567890',
    articles: [article],
  });

  const detail = scripts.postDetail();

  assert.equal(detail.ok, true);
  assert.equal(detail.id, '1234567890');
  assert.equal(detail.author.username, 'openai');
  assert.equal(detail.text, 'Fixture post');
  assert.equal(detail.media.length, 1);
  assert.equal(detail.media[0].type, 'image');
  assert.equal(detail.metrics.replies, 12);
  assert.equal(detail.metrics.reposts, 34);
  assert.equal(detail.metrics.likes, 56);
});

test('post-open state requires hydrated author, timestamp, and root content', () => {
  const hydrated = tweetFixture('1234567890');
  const hydratedScripts = loadScripts({
    href: 'https://x.com/openai/status/1234567890',
    pathname: '/openai/status/1234567890',
    articles: [hydrated],
  });
  assert.equal(hydratedScripts.postOpenState({ id: '1234567890' }).ok, true);

  const missingAuthor = tweetFixture('1234567890');
  missingAuthor.selectors['[data-testid="User-Name"]'] = [];
  const missingAuthorScripts = loadScripts({
    href: 'https://x.com/openai/status/1234567890',
    pathname: '/openai/status/1234567890',
    articles: [missingAuthor],
  });
  assert.equal(missingAuthorScripts.postOpenState({ id: '1234567890' }).status, 'post_unhydrated');

  const emptyAuthor = tweetFixture('1234567890');
  const emptyAuthorRoot = emptyAuthor.querySelector('[data-testid="User-Name"]');
  emptyAuthorRoot.innerText = '';
  emptyAuthorRoot.textContent = '';
  emptyAuthorRoot.selectors['a[href]'] = [];
  const emptyAuthorScripts = loadScripts({
    href: 'https://x.com/openai/status/1234567890',
    pathname: '/openai/status/1234567890',
    articles: [emptyAuthor],
  });
  assert.equal(emptyAuthorScripts.postOpenState({ id: '1234567890' }).status, 'post_unhydrated');

  const missingTimestamp = tweetFixture('1234567890');
  missingTimestamp.selectors['a[href*="/status/"] time[datetime]'] = [];
  const missingTimestampScripts = loadScripts({
    href: 'https://x.com/openai/status/1234567890',
    pathname: '/openai/status/1234567890',
    articles: [missingTimestamp],
  });
  assert.equal(missingTimestampScripts.postOpenState({ id: '1234567890' }).status, 'post_unhydrated');
});

test('restoration metadata stays out of the legacy page-state shape', () => {
  const article = tweetFixture('555');
  const scripts = loadScripts({
    href: 'https://x.com/search?q=AI%20agents&src=typed_query&f=live',
    pathname: '/search',
    articles: [article],
  });
  const legacy = scripts.pageState();
  const restoration = scripts.sourceSurfaceState();

  assert.equal(Object.hasOwn(legacy, 'search_query'), false);
  assert.equal(Object.hasOwn(legacy, 'scroll_y'), false);
  assert.equal(restoration.search_query, 'AI agents');
  assert.equal(restoration.scroll_y, 0);
});

test('comments exclude the root post and retain reply posts', () => {
  const root = tweetFixture('100');
  const reply = tweetFixture('101', { username: 'reply_user', text: 'A visible reply', reply: true });
  conversationFixture([root, reply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, reply],
  });

  const comments = scripts.comments({ limit: 10 });

  assert.equal(comments.length, 1);
  assert.equal(comments[0].id, '101');
  assert.equal(comments[0].author.username, 'reply_user');
  assert.equal(comments[0].text, 'A visible reply');
});

test('comments fail closed on an unlabeled post inside the conversation region', () => {
  const root = tweetFixture('100');
  const directReply = tweetFixture('101', {
    username: 'reply_user',
    text: 'An unlabeled module that may be a reply or recommendation',
    reply: false,
  });
  conversationFixture([root, directReply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, directReply],
  });

  const comments = scripts.comments({ limit: 10 });

  assert.deepEqual(Array.from(comments, (comment) => comment.id), []);
});

test('comments ignore reply-like text inside the post body', () => {
  const root = tweetFixture('100');
  const unrelated = tweetFixture('101', {
    username: 'other_user',
    text: 'This article literally says Replying to @openai but is not a reply',
    reply: false,
  });
  conversationFixture([root, unrelated]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, unrelated],
  });

  assert.deepEqual(Array.from(scripts.comments({ limit: 10 }), (comment) => comment.id), []);
});

test('comments reject unrelated recommendations and conversation ancestors', () => {
  const ancestor = tweetFixture('99', { username: 'ancestor', text: 'Earlier post' });
  const root = tweetFixture('100');
  const reply = tweetFixture('101', { username: 'reply_user', text: 'A visible reply', reply: true });
  const recommendation = tweetFixture('102', {
    username: 'recommended_user',
    text: 'Unrelated recommendation',
    reply: true,
    replyTo: 'someone_else',
  });
  conversationFixture([ancestor, root, reply]);
  conversationFixture([recommendation]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [ancestor, root, reply, recommendation],
  });

  const comments = scripts.comments({ limit: 10 });

  assert.deepEqual(Array.from(comments, (comment) => comment.id), ['101']);
});

test('rendered reply state requires the signed-in author and active conversation relationship', () => {
  const root = tweetFixture('100');
  const reply = tweetFixture('101', {
    username: 'asklv',
    text: 'An exact visible reply',
    reply: false,
  });
  conversationFixture([root, reply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, reply],
    accountUsername: 'asklv',
  });

  const state = scripts.renderedReplyState({ post_id: '100', text: 'An exact visible reply' });

  assert.equal(state.ok, true);
  assert.equal(state.visible, true);
  assert.equal(state.count, 1);
  assert.deepEqual(Array.from(state.ids), ['101']);

  const unrelated = tweetFixture('102', {
    username: 'asklv',
    text: 'An exact visible reply',
    reply: true,
    replyTo: 'someone_else',
  });
  conversationFixture([root, unrelated]);
  const unrelatedScripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, unrelated],
    accountUsername: 'asklv',
  });
  assert.equal(unrelatedScripts.renderedReplyState({
    post_id: '100',
    text: 'An exact visible reply',
  }).visible, false);
});

test('localized reply labels are retained as conversation replies', () => {
  const root = tweetFixture('200');
  const reply = tweetFixture('201', {
    username: 'reply_user',
    text: '本地化界面的回复',
    reply: true,
    replyLabel: '正在回复',
  });
  conversationFixture([root, reply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/200',
    pathname: '/openai/status/200',
    articles: [root, reply],
  });

  const comments = scripts.comments({ limit: 10 });

  assert.equal(comments.length, 1);
  assert.equal(comments[0].id, '201');
  assert.equal(comments[0].is_reply, true);
});

test('write helpers expose trusted geometry and draft state without clicking', () => {
  let clicked = false;
  const article = tweetFixture('777');
  const statusLink = article.querySelectorAll('a[href*="/status/"]')[0];
  statusLink.getBoundingClientRect = () => ({ left: 500, top: 100, width: 80, height: 24, right: 580, bottom: 124 });
  statusLink.click = () => { clicked = true; };
  const search = new FakeNode();
  search.value = 'AI agents';
  search.getBoundingClientRect = () => ({ left: 300, top: 20, width: 320, height: 44, right: 620, bottom: 64 });
  const submit = new FakeNode({
    text: 'Responder',
    attributes: { 'data-testid': 'tweetButtonInline', role: 'button' },
  });
  submit.disabled = false;
  submit.getBoundingClientRect = () => ({ left: 900, top: 700, width: 80, height: 40, right: 980, bottom: 740 });
  submit.click = () => { clicked = true; };
  const editor = new FakeNode({
    text: 'A contextual reply',
    attributes: { 'data-testid': 'tweetTextarea_0', contenteditable: 'true' },
  });
  let editorTop = 620;
  editor.getBoundingClientRect = () => ({ left: 500, top: editorTop, width: 400, height: 70, right: 900, bottom: editorTop + 70 });
  const composer = new FakeNode({
    selectors: { '[data-testid="tweetButtonInline"], [data-testid="tweetButton"]': [submit] },
  });
  const region = conversationFixture([article]);
  editor.parentElement = composer;
  submit.parentElement = composer;
  composer.parentElement = region;

  const document = {
    body: new FakeNode({ text: 'Hydrated X post' }),
    title: 'X fixture',
    readyState: 'complete',
    activeElement: editor,
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === 'article[data-testid="tweet"], article') return [article];
      if (selector.includes('SearchBox_Search_Input')) return [search];
      if (selector.includes('tweetTextarea_0')) return [editor];
      return [];
    },
    elementFromPoint: (x, y) => {
      if (y < 80) return search;
      if (y < 200) return statusLink;
      if (x >= 900) return submit;
      return editor;
    },
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  const context = {
    URL,
    document,
    location: { href: 'https://x.com/openai/status/777', pathname: '/openai/status/777' },
    window,
    setTimeout,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  const scripts = window.SocaiXPageScripts;

  assert.equal(scripts.searchInputTarget().value, 'AI agents');
  assert.equal(scripts.postLinkTarget({ id: '777' }).hit_owned, true);
  const args = { post_id: '777' };
  assert.equal(scripts.replyEditorTarget(args).ok, true);
  assert.equal(editorTop, 620);
  assert.equal(scripts.replyDraftState(args).value, 'A contextual reply');
  assert.equal(scripts.replyDraftState(args).focused, true);
  assert.equal(scripts.replySubmitTarget(args).status, 'reply_submit_ready');
  assert.equal(scripts.replyEditorTarget({ post_id: '778' }).status, 'wrong_post');
  assert.equal(clicked, false);
});
