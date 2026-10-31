const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');
const {spawnSync} = require('node:child_process');

const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');

test('the browser regression helper fails closed before CDP without explicit account scope', () => {
  const helper = path.resolve(__dirname, '../../../../scripts/check-xhs-comment-dom.mjs');
  const result = spawnSync(process.execPath, [helper, path.resolve('unused-account-check-evidence.json')], {
    encoding:'utf8', env:{...process.env, SOCAI_ACCEPTANCE_TARGET:'', SOCAI_ACCEPTANCE_EMAIL:'', SOCAI_ACCEPTANCE_ACCOUNT_ID:''},
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /SOCAI_ACCEPTANCE_TARGET, SOCAI_ACCEPTANCE_EMAIL and SOCAI_ACCEPTANCE_ACCOUNT_ID are required/);
});

function fixture({ rows = [], ends = [], replies = [], feed = false } = {}) {
  class Element {
    constructor(className, parentElement = null) {
      Object.assign(this, {className, parentElement, scrollTop:0, scrollHeight:400,
        clientHeight:400, offsetHeight:400, style:{display:'block', visibility:'visible', opacity:'1', overflowY:'auto'}});
    }
    getBoundingClientRect() { return {top:0, left:0, bottom:400, right:500, width:500, height:400}; }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    querySelectorAll() { return []; }
    closest() { return null; }
    scrollBy({top}) { this.scrollTop = Math.min(this.scrollHeight-this.clientHeight, Math.max(0, this.scrollTop+top)); }
    scrollIntoView() {}
    getAttribute() { return null; }
  }
  const body = new Element('body');
  const overlay = new Element('note-detail-mask', body);
  const photo = new Element('note-content', overlay);
  photo.scrollHeight = 10000;
  const area = new Element('comments-el', overlay);
  area.scrollHeight = 2000;
  for (const row of rows) row.parentElement = area;
  area.querySelectorAll = selector => {
    if (selector === '.show-more') return replies;
    if (selector === '.end-container') return ends;
    if (selector === '.total') return [{innerText:'共 300 条评论'}];
    return [];
  };
  overlay.querySelectorAll = selector => {
    if (selector.includes('.note-content')) return [photo];
    if (selector === '.comments-el' || selector.includes('.comments-container, .comment-list')) return [area];
    if (selector.startsWith('.comment-item, .parent-comment')) return rows;
    return area.querySelectorAll(selector);
  };
  const document = {
    body, documentElement:body, scrollingElement:body,
    querySelector(selector) {
      if (selector.startsWith('.note-detail-mask,') || selector === '#noteContainer') return overlay;
      if (feed && selector.startsWith('section.note-item,')) return new Element('note-item');
      return null;
    },
    querySelectorAll: () => [],
  };
  const window = {getComputedStyle: element => element.style,
    innerHeight:800, innerWidth:1200, scrollY:0, scrollBy(){ throw Error('Must not scroll the feed window'); }};
  const context = {document, window, HTMLElement:Element, URL, location:{href:'https://www.xiaohongshu.com/explore/fixture'},
    innerHeight:800, innerWidth:1200, setTimeout: callback => callback()};
  vm.createContext(context);
  vm.runInContext(source, context);
  return {scripts:vm.runInContext('SocaiXhsPageScripts', context), area, photo, overlay, Element};
}

function comment(id, text) {
  return {
    querySelector(selector) {
      if (selector === '.name') return {innerText:'同一作者'};
      if (selector === '.content') return {innerText:text};
      if (selector === '.time') return {innerText:'2026-10-30'};
      return null;
    },
    querySelectorAll: () => [],
    getAttribute: name => name === 'data-comment-id' ? id : null,
  };
}

test('comment pagination scrolls the comments column instead of the taller image column', async () => {
  const f = fixture();
  const scroll = f.scripts.scrollComments || f.scripts.scrollInNote;
  await scroll({pixels:600});
  assert.equal(f.area.scrollTop, 600);
  assert.equal(f.photo.scrollTop, 0);
});

test('a reply-thread end marker does not exhaust the parent-comment list', () => {
  const nestedEnd = {closest: () => ({className:'parent-comment'}),
    getBoundingClientRect: () => ({width:100,height:20}), style:{display:'block',visibility:'visible',opacity:'1'}};
  const f = fixture({ends:[nestedEnd]});
  assert.equal(f.scripts.commentAreaState().has_end, false);
});

test('a hidden parent-list end marker is not exhaustion', () => {
  const hiddenEnd = {closest: () => null,
    getBoundingClientRect: () => ({width:0,height:0}), style:{display:'none',visibility:'visible',opacity:'1'}};
  assert.equal(fixture({ends:[hiddenEnd]}).scripts.commentAreaState().has_end, false);
});

test('distinct comment IDs survive identical author and text prefixes', () => {
  const prefix = '这是相同的长评论开头'.repeat(5);
  const f = fixture({rows:[comment('a', prefix+'第一条'), comment('b', prefix+'第二条')]});
  assert.equal(f.scripts.comments({prefer_hot:false}).length, 2);
});

test('comments without IDs use the whole text, not the first 30 characters', () => {
  const prefix = '这是相同的长评论开头'.repeat(5);
  assert.equal(fixture({rows:[comment('', prefix+'第一条'), comment('', prefix+'第二条')]})
    .scripts.comments({prefer_hot:false}).length, 2);
});

test('reply expansion is bounded and skips collapsed and offscreen controls', async () => {
  const f = fixture();
  let clicks = 0;
  const button = (label, top=0) => ({innerText:label, style:{display:'block',visibility:'visible',opacity:'1'},
    getBoundingClientRect: () => ({top, bottom:top+20, left:0, right:100, width:100, height:20}),
    click() { clicks++; }});
  const controls = [button('收起'), button('展开 20 条回复', 900), ...Array.from({length:5}, () => button('展开更多回复'))];
  const originalQuery = f.area.querySelectorAll;
  f.area.querySelectorAll = selector => selector === '.show-more' ? controls : originalQuery(selector);
  assert.equal((await f.scripts.expandCommentReplies()).clicked, 1);
  assert.equal(clicks, 1);
  assert.equal((await f.scripts.expandCommentReplies({max_clicks:60})).clicked, 3);
  assert.equal(clicks, 4);
});

test('scroll progress is included in state even before the next comment page', async () => {
  const f = fixture();
  assert.equal(f.scripts.commentAreaState().scroll_top, 0);
  await f.scripts.scrollComments({pixels:600});
  assert.equal(f.scripts.commentAreaState().scroll_top, 600);
  assert.equal(f.scripts.commentAreaState().scroll_height, 2000);
});

test('a small comment overflow can still reach its pagination sentinel', async () => {
  const f = fixture();
  f.area.scrollHeight = f.area.clientHeight + 10;
  await f.scripts.scrollComments({pixels:600});
  assert.equal(f.area.scrollTop, 10);
});

test('revealing a reply never invokes scrollIntoView on the underlying feed', async () => {
  const f = fixture({feed:true});
  const button = {innerText:'展开 20 条回复', style:{display:'block',visibility:'visible',opacity:'1'},
    getBoundingClientRect: () => ({top:900, bottom:920, width:100,height:20}),
    scrollIntoView() { throw Error('Must not move a feed ancestor'); }};
  const originalQuery = f.area.querySelectorAll;
  f.area.querySelectorAll = selector => selector === '.show-more' ? [button] : originalQuery(selector);
  f.area.scrollHeight = f.area.clientHeight;
  assert.equal((await f.scripts.scrollComments({reveal_reply:true})).ok, false);
});

test('a real visible parent-list end marker remains detectable', () => {
  const end = {closest: () => null,
    getBoundingClientRect: () => ({width:100,height:20}), style:{display:'block',visibility:'visible',opacity:'1'}};
  assert.equal(fixture({ends:[end]}).scripts.commentAreaState().has_end, true);
});

test('comment scrolling never falls through to an underlying results feed', async () => {
  const f = fixture({feed:true});
  f.area.scrollHeight = f.area.clientHeight;
  f.photo.scrollHeight = f.photo.clientHeight;
  const scroll = f.scripts.scrollComments || f.scripts.scrollInNote;
  const result = await scroll({pixels:600});
  assert.equal(result.ok, false);
  assert.equal(f.area.scrollTop, 0);
  assert.equal(f.photo.scrollTop, 0);
});
