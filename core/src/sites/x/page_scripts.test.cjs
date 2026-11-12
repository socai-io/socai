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
  replyCount = 12,
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
  const replyButton = new FakeNode({ attributes: { 'aria-label': `${replyCount} Replies` } });
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

function loadScripts({ href, pathname, articles = [], inputs = [], accountUsername = '', primary = null }) {
  const profileLink = accountUsername ? new FakeNode({ href: `https://x.com/${accountUsername}` }) : null;
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    title: 'X fixture',
    readyState: 'complete',
    querySelector: (selector) => {
      if (selector === '[data-testid="primaryColumn"]' && primary) return primary;
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
  assert.equal(state.authenticated, false);
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
    accountUsername: 'signed_in_user',
  });
  const legacy = scripts.pageState();
  const restoration = scripts.sourceSurfaceState();

  assert.equal(Object.hasOwn(legacy, 'search_query'), false);
  assert.equal(Object.hasOwn(legacy, 'scroll_y'), false);
  assert.equal(legacy.authenticated, true);
  assert.equal(restoration.search_query, 'AI agents');
  assert.equal(restoration.scroll_y, 0);
  assert.deepEqual(Array.from(restoration.post_ids), ['555']);

  const profileRestoration = loadScripts({
    href: 'https://x.com/openai/with_replies',
    pathname: '/openai/with_replies',
    articles: [article],
  }).sourceSurfaceState();
  assert.equal(profileRestoration.profile_username, 'openai');
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

test('comments retain unlabeled direct replies within the root reply budget', () => {
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

  assert.deepEqual(Array.from(comments, (comment) => comment.id), ['101']);
  assert.equal(comments[0].is_reply, true);
  assert.deepEqual(Array.from(comments[0].replying_to), ['openai']);
  assert.equal(comments[0].relationship_evidence, 'conversation_region_with_root_reply_budget');
});

test('comments ignore reply-like text inside the post body', () => {
  const root = tweetFixture('100', { replyCount: 0 });
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

test('comments cap unlabeled conversation rows at the rendered root reply count', () => {
  const root = tweetFixture('100', { replyCount: 1 });
  const directReply = tweetFixture('101', {
    username: 'reply_user',
    text: 'The one rendered direct reply',
  });
  const extraRow = tweetFixture('102', {
    username: 'suggested_user',
    text: 'An unlabeled row beyond the root reply budget',
  });
  conversationFixture([root, directReply, extraRow]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, directReply, extraRow],
  });

  assert.deepEqual(Array.from(scripts.comments({ limit: 10 }), (comment) => comment.id), ['101']);
});

test('comments reject an older unlabeled row even when the root has reply budget', () => {
  const root = tweetFixture('100', { replyCount: 1 });
  const olderSuggestion = tweetFixture('99', {
    username: 'suggested_user',
    text: 'An older suggested post rendered after the root',
  });
  const directReply = tweetFixture('101', {
    username: 'reply_user',
    text: 'A chronologically valid direct reply',
  });
  conversationFixture([root, olderSuggestion, directReply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, olderSuggestion, directReply],
  });

  assert.deepEqual(Array.from(scripts.comments({ limit: 10 }), (comment) => comment.id), ['101']);
});

test('comments retain explicit root relationships when the reply metric is zero', () => {
  const root = tweetFixture('100', { replyCount: 0 });
  const explicitReply = tweetFixture('101', {
    username: 'reply_user',
    text: 'An explicitly related reply',
    reply: true,
  });
  conversationFixture([root, explicitReply]);
  const scripts = loadScripts({
    href: 'https://x.com/openai/status/100',
    pathname: '/openai/status/100',
    articles: [root, explicitReply],
  });

  const comments = scripts.comments({ limit: 10 });
  assert.deepEqual(Array.from(comments, (comment) => comment.id), ['101']);
  assert.equal(comments[0].relationship_evidence, 'explicit_reply_target');
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

test('search state accepts the selected Top, People, and Lists tabs', () => {
  function tab(href, selected) {
    return new FakeNode({
      attributes: { role: 'tab', href, 'aria-selected': selected ? 'true' : 'false' },
    });
  }
  const top = tab('/search?q=openai&src=typed_query', true);
  const people = tab('/search?q=openai&src=typed_query&f=user', false);
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    readyState: 'complete',
    querySelector: (selector) => selector.includes('aria-selected="true"') ? top : null,
    querySelectorAll: (selector) => {
      if (selector.includes('role="tab"')) return [top, people];
      if (selector === 'article[data-testid="tweet"], article') return [tweetFixture('1')];
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
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  const context = {
    URL,
    document,
    location: { href: 'https://x.com/search?q=openai&src=typed_query', pathname: '/search' },
    window,
    setTimeout,
  };
  vm.runInNewContext(source, context);
  const topState = window.SocaiXPageScripts.searchState({ query: 'openai', filter: 'top' });
  assert.equal(topState.ok, true);
  assert.equal(topState.filter, 'top');
  const peopleState = window.SocaiXPageScripts.searchState({ query: 'openai', filter: 'people' });
  assert.equal(peopleState.ok, false);
  assert.equal(peopleState.status, 'filter_mismatch');
});

test('people and list collectors read the primary-column controls', () => {
  const profile = new FakeNode({ href: '/OpenAINewsroom' });
  const follow = new FakeNode({ text: 'Follow', attributes: { 'data-testid': '1-follow' } });
  const cell = new FakeNode({
    text: 'OpenAI Newsroom\n@OpenAINewsroom\nFollow\nThe official newsroom',
    attributes: { 'data-testid': 'UserCell' },
    selectors: { 'a[href]': [profile] },
  });
  profile.parentElement = cell;
  follow.parentElement = cell;
  const list = new FakeNode({
    text: 'OpenAI folks\n148 members\nAlex Volkov\n@altryne',
    attributes: { 'data-testid': 'listCell' },
    selectors: {
      span: [new FakeNode({ text: 'OpenAI folks' })],
      'a[href]': [new FakeNode({ href: '/altryne', text: '@altryne' })],
    },
  });
  list['__reactProps$fixture'] = { link: { pathname: '/i/lists/1676646159539130369' } };
  const primary = new FakeNode({
    attributes: { 'data-testid': 'primaryColumn' },
    selectors: {
      '[data-testid="UserCell"]': [cell],
      '[data-testid="listCell"]': [list],
    },
  });
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    readyState: 'complete',
    querySelector: (selector) => selector.includes('primaryColumn') ? primary : null,
    querySelectorAll: () => [],
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), {
    URL, document, location: { href: 'https://x.com/search?q=openai&src=typed_query&f=user', pathname: '/search' }, window, setTimeout,
  });
  const people = window.SocaiXPageScripts.searchPeople({ limit: 5 });
  assert.equal(people.length, 1);
  assert.equal(people[0].username, 'openainewsroom');
  assert.equal(people[0].display_name, 'OpenAI Newsroom');
  assert.equal(people[0].bio, 'The official newsroom');
  const lists = window.SocaiXPageScripts.searchLists({ limit: 5 });
  assert.equal(lists[0].id, '1676646159539130369');
  assert.equal(lists[0].name, 'OpenAI folks');
  assert.equal(lists[0].members, 148);
  assert.equal(lists[0].url, 'https://x.com/i/lists/1676646159539130369');
});

test('like and follow targets stay on the active post and profile header', () => {
  let clicked = false;
  const article = tweetFixture('777');
  const like = new FakeNode({
    text: '12',
    attributes: { 'data-testid': 'like', 'aria-label': '12 Likes. Like' },
  });
  like.getBoundingClientRect = () => ({ left: 80, top: 400, width: 40, height: 30, right: 120, bottom: 430 });
  like.click = () => { clicked = true; };
  like.parentElement = article;
  article.selectors['[data-testid="like"], [data-testid="unlike"]'] = [like];
  const follow = new FakeNode({
    text: 'Follow',
    attributes: { 'data-testid': '4398626122-follow', 'aria-label': 'Follow @openai' },
  });
  follow.getBoundingClientRect = () => ({ left: 700, top: 180, width: 90, height: 36, right: 790, bottom: 216 });
  follow.click = () => { clicked = true; };
  const name = new FakeNode({ text: 'OpenAI\n@openai', attributes: { 'data-testid': 'UserName' } });
  const followers = new FakeNode({ href: '/OpenAI/followers', text: '1,234 Followers' });
  const primary = new FakeNode({
    attributes: { 'data-testid': 'primaryColumn' },
    selectors: {
      '[data-testid="UserName"]': [name],
      'a[href]': [followers],
      'button[data-testid$="-follow"], button[data-testid$="-unfollow"]': [follow],
      'article[data-testid="tweet"], article': [article],
    },
  });
  follow.parentElement = primary;
  const profileLink = new FakeNode({ href: 'https://x.com/asklv' });
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    readyState: 'complete',
    querySelector: (selector) => {
      if (selector.includes('primaryColumn')) return primary;
      return null;
    },
    querySelectorAll: (selector) => {
      if (selector === 'article[data-testid="tweet"], article') return [article];
      if (selector.includes('AppTabBar_Profile_Link')) return [profileLink];
      if (selector.includes('primaryColumn')) return [primary];
      return [];
    },
    elementFromPoint: (x, y) => y > 300 ? like : follow,
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  function load(href, pathname) {
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), {
      URL, document, location: { href, pathname }, window, setTimeout,
    });
    return window.SocaiXPageScripts;
  }
  const postScripts = load('https://x.com/openai/status/777', '/openai/status/777');
  const liked = postScripts.likeTarget({ post_id: '777' });
  assert.equal(liked.ok, true);
  assert.equal(liked.liked, false);
  assert.equal(liked.actor, 'asklv');
  assert.equal(liked.hit_owned, true);
  const profileScripts = load('https://x.com/OpenAI/reposts', '/OpenAI/reposts');
  const detail = profileScripts.profileDetail();
  assert.equal(detail.username, 'openai');
  assert.equal(detail.tab, 'reposts');
  assert.equal(detail.followers, 1234);
  assert.equal(detail.viewer_following, false);
  const followed = profileScripts.followTarget({ username: 'openai' });
  assert.equal(followed.ok, true);
  assert.equal(followed.following, false);
  assert.equal(followed.actor, 'asklv');
  assert.equal(clicked, false);
});

test('home tabs, timeline reply icon, and hover card stay inspection-only', () => {
  let clicked = false;
  const forYou = new FakeNode({
    text: 'For you',
    attributes: { role: 'tab', 'aria-selected': 'true' },
  });
  const following = new FakeNode({
    text: 'Following',
    attributes: { role: 'tab', 'aria-selected': 'false' },
  });
  forYou.click = () => { clicked = true; };
  following.getBoundingClientRect = () => ({ left: 200, top: 10, width: 80, height: 40, right: 280, bottom: 50 });
  const article = tweetFixture('55');
  const reply = article.querySelector('[data-testid="reply"]');
  reply.click = () => { clicked = true; };
  const primary = new FakeNode({
    attributes: { 'data-testid': 'primaryColumn' },
    selectors: {
      '[role="tab"]': [forYou, following],
      'article[data-testid="tweet"], article': [article],
      '[data-testid="User-Name"] a[href]': article.querySelectorAll('[data-testid="User-Name"] a[href]'),
    },
  });
  const name = new FakeNode({ text: 'Fox News', href: '/FoxNews' });
  const handle = new FakeNode({ text: '@FoxNews', href: '/FoxNews' });
  const followingLink = new FakeNode({ text: '290 Following', href: '/FoxNews/following' });
  const followersLink = new FakeNode({ text: '29.4M Followers', href: '/FoxNews/verified_followers' });
  const follow = new FakeNode({
    text: 'Following',
    attributes: { 'data-testid': '1367531-unfollow', 'aria-label': 'Following @FoxNews' },
  });
  follow.click = () => { clicked = true; };
  follow.getBoundingClientRect = () => ({ left: 10, top: 420, width: 80, height: 36, right: 90, bottom: 456 });
  const card = new FakeNode({
    attributes: { 'data-testid': 'HoverCard' },
    selectors: {
      'a[href]': [name, handle, followingLink, followersLink],
      'button': [follow],
      '[data-testid="UserDescription"], [data-testid="UserBio"]': [],
    },
  });
  const homeLink = new FakeNode({
    text: 'Home',
    attributes: { 'data-testid': 'AppTabBar_Home_Link' },
  });
  homeLink.getBoundingClientRect = () => ({ left: 0, top: 200, width: 40, height: 40, right: 40, bottom: 240 });
  const document = {
    body: new FakeNode({ text: 'Hydrated X home' }),
    readyState: 'complete',
    activeElement: null,
    querySelector: (selector) => {
      if (selector.includes('primaryColumn')) return primary;
      if (selector.includes('AppTabBar_Home_Link')) return homeLink;
      if (selector.includes('AppTabBar_Profile_Link')) return new FakeNode({ href: 'https://x.com/tonychonglgtm' });
      return null;
    },
    querySelectorAll: (selector) => {
      if (selector === 'article[data-testid="tweet"], article') return [article];
      if (selector.includes('HoverCard')) return [card];
      if (selector.includes('[role="dialog"]')) return [];
      if (selector.includes('AppTabBar_Profile_Link')) return [new FakeNode({ href: 'https://x.com/tonychonglgtm' })];
      return [];
    },
    elementFromPoint: (x, y) => {
      if (x >= 200 && x <= 280 && y >= 10 && y <= 50) return following;
      if (x < 40 && y >= 200 && y <= 240) return homeLink;
      if (y > 400) return follow;
      return reply;
    },
  };
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), {
    URL, document, location: { href: 'https://x.com/home', pathname: '/home', search: '' }, window, setTimeout,
  });
  const scripts = window.SocaiXPageScripts;
  const feed = scripts.feedState({ tab: 'for-you' });
  assert.equal(feed.ok, true);
  assert.equal(feed.tab, 'For you');
  assert.equal(feed.first_id, '55');
  assert.equal(scripts.feedState({ tab: 'following' }).status, 'tab_mismatch');
  const tab = scripts.feedTabTarget({ tab: 'Following' });
  assert.equal(tab.ok, true);
  assert.equal(tab.selected, false);
  const replyTarget = scripts.streamReplyTarget({ post_id: '55' });
  assert.equal(replyTarget.ok, true);
  assert.equal(replyTarget.username, 'openai');
  const hover = scripts.hoverCardState({ username: 'foxnews' });
  assert.equal(hover.ok, true);
  assert.equal(hover.display_name, 'Fox News');
  assert.equal(hover.following, true);
  assert.equal(hover.followers_count, 29400000);
  assert.equal(scripts.hoverFollowTarget({ username: 'foxnews' }).status, 'following');
  assert.equal(scripts.hoverDismissTarget().ok, true);
  assert.equal(scripts.overlayClosed({ path: '/home' }).status, 'returned');
  assert.equal(clicked, false);
});

test('home inline composer stays inspection-only', () => {
  let clicked = false;
  const profile = new FakeNode({
    href: 'https://x.com/tonyisntstark',
    attributes: { 'data-testid': 'AppTabBar_Profile_Link' },
  });
  const home = new FakeNode({
    href: 'https://x.com/home',
    attributes: { 'data-testid': 'AppTabBar_Home_Link' },
  });
  home.getBoundingClientRect = () => ({ left: 16, top: 58, width: 120, height: 40, right: 136, bottom: 98 });
  home.click = () => { clicked = true; };
  const editor = new FakeNode({
    text: '\n',
    attributes: { 'data-testid': 'tweetTextarea_0', contenteditable: 'true', 'aria-label': 'Post text' },
  });
  editor.getBoundingClientRect = () => ({ left: 350, top: 76, width: 514, height: 28, right: 864, bottom: 104 });
  const postButton = new FakeNode({
    text: 'Post',
    attributes: { 'data-testid': 'tweetButtonInline', role: 'button', 'aria-disabled': 'true' },
  });
  postButton.disabled = true;
  postButton.getBoundingClientRect = () => ({ left: 799, top: 128, width: 67, height: 36, right: 866, bottom: 164 });
  postButton.click = () => { clicked = true; };
  const primary = new FakeNode({
    attributes: { 'data-testid': 'primaryColumn' },
    selectors: {
      '[data-testid="tweetTextarea_0"][contenteditable="true"]': [editor],
      '[data-testid="tweetButtonInline"]': [postButton],
    },
  });
  editor.parentElement = primary;
  postButton.parentElement = primary;
  const mine = tweetFixture('99', { username: 'tonyisntstark', text: 'already posted' });
  const document = {
    body: new FakeNode({ text: 'Hydrated X page' }),
    readyState: 'complete',
    activeElement: editor,
    querySelector: (selector) => {
      if (selector.includes('primaryColumn')) return primary;
      if (selector.includes('AppTabBar_Home_Link')) return home;
      return null;
    },
    querySelectorAll: (selector) => {
      if (selector.includes('AppTabBar_Profile_Link')) return [profile];
      if (selector === 'article[data-testid="tweet"], article') return [mine];
      if (selector.includes('AppTabBar_Home_Link')) return [home];
      return [];
    },
    elementFromPoint: (x, y) => {
      if (y >= 76 && y <= 104 && x >= 350 && x <= 864) return editor;
      if (y >= 128 && y <= 164 && x >= 799) return postButton;
      if (y >= 58 && y <= 98 && x <= 136) return home;
      return null;
    },
  };
  const window = {
    innerHeight: 763,
    innerWidth: 1280,
    scrollY: 0,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block' }),
    scrollBy: () => {},
  };
  const location = { href: 'https://x.com/home', pathname: '/home', search: '' };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), {
    URL, document, location, window, setTimeout,
  });
  const scripts = window.SocaiXPageScripts;
  const composer = scripts.postComposerTarget();
  assert.equal(composer.ok, true);
  assert.equal(composer.actor, 'tonyisntstark');
  assert.equal(composer.status, 'post_editor_ready');
  assert.equal(scripts.postDraftState().value, '\n');
  assert.equal(scripts.postDraftState().focused, true);
  const submit = scripts.postSubmitTarget();
  assert.equal(submit.status, 'post_submit_disabled');
  assert.equal(submit.ok, false);
  assert.equal(scripts.postCloseTarget().status, 'post_close_not_found');
  assert.equal(scripts.homeLinkTarget().ok, true);
  assert.equal(scripts.renderedPostState({ text: 'already posted' }).ids[0], '99');
  location.pathname = '/compose/post';
  location.href = 'https://x.com/compose/post';
  assert.equal(scripts.postComposerTarget().status, 'not_home');
  assert.equal(clicked, false);
});

test('notifications keep replies separate from follows, and follow back skips people already followed', () => {
  const reply = tweetFixture('210', {
    username: 'alex',
    text: 'Happy to connect, the browser line made me smile.',
    reply: true,
    replyTo: 'tonyisntstark',
  });
  const likeButton = reply.selectors['[data-testid="like"], [data-testid="unlike"]'][0];
  likeButton.attributes.set('data-testid', 'unlike');
  reply.selectors['[data-testid="unlike"]'] = [likeButton];
  const follow = new FakeNode({
    text: 'Aman and 2 others followed you',
    attributes: { 'data-testid': 'notification' },
    selectors: {
      '[data-testid^="UserAvatar-Container-"]': [
        new FakeNode({ attributes: { 'data-testid': 'UserAvatar-Container-BuildByAman' } }),
      ],
      'time[datetime]': [new FakeNode({ datetime: '2026-10-05T02:52:34.000Z' })],
    },
  });
  const already = new FakeNode({
    text: 'Aman\n@buildbyaman\nFollows you\nFollowing',
    attributes: { 'data-testid': 'UserCell' },
    selectors: {
      button: [new FakeNode({ text: 'Following', attributes: { 'data-testid': '8-unfollow' } })],
    },
  });
  const followBack = new FakeNode({
    text: 'Ravi\n@ravi_g\nFollows you\nFollow back\nbuilding small tools',
    attributes: { 'data-testid': 'UserCell' },
    selectors: {
      button: [new FakeNode({ text: 'Follow back', attributes: { 'data-testid': '9-follow' } })],
      '[data-testid="UserDescription"]': [new FakeNode({ text: 'building small tools' })],
    },
  });
  const primary = new FakeNode({
    selectors: {
      'article[data-testid="tweet"], article[data-testid="notification"]': [reply, follow],
      '[data-testid="UserCell"]': [already, followBack],
    },
  });
  const notes = loadScripts({
    href: 'https://x.com/notifications',
    pathname: '/notifications',
    primary,
    accountUsername: 'tonyisntstark',
  });
  const items = notes.notificationItems({ limit: 10 });
  assert.equal(items[0].kind, 'reply');
  assert.equal(items[0].author.username, 'alex');
  assert.equal(items[0].liked, true);
  assert.equal(items[0].replying_to[0], 'tonyisntstark');
  assert.equal(items[1].kind, 'follow');
  assert.equal(items[1].accounts[0], 'buildbyaman');

  const followers = loadScripts({
    href: 'https://x.com/TonyisntStark/followers',
    pathname: '/TonyisntStark/followers',
    primary,
  });
  const backs = followers.followBackCandidates({ limit: 10 });
  assert.equal(backs.length, 1);
  assert.equal(backs[0].username, 'ravi_g');
  assert.equal(backs[0].bio, 'building small tools');
  assert.equal(backs[0].follows_you, true);
});

test('post cards read author followers from React props and the verified badge from the DOM', () => {
  const article = tweetFixture('3001', { username: 'fixture_author' });
  const userName = article.querySelector('[data-testid="User-Name"]');
  userName.selectors['[data-testid="icon-verified"]'] = [new FakeNode()];
  const quotedUser = { screen_name: 'someone_else', followers_count: 999 };
  const authorUser = { legacy: { screen_name: 'Fixture_Author', followers_count: 48210 }, is_blue_verified: true };
  article['__reactFiber$fixture'] = {
    memoizedProps: { className: 'css-fixture', children: [] },
    return: {
      memoizedProps: { quotedTweet: { user: quotedUser } },
      return: { memoizedProps: { tweet: { id_str: '3001', user: authorUser } }, return: null },
    },
  };
  const plain = tweetFixture('3002', { username: 'plain_author' });
  const scripts = loadScripts({
    href: 'https://x.com/search?q=fixture&f=live',
    pathname: '/search',
    articles: [article, plain],
  });

  const [first, second] = scripts.searchResults({ limit: 10 });

  assert.equal(first.author.username, 'fixture_author');
  assert.equal(first.author.followers, 48210);
  assert.equal(first.author.verified, true);
  assert.equal(second.author.followers, null);
  assert.equal(second.author.verified, false);
});

test('post cards keep followers null when React props only carry another account', () => {
  const article = tweetFixture('3003', { username: 'fixture_author' });
  article['__reactProps$fixture'] = { tweet: { user: { screen_name: 'other_account', followers_count: 12, verified: true } } };
  const scripts = loadScripts({
    href: 'https://x.com/fixture_author',
    pathname: '/fixture_author',
    articles: [article],
  });

  const [post] = scripts.profilePosts({ limit: 10 });

  assert.equal(post.author.followers, null);
  assert.equal(post.author.verified, false);
});

test('profile header reads followers from the verified_followers link and the verified badge', () => {
  const name = new FakeNode({
    text: 'Fixture\n@fixture_author',
    attributes: { 'data-testid': 'UserName' },
    selectors: { '[data-testid="icon-verified"]': [new FakeNode()] },
  });
  const followers = new FakeNode({ href: '/fixture_author/verified_followers', text: '12.5K Followers' });
  const following = new FakeNode({ href: '/fixture_author/following', text: '310 Following' });
  const article = tweetFixture('3004', { username: 'fixture_author' });
  const primary = new FakeNode({
    attributes: { 'data-testid': 'primaryColumn' },
    selectors: {
      '[data-testid="UserName"]': [name],
      'a[href]': [followers, following],
      'article[data-testid="tweet"], article': [article],
    },
  });
  const scripts = loadScripts({
    href: 'https://x.com/fixture_author',
    pathname: '/fixture_author',
    articles: [article],
    primary,
  });

  const detail = scripts.profileDetail();

  assert.equal(detail.followers, 12500);
  assert.equal(detail.following, 310);
  assert.equal(detail.verified, true);
});
