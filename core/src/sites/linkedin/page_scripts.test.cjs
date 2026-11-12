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
  pathname = `/search/results/${resultType}/`, href, main = null, documentSelectors = {},
}) {
  const document = {
    querySelector: (selector) => {
      if (documentSelectors[selector]) return documentSelectors[selector][0] || null;
      return selector === 'main' ? main : null;
    },
    querySelectorAll: (selector) => {
      if (documentSelectors[selector]) return documentSelectors[selector];
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

test('content search accepts a localized feed-post heading through the full-update marker', () => {
  const profile = new FakeNode({ text: 'Localized Author', href: 'https://www.linkedin.com/in/localized-author/' });
  const body = new FakeNode({ text: 'Localized post body' });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Publicación en el feed' })],
      '[data-view-name="feed-full-update"]': [new FakeNode({})],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [body],
    },
  });
  const activityId = '7393000000000000004';
  const breadcrumb = JSON.stringify({ updateUrn: `urn:li:activity:${activityId}` });
  card.__reactProps$fixture = {
    children: { _payload: { value: JSON.stringify({ breadcrumb: { content: { type: 'Buffer', data: Array.from(Buffer.from(breadcrumb)) } } }) } },
  };

  const results = loadScripts({ semanticCards: [card] }).searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].id, activityId);
});

test('content search reads the share id from the commentary componentkey when no URN is in the DOM', () => {
  const profile = new FakeNode({ text: 'Server Driven', href: 'https://www.linkedin.com/in/server-driven/' });
  const commentary = new FakeNode({});
  commentary.attributes.set(
    'componentkey',
    'translatable-commentary-FeTranslationUrn(contentUrnCommentUrn=null, contentUrnGroupPostUrn=null, contentUrnShareUrn=ContentUrnShareUrn(shareUrn=ShareUrn(shareId=7393000000000000001)), contentUrnUgcPostUrn=null, detectedLocale=, targetLocale=es)',
  );
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Publicación en el feed' })],
      '[data-view-name="feed-full-update"]': [new FakeNode({})],
      '[componentkey*="translatable-commentary-"]': [commentary, commentary],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Body' })],
    },
  });

  const results = loadScripts({ semanticCards: [card] }).searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].id, '7393000000000000001');
  assert.equal(results[0].url, 'https://www.linkedin.com/feed/update/urn:li:share:7393000000000000001/');
});

test('content search reads a ugcPost id and the document page count without a URN in the DOM', () => {
  const profile = new FakeNode({ text: 'Doc Author', href: 'https://www.linkedin.com/in/doc-author/' });
  const commentary = new FakeNode({});
  commentary.attributes.set(
    'componentkey',
    'translatable-commentary-FeTranslationUrn(contentUrnCommentUrn=null, contentUrnGroupPostUrn=null, contentUrnShareUrn=null, contentUrnUgcPostUrn=ContentUrnUgcPostUrn(userGeneratedContentPostUrn=UserGeneratedContentPostUrn(userGeneratedContentId=7393000000000000002)), detectedLocale=, targetLocale=es)',
  );
  const pages = [new FakeNode({}), new FakeNode({}), new FakeNode({})];
  const documentContainer = new FakeNode({ selectors: { '[data-page-card="true"]': pages } });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Publicación en el feed' })],
      '[componentkey*="translatable-commentary-"]': [commentary],
      '[componentkey^="document-container-"]': [documentContainer],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Swipe through' })],
    },
  });

  const results = loadScripts({ semanticCards: [card] }).searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 1);
  assert.equal(results[0].url, 'https://www.linkedin.com/feed/update/urn:li:ugcPost:7393000000000000002/');
  assert.equal(results[0].document_pages, 3);
});

test('content search still rejects a localized heading without the full-update marker', () => {
  const profile = new FakeNode({ text: 'Other Author', href: 'https://www.linkedin.com/in/other-author/' });
  const card = new FakeNode({
    role: 'listitem',
    selectors: {
      h2: [new FakeNode({ text: 'Publicación en el feed' })],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [profile],
      'a[href*="/in/"]': [profile],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Body' })],
    },
  });
  card.__reactProps$fixture = { children: { props: { updateUrn: 'urn:li:activity:7393000000000000005' } } };

  const results = loadScripts({ semanticCards: [card] }).searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 0);
});

test('post detail reads localized engagement counts and the document page count', () => {
  const activityId = '7393000000000000003';
  const postPath = `/feed/update/urn:li:activity:${activityId}/`;
  const author = new FakeNode({ text: 'Detail Author', href: 'https://www.linkedin.com/in/detail-author/' });
  const pages = [new FakeNode({}), new FakeNode({})];
  const documentContainer = new FakeNode({ selectors: { '[data-page-card="true"]': pages } });
  const root = new FakeNode({
    text: 'Detail Author\nA document post\n10 reacciones\n3 comentarios',
    selectors: {
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'A document post' })],
      'a[href*="/in/"]': [author],
      'a[href], button, [role="button"]': [
        author,
        new FakeNode({ text: '10 reacciones' }),
        new FakeNode({ text: '3 comentarios' }),
      ],
      '[componentkey^="document-container-"]': [documentContainer],
    },
  });
  root.attributes.set('data-activity-urn', `urn:li:activity:${activityId}`);

  const detail = loadScripts({
    pathname: postPath,
    href: `https://www.linkedin.com${postPath}`,
    documentSelectors: { 'article[data-activity-urn]': [root] },
  }).postDetail();

  assert.equal(detail.ok, true);
  assert.equal(detail.post_id, activityId);
  assert.equal(detail.engagement.reactions, '10 reacciones');
  assert.equal(detail.engagement.comments, '3 comentarios');
  assert.equal(detail.document_pages, 2);
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

// Synthetic post card for the newer server-driven UI: share id in the
// commentary componentkey, counts as localized text. Fake ids and names.
function socialPostCard({ shareId, reactionsLabel = '', commentsText = '', ageText = '', author = 'fixture-author', header = '', authorPath = '/in/' }) {
  const profile = new FakeNode({ text: 'Fixture Author', href: `https://www.linkedin.com${authorPath}${author}/` });
  const commentary = new FakeNode({});
  commentary.attributes.set(
    'componentkey',
    `translatable-commentary-FeTranslationUrn(contentUrnShareUrn=ContentUrnShareUrn(shareUrn=ShareUrn(shareId=${shareId})), targetLocale=es)`,
  );
  const reactionsButton = reactionsLabel ? new FakeNode({ text: reactionsLabel.replace(/\D+/g, ''), ariaLabel: reactionsLabel }) : null;
  const commentsLink = commentsText ? new FakeNode({ text: commentsText, href: 'https://www.linkedin.com/feed/' }) : null;
  const age = ageText ? new FakeNode({ text: ageText }) : null;
  const social = [reactionsButton, commentsLink].filter(Boolean);
  const card = new FakeNode({
    role: 'listitem',
    text: [header, 'Fixture Author', ageText, 'Fixture body', reactionsLabel, commentsText].filter(Boolean).join('\n'),
    selectors: {
      h2: [new FakeNode({ text: 'Publicación en el feed' })],
      '[data-view-name="feed-full-update"]': [new FakeNode({})],
      '[componentkey*="translatable-commentary-"]': [commentary],
      'a[href*="/in/"], a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': authorPath === '/in/' ? [profile] : [],
      'a[href*="/in/"]': authorPath === '/in/' ? [profile] : [],
      'a[href*="/company/"]': authorPath === '/company/' ? [profile] : [],
      'a[href*="/in/"][aria-label]': [],
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"]': [],
      '[data-testid="expandable-text-box"]': [new FakeNode({ text: 'Fixture body' })],
      'a[href], button, [role="button"], [aria-label]': social,
      'time, p, span': age ? [age] : [],
    },
  });
  return card;
}

test('content search cards expose numeric reactions, comments and the author link', () => {
  const counted = socialPostCard({ shareId: '7393000000000000011', reactionsLabel: '1.234 reacciones', commentsText: '12 comentarios' });
  const bare = socialPostCard({ shareId: '7393000000000000012', author: 'other-author' });
  const results = loadScripts({ semanticCards: [counted, bare] }).searchResults({ result_type: 'content', limit: 5 });

  assert.equal(results.length, 2);
  assert.equal(results[0].reactions, 1234);
  assert.equal(results[0].comments, 12);
  assert.equal(results[0].author_id, 'fixture-author');
  assert.equal(results[0].author_url, 'https://www.linkedin.com/in/fixture-author/');
  // Not shown on the card: unknown, not zero.
  assert.equal(results[1].reactions, null);
  assert.equal(results[1].comments, null);
  assert.equal(results[1].author_id, 'other-author');
});

test('search state checks the content filters carried in the URL', () => {
  const card = socialPostCard({ shareId: '7393000000000000013' });
  const filtered = loadScripts({
    semanticCards: [card],
    href: 'https://www.linkedin.com/search/results/content/?keywords=fixture&sortBy=%22date_posted%22&datePosted=%22past-week%22&contentType=%22documents%22',
  }).searchState({
    query: 'fixture',
    result_type: 'content',
    filters: { sortBy: 'date_posted', datePosted: 'past-week', contentType: 'documents' },
  });
  assert.equal(filtered.ok, true);
  assert.equal(filtered.filters_match, true);
  assert.deepEqual({ ...filtered.filters }, { sortBy: 'date_posted', datePosted: 'past-week', contentType: 'documents' });

  const dropped = loadScripts({
    semanticCards: [card],
    href: 'https://www.linkedin.com/search/results/content/?keywords=fixture',
  }).searchState({ query: 'fixture', result_type: 'content', filters: { contentType: 'documents' } });
  assert.equal(dropped.ok, false);
  assert.equal(dropped.error, 'filter_mismatch');
});

function loadWithWindow({ pathname, href, cards = [], main = null, buttons = [] }) {
  const calls = [];
  const document = {
    readyState: 'complete',
    body: new FakeNode({ text: 'body' }),
    documentElement: { scrollHeight: 900 },
    querySelector: (selector) => selector === 'main' ? main : null,
    querySelectorAll: (selector) => selector === 'main [role="listitem"]' ? cards : [],
  };
  if (main) main.selectors.button = buttons;
  const window = {
    innerHeight: 900,
    innerWidth: 1440,
    scrollY: 0,
    scrollBy: (options) => calls.push(['window', options.top]),
    scrollTo: () => calls.push(['window-top', 0]),
    getComputedStyle: (node) => ({ overflowY: node.scrollable ? 'auto' : 'visible', visibility: 'visible', display: 'block' }),
  };
  const context = {
    URL, document, window, location: { href, pathname },
    setTimeout: (resolve) => resolve(),
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'page_scripts.js'), 'utf8'), context);
  return { scripts: window.SocaiLinkedInPageScripts, calls };
}

function scrollContainer() {
  const container = new FakeNode({});
  container.scrollable = true;
  container.scrollTop = 0;
  container.scrollHeight = 5000;
  container.clientHeight = 800;
  container.getBoundingClientRect = () => ({ width: 800, height: 800 });
  container.scrollBy = (options) => { container.scrollTop += options.top; };
  return container;
}

test('scrollResults scrolls the inner results container, not only the window', async () => {
  const container = scrollContainer();
  const card = socialPostCard({ shareId: '7393000000000000014' });
  card.parentElement = container;
  let intoView = 0;
  card.scrollIntoView = () => { intoView += 1; };
  const main = new FakeNode({ selectors: {} });
  const { scripts, calls } = loadWithWindow({
    pathname: '/search/results/content/',
    href: 'https://www.linkedin.com/search/results/content/?keywords=fixture',
    cards: [card],
    main,
  });

  const result = await scripts.scrollResults({});

  assert.equal(result.ok, true);
  assert.equal(result.scroll_container, true);
  assert.equal(result.clicked_more, false);
  assert.equal(container.scrollTop, 640);
  assert.equal(intoView, 1);
  assert.deepEqual(calls.map((call) => call[0]), ['window']);
  assert.equal(result.at_end, false);
});

test('scrollResults presses a visible localized show-more button instead of scrolling', async () => {
  const card = socialPostCard({ shareId: '7393000000000000015' });
  let clicked = 0;
  const more = new FakeNode({ text: 'Mostrar más resultados' });
  more.getBoundingClientRect = () => ({ width: 200, height: 40 });
  more.click = () => { clicked += 1; };
  const main = new FakeNode({ selectors: {} });
  const { scripts, calls } = loadWithWindow({
    pathname: '/search/results/content/',
    href: 'https://www.linkedin.com/search/results/content/?keywords=fixture',
    cards: [card],
    main,
    buttons: [more],
  });

  const result = await scripts.scrollResults({});

  assert.equal(clicked, 1);
  assert.equal(result.clicked_more, true);
  assert.equal(calls.length, 0);
});

test('activity posts list a member\'s posts with age, counts, documents and reposts', () => {
  const own = socialPostCard({ shareId: '7393000000000000021', reactionsLabel: '45 reacciones', commentsText: '3 comentarios', ageText: '2 días' });
  const pages = [new FakeNode({}), new FakeNode({})];
  own.selectors['[componentkey^="document-container-"]'] = [new FakeNode({ selectors: { '[data-page-card="true"]': pages } })];
  const reposted = socialPostCard({ shareId: '7393000000000000022', author: 'someone-else', ageText: '1 sem', header: 'Fixture Author ha compartido esto' });
  const nested = socialPostCard({ shareId: '7393000000000000023' });
  nested.parentElement = own;
  const { scripts } = loadWithWindow({
    pathname: '/in/fixture-author/recent-activity/all/',
    href: 'https://www.linkedin.com/in/fixture-author/recent-activity/all/',
    cards: [own, reposted, nested],
    main: new FakeNode({ text: 'Activity', selectors: {} }),
  });

  const state = scripts.activityState();
  assert.equal(state.ok, true);
  assert.equal(state.owner_kind, 'profile');
  assert.equal(state.owner_id, 'fixture-author');
  assert.equal(state.post_count, 2);

  const posts = scripts.activityPosts({ limit: 10 });
  assert.equal(posts.length, 2);
  assert.equal(posts[0].id, '7393000000000000021');
  assert.equal(posts[0].url, 'https://www.linkedin.com/feed/update/urn:li:share:7393000000000000021/');
  assert.equal(posts[0].author.id, 'fixture-author');
  assert.equal(posts[0].is_repost, false);
  assert.equal(posts[0].published_at, '2 días');
  assert.equal(posts[0].reactions, 45);
  assert.equal(posts[0].comments, 3);
  assert.equal(posts[0].document_pages, 2);
  assert.equal(posts[0].text, 'Fixture body');
  assert.equal(posts[1].is_repost, true);
  assert.equal(posts[1].published_at, '1 sem');
  assert.equal(posts[1].reactions, null);
});

test('activity state reports an explicit empty list and rejects other routes', () => {
  const empty = loadWithWindow({
    pathname: '/in/fixture-author/recent-activity/all/',
    href: 'https://www.linkedin.com/in/fixture-author/recent-activity/all/',
    main: new FakeNode({ text: 'Fixture Author aún no ha publicado nada', selectors: {} }),
  }).scripts.activityState();
  assert.equal(empty.ok, true);
  assert.equal(empty.empty, true);
  assert.equal(empty.post_count, 0);

  const company = loadWithWindow({
    pathname: '/company/fixture-co/posts/',
    href: 'https://www.linkedin.com/company/fixture-co/posts/',
    main: new FakeNode({ text: '', selectors: {} }),
  }).scripts.activityState();
  assert.equal(company.owner_kind, 'company');
  assert.equal(company.owner_id, 'fixture-co');
  assert.equal(company.error, 'not_hydrated');

  const other = loadWithWindow({
    pathname: '/in/fixture-author/',
    href: 'https://www.linkedin.com/in/fixture-author/',
    main: new FakeNode({ text: '', selectors: {} }),
  }).scripts.activityState();
  assert.equal(other.error, 'wrong_activity_route');
});
