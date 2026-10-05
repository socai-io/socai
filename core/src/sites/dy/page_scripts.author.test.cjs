const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

// The fixture is real page markup, so these tests run the script's own
// selectors against it through a small DOM: enough HTML parsing and selector
// matching (tag, #id, .class, [attr], [attr="v"], *=, ^=, $=, descendant
// combinator, comma lists) for what page_scripts.js asks of a page.

const VOID_TAGS = new Set(['img', 'meta', 'link', 'input', 'br', 'hr', 'source']);
const BLOCK_TAGS = new Set(['div', 'p', 'h1', 'h2', 'ul', 'li', 'header', 'button', 'body']);
const ENTITIES = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };
const decode = (value) => value.replace(/&(amp|lt|gt|quot|#39);/g, (entity) => ENTITIES[entity]);

class TextNode {
  constructor(value) {
    this.nodeType = 3;
    this.nodeValue = value;
  }
}

class Element {
  constructor(tag, attributes) {
    this.nodeType = 1;
    this.tagName = tag.toUpperCase();
    this.attributes = attributes;
    this.childNodes = [];
    this.parentElement = null;
    this.clicks = 0;
  }

  get children() { return this.childNodes.filter((node) => node.nodeType === 1); }
  get id() { return this.attributes.get('id') || ''; }
  get className() { return this.attributes.get('class') || ''; }
  get alt() { return this.attributes.get('alt') || ''; }
  get src() { return this.attributes.get('src') || ''; }
  get currentSrc() { return this.src; }
  get href() {
    const href = this.attributes.get('href');
    return href ? new URL(href, 'https://www.douyin.com/').href : '';
  }
  get textContent() {
    return this.childNodes.map((node) => (node.nodeType === 3 ? node.nodeValue : node.textContent)).join('');
  }
  // innerText leaves out script text and breaks lines around block boxes.
  get innerText() {
    if (this.tagName === 'SCRIPT') return '';
    const inner = this.childNodes.map((node) => (node.nodeType === 3 ? node.nodeValue : node.innerText)).join('');
    return BLOCK_TAGS.has(this.tagName.toLowerCase()) ? `\n${inner}\n` : inner;
  }

  getAttribute(name) { return this.attributes.has(name) ? this.attributes.get(name) : null; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 120, height: 24, right: 120, bottom: 24 }; }
  click() { this.clicks += 1; }
  contains(node) {
    for (let current = node; current; current = current.parentElement) {
      if (current === this) return true;
    }
    return false;
  }
  matches(selector) { return parseSelectorList(selector).some((chain) => matchesChain(this, chain)); }
  closest(selector) {
    for (let current = this; current; current = current.parentElement) {
      if (current.matches(selector)) return current;
    }
    return null;
  }
  querySelectorAll(selector) {
    const chains = parseSelectorList(selector);
    const found = [];
    const visit = (node) => {
      for (const child of node.children) {
        if (chains.some((chain) => matchesChain(child, chain))) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
}

function splitOutsideBrackets(selector, separator) {
  const parts = [];
  let depth = 0;
  let current = '';
  for (const char of selector) {
    if (char === '[') depth += 1;
    if (char === ']') depth -= 1;
    if (depth === 0 && separator.test(char)) {
      if (current) parts.push(current);
      current = '';
    } else {
      current += char;
    }
  }
  if (current) parts.push(current);
  return parts;
}

function parseCompound(source) {
  const shape = source.match(/^([a-zA-Z][\w-]*)?((?:#[\w-]+|\.[\w-]+|\[[^\]]+\])*)$/);
  if (!shape) throw new Error(`unsupported selector: ${source}`);
  const tests = [];
  if (shape[1]) tests.push((el) => el.tagName === shape[1].toUpperCase());
  for (const part of shape[2].match(/#[\w-]+|\.[\w-]+|\[[^\]]+\]/g) || []) {
    if (part[0] === '#') {
      tests.push((el) => el.id === part.slice(1));
    } else if (part[0] === '.') {
      tests.push((el) => el.className.split(/\s+/).includes(part.slice(1)));
    } else {
      const [, name, operator, value] = part.match(/^\[([\w:-]+)(?:([*^$]?=)"([^"]*)")?\]$/) || [];
      if (!name) throw new Error(`unsupported selector: ${source}`);
      tests.push((el) => {
        const actual = el.getAttribute(name);
        if (actual === null) return false;
        if (operator === '=') return actual === value;
        if (operator === '*=') return actual.includes(value);
        if (operator === '^=') return actual.startsWith(value);
        if (operator === '$=') return actual.endsWith(value);
        return true;
      });
    }
  }
  return (el) => tests.every((check) => check(el));
}

const selectorCache = new Map();
function parseSelectorList(selector) {
  if (!selectorCache.has(selector)) {
    selectorCache.set(selector, splitOutsideBrackets(selector, /,/)
      .map((chain) => splitOutsideBrackets(chain.trim(), /\s/).map(parseCompound)));
  }
  return selectorCache.get(selector);
}

function matchesChain(el, chain) {
  if (!chain[chain.length - 1](el)) return false;
  let ancestor = el.parentElement;
  for (let index = chain.length - 2; index >= 0; index -= 1) {
    while (ancestor && !chain[index](ancestor)) ancestor = ancestor.parentElement;
    if (!ancestor) return false;
    ancestor = ancestor.parentElement;
  }
  return true;
}

function parseHtml(html) {
  const root = new Element('#document', new Map());
  const stack = [root];
  const token = /<!--[\s\S]*?-->|<\/([\w-]+)>|<([\w-]+)((?:\s+[\w:-]+(?:="[^"]*")?)*)\s*(\/?)>|([^<]+)/g;
  let match;
  while ((match = token.exec(html))) {
    const parent = stack[stack.length - 1];
    if (match[1]) {
      stack.pop();
    } else if (match[2]) {
      const attributes = new Map();
      for (const [, name, value] of match[3].matchAll(/([\w:-]+)(?:="([^"]*)")?/g)) {
        attributes.set(name, decode(value || ''));
      }
      const element = new Element(match[2], attributes);
      element.parentElement = parent === root ? null : parent;
      parent.childNodes.push(element);
      if (match[2] === 'script') {
        const end = html.indexOf('</script>', token.lastIndex);
        element.childNodes.push(new TextNode(html.slice(token.lastIndex, end)));
        token.lastIndex = end + '</script>'.length;
      } else if (!match[4] && !VOID_TAGS.has(match[2])) {
        stack.push(element);
      }
    } else if (match[5]) {
      parent.childNodes.push(new TextNode(decode(match[5])));
    }
  }
  return root;
}

const FIXTURE = fs.readFileSync(path.join(__dirname, 'fixtures', 'author_profile.html'), 'utf8');
const SOURCE = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
const AUTHOR_ID = 'MS4wLjABAAAAfixtureAuthor000000000000000000000000000000000';

function replaceOnce(html, from, to) {
  assert.ok(html.includes(from), `fixture no longer contains: ${from.slice(0, 60)}`);
  return html.replace(from, to);
}

function withoutRecord(html) {
  const start = html.indexOf('<script');
  return html.slice(0, start) + html.slice(html.indexOf('</script>') + '</script>'.length);
}

function loadPage(html) {
  const root = parseHtml(html);
  const document = {
    readyState: 'complete',
    title: root.querySelector('title').textContent,
    body: root.querySelector('body'),
    querySelector: (selector) => root.querySelector(selector),
    querySelectorAll: (selector) => root.querySelectorAll(selector),
  };
  const window = {
    innerWidth: 1600,
    innerHeight: 1000,
    getComputedStyle: () => ({ visibility: 'visible', display: 'block', opacity: '1' }),
  };
  const context = {
    URL,
    document,
    location: { href: `https://www.douyin.com/user/${AUTHOR_ID}` },
    performance: { getEntriesByType: () => [] },
    window,
  };
  vm.runInNewContext(SOURCE, context);
  const scripts = window.SocaiDouyinPageScripts;
  // Results are built inside the vm realm; a JSON round trip makes them plain.
  const plain = (value) => JSON.parse(JSON.stringify(value));
  return {
    document,
    scripts,
    profile: (arg) => plain(scripts.authorProfile(arg || { limit: 6 })),
    state: () => plain(scripts.authorState()),
  };
}

// Header variants below are the markup Douyin rendered for other live
// profiles on the same day.
const BIO_ROW = '<div class="t_m2gjO6"><span><span class="m53pwJvW"><span><span><span><span>山高水长，一期一会 estj<img draggable="false" class="qz6Hz920" alt="♌" src="//p-pc-weboff.byteimg.com/tos-cn-i-9r5gewecjs/twemoji/72x72/264c.png"></span></span></span></span></span></span></div>';
const CUT_BIO_ROW = '<div class="t_m2gjO6"><div class="Ae8QdcOg"><span><span class="m53pwJvW"><span><span><span><span>发布示例信息  传递示例力量 示例投稿邮箱demo@...</span></span></span></span></span></span><div class="XuFbeDJO"><span class="udo3cLwt">更多</span></div></div></div>';
const ENTRY_CARD_ROW = '<div class="Fb9QG84G NtWWWbnJ"><div class="vrza29K4"><svg width="16" height="16"></svg><div class="vyfxiR8y">精彩直播回放</div></div><svg width="12" height="12"></svg></div>';
const AGE_TAG = '<span class="rTzhSEM4"><span>35岁</span></span>';
const FEMALE_ICON = '<svg width="12" height="12" fill="none" xmlns="http://www.w3.org/2000/svg" class="" viewBox="0 0 12 12" style="margin-right: 4px;"><mask id="woman_svg__a" maskUnits="userSpaceOnUse" x="-2" y="-2" width="16" height="16" style="mask-type: alpha;"><path fill="#C4C4C4" d="M-2-2h16v16H-2z"></path></mask><g mask="url(#woman_svg__a)" stroke="#F5588E" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="7.2" cy="4.896" r="3.25"></circle><path d="M1.617 10.511l3.115-3.115M1.904 7.396l2.828 2.829"></path></g></svg>';
// No profile in the capture showed the male tag; this is the icon the profile
// route's own bundle renders for it.
const MALE_ICON = '<svg width="12" height="12" fill="none" xmlns="http://www.w3.org/2000/svg" class="" viewBox="0 0 12 12" style="margin-right: 4px;"><path fill-rule="evenodd" clip-rule="evenodd" d="M8 1.25a.75.75 0 0 0 0 1.5h1.09L7.54 4.298a.757.757 0 0 0-.058.066 4 4 0 1 0 .968 1.112.752.752 0 0 0 .15-.117L10.25 3.71V5a.75.75 0 0 0 1.5 0V2a.75.75 0 0 0-.75-.75H8zM5 10a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z" fill="#168EF9"></path></svg>';
const NAME_ROW = '<div class="REOLM3RC"><h1 class="m5re_8jG"><span><span class="m53pwJvW"><span><span><span><span>示例作者</span></span></span></span></span></span></h1></div>';
const VERIFIED_NAME_ROW = '<div class="REOLM3RC"><h1 class="m5re_8jG"><span><span class="m53pwJvW"><span><span><span><span>示例作者</span></span></span></span></span></span></h1><div><div>认证徽章</div><span data-e2e="badge-role-name">示例传媒集团官方抖音号</span></div></div>';
const EMPTY_LIST = '<ul class="cPDrcaOY QhXy7t32" data-e2e="scroll-list"></ul><div class="Zj5Fgmnv" style="width: 100%;"><div class="_98IcTwY">服务异常，重新<span class="wRwYD7Ar">刷新</span>拉取数据</div></div>';
// A stand-in card: the logged-out capture never received a loaded works list.
const LOADED_LIST = '<ul class="cPDrcaOY QhXy7t32" data-e2e="scroll-list"><li><a href="/video/7000000000000000001"><img src="https://p3-pc-sign.douyinpic.com/fixture-cover.jpeg" alt="第一条示例作品的标题"></a></li></ul>';

test('author header reads the bio, avatar, IP location and age of the profile', () => {
  const profile = loadPage(FIXTURE).profile();

  assert.equal(profile.display_name, '示例作者');
  assert.equal(profile.handle, '100000001');
  assert.equal(profile.bio, '山高水长，一期一会 estj♌');
  assert.equal(profile.avatar_url, 'https://p3-pc.douyinpic.com/img/aweme-avatar/fixture-avatar~c5_300x300.jpeg?from=1');
  assert.equal(profile.ip_location, '浙江');
  assert.equal(profile.age, '35');
  assert.equal(profile.gender, '');
  assert.equal(profile.verified, false);
  assert.equal(profile.followers, '31.9万');
  assert.equal(profile.following, '3474');
  assert.equal(profile.likes, '7145');
  assert.equal(profile.video_count, '448');
});

test('bio comes from the header row, emoji included, when the page has no author record', () => {
  const profile = loadPage(withoutRecord(FIXTURE)).profile();

  assert.equal(profile.bio, '山高水长，一期一会 estj♌');
});

test('a bio the header cuts is returned whole from the author record', () => {
  const full = '发布示例信息  传递示例力量\n示例投稿邮箱demo@example.com';
  const html = replaceOnce(
    replaceOnce(FIXTURE, BIO_ROW, CUT_BIO_ROW),
    '山高水长，一期一会\\\\nestj♌',
    full.replace('\n', '\\\\n'),
  );

  assert.equal(loadPage(html).profile().bio, '发布示例信息 传递示例力量 示例投稿邮箱demo@example.com');
  assert.equal(loadPage(withoutRecord(html)).profile().bio, '发布示例信息 传递示例力量 示例投稿邮箱demo@...');
});

test('a profile without a bio returns an empty bio, never the meta description', () => {
  const noBio = replaceOnce(FIXTURE, BIO_ROW, ENTRY_CARD_ROW);
  const emptyRecord = replaceOnce(noBio, '山高水长，一期一会\\\\nestj♌', '');

  assert.equal(loadPage(emptyRecord).profile().bio, '');
  assert.equal(loadPage(withoutRecord(noBio)).profile().bio, '');
});

test('gender comes from the icon in the age tag, or its 男 / 女 label', () => {
  const tagged = (tag) => loadPage(replaceOnce(FIXTURE, AGE_TAG, tag)).profile();

  const female = tagged(`<span class="rTzhSEM4">${FEMALE_ICON}<span>36岁</span></span>`);
  assert.deepEqual([female.gender, female.age], ['female', '36']);
  const male = tagged(`<span class="rTzhSEM4">${MALE_ICON}<span>28岁</span></span>`);
  assert.deepEqual([male.gender, male.age], ['male', '28']);
  const ageHidden = tagged(`<span class="rTzhSEM4">${FEMALE_ICON}<span>女</span></span>`);
  assert.deepEqual([ageHidden.gender, ageHidden.age], ['female', '']);
  const noTag = tagged('');
  assert.deepEqual([noTag.gender, noTag.age, noTag.ip_location], ['', '', '浙江']);
});

test('the 认证 badge marks a verified profile', () => {
  const profile = loadPage(replaceOnce(FIXTURE, NAME_ROW, VERIFIED_NAME_ROW)).profile();

  assert.equal(profile.verified, true);
  assert.equal(profile.display_name, '示例作者');
});

test('a refused works list is reported with its message and the logged-out session', async () => {
  const page = loadPage(FIXTURE);
  const state = page.state();

  assert.equal(state.ok, true);
  assert.equal(state.posts_error, '服务异常，重新刷新拉取数据');
  assert.equal(state.logged_out, true);
  assert.deepEqual(page.profile().video_cards, []);

  const control = page.document.querySelectorAll('[data-e2e="user-post-list"] span')
    .find((node) => node.textContent === '刷新');
  const pressed = await page.scripts.refreshAuthorPosts();
  assert.equal(pressed.ok, true);
  assert.equal(control.clicks, 1);
});

test('a loaded works list has no error and yields its cards', async () => {
  const signedIn = replaceOnce(
    replaceOnce(FIXTURE, '<p class="VQdYTqcZ">登录</p>', ''),
    '<div id="login-panel-new" class="X8wfH_kc" data-bytereplay-mask="strict"></div>',
    '',
  );
  const page = loadPage(replaceOnce(signedIn, EMPTY_LIST, LOADED_LIST));
  const state = page.state();
  const profile = page.profile();

  assert.equal(state.posts_error, '');
  assert.equal(state.logged_out, false);
  assert.equal(profile.video_cards.length, 1);
  assert.equal(profile.video_cards[0].video_id, '7000000000000000001');
  assert.equal(profile.video_cards[0].title, '第一条示例作品的标题');
  assert.equal(profile.video_cards[0].author, '示例作者');
  assert.equal((await page.scripts.refreshAuthorPosts()).ok, false);
});
