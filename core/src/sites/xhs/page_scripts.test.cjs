const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

class FakeNode {
  constructor({ text = '', href = '', rect = { left: 0, top: 0, width: 100, height: 40 } } = {}) {
    this.innerText = text;
    this.textContent = text;
    this.href = href;
    this.rect = rect;
    this.parentElement = null;
    this.offsetHeight = rect.height;
    this.attributes = new Map();
    this.selectors = new Map();
  }

  getBoundingClientRect() {
    return {
      ...this.rect,
      right: this.rect.left + this.rect.width,
      bottom: this.rect.top + this.rect.height,
    };
  }

  getAttribute(name) {
    if (name === 'href') return this.href;
    return this.attributes.get(name) || '';
  }

  querySelector(selector) {
    return (this.selectors.get(selector) || [])[0] || null;
  }

  querySelectorAll(selector) {
    return this.selectors.get(selector) || [];
  }

  closest(selector) {
    if (selector.includes('comment')) return null;
    if (selector.includes('.author-container') || selector.includes('.author-wrapper') || selector.includes('.author')) {
      return this.parentElement;
    }
    return null;
  }

  contains(node) {
    return node === this;
  }
}

function loadFixture({ label = '关注', hitOwned = true, expectedAuthor = 'target-author' } = {}) {
  const noteId = '6a8e6586000000002003fe1c';
  const root = new FakeNode({ rect: { left: 100, top: 80, width: 900, height: 700 } });
  root.attributes.set('data-note-id', noteId);

  const authorScope = new FakeNode({ rect: { left: 420, top: 180, width: 260, height: 80 } });
  const authorLink = new FakeNode({
    text: 'Target Author',
    href: 'https://www.xiaohongshu.com/user/profile/target-author',
    rect: { left: 430, top: 190, width: 120, height: 32 },
  });
  authorLink.parentElement = authorScope;
  const follow = new FakeNode({
    text: label,
    rect: { left: 580, top: 190, width: 64, height: 32 },
  });
  follow.parentElement = authorScope;

  const actor = new FakeNode({
    text: 'Signed In User',
    href: 'https://www.xiaohongshu.com/user/profile/signed-in-user',
    rect: { left: 10, top: 100, width: 48, height: 48 },
  });

  const noteIdSelector = '[data-note-id], [data-noteid]';
  const authorSelector = [
    '.author-container a[href*="/user/profile/"]',
    '.author-wrapper a[href*="/user/profile/"]',
    '.author a[href*="/user/profile/"]',
  ].join(', ');
  const followSelector = '.follow-button, button, [role="button"]';
  root.selectors.set(noteIdSelector, [root]);
  root.selectors.set(authorSelector, [authorLink]);
  authorScope.selectors.set(followSelector, [follow]);

  const actorSelector = [
    '.user.side-bar-component a[href*="/user/profile/"]',
    '.side-bar-component.user a[href*="/user/profile/"]',
    '.user.side-bar-component[href*="/user/profile/"]',
    '.side-bar-component.user[href*="/user/profile/"]',
    'a.user[href*="/user/profile/"]',
  ].join(', ');
  const overlaySelector = '.note-detail-mask, .note-overlay, .note-detail-modal';
  const document = {
    activeElement: null,
    body: new FakeNode(),
    querySelector: (selector) => selector === overlaySelector ? root : null,
    querySelectorAll: (selector) => selector === actorSelector ? [actor] : [],
    elementFromPoint: () => hitOwned ? follow : root,
  };
  const window = {
    getComputedStyle: () => ({ display: 'block', visibility: 'visible', opacity: '1' }),
    __INITIAL_STATE__: {},
  };
  const context = {
    URL,
    document,
    location: {
      href: `https://www.xiaohongshu.com/explore/${noteId}`,
      pathname: `/explore/${noteId}`,
    },
    window,
    innerHeight: 900,
    innerWidth: 1200,
    HTMLElement: FakeNode,
    HTMLInputElement: FakeNode,
    HTMLTextAreaElement: FakeNode,
    SVGElement: FakeNode,
    InputEvent: class {},
    Event: class {},
    setTimeout,
    clearTimeout,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(`${source}\nglobalThis.__xhs = SocaiXhsPageScripts;`, context);
  return {
    scripts: context.__xhs,
    args: { note_id: noteId, author_id: expectedAuthor },
    follow,
  };
}

test('follow state binds one visible control to the active note author', () => {
  const { scripts, args } = loadFixture();
  const state = scripts.followState(args);

  assert.equal(state.ok, true);
  assert.equal(state.status, 'follow_ready');
  assert.equal(state.note_id, args.note_id);
  assert.equal(state.author.id, 'target-author');
  assert.equal(state.actor.id, 'signed-in-user');
  assert.equal(state.following, false);
  assert.equal(state.hit_owned, true);
  assert.equal(state.x, 612);
  assert.equal(state.y, 206);
});

test('follow state reports an already-followed author without a commit target', () => {
  const { scripts, args } = loadFixture({ label: '已关注' });
  const state = scripts.followState(args);

  assert.equal(state.ok, true);
  assert.equal(state.status, 'already_following');
  assert.equal(state.following, true);
  assert.equal(state.hit_owned, false);
  assert.equal('x' in state, false);
});

test('follow state fails closed for author mismatch or an obscured control', () => {
  const wrong = loadFixture({ expectedAuthor: 'other-author' });
  assert.equal(wrong.scripts.followState(wrong.args).status, 'wrong_author');

  const obscured = loadFixture({ hitOwned: false });
  assert.equal(obscured.scripts.followState(obscured.args).status, 'follow_control_obscured');
  assert.equal(obscured.scripts.followState(obscured.args).ok, false);
});

test('follow state drops stale click geometry when the toggle changes before dispatch', () => {
  const { scripts, args, follow } = loadFixture();
  assert.equal(scripts.followState(args).status, 'follow_ready');

  follow.innerText = '已关注';
  follow.textContent = '已关注';
  const revalidated = scripts.followState(args);

  assert.equal(revalidated.status, 'already_following');
  assert.equal(revalidated.following, true);
  assert.equal(revalidated.hit_owned, false);
  assert.equal('x' in revalidated, false);
});
