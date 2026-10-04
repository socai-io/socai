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
    },
  });
  for (const child of [status, time, userName, body, photo, image, replyButton, repostButton, likeButton]) {
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
