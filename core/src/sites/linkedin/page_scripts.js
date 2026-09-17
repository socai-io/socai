(function () {
  const PROFILE_PATH = /^\/in\/([^/?#]+)/i;
  const PROFILE_LANDING_PATH = /^\/in\/[^/?#]+\/?$/i;
  const COMPANY_PATH = /^\/(?:company|showcase)\/([^/?#]+)/i;
  const COMPANY_LANDING_PATH = /^\/(?:company|showcase)\/[^/?#]+\/?$/i;
  const POST_PATH = /\/(?:posts\/|feed\/update\/urn:li:)/i;
  const SEARCH_PATH = /^\/search\/results\/(people|content|companies|all)\/?/i;

  function cleanText(value, maxLength) {
    const raw = typeof value === 'string'
      ? value
      : value && (value.innerText || value.textContent) || '';
    const normalized = String(raw).replace(/\u00a0/g, ' ').replace(/[ \t]+/g, ' ')
      .replace(/\s*\n\s*/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
    return normalized.slice(0, Math.max(0, Number(maxLength || 12000)));
  }

  function visible(node) {
    if (!node || !node.getBoundingClientRect) return false;
    const rect = node.getBoundingClientRect();
    const style = window.getComputedStyle(node);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }

  function inViewport(node) {
    if (!visible(node)) return false;
    const rect = node.getBoundingClientRect();
    return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
  }

  function firstNode(root, selectors) {
    if (!root || !root.querySelector) return null;
    const scope = root;
    for (const selector of selectors) {
      const node = scope.querySelector(selector);
      if (node) return node;
    }
    return null;
  }

  function firstText(root, selectors, maxLength) {
    return cleanText(firstNode(root, selectors), maxLength);
  }

  function firstNonemptyText(root, selectors, maxLength) {
    if (!root || !root.querySelectorAll) return '';
    for (const selector of selectors) {
      for (const node of root.querySelectorAll(selector)) {
        const value = cleanText(node, maxLength);
        if (value) return value;
      }
    }
    return '';
  }

  function firstVisibleNode(root, selectors) {
    if (!root || !root.querySelectorAll) return null;
    for (const selector of selectors) {
      const node = Array.from(root.querySelectorAll(selector)).find(visible);
      if (node) return node;
    }
    return null;
  }

  function firstTextMatching(root, selector, pattern, maxLength) {
    if (!root || !root.querySelectorAll) return '';
    for (const node of root.querySelectorAll(selector)) {
      const value = cleanText(node, maxLength);
      if (pattern.test(value)) return value;
    }
    return '';
  }

  function linkedInUrl(raw) {
    if (!raw) return '';
    try {
      const url = new URL(raw, location.href);
      const host = url.hostname.toLowerCase();
      if (url.protocol !== 'https:' || !(host === 'linkedin.com' || host.endsWith('.linkedin.com'))) return '';
      url.hash = '';
      url.search = '';
      return url.href;
    } catch (_) {
      return '';
    }
  }

  function assetUrl(raw) {
    if (!raw) return '';
    try {
      const url = new URL(raw, location.href);
      return url.protocol === 'https:' ? url.href : '';
    } catch (_) {
      return '';
    }
  }

  function linkedInAssetUrl(raw) {
    const url = assetUrl(raw);
    if (!url) return '';
    const host = new URL(url).hostname.toLowerCase();
    return host === 'linkedin.com' || host.endsWith('.linkedin.com') ||
      host === 'licdn.com' || host.endsWith('.licdn.com') ? url : '';
  }

  function profileImageUrl(root) {
    if (!root || !root.querySelectorAll) return '';
    for (const selector of ['img[src]', 'img[data-delayed-url]']) {
      for (const image of root.querySelectorAll(selector)) {
        for (const raw of [image.currentSrc, image.getAttribute('src'), image.getAttribute('data-delayed-url')]) {
          const url = linkedInAssetUrl(raw);
          if (/profile-displayphoto/i.test(url)) return url;
        }
      }
    }
    return '';
  }

  function companyLogoUrl(root) {
    if (!root || !root.querySelectorAll) return '';
    for (const selector of ['img[src]', 'img[data-delayed-url]']) {
      for (const image of root.querySelectorAll(selector)) {
        const label = cleanText(image.getAttribute('alt') || image.getAttribute('aria-label') || '', 500);
        for (const raw of [image.currentSrc, image.getAttribute('src'), image.getAttribute('data-delayed-url')]) {
          const url = linkedInAssetUrl(raw);
          if (/company-logo|entity-logo/i.test(url) || url && /\b(?:company|page)\s+logo\b|公司标志/i.test(label)) return url;
        }
      }
    }
    return '';
  }

  function metaContent(name) {
    const node = document.querySelector(`meta[property="${name}"], meta[name="${name}"]`);
    return node && (node.getAttribute('content') || '').trim() || '';
  }

  function canonicalPageUrl() {
    const canonical = document.querySelector('link[rel="canonical"]');
    return linkedInUrl(canonical && canonical.href) || linkedInUrl(metaContent('og:url')) || linkedInUrl(location.href);
  }

  function profileIdFromUrl(raw) {
    try {
      const match = new URL(raw, location.href).pathname.match(PROFILE_PATH);
      return match ? decodeURIComponent(match[1]) : '';
    } catch (_) {
      return '';
    }
  }

  function companyIdFromUrl(raw) {
    try {
      const match = new URL(raw, location.href).pathname.match(COMPANY_PATH);
      return match ? decodeURIComponent(match[1]) : '';
    } catch (_) {
      return '';
    }
  }

  function activityIdFromText(value) {
    const match = String(value || '').match(/(?:activity-|urn:li:(?:activity|ugcPost|share):)(\d+)/i);
    return match ? match[1] : '';
  }

  function activityIdFromReact(root) {
    if (!root) return '';
    const visited = new Set();
    let inspected = 0;
    let remainingBytes = 256 * 1024;
    const candidates = new Set();

    function remember(value) {
      const id = activityIdFromText(value);
      if (id) candidates.add(id);
    }

    function inspect(value, depth, parsedPayload) {
      if (value == null || depth > 18 || inspected > 1500 || remainingBytes <= 0 || candidates.size > 1) return;
      inspected += 1;
      if (typeof value === 'string') {
        if (value.length > remainingBytes) return;
        remainingBytes -= value.length;
        if (/^[\[{]/.test(value.trim())) {
          try { inspect(JSON.parse(value), depth + 1, true); } catch (_) { /* not JSON */ }
        }
        return;
      }
      if (typeof value !== 'object' && typeof value !== 'function') return;
      if (visited.has(value)) return;
      visited.add(value);

      if (value.type === 'Buffer' && Array.isArray(value.data)) {
        if (value.data.length > remainingBytes) return;
        remainingBytes -= value.data.length;
        let decoded = '';
        for (let offset = 0; offset < value.data.length; offset += 8192) {
          decoded += String.fromCharCode(...value.data.slice(offset, offset + 8192));
        }
        const updateUrn = decoded.match(/"updateUrn"\s*:\s*"urn:li:(?:activity|ugcPost|share):(\d+)"/i);
        if (updateUrn) candidates.add(updateUrn[1]);
      }

      let keyCount = 0;
      try {
        for (const key in value) {
          if (!Object.prototype.hasOwnProperty.call(value, key)) continue;
          if (keyCount >= 200) break;
          keyCount += 1;
          if (!parsedPayload && !/^\d+$/.test(key) && !['children', 'props', '_payload', 'value'].includes(key)) {
            continue;
          }
          let child;
          try { child = value[key]; } catch (_) { continue; }
          if (parsedPayload && /^(?:update|activity)Urn$/i.test(key)) {
            remember(child);
          }
          inspect(child, depth + 1, parsedPayload);
          if (candidates.size > 1) return;
        }
      } catch (_) {
        return;
      }
    }

    let rootKeyCount = 0;
    try {
      for (const key in root) {
        if (!Object.prototype.hasOwnProperty.call(root, key)) continue;
        if (rootKeyCount >= 200) break;
        rootKeyCount += 1;
        if (!key.startsWith('__reactProps$')) continue;
        inspect(root[key], 0, false);
        if (candidates.size > 1) break;
      }
    } catch (_) {
      return '';
    }
    return candidates.size === 1 ? Array.from(candidates)[0] : '';
  }

  function activityId(raw, root) {
    const urn = root && (
      root.getAttribute('data-activity-urn') ||
      root.getAttribute('data-featured-activity-urn') ||
      root.getAttribute('data-urn') ||
      root.getAttribute('data-id')
    ) || '';
    const urnMatch = String(urn).match(/urn:li:(?:activity|ugcPost|share):(\d+)/i);
    if (urnMatch) return urnMatch[1];
    return activityIdFromText(raw) || activityIdFromReact(root);
  }

  function activityUrl(id) {
    return id ? `https://www.linkedin.com/feed/update/urn:li:activity:${id}/` : '';
  }

  function stripConnectionDegree(value) {
    return cleanText(value, 500).replace(/\s*[•·]\s*(?:1st|2nd|3rd\+?).*$/i, '').trim();
  }

  function authorNameFromLink(link) {
    if (!link) return '';
    const aria = cleanText(link.getAttribute('aria-label') || '', 500);
    if (/^View\s+/i.test(aria)) {
      return aria.replace(/^View(?::)?\s+/i, '')
        .replace(/[’']s\s+(?:graphic link|profile)$/i, '')
        .replace(/\s+(?:graphic link|profile)$/i, '')
        .trim();
    }
    const firstLine = cleanText(link, 500).split('\n').find((line) => line.trim()) || '';
    return stripConnectionDegree(firstLine);
  }

  function relativeTimeNear(node, boundary) {
    let scope = node;
    for (let depth = 0; scope && scope !== boundary && depth < 5; depth += 1, scope = scope.parentElement) {
      const value = firstTextMatching(
        scope,
        'time, p, span',
        /^\d+\s*(?:m|h|d|w|mo|yr)s?\b(?:\s*•\s*Edited)?(?:\s*•)?$/i,
        500,
      );
      if (value) return value;
    }
    return '';
  }

  function pageType() {
    const path = location.pathname;
    const search = path.match(SEARCH_PATH);
    if (search) return `search_${search[1].toLowerCase()}`;
    if (PROFILE_PATH.test(path)) return 'profile';
    if (COMPANY_PATH.test(path)) return /\/people\/?$/i.test(path) ? 'company_people' : 'company';
    if (POST_PATH.test(path)) return 'post';
    if (/^\/checkpoint(?:\/|$)/i.test(path)) return 'challenge';
    if (/^\/(?:authwall|uas\/login|login|signup)/i.test(path)) return 'login';
    if (/^\/feed\/?/i.test(path)) return 'feed';
    return 'unknown';
  }

  function challengeRequired() {
    if (/^\/checkpoint(?:\/|$)/i.test(location.pathname)) return true;
    const marker = firstVisibleNode(document, [
      '#challenge-dialog-modal-header',
      'iframe[title*="Security Verification"]',
      'iframe[title*="安全验证"]',
      '[data-test-id*="captcha"]',
      '[class*="challenge-dialog"]',
      '[class*="captcha"]',
    ]);
    return !!marker;
  }

  function rateLimited() {
    const marker = firstVisibleNode(document, [
      '[data-test-id*="rate-limit"]',
      '[class*="rate-limit"]',
      '[class*="commercial-use-limit"]',
      '[data-test-id*="commercial-use-limit"]',
    ]);
    if (marker) return true;
    const normalContent = postHasHydratedContent(postRoot()) || hasProfileContent() || hasCompanyContent() ||
      searchResultNodes(searchRouteType()).length > 0;
    if (normalContent) return false;
    const error = firstVisibleNode(document, [
      'main [role="alert"]',
      'main .artdeco-inline-feedback--error',
      'main .error-container',
      'main h1',
    ]);
    return /too many requests|temporarily restricted|commercial use limit|unusual activity|请求过多|暂时受到限制|商业用途限制/i.test(cleanText(error, 2000));
  }

  function loginGatePresent() {
    return !!firstNode(document, [
      'form[action*="/uas/login"]',
      'form[data-id="sign-in-form"]',
      '.contextual-sign-in-modal',
      '.authwall-sign-in-form',
      '.authwall-join-form__title',
      'a[href*="/login"]',
    ]);
  }

  function loginRoute() {
    return /^\/(?:authwall|uas\/login|login|signup)/i.test(location.pathname);
  }

  function authenticated() {
    return !!firstNode(document, [
      '.global-nav__me',
      '[data-control-name="identity_welcome_message"]',
      'a[href*="/mynetwork/"]',
      'a[href*="/messaging/"]',
    ]);
  }

  function postRoot() {
    const classic = firstNode(document, [
      'article[data-activity-urn]',
      'article[data-featured-activity-urn]',
      'main article.feed-shared-update-v2',
      'main [data-urn^="urn:li:activity:"]',
      'main article',
    ]);
    if (classic) return classic;
    const body = firstNode(document, [
      'main [data-testid="expandable-text-box"]',
    ]);
    return body && body.closest('[role="listitem"]') || null;
  }

  function postHasHydratedContent(root) {
    if (!root) return false;
    const commentary = firstNode(root, [
      '[data-test-id="main-feed-activity-card__commentary"]',
      '[data-testid="expandable-text-box"]',
      '.update-components-text',
      '[data-ad-preview="message"]',
      '.feed-shared-update-v2__description',
    ]);
    const actor = firstNode(root, [
      'a[data-tracking-control-name*="feed-actor-name"]',
      '.update-components-actor__name',
      '.feed-shared-actor__name',
      'a[href*="/in/"]',
    ]);
    const media = firstNode(root, [
      '.update-components-image img[src]',
      '.feed-shared-image img[src]',
      '.update-components-video video',
      '.feed-shared-external-video',
      '.document-s-container',
      '[data-test-id*="media"] img[src]',
    ]);
    return !!cleanText(commentary, 200) || !!cleanText(actor, 200) || !!media;
  }

  function hasProfileContent() {
    if (!PROFILE_PATH.test(location.pathname)) return false;
    return !!firstNode(document, [
      'main h1',
      'main h2',
      '.top-card-layout__title',
      '.pv-text-details__left-panel',
    ]);
  }

  function hasCompanyContent() {
    return COMPANY_PATH.test(location.pathname) && !!firstNode(document, ['main h1']);
  }

  function pageState() {
    const bodyLength = cleanText(document.body, 200000).length;
    const root = postRoot();
    const hasPost = postHasHydratedContent(root);
    const hasProfile = hasProfileContent();
    const hasCompany = hasCompanyContent();
    const resultCount = searchResultNodes(searchRouteType()).length;
    const challenge = challengeRequired();
    const limited = rateLimited();
    const gate = loginGatePresent();
    const contentAvailable = hasPost || hasProfile || hasCompany || resultCount > 0;
    return {
      ok: !challenge && !limited && !loginRoute(),
      site: 'linkedin',
      url: location.href,
      canonical_url: canonicalPageUrl(),
      title: document.title || '',
      page_type: pageType(),
      ready_state: document.readyState,
      body_text_len: bodyLength,
      authenticated: authenticated(),
      login_required: loginRoute(),
      login_gate_present: gate,
      challenge_required: challenge,
      rate_limited: limited,
      content_available: contentAvailable,
      result_count: resultCount,
      hydrated: document.readyState !== 'loading' && (bodyLength > 20 || challenge || limited),
      blank_or_throttled: document.readyState === 'loading' || (bodyLength < 20 && !challenge && !limited),
    };
  }

  function searchRouteType() {
    const match = location.pathname.match(SEARCH_PATH);
    return match ? match[1].toLowerCase() : 'all';
  }

  function resultLinks(card, resultType) {
    if (resultType === 'people') return Array.from(card.querySelectorAll('a[href*="/in/"]'));
    if (resultType === 'companies') {
      return Array.from(card.querySelectorAll('a[href*="/company/"], a[href*="/showcase/"]'));
    }
    if (resultType === 'content') {
      return Array.from(card.querySelectorAll('a[href*="/posts/"], a[href*="/feed/update/urn:li:"]'));
    }
    return Array.from(card.querySelectorAll(
      'a[href*="/posts/"], a[href*="/feed/update/urn:li:"], a[href*="/in/"], a[href*="/company/"], a[href*="/showcase/"]',
    ));
  }

  function semanticResultCard(node, resultType) {
    if (!node || node.getAttribute('role') !== 'listitem') return true;
    const nested = node.parentElement && node.parentElement.closest('[role="listitem"]');
    if (nested) return false;
    if (resultType === 'content') {
      const heading = firstText(node, ['h2'], 100);
      return !!firstNode(node, ['[data-testid="expandable-text-box"]']) && /^Feed post$/i.test(heading);
    }
    return resultLinks(node, resultType).length > 0;
  }

  function hasResultIdentity(card, resultType) {
    return resultLinks(card, resultType).length > 0 ||
      (resultType === 'content' && !!activityId('', card));
  }

  function searchResultNodes(resultType) {
    if (!SEARCH_PATH.test(location.pathname)) return [];
    const expectedType = normalizeResultType(resultType || searchRouteType());
    const selectors = [
      'main li.reusable-search__result-container',
      'main .entity-result',
      'main [data-chameleon-result-urn]',
      'main [data-view-name="search-entity-result-universal-template"]',
      'main [role="listitem"]',
    ];
    const nodes = [];
    const seen = new Set();
    for (const selector of selectors) {
      for (const node of document.querySelectorAll(selector)) {
        const card = node.closest('li.reusable-search__result-container, .entity-result, [data-chameleon-result-urn], [role="listitem"]') || node;
        if (!semanticResultCard(card, expectedType)) continue;
        if (seen.has(card) || card.closest('header, nav, aside')) continue;
        if (!hasResultIdentity(card, expectedType)) continue;
        seen.add(card);
        nodes.push(card);
      }
    }
    if (nodes.length) return nodes;

    for (const link of document.querySelectorAll('main a[href*="/in/"], main a[href*="/posts/"], main a[href*="/feed/update/urn:li:"], main a[href*="/company/"], main a[href*="/showcase/"]')) {
      const card = link.closest('li, article, [data-view-name], [data-urn], [role="listitem"]') || link;
      if (!semanticResultCard(card, expectedType)) continue;
      if (seen.has(card) || card.closest('header, nav, aside')) continue;
      if (!hasResultIdentity(card, expectedType)) continue;
      seen.add(card);
      nodes.push(card);
    }
    return nodes;
  }

  function resultLink(card, expectedType) {
    const links = resultLinks(card, expectedType);
    if (expectedType === 'people') {
      return links.filter((link) => cleanText(link, 500)).sort((left, right) =>
        cleanText(left, 500).length - cleanText(right, 500).length)[0] || links[0] || null;
    }
    if (expectedType === 'companies') return links.find((link) => cleanText(link, 500)) || links[0] || null;
    return links[0] || null;
  }

  function normalizeResultType(value) {
    const kind = String(value || 'all').trim().toLowerCase();
    return ['people', 'content', 'companies', 'all'].includes(kind) ? kind : 'all';
  }

  function searchResults(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const expectedType = normalizeResultType(input.result_type);
    const viewportOnly = !!input.viewport_only;
    const output = [];
    const seen = new Set();

    for (const card of searchResultNodes(expectedType)) {
      if (viewportOnly && !inViewport(card)) continue;
      const link = resultLink(card, expectedType);
      let url = linkedInUrl(link && (link.href || link.getAttribute('href')) || '');
      const path = url && new URL(url).pathname || '';
      const postId = expectedType === 'content' || expectedType === 'all' ? activityId(url, card) : '';
      let kind = PROFILE_PATH.test(path) ? 'profile' : COMPANY_PATH.test(path) ? 'company' : 'post';
      if (postId) {
        kind = 'post';
        url = activityUrl(postId);
      }
      if (!url) continue;
      if (expectedType === 'people' && kind !== 'profile') continue;
      if (expectedType === 'content' && kind !== 'post') continue;
      if (expectedType === 'companies' && kind !== 'company') continue;
      const id = kind === 'profile' ? profileIdFromUrl(url) :
        kind === 'company' ? companyIdFromUrl(url) : postId || activityId(url, card);
      const key = id ? `${kind}:${id}` : url;
      if (seen.has(key)) continue;
      seen.add(key);

      const title = firstText(card, [
        '.entity-result__title-text span[aria-hidden="true"]',
        '[data-anonymize="person-name"]',
        '[data-view-name="search-entity-result-universal-template"] h3',
        'h3',
      ], 500) || (kind === 'post' ? stripConnectionDegree(firstNonemptyText(card, [
        'a[href*="/in/"][aria-label]',
        'a[href*="/in/"]',
      ], 500)) : cleanText(link, 500));
      let subtitle = firstText(card, [
        '.entity-result__primary-subtitle',
        '.entity-result__secondary-subtitle',
        '[data-anonymize="headline"]',
        '.t-14.t-black.t-normal',
      ], 1000);
      let snippet = firstText(card, [
        '.entity-result__summary',
        '.entity-result__content-summary',
        '[data-testid="expandable-text-box"]',
        '.update-components-text',
        '[data-ad-preview="message"]',
      ], 3000);
      const cardLines = cleanText(card, 5000).split('\n').map((line) => line.trim()).filter(Boolean);
      const connectionDegree = cardLines.find((line) => /^(?:•\s*)?(?:1st|2nd|3rd\+?)$/i.test(line)) || '';
      const followers = cardLines.find((line) => /\b[\d,.]+[KM]?\s+followers?\b/i.test(line)) || '';
      const mutualConnections = cardLines.find((line) => /\bmutual connections?\b/i.test(line)) || '';
      const detailLines = cardLines.filter((line) => line !== title && line !== connectionDegree &&
        line !== followers && line !== mutualConnections && !/^(?:connect|follow|message)$/i.test(line));
      let locationText = '';
      if (kind === 'profile') {
        subtitle ||= detailLines[0] || '';
        locationText = detailLines[1] || '';
      } else if (kind === 'company') {
        subtitle ||= detailLines[0] || '';
        const descriptionIndex = detailLines.findIndex((line, index) => index > 0 && line.length >= 80);
        locationText = descriptionIndex > 1 ? detailLines.slice(1, descriptionIndex).join(' ') :
          descriptionIndex < 0 && detailLines.length === 2 ? detailLines[1] : '';
        if (/^page by\b/i.test(locationText)) locationText = '';
        snippet ||= descriptionIndex >= 0 ? detailLines[descriptionIndex] : '';
      }
      const imageUrl = kind === 'profile' ? profileImageUrl(card) :
        kind === 'company' ? companyLogoUrl(card) : '';
      output.push({
        kind,
        id,
        url,
        title,
        subtitle,
        snippet,
        image_url: imageUrl,
        location: locationText,
        connection_degree: kind === 'profile' ? connectionDegree.replace(/^•\s*/, '') : '',
        followers,
        mutual_connections: kind === 'profile' ? mutualConnections : '',
        position: output.length,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function normalizedQuery(value) {
    return String(value || '').replace(/\+/g, ' ').replace(/\s+/g, ' ').trim().toLowerCase();
  }

  function searchState(arg) {
    const input = arg || {};
    const match = location.pathname.match(SEARCH_PATH);
    const actualType = match ? match[1].toLowerCase() : '';
    const expectedType = normalizeResultType(input.result_type);
    const url = new URL(location.href);
    const actualQuery = url.searchParams.get('keywords') || url.searchParams.get('q') || '';
    const expectedQuery = String(input.query || '').trim();
    const results = searchResults({ limit: 100, result_type: expectedType });
    const mainText = cleanText(document.querySelector('main') || document.body, 10000);
    const emptyMarker = firstVisibleNode(document, [
      'main .search-reusables__no-results',
      'main [data-test-id*="no-results"]',
      'main .artdeco-empty-state',
    ]);
    const empty = results.length === 0 && (
      !!emptyMarker || /no results found|try shortening or rephrasing your search|未找到结果|没有找到结果/i.test(mainText)
    );
    const challenge = challengeRequired();
    const limited = rateLimited();
    const needsLogin = loginRoute();
    const routeMatches = !!match && (expectedType === 'all' || actualType === expectedType);
    const queryMatches = !expectedQuery || normalizedQuery(actualQuery) === normalizedQuery(expectedQuery);
    const hydrated = document.readyState !== 'loading' && (results.length > 0 || empty || challenge || limited || needsLogin);
    let error = '';
    if (challenge) error = 'challenge_required';
    else if (limited) error = 'rate_limited';
    else if (needsLogin) error = 'login_required';
    else if (!match) error = 'wrong_search_route';
    else if (!routeMatches) error = 'result_type_mismatch';
    else if (!queryMatches) error = 'query_mismatch';
    else if (!hydrated) error = 'not_hydrated';
    return {
      ok: !error,
      error,
      url: location.href,
      on_search_page: !!match,
      actual_result_type: actualType,
      expected_result_type: expectedType,
      actual_query: actualQuery,
      expected_query: expectedQuery,
      valid_transition: !!match && routeMatches && queryMatches,
      hydrated,
      empty,
      result_count: results.length,
      login_required: needsLogin,
      challenge_required: challenge,
      rate_limited: limited,
    };
  }

  async function scrollResults(arg) {
    if (!SEARCH_PATH.test(location.pathname)) {
      return { ok: false, error: 'wrong_search_route', url: location.href };
    }
    const resultType = searchRouteType();
    const before = { scroll_y: window.scrollY, result_count: searchResultNodes(resultType).length };
    const input = arg || {};
    if (input.to_top) window.scrollTo({ top: 0, behavior: 'auto' });
    else {
      const step = Math.max(320, Math.min(900, Math.floor(window.innerHeight * 0.8)));
      window.scrollBy({ top: input.nudge_up ? -Math.floor(step / 2) : step, behavior: 'auto' });
    }
    await new Promise((resolve) => setTimeout(resolve, 350));
    return {
      ok: true,
      url: location.href,
      before,
      after: { scroll_y: window.scrollY, result_count: searchResultNodes(resultType).length },
    };
  }

  function sectionByAnchor(id) {
    const anchor = document.getElementById(id);
    return anchor && (anchor.closest('section') || anchor.parentElement && anchor.parentElement.closest('section')) || null;
  }

  function sectionByHeading(pattern) {
    for (const section of document.querySelectorAll('main section')) {
      const heading = section.querySelector('h2');
      if (heading && pattern.test(cleanText(heading, 500))) return section;
    }
    for (const heading of document.querySelectorAll('main h1, main h2, main h3')) {
      if (!pattern.test(cleanText(heading, 500))) continue;
      let container = heading.parentElement;
      for (let depth = 0; container && depth < 6; depth += 1, container = container.parentElement) {
        if (container.querySelector('a[href*="/in/"], [role="listitem"]')) return container;
      }
    }
    return null;
  }

  function textLines(value, maxLength) {
    const seen = new Set();
    return cleanText(value, maxLength).split('\n').map((line) => line.trim()).filter((line) => {
      if (!line || seen.has(line)) return false;
      seen.add(line);
      return true;
    });
  }

  function nearestTextScope(node, pattern) {
    let fallback = node && node.parentElement || node;
    for (let current = fallback, depth = 0; current && depth < 6; current = current.parentElement, depth += 1) {
      if (pattern.test(cleanText(current, 5000))) return current;
      if (current.matches && current.matches('main')) break;
    }
    return fallback;
  }

  function profileValue(value) {
    const normalized = cleanText(value, 2000).replace(/\bundefined\b/gi, '').trim();
    if (!normalized || /^[\s*•_-]+$/.test(normalized)) return '';
    return normalized;
  }

  function experienceItems(section) {
    if (!section) return [];
    for (const selector of [
      '[componentkey^="entity-collection-item-"]',
      'ul.visible-list > li.profile-section-card',
      '.pvs-list__paged-list-item',
      'li.artdeco-list__item',
      '.experience-item',
      '[role="listitem"]',
    ]) {
      const items = Array.from(section.querySelectorAll(selector));
      if (items.length) {
        const itemSet = new Set(items);
        return items.filter((item) => {
          for (let parent = item.parentElement; parent && parent !== section; parent = parent.parentElement) {
            if (itemSet.has(parent)) return false;
          }
          return true;
        });
      }
    }
    return [];
  }

  function experienceRole(item, inheritedOrganization) {
    if (!item) return { title: '', organization: '', date_range: '', location: '', description: '', text: '' };
    const lines = textLines(item, 6000).filter((line) => !/^(?:show all|see more)$/i.test(line));
    const dateIndex = lines.findIndex(dateRangeLine);
    const title = profileValue(firstText(item, [
      '.t-bold span[aria-hidden="true"]',
      '.experience-item__title',
      'h4',
    ], 500)) || profileValue(lines[0]);
    const organizationRaw = profileValue(firstText(item, [
      '.t-normal span[aria-hidden="true"]',
      '.experience-item__subtitle',
      'h3',
    ], 500)) || profileValue(lines[1]);
    const organization = profileValue(inheritedOrganization) || organizationRaw.split(/\s+·\s+/)[0].trim();
    const dateRange = dateIndex >= 0 ? profileValue(lines[dateIndex]) : '';
    const locationText = dateIndex >= 0 ? profileValue(lines[dateIndex + 1]) : '';
    const description = dateIndex >= 0 ? profileValue(lines.slice(dateIndex + 2).join('\n')) : '';
    return {
      title,
      organization,
      date_range: dateRange,
      location: locationText,
      description,
      is_current: /\bpresent\b|\bcurrent(?:ly)?\b|至今|目前/i.test(dateRange),
      text: [title, organization].filter(Boolean).join(' — '),
    };
  }

  function educationItem(item) {
    const lines = textLines(item, 4000).filter((line) => !/^(?:show all|see more)$/i.test(line));
    const dateIndex = lines.findIndex(dateRangeLine);
    return {
      institution: profileValue(lines[0]),
      degree: profileValue(lines[1]),
      date_range: dateIndex >= 0 ? profileValue(lines[dateIndex]) : '',
      text: cleanText(item, 4000),
    };
  }

  function dateRangeLine(value) {
    const text = profileValue(value);
    return /\b(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)?\s*(?:19|20)\d{2}\s*(?:-|–|—|to)\s*(?:(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)?\s*(?:19|20)\d{2}|present|current)\b/i.test(text) ||
      /(?:19|20)\d{2}年[^\n]{0,30}(?:至今|现在|(?:19|20)\d{2}年)/.test(text);
  }

  function historyEntries(section, sectionName) {
    const output = [];
    const seen = new Set();
    for (const item of experienceItems(section)) {
      const lines = textLines(item, 8000);
      if (!lines.some(dateRangeLine)) continue;
      const nested = sectionName === 'experience' ? Array.from(item.querySelectorAll(
        '[componentkey^="entity-collection-item-"], .pvs-list__paged-list-item, li.artdeco-list__item, [role="listitem"]',
      )).filter((candidate) => candidate !== item && textLines(candidate, 6000).some(dateRangeLine)) : [];
      const organization = nested.length ? firstNonemptyText(item, ['a[href*="/company/"]'], 500) || lines[0] : '';
      for (const candidate of nested.length ? nested : [item]) {
        const entry = sectionName === 'experience'
          ? experienceRole(candidate, organization)
          : educationItem(candidate);
        if (!entry.date_range || !dateRangeLine(entry.date_range)) continue;
        const key = sectionName === 'experience'
          ? `${entry.title}|${entry.organization}|${entry.date_range}`
          : `${entry.institution}|${entry.degree}|${entry.date_range}`;
        if (seen.has(key)) continue;
        seen.add(key);
        output.push(entry);
      }
    }
    return output;
  }

  function profileDetail() {
    const state = pageState();
    if (state.challenge_required) return { ok: false, error: 'challenge_required', url: location.href };
    if (state.rate_limited) return { ok: false, error: 'rate_limited', url: location.href };
    if (state.login_required) return { ok: false, error: 'login_required', url: location.href };
    if (!PROFILE_LANDING_PATH.test(location.pathname)) return { ok: false, error: 'wrong_profile_route', url: location.href };

    const main = document.querySelector('main') || document;
    const semanticTop = Array.from(main.querySelectorAll('section')).filter((section) => {
      const text = cleanText(section, 5000);
      return !!firstNode(section, ['h1', 'h2']) && /\bcontact info\b|联系信息/i.test(text);
    }).sort((left, right) => cleanText(left, 5000).length - cleanText(right, 5000).length)[0];
    const top = firstNode(main, ['.pv-text-details__left-panel', '.top-card-layout__card']) || semanticTop || main;
    const name = firstText(top, ['h1', 'h2', '.top-card-layout__title'], 500);
    let headline = firstText(top, [
      '.text-body-medium.break-words',
      '.top-card-layout__headline',
      '[data-anonymize="headline"]',
    ], 1200);
    let locationText = firstText(top, [
      '.profile-info-subheader > span:first-child',
      '.text-body-small.inline.t-black--light.break-words',
      '.top-card-layout__first-subline',
      '[data-anonymize="location"]',
    ], 500);
    const topLines = textLines(top, 5000).filter((line) => line !== name &&
      !/^(?:contact info|connect|message|more)$/i.test(line));
    headline ||= topLines[0] || '';
    locationText ||= topLines[1] || '';
    const aboutSection = sectionByAnchor('about') || firstNode(main, ['section.summary', 'section[data-section="summary"]']);
    const aboutContent = firstNode(aboutSection, ['.core-section-container__content > div:first-child']) || aboutSection;
    const experienceSection = sectionByAnchor('experience') ||
      sectionByHeading(/experience|工作经历|职业经历/i) || firstNode(main, ['section.experience']);
    const experience = experienceItems(experienceSection).map(experienceRole)
      .filter((role) => role.title || role.organization);
    const educationSection = sectionByAnchor('education') || sectionByHeading(/education|教育经历|教育背景/i);
    const education = experienceItems(educationSection).map(educationItem)
      .filter((item) => item.institution);
    const latestRole = experience[0] || experienceRole(null);
    const currentRole = experience.find((role) => role.is_current) || null;
    const url = canonicalPageUrl();
    const profileId = profileIdFromUrl(url || location.href);
    const about = cleanText(aboutContent, 6000).replace(/^about\s*/i, '')
      .replace(/\s*(?:see more|展开)\s*$/i, '').trim();
    if (!name) {
      return { ok: false, error: 'profile_not_hydrated', url: location.href, profile_id: profileId };
    }
    const activitySection = sectionByHeading(/^activity$|^动态$/i);
    const followerText = cleanText(activitySection || top, 10000);
    const connectionText = cleanText(top, 10000);
    return {
      ok: true,
      profile_id: profileId,
      url,
      name,
      headline,
      location: locationText,
      avatar_url: profileImageUrl(top),
      about,
      current_role: currentRole,
      latest_role: latestRole,
      experience,
      education,
      connection_degree: firstTextMatching(top, '*', /^(?:1st|2nd|3rd\+?)$/i, 100),
      followers: (followerText.match(/\b[\d,.]+[KMB]?\s+followers?\b/i) || [])[0] || '',
      connections: (connectionText.match(/\b[\d,.]+[KMB]?\s+connections?\b/i) || [])[0] || '',
      login_gate_present: state.login_gate_present,
    };
  }

  function profileHistory() {
    if (challengeRequired()) return { ok: false, error: 'challenge_required', url: location.href };
    if (rateLimited()) return { ok: false, error: 'rate_limited', url: location.href };
    if (loginRoute()) return { ok: false, error: 'login_required', url: location.href };
    const match = location.pathname.match(/^\/in\/([^/?#]+)\/details\/(experience|education)\/?$/i);
    if (!match) return { ok: false, error: 'wrong_profile_history_route', url: location.href };
    const sectionName = match[2].toLowerCase();
    const section = firstNode(document, [sectionName === 'experience'
      ? 'main [data-testid^="profile_ExperienceDetailsSection_"]'
      : 'main [data-testid^="profile_EducationDetailsSection_"]']) || sectionByHeading(sectionName === 'experience'
      ? /^(?:experience|工作经历|职业经历)$/i
      : /^(?:education|教育经历|教育背景)$/i);
    const entries = historyEntries(section, sectionName);
    if (!entries.length) {
      return { ok: false, error: 'profile_history_not_hydrated', url: location.href, section: sectionName };
    }
    return {
      ok: true,
      profile_id: decodeURIComponent(match[1]),
      url: canonicalPageUrl(),
      section: sectionName,
      entries,
    };
  }

  function companyDetail() {
    if (challengeRequired()) return { ok: false, error: 'challenge_required', url: location.href };
    if (rateLimited()) return { ok: false, error: 'rate_limited', url: location.href };
    if (loginRoute()) return { ok: false, error: 'login_required', url: location.href };
    if (!COMPANY_LANDING_PATH.test(location.pathname)) return { ok: false, error: 'wrong_company_route', url: location.href };
    const main = document.querySelector('main') || document;
    const nameNode = firstNode(main, ['h1']);
    const name = cleanText(nameNode, 500);
    if (!name) return { ok: false, error: 'company_not_hydrated', url: location.href };
    const header = nearestTextScope(nameNode, /\b(?:followers?|employees?)\b/i);
    const allText = cleanText(header, 10000);
    const followers = (allText.match(/\b[\d,.]+[KMB]?\s+followers?\b/i) || [])[0] || '';
    const employees = (allText.match(/\b[\d,.]+[KMB]?(?:-[\d,.]+[KMB]?)?\s+employees?\b/i) || [])[0] || '';
    const aboutSection = sectionByHeading(/^(?:about|overview|简介|概览)$/i);
    const summary = firstText(aboutSection, [
      '.break-words',
      '.text-body-medium',
      '[data-test-id="about-us__description"]',
      'p',
    ], 6000).replace(/^(?:about|overview|简介|概览)\s*/i, '').trim();
    return {
      ok: true,
      company_id: companyIdFromUrl(location.href),
      url: canonicalPageUrl(),
      name,
      logo_url: companyLogoUrl(main),
      followers,
      employees,
      associated_members: '',
      summary,
    };
  }

  function peopleFromSection(section, sourceSection, limit) {
    if (!section) return [];
    const output = [];
    const seen = new Set();
    for (const link of section.querySelectorAll('a[href*="/in/"]')) {
      const url = linkedInUrl(link.href || link.getAttribute('href'));
      const profileId = profileIdFromUrl(url);
      if (!profileId || seen.has(profileId) || profileId === profileIdFromUrl(location.href)) continue;
      let card = link.closest('[role="listitem"], li') || link.parentElement && link.parentElement.parentElement || link;
      const name = stripConnectionDegree(cleanText(link, 500));
      if (!name) continue;
      for (let parent = card, depth = 0; parent && depth < 5; depth += 1, parent = parent.parentElement) {
        if (parent.matches && parent.matches('section, main')) break;
        const profileIds = new Set(Array.from(parent.querySelectorAll('a[href*="/in/"]'))
          .map((candidate) => profileIdFromUrl(candidate.href || candidate.getAttribute('href'))).filter(Boolean));
        if (profileIds.size > 1 || profileIds.size === 1 && !profileIds.has(profileId)) break;
        card = parent;
        if (profileImageUrl(parent)) break;
      }
      const lines = textLines(card, 3000).filter((line) => line !== name &&
        !/^(?:connect|follow|message)$/i.test(line) && !/\b[\d,.]+[KM]?\s+followers?\b/i.test(line));
      seen.add(profileId);
      output.push({
        profile_id: profileId,
        url,
        name,
        headline: lines[0] || '',
        image_url: profileImageUrl(card),
        source_section: sourceSection,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function companyPeople(arg) {
    if (challengeRequired()) return { ok: false, error: 'challenge_required', url: location.href, people: [] };
    if (rateLimited()) return { ok: false, error: 'rate_limited', url: location.href, people: [] };
    if (loginRoute()) return { ok: false, error: 'login_required', url: location.href, people: [] };
    if (!COMPANY_PATH.test(location.pathname) || !/\/people\/?$/i.test(location.pathname)) {
      return { ok: false, error: 'wrong_company_people_route', url: location.href, people: [] };
    }
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 30)));
    const section = sectionByHeading(/people you may know|你可能认识的人/i);
    if (!section && loginGatePresent()) {
      return { ok: false, error: 'login_required', url: location.href, people: [] };
    }
    const main = document.querySelector('main') || document;
    if (!section && (document.readyState === 'loading' || cleanText(main, 5000).length < 20)) {
      return { ok: false, error: 'company_people_not_hydrated', url: location.href, people: [] };
    }
    const heading = section ? firstText(section, ['h2'], 500) : '';
    const associatedMembers = firstTextMatching(main, 'h1, h2, h3, p', /\b[\d,.]+[KMB]?\s+associated members?\b/i, 500);
    return {
      ok: true,
      url: canonicalPageUrl(),
      associated_members: associatedMembers,
      source_section: heading,
      section_present: !!section,
      people: peopleFromSection(section, heading, limit),
    };
  }

  function relatedPeople(arg) {
    if (challengeRequired()) return { ok: false, error: 'challenge_required', url: location.href, people: [] };
    if (rateLimited()) return { ok: false, error: 'rate_limited', url: location.href, people: [] };
    if (loginRoute()) return { ok: false, error: 'login_required', url: location.href, people: [] };
    if (!PROFILE_PATH.test(location.pathname) && !COMPANY_PATH.test(location.pathname)) {
      return { ok: false, error: 'wrong_related_people_route', url: location.href, people: [] };
    }
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 30)));
    const pattern = /people also viewed|people you may know|more profiles for you|you might like|你可能认识的人|其他人还查看了/i;
    const output = [];
    const seen = new Set();
    let sectionPresent = false;
    for (const section of document.querySelectorAll('main section')) {
      const heading = firstText(section, ['h2'], 500);
      if (!pattern.test(heading)) continue;
      sectionPresent = true;
      for (const person of peopleFromSection(section, heading, limit)) {
        if (seen.has(person.profile_id)) continue;
        seen.add(person.profile_id);
        output.push(person);
        if (output.length >= limit) break;
      }
      if (output.length >= limit) break;
    }
    if (!sectionPresent && loginGatePresent()) {
      return { ok: false, error: 'login_required', url: location.href, people: [] };
    }
    const main = document.querySelector('main') || document;
    if (!sectionPresent && (document.readyState === 'loading' || cleanText(main, 5000).length < 20)) {
      return { ok: false, error: 'related_people_not_hydrated', url: location.href, people: [] };
    }
    return {
      ok: true,
      url: canonicalPageUrl(),
      section_present: sectionPresent,
      people: output,
    };
  }

  function postMedia(root) {
    const output = [];
    const seen = new Set();
    const selectors = [
      '.update-components-image img[src]',
      '.feed-shared-image img[src]',
      '[data-test-id*="media"] img[src]',
      'video[src]',
      'video source[src]',
      '.document-s-container a[href]',
    ];
    for (const selector of selectors) {
      for (const node of root.querySelectorAll(selector)) {
        const raw = node.currentSrc || node.src || node.href || node.getAttribute('src') || node.getAttribute('href');
        const url = assetUrl(raw);
        if (!url || seen.has(url)) continue;
        seen.add(url);
        const video = node.tagName === 'VIDEO' || node.tagName === 'SOURCE';
        output.push({
          type: video ? 'video' : selector.includes('document-s-container') ? 'document' : 'image',
          url,
          poster_url: video ? assetUrl(node.poster || node.parentElement && node.parentElement.poster) : '',
          alt: video ? '' : cleanText(node.getAttribute('alt') || '', 1000),
        });
      }
    }
    return output;
  }

  function postDetail() {
    const state = pageState();
    if (state.challenge_required) return { ok: false, error: 'challenge_required', url: location.href };
    if (state.rate_limited) return { ok: false, error: 'rate_limited', url: location.href };
    if (!POST_PATH.test(location.pathname)) return { ok: false, error: 'wrong_post_route', url: location.href };
    const root = postRoot();
    if (!root) {
      return { ok: false, error: state.login_required ? 'login_required' : 'post_not_hydrated', url: location.href };
    }
    const body = firstText(root, [
      '[data-test-id="main-feed-activity-card__commentary"]',
      '[data-testid="expandable-text-box"]',
      '.update-components-text',
      '[data-ad-preview="message"]',
      '.feed-shared-update-v2__description',
      '.attributed-text-segment-list__container',
    ], 20000);
    const authorLinks = Array.from(root.querySelectorAll('a[href*="/in/"]'));
    const authorLink = firstNode(root, [
      'a[data-tracking-control-name*="feed-actor-name"]',
      '.update-components-actor__name a',
      '.feed-shared-actor__name a',
    ]) || authorLinks[0] || null;
    const authorUrl = linkedInUrl(authorLink && (authorLink.href || authorLink.getAttribute('href')) || '');
    const authorName = authorNameFromLink(authorLink) || firstText(root, [
      'a[data-tracking-control-name*="feed-actor-name"]',
      '.update-components-actor__name span[aria-hidden="true"]',
      '.feed-shared-actor__name span[aria-hidden="true"]',
    ], 500);
    const published = relativeTimeNear(authorLink, root) || firstText(root, [
      'time',
      '.update-components-actor__sub-description span[aria-hidden="true"]',
      '.feed-shared-actor__sub-description',
    ], 500);
    const reactionsNode = firstNode(root, [
      '[data-id="social-actions__reactions"]',
      '.social-details-social-counts__reactions-count',
    ]);
    const commentsNode = firstNode(root, [
      '[data-id="social-actions__comments"]',
      '.social-details-social-counts__comments',
    ]);
    const semanticReactions = firstTextMatching(root, 'a[href], button, [role="button"]', /\b\d[\d,.]*\s+reactions?\b/i, 500);
    const semanticComments = firstTextMatching(root, 'a[href], button, [role="button"]', /\b\d[\d,.]*\s+comments?\b/i, 500);
    const media = postMedia(root);
    const url = canonicalPageUrl();
    const postId = activityId(url || location.href, root);
    const rootText = cleanText(root, 5000);
    if (/post (?:is )?(?:no longer available|cannot be displayed)|content (?:is )?unavailable|动态已不可用|内容不可用/i.test(rootText)) {
      return { ok: false, error: 'post_unavailable', url: location.href, post_id: postId };
    }
    if (!postHasHydratedContent(root)) {
      return { ok: false, error: 'post_not_hydrated', url: location.href, post_id: postId };
    }
    return {
      ok: true,
      post_id: postId,
      activity_urn: root.getAttribute('data-activity-urn') || root.getAttribute('data-featured-activity-urn') || root.getAttribute('data-urn') || '',
      url,
      author: {
        name: authorName,
        profile_id: profileIdFromUrl(authorUrl),
        url: authorUrl,
      },
      text: body,
      has_media: media.length > 0,
      media,
      published_at: published,
      engagement: {
        reactions: reactionsNode && (reactionsNode.getAttribute('data-num-reactions') || cleanText(reactionsNode, 500)) || semanticReactions,
        comments: commentsNode && (commentsNode.getAttribute('data-num-comments') || cleanText(commentsNode, 500)) || semanticComments,
      },
      login_gate_present: state.login_gate_present,
    };
  }

  function commentNodes(root) {
    const scope = root && root.querySelectorAll ? root : document;
    const selectors = [
      'section.comment',
      '.comments-comment-item',
      '[data-id^="urn:li:comment"]',
      '[data-urn^="urn:li:comment"]',
    ];
    const output = [];
    const seen = new Set();
    for (const selector of selectors) {
      for (const node of scope.querySelectorAll(selector)) {
        const item = node.closest('section.comment, .comments-comment-item, [data-id^="urn:li:comment"], [data-urn^="urn:li:comment"]') || node;
        if (seen.has(item)) continue;
        seen.add(item);
        output.push(item);
      }
    }
    return output;
  }

  function comments(arg) {
    const root = postRoot();
    if (!root) return [];
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 30)));
    const output = [];
    const seen = new Set();
    for (const node of commentNodes(root)) {
      const authorLink = firstNode(node, [
        'a[data-tracking-control-name*="comment_actor-name"]',
        '.comments-post-meta__name-text a',
        '.comments-comment-meta__description-container a[href*="/in/"]',
        'a[href*="/in/"]',
      ]);
      const authorUrl = linkedInUrl(authorLink && (authorLink.href || authorLink.getAttribute('href')) || '');
      const author = cleanText(authorLink, 500) || firstText(node, [
        '.comments-post-meta__name-text',
        '.comments-comment-meta__description-title',
      ], 500);
      const body = firstText(node, [
        'p.comment__text',
        '.comments-comment-item__main-content',
        '.comments-comment-item-content-body',
        '[data-test-id="comment-content"]',
      ], 6000);
      const published = firstTextMatching(
        node,
        'time, p, span',
        /^\d+\s*(?:m|h|d|w|mo|yr)s?\b(?:\s*•\s*Edited)?(?:\s*•)?$/i,
        500,
      ) || firstText(node, [
        '.comment__duration-since',
        'time',
      ], 500);
      const reactions = firstText(node, [
        '.comment__reactions-count:not(.hidden)',
        '.comments-comment-social-bar__reactions-count',
      ], 500);
      if (!body) continue;
      const key = `${authorUrl}|${author}|${published}|${body}`;
      if (seen.has(key)) continue;
      seen.add(key);
      output.push({
        comment_id: node.getAttribute('data-id') || node.getAttribute('data-urn') || '',
        author,
        author_id: profileIdFromUrl(authorUrl),
        author_url: authorUrl,
        text: body,
        published_at: published,
        reactions,
        position: output.length,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  window.SocaiLinkedInPageScripts = Object.freeze({
    pageState,
    searchState,
    searchResults,
    scrollResults,
    profileDetail,
    profileHistory,
    companyDetail,
    companyPeople,
    relatedPeople,
    postDetail,
    comments,
  });
})();
