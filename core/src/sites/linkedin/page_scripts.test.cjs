const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

class FakeNode {
  constructor({ text = '', href = '', src = '', ariaLabel = '', role = '', legacy = false, selectors = {} } = {}) {
    this.innerText = text;
    this.textContent = text;
    this.href = href;
    this.src = src;
    this.currentSrc = src;
    this.parentElement = null;
    this.selectors = selectors;
    this.legacy = legacy;
    this.attributes = new Map();
    if (href) this.attributes.set('href', href);
    if (src) this.attributes.set('src', src);
    if (ariaLabel) this.attributes.set('aria-label', ariaLabel);
    if (role) this.attributes.set('role', role);
  }

  getAttribute(name) {
    return this.attributes.get(name) || '';
  }

  querySelector(selector) {
    return (this.selectors[selector] || [])[0] || null;
  }

  querySelectorAll(selector) {
    return this.selectors[selector] || [];
  }

  closest(selector) {
    if (selector.includes('[role="listitem"]') && this.getAttribute('role') === 'listitem') return this;
    if (this.legacy && selector.includes('li')) return this;
    if (this.parentElement && this.parentElement.closest) return this.parentElement.closest(selector);
    return null;
  }
}

function loadScripts({
  semanticCards = [], legacyCards = [], globalLinks = [], sections = [], resultType = 'content',
  pathname = `/search/results/${resultType}/`, href, main = null,
}) {
  const document = {
    querySelector: (selector) => selector === 'main' ? main : null,
    querySelectorAll: (selector) => {
      if (selector === 'main [role="listitem"]') return semanticCards;
      if (selector === 'main li.reusable-search__result-container') return legacyCards;
      if (selector === 'main section') return sections;
      if (selector === 'main a[href*="/in/"], main a[href*="/posts/"], main a[href*="/feed/update/urn:li:"], main a[href*="/company/"], main a[href*="/showcase/"]') return globalLinks;
      return [];
    },
  };
  const window = {};
  const context = {
    URL,
    document,
    location: {
      href: href || `https://www.linkedin.com${pathname}?keywords=OpenAI`,
      pathname,
    },
    window,
  };
  const source = fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8');
  vm.runInNewContext(source, context);
  return window.SocaiLinkedInPageScripts;
}

test('content search recovers the canonical activity URL from the semantic card payload', () => {
  const profile = new FakeNode({
    text: 'Rayha Rehman\n• 3rd+',
    href: 'https://www.linkedin.com/in/rayha-rehman/',
  });
  const body = new FakeNode({ text: 'OpenAI agent safety analysis' });
  const heading = new FakeNode({ text: 'Feed post' });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [heading],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [body],
    },
  });
  const activityId = '7506306130294288384';
  const breadcrumb = JSON.stringify({ updateUrn: `urn:li:activity:${activityId}` });
  card.__reactProps$fixture = {
    children: {
      _payload: {
        value: JSON.stringify({
          unrelatedEntity: 'urn:li:ugcPost:7506267579825545216',
          breadcrumb: { content: { type: 'Buffer', data: Array.from(Buffer.from(breadcrumb)) } },
        }),
      },
    },
  };
  const sibling = { memoizedProps: { updateUrn: 'urn:li:activity:9999999999999999999' } };
  sibling.return = sibling;
  card.__reactFiber$fixture = { sibling };

  const scripts = loadScripts({ semanticCards: [card] });
  const results = scripts.searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].id, activityId);
  assert.equal(
    results[0].url,
    `https://www.linkedin.com/feed/update/urn:li:activity:${activityId}/`,
  );
  assert.equal(results[0].title, 'Rayha Rehman');
  assert.equal(results[0].snippet, 'OpenAI agent safety analysis');
  assert.equal(typeof scripts.scrollComments, 'function');
  assert.equal(typeof scripts.commentEditorTarget, 'function');
  assert.equal(typeof scripts.commentDraftState, 'function');
  assert.equal(typeof scripts.commentSubmitTarget, 'function');
  assert.equal(typeof scripts.renderedCommentState, 'function');
});

test('semantic content search rejects unrelated and nested list items', () => {
  const profile = new FakeNode({
    text: 'Recommended Member',
    href: 'https://www.linkedin.com/in/recommended-member/',
  });
  const unrelated = new FakeNode({
    role: 'listitem',
    selectors: {
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
    },
  });
  const nested = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Feed post' })],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Nested post' })],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
    },
  });
  profile.parentElement = unrelated;
  nested.parentElement = { closest: () => unrelated };

  const results = loadScripts({
    semanticCards: [unrelated, nested],
    legacyCards: [unrelated],
    globalLinks: [profile],
  })
    .searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 0);
});

test('people search accepts semantic person cards and preserves visible relationship clues', () => {
  const profile = new FakeNode({
    text: 'Charlene Fung',
    href: 'https://www.linkedin.com/in/charlene-fung/?miniProfileUrn=fixture',
  });
  const avatar = new FakeNode({ src: 'https://media.licdn.com/dms/image/fixture/profile-displayphoto' });
  const card = new FakeNode({
    text: 'Charlene Fung\n• 3rd+\nRecruiting Team at OpenAI\nSingapore\n5K followers\nFollow',
    role: 'listitem',
    selectors: {
      'a[href*="/in/"]': [profile],
      'img[src]': [avatar],
    },
  });

  const results = loadScripts({ semanticCards: [card], resultType: 'people' })
    .searchResults({ result_type: 'people', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].kind, 'profile');
  assert.equal(results[0].id, 'charlene-fung');
  assert.equal(results[0].title, 'Charlene Fung');
  assert.equal(results[0].subtitle, 'Recruiting Team at OpenAI');
  assert.equal(results[0].location, 'Singapore');
  assert.equal(results[0].connection_degree, '3rd+');
  assert.equal(results[0].followers, '5K followers');
  assert.equal(results[0].image_url, avatar.src);
});

test('company search accepts semantic company cards without treating profile links as results', () => {
  const company = new FakeNode({
    text: 'OpenAI',
    href: 'https://www.linkedin.com/company/openai/?trk=fixture',
  });
  const employee = new FakeNode({
    text: 'Unrelated profile',
    href: 'https://www.linkedin.com/in/unrelated/',
  });
  const logo = new FakeNode({ src: 'https://media.licdn.com/dms/image/fixture/company-logo' });
  const card = new FakeNode({
    text: 'OpenAI\nResearch Services\nSan Francisco, CA\n12M followers',
    role: 'listitem',
    selectors: {
      'a[href*="/company/"], a[href*="/showcase/"]': [company],
      'a[href*="/in/"]': [employee],
      'img[src]': [logo],
    },
  });

  const results = loadScripts({ semanticCards: [card], resultType: 'companies' })
    .searchResults({ result_type: 'companies', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].kind, 'company');
  assert.equal(results[0].id, 'openai');
  assert.equal(results[0].title, 'OpenAI');
  assert.equal(results[0].followers, '12M followers');
  assert.equal(results[0].image_url, logo.src);
});

test('company people preserves recommendation provenance and never marks suggestions as employees', () => {
  const profile = new FakeNode({
    text: 'Dale Finlay',
    href: 'https://www.linkedin.com/in/dale-finlay-2544903?miniProfileUrn=fixture',
  });
  const card = new FakeNode({
    text: 'Dale Finlay\nGeneral Manager at Anthropic\nConnect',
    role: 'listitem',
    selectors: { 'a[href*="/in/"]': [profile] },
  });
  profile.parentElement = card;
  const section = new FakeNode({
    selectors: {
      h2: [new FakeNode({ text: 'People you may know' })],
      'a[href*="/in/"]': [profile],
    },
  });

  const result = loadScripts({
    sections: [section],
    pathname: '/company/openai/people/',
    href: 'https://www.linkedin.com/company/openai/people/',
  }).companyPeople({ limit: 10 });
  const people = result.people;

  assert.equal(result.ok, true);
  assert.equal(result.section_present, true);
  assert.equal(people.length, 1);
  assert.equal(people[0].name, 'Dale Finlay');
  assert.equal(people[0].headline, 'General Manager at Anthropic');
  assert.equal(people[0].source_section, 'People you may know');
  assert.equal(Object.hasOwn(people[0], 'employee'), false);
  assert.equal(Object.hasOwn(people[0], 'employment_verified'), false);
});

test('profile history reads explicit dated experience entries from the dedicated route', () => {
  const currentRole = new FakeNode({
    text: 'Recruiting Team\nOpenAI · Full-time\nJan 2025 - Present · 1 yr 9 mos\nSingapore\nAPAC recruiting',
    role: 'listitem',
  });
  const section = new FakeNode({
    selectors: {
      h2: [new FakeNode({ text: 'Experience' })],
      '[role="listitem"]': [currentRole],
    },
  });
  const main = new FakeNode({
    text: 'Experience\nRecruiting Team\nOpenAI · Full-time\nJan 2025 - Present · 1 yr 9 mos\nSingapore\nAPAC recruiting\nMore profiles for you',
  });

  const history = loadScripts({
    sections: [section],
    main,
    pathname: '/in/charlene-fung/details/experience/',
    href: 'https://www.linkedin.com/in/charlene-fung/details/experience/',
  }).profileHistory();

  assert.equal(history.ok, true);
  assert.equal(history.profile_id, 'charlene-fung');
  assert.equal(history.section, 'experience');
  assert.equal(history.entries.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(history.entries[0])), {
    title: 'Recruiting Team',
    organization: 'OpenAI',
    date_range: 'Jan 2025 - Present · 1 yr 9 mos',
    location: 'Singapore',
    description: 'APAC recruiting',
    is_current: true,
    text: 'Recruiting Team — OpenAI',
  });
});

test('profile history flattens a grouped company without fabricating the company as a role', () => {
  const role = new FakeNode({
    text: 'Research Lead\nFull-time\nJan 2024 - Present · 2 yrs',
  });
  const company = new FakeNode({ text: 'OpenAI', href: 'https://www.linkedin.com/company/openai/' });
  const group = new FakeNode({
    text: 'OpenAI\nResearch Lead\nFull-time\nJan 2024 - Present · 2 yrs',
    selectors: {
      'a[href*="/company/"]': [company],
      '[componentkey^="entity-collection-item-"], .pvs-list__paged-list-item, li.artdeco-list__item, [role="listitem"]': [role],
    },
  });
  role.parentElement = group;
  const section = new FakeNode({
    selectors: {
      h2: [new FakeNode({ text: 'Experience' })],
      '[componentkey^="entity-collection-item-"]': [group],
    },
  });

  const history = loadScripts({
    sections: [section],
    pathname: '/in/example/details/experience/',
    href: 'https://www.linkedin.com/in/example/details/experience/',
  }).profileHistory();

  assert.equal(history.ok, true);
  assert.equal(history.entries.length, 1);
  assert.equal(history.entries[0].title, 'Research Lead');
  assert.equal(history.entries[0].organization, 'OpenAI');
});

test('legacy content cards keep their direct activity link behavior', () => {
  const profile = new FakeNode({
    text: 'Legacy Author',
    href: 'https://www.linkedin.com/in/legacy-author/',
  });
  const post = new FakeNode({
    text: 'Legacy post link',
    href: 'https://www.linkedin.com/feed/update/urn:li:activity:7123456789012345678/',
  });
  const legacy = new FakeNode({
    legacy: true,
    selectors: {
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [post],
      '.update-components-text': [new FakeNode({ text: 'Legacy body' })],
    },
  });

  const results = loadScripts({ legacyCards: [legacy] })
    .searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].id, '7123456789012345678');
  assert.equal(results[0].url, post.href);
  assert.equal(results[0].snippet, 'Legacy body');
});

test('content search never reads an activity URN from React fiber linkage alone', () => {
  const profile = new FakeNode({
    text: 'Fiber Neighbor',
    href: 'https://www.linkedin.com/in/fiber-neighbor/',
  });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Feed post' })],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'No local activity' })],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
    },
  });
  const neighbor = { memoizedProps: { updateUrn: 'urn:li:activity:9999999999999999999' } };
  neighbor.return = neighbor;
  card.__reactFiber$fixture = { child: neighbor, sibling: neighbor, return: neighbor };

  const results = loadScripts({ semanticCards: [card] })
    .searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 0);
});

test('content search rejects ambiguous card-local activity identities', () => {
  const profile = new FakeNode({ text: 'Ambiguous Author', href: 'https://www.linkedin.com/in/ambiguous/' });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Feed post' })],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Ambiguous repost' })],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
    },
  });
  card.__reactProps$fixture = {
    children: {
      props: {
        updateUrn: 'urn:li:activity:7111111111111111111',
        child: { updateUrn: 'urn:li:activity:7222222222222222222' },
      },
    },
  };

  const results = loadScripts({ semanticCards: [card] })
    .searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 0);
});

test('detail readers reject lookalike routes instead of returning unrelated page content', () => {
  const scripts = loadScripts({
    pathname: '/in/charlene-fung/details/experience/',
    href: 'https://www.linkedin.com/in/charlene-fung/details/experience/',
  });

  assert.equal(scripts.profileDetail().error, 'wrong_profile_route');
  assert.equal(scripts.postDetail().error, 'wrong_post_route');
  assert.equal(scripts.companyPeople({ limit: 5 }).error, 'wrong_company_people_route');
});

test('detail readers report redirected login routes before route mismatch', () => {
  const scripts = loadScripts({
    pathname: '/login',
    href: 'https://www.linkedin.com/login',
  });

  assert.equal(scripts.profileHistory().error, 'login_required');
  assert.equal(scripts.companyDetail().error, 'login_required');
});

test('company detail scopes counts to the active company header', () => {
  const name = new FakeNode({ text: 'OpenAI' });
  const header = new FakeNode({ text: 'OpenAI\n12M followers\n1K-5K employees' });
  name.parentElement = header;
  const main = new FakeNode({
    text: 'OpenAI\n12M followers\n1K-5K employees\nRecommended Page\n99M followers',
    selectors: { h1: [name] },
  });

  const detail = loadScripts({
    main,
    pathname: '/company/openai/',
    href: 'https://www.linkedin.com/company/openai/',
  }).companyDetail();

  assert.equal(detail.ok, true);
  assert.equal(detail.followers, '12M followers');
  assert.equal(detail.employees, '1K-5K employees');
  assert.equal(detail.associated_members, '');
});
