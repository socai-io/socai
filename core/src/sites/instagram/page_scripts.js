(function () {
  const POST_PATH = /^\/(?:[A-Za-z0-9._]+\/)?(p|reel)\/([A-Za-z0-9_-]+)\/?/i;
  const SEARCH_PATH = /^\/explore\/search\/keyword\/?/i;
  const RESERVED_PROFILE_NAMES = new Set([
    'accounts', 'about', 'api', 'challenge', 'developer', 'direct', 'emails',
    'download', 'explore', 'graphql', 'legal', 'oauth', 'p', 'privacy', 'reel',
    'reels', 'settings', 'stories', 'terms', 'web',
  ]);

  function cleanText(value, maxLength) {
    const raw = typeof value === 'string'
      ? value
      : value && (value.innerText || value.textContent) || '';
    const normalized = String(raw).replace(/\u00a0/g, ' ').replace(/[ \t]+/g, ' ')
      .replace(/\s*\n\s*/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
    return normalized.slice(0, Math.max(0, Number(maxLength || 12000)));
  }

  function editableText(node, maxLength) {
    const raw = node && ('value' in node ? node.value : node.innerText ?? node.textContent) || '';
    return String(raw).replace(/\u00a0/g, ' ').replace(/\r\n?/g, '\n')
      .slice(0, Math.max(0, Number(maxLength || 12000)));
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

  function firstVisibleNode(root, selectors) {
    if (!root || !root.querySelectorAll) return null;
    for (const selector of selectors) {
      const node = Array.from(root.querySelectorAll(selector)).find(visible);
      if (node) return node;
    }
    return null;
  }

  function metaContent(name) {
    const node = document.querySelector(`meta[property="${name}"], meta[name="${name}"]`);
    return node && cleanText(node.getAttribute('content') || '', 30000) || '';
  }

  function instagramUrl(raw) {
    if (!raw) return '';
    try {
      const url = new URL(raw, location.href);
      const host = url.hostname.toLowerCase();
      if (url.protocol !== 'https:' || !(host === 'instagram.com' || host.endsWith('.instagram.com'))) return '';
      url.hash = '';
      url.search = '';
      return url.href;
    } catch (_) {
      return '';
    }
  }

  function canonicalPageUrl() {
    const canonical = document.querySelector('link[rel="canonical"]');
    return instagramUrl(canonical && canonical.href) || instagramUrl(metaContent('og:url')) || instagramUrl(location.href);
  }

  function postIdentity(raw) {
    try {
      const canonical = instagramUrl(raw);
      if (!canonical) return null;
      const match = new URL(canonical).pathname.match(POST_PATH);
      return match ? { kind: match[1].toLowerCase() === 'p' ? 'post' : 'reel', shortcode: match[2] } : null;
    } catch (_) {
      return null;
    }
  }

  function activePostIdentity() {
    const live = postIdentity(location.href);
    if (!live) return null;
    const metadata = postIdentity(metaContent('og:url'));
    return metadata && metadata.shortcode === live.shortcode ? metadata : live;
  }

  function profileUsername(raw) {
    try {
      const parts = new URL(raw, location.href).pathname.split('/').filter(Boolean);
      if (parts.length !== 1) return '';
      const username = parts[0].toLowerCase();
      return /^[a-z0-9._]+$/i.test(username) && !RESERVED_PROFILE_NAMES.has(username) ? username : '';
    } catch (_) {
      return '';
    }
  }

  function pageType() {
    const path = location.pathname;
    if (/^\/accounts\/(?:login|signup)/i.test(path)) return 'login';
    if (/^\/(?:challenge|accounts\/suspended|checkpoint)(?:\/|$)/i.test(path)) return 'challenge';
    if (SEARCH_PATH.test(path)) return 'search';
    if (POST_PATH.test(path)) return postIdentity(location.href).kind;
    if (/^\/explore\/tags\//i.test(path)) return 'hashtag';
    if (/^\/explore\/locations\//i.test(path)) return 'location';
    if (profileUsername(location.href)) return 'profile';
    if (/^\/(?:reels|explore)(?:\/|$)/i.test(path)) return 'explore';
    if (path === '/' || path === '') return 'home';
    return 'unknown';
  }

  function challengeRequired() {
    if (/^\/(?:challenge|accounts\/suspended|checkpoint)(?:\/|$)/i.test(location.pathname)) return true;
    return !!firstVisibleNode(document, [
      'form[action*="/challenge/"]',
      'iframe[title*="challenge" i]',
      'iframe[title*="captcha" i]',
      '[data-testid*="challenge" i]',
    ]);
  }

  function loginRoute() {
    return /^\/accounts\/(?:login|signup)/i.test(location.pathname);
  }

  function loginGatePresent() {
    if (loginRoute()) return true;
    const password = firstVisibleNode(document, ['input[type="password"]']);
    if (password) return true;
    return Array.from(document.querySelectorAll('[role="dialog"]')).some((dialog) => {
      const text = cleanText(dialog, 2000);
      return /(log in|sign up|login|注册|登录|iniciar sesi[oó]n|connexion)/i.test(text);
    });
  }

  function authenticated() {
    return !!firstVisibleNode(document, [
      'a[href^="/direct/inbox"]',
      'a[href^="/accounts/edit"]',
      'a[href^="/accounts/activity"]',
      'svg[aria-label="New post" i]',
      'svg[aria-label="新帖子"]',
    ]);
  }

  function hasPostContent() {
    const identity = postIdentity(location.href);
    if (!identity) return false;
    const dialog = activePostDialog(identity);
    if (dialog && firstVisibleNode(dialog, [
      'h1',
      'img[src]',
      'video',
      'time[datetime]',
      'textarea',
      '[contenteditable="true"]',
    ])) return true;
    return metaContent('og:type') === 'article' && !!(
      metaContent('og:description') || metaContent('description') ||
      document.querySelector('main img[src], main video')
    );
  }

  function hasProfileContent() {
    if (!profileUsername(location.href)) return false;
    const title = metaContent('og:title');
    return !!title || !!document.querySelector('main h1, main h2');
  }

  function rateLimited() {
    const marker = firstVisibleNode(document, [
      '[role="alert"]',
      '[data-testid*="rate" i]',
      'main h1',
    ]);
    if (!marker || hasPostContent() || hasProfileContent() || searchResultLinks().length) return false;
    return /(please wait a few minutes|try again later|too many requests|rate limit|稍后再试|请求过于频繁|操作过于频繁)/i
      .test(cleanText(marker, 2000));
  }

  function searchInput() {
    const inputs = Array.from(document.querySelectorAll('input:not([type]), input[type="text"], input[type="search"]'));
    return inputs.find((input) => {
      if (!visible(input)) return false;
      const label = `${input.placeholder || ''} ${input.getAttribute('aria-label') || ''}`;
      return /(search|搜索|buscar|rechercher|suchen|cerca|検索)/i.test(label);
    }) || (SEARCH_PATH.test(location.pathname) ? inputs.find(visible) || null : null);
  }

  function resultKind(url) {
    const identity = postIdentity(url);
    if (identity) return identity.kind;
    try {
      const path = new URL(url, location.href).pathname;
      if (/^\/explore\/tags\//i.test(path)) return 'hashtag';
      if (/^\/explore\/locations\//i.test(path)) return 'location';
      if (/^\/audio\//i.test(path)) return 'audio';
      if (profileUsername(url)) return 'profile';
    } catch (_) {}
    return '';
  }

  function searchSurfaceActive() {
    if (SEARCH_PATH.test(location.pathname)) return true;
    const input = searchInput();
    if (!input) return false;
    const expanded = input.getAttribute('aria-expanded');
    if (expanded === 'true') return true;
    return searchResultLinks().length > 0;
  }

  function searchResultLinks() {
    const activeInput = searchInput();
    const candidates = [];
    const seen = new Set();
    const roots = [];
    if (SEARCH_PATH.test(location.pathname)) {
      roots.push(document.querySelector('main') || document.body);
    } else {
      for (const dialog of document.querySelectorAll('[role="dialog"]')) roots.push(dialog);
      if (activeInput) {
        let root = activeInput.parentElement;
        for (let i = 0; root && i < 7; i += 1, root = root.parentElement) {
          if (root.querySelectorAll('a[href]').length > 0) roots.push(root);
        }
      }
    }
    for (const root of roots) {
      for (const link of root.querySelectorAll('a[href]')) {
        const url = instagramUrl(link.href || link.getAttribute('href'));
        const kind = resultKind(url);
        if (!url || !kind || seen.has(url)) continue;
        if (link.closest('footer')) continue;
        seen.add(url);
        candidates.push(link);
      }
    }
    return candidates;
  }

  const GENERIC_CARD_LABEL = /^(reels?|clips?|videos?|photos?|posts?|carousels?|视频|图片|帖子)$/i;

  function cardMedia(link) {
    const image = link.querySelector('img[src], img[srcset]');
    const video = link.querySelector('video');
    const source = video && video.querySelector('source[src]');
    const videoUrl = video && (video.currentSrc || video.src || (source && source.src) || '') || '';
    const poster = video && video.poster || '';
    const imageUrl = image && (image.currentSrc || image.src) || (/^https:\/\//i.test(poster) ? poster : '');
    const badge = link.querySelector('svg[aria-label], [aria-label]');
    const badgeLabel = cleanText(badge && badge.getAttribute('aria-label') || '', 80);
    const alt = cleanText(image && image.alt || '', 1000);
    return {
      videoUrl: /^https:\/\//i.test(videoUrl) ? videoUrl : '',
      imageUrl: /^https:\/\//i.test(imageUrl) ? imageUrl : '',
      badgeLabel,
      alt,
      isReel: !!(videoUrl || /^reels?$/i.test(badgeLabel)),
    };
  }

  function meaningfulLine(value, badgeLabel) {
    const text = cleanText(value || '', 500);
    if (!text || GENERIC_CARD_LABEL.test(text) || text === badgeLabel) return '';
    return text;
  }

  function searchResults(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    for (const link of searchResultLinks()) {
      if (viewportOnly && !inViewport(link)) continue;
      const url = instagramUrl(link.href || link.getAttribute('href'));
      let kind = resultKind(url);
      const identity = postIdentity(url);
      const username = profileUsername(url);
      let id = identity && identity.shortcode || username;
      if (!id) {
        const parts = new URL(url).pathname.split('/').filter(Boolean);
        id = parts[parts.length - 1] || url;
      }
      const card = link.closest('li, article, [role="listitem"]') || link;
      const media = cardMedia(link);
      if (kind === 'post' && media.isReel) kind = 'reel';
      const lines = cleanText(card, 2000).split('\n').map((line) => meaningfulLine(line, media.badgeLabel)).filter(Boolean);
      const title = meaningfulLine(cleanText(link, 500).split('\n')[0], media.badgeLabel) || lines[0] || meaningfulLine(media.alt, media.badgeLabel) || '';
      const subtitle = lines.filter((line) => line !== title).slice(0, 3).join('\n');
      output.push({
        kind,
        id,
        url,
        title,
        subtitle,
        thumbnail_url: media.imageUrl,
        video_url: media.videoUrl,
        media_description: meaningfulLine(media.alt, media.badgeLabel),
        position: output.length + 1,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function normalizedQuery(value) {
    return cleanText(value || '', 1000).toLocaleLowerCase().replace(/\s+/g, ' ');
  }

  function currentSearchQuery() {
    const query = new URL(location.href).searchParams.get('q');
    if (query) return cleanText(query, 1000);
    if (loginRoute()) {
      try {
        const next = new URL(location.href).searchParams.get('next');
        const nestedQuery = next && new URL(next, location.origin).searchParams.get('q');
        if (nestedQuery) return cleanText(nestedQuery, 1000);
      } catch (_) {}
    }
    return cleanText((searchInput() && searchInput().value) || '', 1000);
  }

  function explicitSearchEmpty() {
    if (!searchSurfaceActive() || searchResultLinks().length) return false;
    const root = document.querySelector('main') || document.body;
    const text = cleanText(root, 12000);
    return /(no results found|no results|we couldn't find|未找到结果|没有搜索结果|找不到结果|无搜索结果)/i.test(text);
  }

  function searchState(arg) {
    const expected = cleanText(arg && arg.query || '', 1000);
    const actual = currentSearchQuery();
    const challenge = challengeRequired();
    const limited = rateLimited();
    const login = loginRoute();
    const validRoute = searchSurfaceActive();
    const resultCount = validRoute ? searchResultLinks().length : 0;
    const empty = validRoute && explicitSearchEmpty();
    const queryMatches = !expected || normalizedQuery(expected) === normalizedQuery(actual);
    const stableFor = searchResultStability(resultCount, `${location.pathname}\n${actual}`);
    const resultsSettled = resultCount > 0 && stableFor >= 800;
    const hydrated = document.readyState !== 'loading' && (resultsSettled || empty || login || challenge || limited);
    let status = 'unhydrated';
    if (login) status = 'login_required';
    else if (challenge) status = 'challenge_required';
    else if (limited) status = 'rate_limited';
    else if (!validRoute) status = 'not_search_surface';
    else if (!queryMatches) status = 'query_mismatch';
    else if (resultCount > 0 && !resultsSettled) status = 'hydrating';
    else if (resultCount > 0) status = 'results';
    else if (empty) status = 'empty';
    return {
      ok: validRoute && queryMatches && !login && !challenge && !limited && hydrated,
      status,
      url: location.href,
      canonical_url: canonicalPageUrl(),
      valid_route: validRoute,
      expected_query: expected,
      query: actual,
      query_matches: queryMatches,
      result_count: resultCount,
      empty,
      hydrated,
      login_required: login,
      challenge_required: challenge,
      rate_limited: limited,
    };
  }

  function setSearchQuery(arg) {
    const query = cleanText(arg && arg.query || '', 1000);
    if (!query) return { ok: false, status: 'invalid_query', query: '' };
    if (loginRoute()) return { ok: false, status: 'login_required', query: '' };
    const input = searchInput();
    if (!input) return { ok: false, status: 'search_input_not_found', query: '' };
    const descriptor = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value');
    input.focus();
    if (descriptor && descriptor.set) descriptor.set.call(input, query);
    else input.value = query;
    input.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: query }));
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return { ok: input.value === query, status: input.value === query ? 'query_set' : 'query_rejected', query: input.value };
  }

  const SEARCH_CONTROL_LABEL = /^(search|搜索|buscar|rechercher|suchen|cerca|検索)$/i;

  function searchNavControl() {
    const nodes = Array.from(document.querySelectorAll('svg[aria-label], [aria-label], a, [role="link"], [role="button"]'));
    for (const node of nodes) {
      const label = cleanText(
        (node.getAttribute && node.getAttribute('aria-label')) || node.innerText || '',
        40,
      );
      if (!SEARCH_CONTROL_LABEL.test(label)) continue;
      const clickable = (node.closest && node.closest('a, button, [role="link"], [role="button"]')) || node;
      if (clickable.matches && clickable.matches('input, textarea')) continue;
      if (!visible(clickable)) continue;
      const rect = clickable.getBoundingClientRect();
      if (rect.left > 420) continue;
      return clickable;
    }
    return null;
  }

  function openSearch() {
    if (loginRoute()) return { ok: false, status: 'login_required' };
    if (searchInput()) return { ok: true, already_open: true, status: 'search_open' };
    const control = searchNavControl();
    if (!control) return { ok: false, status: 'search_control_not_found' };
    return { ok: false, already_open: false, status: 'search_control', ...elementCenter(control) };
  }

  function suggestionPanel(input) {
    let node = input.parentElement;
    let panel = null;
    for (let i = 0; node && i < 8; i += 1, node = node.parentElement) {
      if (node.querySelector('a[href="/direct/inbox"], a[href="/direct/inbox/"]')) break;
      const links = profileLinksIn(node);
      if (!links.length) continue;
      panel = node;
      break;
    }
    return panel;
  }

  function profileLinksIn(root) {
    const links = [];
    const seen = new Set();
    for (const link of root.querySelectorAll('a[href], [role="link"][href]')) {
      const username = profileUsername(link.href || link.getAttribute('href'));
      if (!username || seen.has(username) || !visible(link)) continue;
      seen.add(username);
      links.push(link);
    }
    return links;
  }

  const ACCOUNT_CHROME = /^(follow|following|requested|message|关注|已关注|发消息)$/i;

  function accountSuggestion(link, username, position) {
    const lines = cleanText(link, 500).split('\n').map((line) => cleanText(line, 200)).filter((line) => {
      if (!line || ACCOUNT_CHROME.test(line)) return false;
      return line.toLocaleLowerCase() !== `${username}'s profile picture`
        && line !== `${username}的头像`;
    });
    const handleIndex = lines.findIndex((line) => line.toLocaleLowerCase() === username);
    const rest = lines.filter((_, index) => index !== handleIndex);
    const name = rest[0] || '';
    const subtitle = rest.slice(1).join('\n');
    const image = link.querySelector('img[src], img[srcset]');
    const avatar = image && (image.currentSrc || image.src) || '';
    return {
      position,
      username,
      name,
      url: instagramUrl(link.href || link.getAttribute('href')),
      subtitle,
      avatar_url: /^https:\/\//i.test(avatar) ? avatar : '',
    };
  }

  function accountSuggestions(arg) {
    const expected = cleanText(arg && arg.query || '', 1000);
    if (loginRoute()) return { ok: false, status: 'login_required', query: '', count: 0, accounts: [] };
    const input = searchInput();
    if (!input) return { ok: false, status: 'search_input_not_found', query: '', count: 0, accounts: [] };
    const actual = cleanText(input.value || '', 1000);
    if (expected && normalizedQuery(expected) !== normalizedQuery(actual)) {
      return { ok: false, status: 'query_mismatch', query: actual, expected_query: expected, count: 0, accounts: [] };
    }
    const panel = suggestionPanel(input);
    const links = panel ? profileLinksIn(panel) : [];
    const accounts = links.map((link, index) => accountSuggestion(
      link,
      profileUsername(link.href || link.getAttribute('href')),
      index + 1,
    ));
    const key = `${actual}\n${accounts.map((account) => account.username).join('\n')}`;
    const stableFor = searchResultStability(accounts.length, `accounts:${key}`);
    const settled = stableFor >= 800;
    let status = 'hydrating';
    if (accounts.length && settled) status = 'results';
    else if (!accounts.length && settled && actual) status = 'empty';
    return {
      ok: status === 'results' || status === 'empty',
      status,
      query: actual,
      count: accounts.length,
      accounts: status === 'hydrating' ? [] : accounts,
    };
  }

  function isScrollable(el) {
    if (!el || !el.getBoundingClientRect) return false;
    const style = window.getComputedStyle(el);
    const overflowY = style.overflowY || style.overflow || '';
    return el.scrollHeight > el.clientHeight + 24 && ['auto', 'scroll', 'overlay'].includes(overflowY);
  }

  function resultsScrollTarget() {
    const links = searchResultLinks();
    let node = links.length ? links[links.length - 1] : (document.querySelector('main') || null);
    while (node && node !== document.body && node !== document.documentElement) {
      if (isScrollable(node)) return node;
      node = node.parentElement;
    }
    return document.scrollingElement || document.documentElement;
  }

  function searchResultStability(count, key) {
    const now = Date.now();
    const watch = window.__socaiIgSearchWatch || (window.__socaiIgSearchWatch = {});
    if (watch.key !== key || watch.count !== count) {
      watch.key = key;
      watch.count = count;
      watch.at = now;
    }
    return now - watch.at;
  }

  function scrollResults(arg) {
    const input = arg || {};
    const target = resultsScrollTarget();
    const scrollingElement = document.scrollingElement || document.documentElement;
    const useWindow = target === scrollingElement || target === document.documentElement || target === document.body;
    const before = target.scrollTop || 0;
    const viewport = useWindow ? window.innerHeight : target.clientHeight;
    const delta = input.to_top ? -before : input.nudge_up
      ? -Math.max(240, Math.floor(viewport * 0.35))
      : Math.max(520, Math.floor(viewport * 0.82));
    if (useWindow) window.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    else target.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    const after = target.scrollTop || 0;
    const extent = (target.scrollHeight || 0) - 8;
    return {
      ok: searchSurfaceActive() && !loginRoute() && !challengeRequired() && !rateLimited(),
      before,
      after,
      result_count: searchResultLinks().length,
      at_end: after + (useWindow ? window.innerHeight : target.clientHeight) >= extent,
    };
  }

  function scrollPosts(arg) {
    if (!profileUsername(location.href) || postIdentity(location.href)) {
      return { ok: false, status: 'not_profile', url: location.href };
    }
    const input = arg || {};
    const before = window.scrollY;
    const beforeCount = profilePosts({ limit: 100 }).length;
    const delta = input.to_top ? -before : input.nudge_up
      ? -Math.max(240, Math.floor(window.innerHeight * 0.35))
      : Math.max(520, Math.floor(window.innerHeight * 0.82));
    window.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return {
      ok: !loginRoute() && !challengeRequired() && !rateLimited(),
      before,
      after: window.scrollY,
      before_count: beforeCount,
      post_count: profilePosts({ limit: 100 }).length,
      at_end: window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 8,
    };
  }

  function parseMetric(raw) {
    const text = cleanText(raw || '', 80).replace(/\s+/g, '');
    const match = text.match(/([\d.,]+)([KMB万亿]?)/i);
    if (!match) return null;
    let number = Number(match[1].replace(/,/g, ''));
    if (!Number.isFinite(number)) return null;
    const unit = match[2].toUpperCase();
    if (unit === 'K') number *= 1e3;
    else if (unit === 'M') number *= 1e6;
    else if (unit === 'B') number *= 1e9;
    else if (unit === '万') number *= 1e4;
    else if (unit === '亿') number *= 1e8;
    return Math.round(number);
  }

  function metricBeforeLabel(text, labels) {
    const match = text.match(new RegExp(`([\\d.,]+\\s*[KMB万亿]?)\\s*(?:位|个)?(?:${labels})`, 'i'));
    return match ? parseMetric(match[1]) : null;
  }

  function metricAfterLabel(text, labels) {
    const match = text.match(new RegExp(`(?:${labels})\\s*([\\d.,]+\\s*[KMB万亿]?)`, 'i'));
    return match ? parseMetric(match[1]) : null;
  }

  function profileStats(description) {
    return {
      followers: metricBeforeLabel(description, 'followers?|粉丝|フォロワー'),
      following: metricAfterLabel(description, 'following|已关注|フォロー中') ??
        metricBeforeLabel(description, 'following|フォロー中'),
      post_count: metricBeforeLabel(description, 'posts?|篇帖子|投稿'),
    };
  }

  function profilePosts(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    const seen = new Set();
    const main = document.querySelector('main') || document;
    const activeUsername = profileUsername(location.href);
    for (const link of main.querySelectorAll('a[href*="/p/"], a[href*="/reel/"]')) {
      if (viewportOnly && !inViewport(link)) continue;
      const url = instagramUrl(link.href || link.getAttribute('href'));
      const identity = postIdentity(url);
      const pathParts = new URL(url).pathname.split('/').filter(Boolean);
      const explicitOwner = pathParts.length >= 3 ? pathParts[0].toLowerCase() : '';
      if (explicitOwner && activeUsername && explicitOwner !== activeUsername) continue;
      if (!identity || seen.has(identity.shortcode)) continue;
      seen.add(identity.shortcode);
      const image = link.querySelector('img[src]');
      const description = cleanText(image && image.alt || '', 2000);
      output.push({
        kind: identity.kind,
        id: identity.shortcode,
        shortcode: identity.shortcode,
        url,
        title: description,
        thumbnail_url: image && (image.currentSrc || image.src) || '',
        position: output.length + 1,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function ownedClickPoint(node) {
    if (!node || !inViewport(node)) return null;
    const point = elementCenter(node);
    const hit = document.elementFromPoint?.(point.x, point.y);
    const owned = !!hit && (hit === node || node.contains?.(hit) ||
      hit.closest?.('button, [role="button"], a[href]') === node);
    return owned ? point : null;
  }

  function activePostDialog(identity) {
    if (!identity) return null;
    for (const dialog of document.querySelectorAll('[role="dialog"]')) {
      if (!visible(dialog)) continue;
      const links = Array.from(dialog.querySelectorAll('a[href*="/p/"], a[href*="/reel/"]'));
      const ownsIdentity = links.some((link) => {
        const candidate = postIdentity(link.href || link.getAttribute('href'));
        return candidate && candidate.shortcode === identity.shortcode;
      });
      if (ownsIdentity) return dialog;
    }
    return null;
  }

  function activePostWriteRoot(identity) {
    const dialog = activePostDialog(identity);
    if (dialog) return dialog;
    if (!identity || Array.from(document.querySelectorAll('[role="dialog"]')).some(visible)) return null;
    const active = activePostIdentity();
    if (!active || active.shortcode !== identity.shortcode) return null;
    const main = document.querySelector('main');
    return main && visible(main) ? main : null;
  }

  function postCardTarget(arg) {
    const input = arg || {};
    const expected = cleanText(input.shortcode || input.id || '', 200);
    const links = [];
    const seen = new Set();
    const append = (candidate) => {
      const sourceUrl = instagramUrl(candidate && (candidate.href || candidate.getAttribute('href')));
      const identity = postIdentity(sourceUrl);
      if (!sourceUrl || !identity || seen.has(sourceUrl)) return;
      seen.add(sourceUrl);
      links.push(candidate);
    };
    if (searchSurfaceActive()) searchResultLinks().forEach(append);
    const main = document.querySelector('main') || document;
    Array.from(main.querySelectorAll('a[href*="/p/"], a[href*="/reel/"]')).forEach(append);
    for (const dialog of document.querySelectorAll('[role="dialog"]')) {
      if (!visible(dialog)) continue;
      Array.from(dialog.querySelectorAll('a[href*="/p/"], a[href*="/reel/"]')).forEach(append);
    }
    let link = expected
      ? links.find((candidate) => postIdentity(candidate.href || candidate.getAttribute('href'))?.shortcode === expected)
      : null;
    if (!link && Number.isInteger(input.index) && input.index >= 0) link = links[input.index] || null;
    if (!link) return { ok: false, status: 'post_card_not_found', shortcode: expected };
    const sourceUrl = instagramUrl(link.href || link.getAttribute('href'));
    const identity = postIdentity(sourceUrl);
    if (!sourceUrl || !identity) return { ok: false, status: 'post_identity_missing', shortcode: expected };
    link.scrollIntoView({ block: 'center', inline: 'center', behavior: 'auto' });
    const cover = link.querySelector('img[src], video');
    for (const target of cover ? [cover, link] : [link]) {
      const rect = target.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;
      const center = elementCenter(target);
      const hit = document.elementFromPoint(center.x, center.y);
      const hitOwned = !!hit && (hit === link || hit === target ||
        (typeof link.contains === 'function' && link.contains(hit)) ||
        (typeof hit.closest === 'function' && hit.closest('a[href]') === link));
      if (!hitOwned) {
        return {
          ok: false,
          status: 'post_card_click_point_obscured',
          shortcode: identity.shortcode,
          source_url: sourceUrl,
        };
      }
      return {
        ok: true,
        target: target === cover ? 'cover' : 'card',
        shortcode: identity.shortcode,
        kind: identity.kind,
        source_url: sourceUrl,
        hit_owned: true,
        ...center,
      };
    }
    return { ok: false, status: 'post_card_zero_sized', shortcode: identity.shortcode };
  }

  function postOpenState(arg) {
    const expected = cleanText(arg && (arg.shortcode || arg.id) || '', 200);
    const identity = activePostIdentity();
    const dialog = activePostDialog(identity);
    const matches = !!identity && (!expected || identity.shortcode === expected);
    const detail = matches && dialog ? postDetail() : null;
    const ready = !!detail && detail.ok === true;
    let status = 'post_not_open';
    if (identity && !matches) status = 'wrong_post';
    else if (matches && !dialog) status = 'full_page_navigation';
    else if (matches && !ready) status = 'post_unhydrated';
    else if (ready) status = 'post_open';
    return {
      ok: ready && !loginRoute() && !loginGatePresent() && !challengeRequired() && !rateLimited(),
      status,
      expected_shortcode: expected,
      shortcode: identity && identity.shortcode || '',
      kind: identity && identity.kind || '',
      url: location.href,
      has_dialog: !!dialog,
      content_ready: ready,
      login_required: loginRoute() || loginGatePresent(),
      challenge_required: challengeRequired(),
      rate_limited: rateLimited(),
    };
  }

  function closePostTarget() {
    const identity = activePostIdentity();
    const dialog = activePostDialog(identity);
    if (!dialog) return { ok: false, status: 'post_dialog_not_found' };
    const labels = /^(?:close|关闭|cerrar|fermer|schlie(?:ß|ss)en|chiudi|閉じる)$/i;
    const controls = Array.from(dialog.querySelectorAll('button, [role="button"]'));
    for (const control of controls) {
      if (!visible(control)) continue;
      const label = cleanText(`${control.getAttribute('aria-label') || ''} ${control.innerText || ''}`, 200);
      if (!labels.test(label)) continue;
      const point = ownedClickPoint(control);
      if (point) return { ok: true, hit_owned: true, shortcode: identity.shortcode, ...point, label };
    }
    const closeIcon = firstVisibleNode(document, [
      'svg[aria-label="Close" i]',
      'svg[aria-label="关闭"]',
      '[data-testid*="close" i]',
    ]);
    const target = closeIcon && (closeIcon.closest('button, [role="button"]') || closeIcon);
    if (!target) return { ok: false, status: 'post_close_control_not_found' };
    const dialogRect = dialog.getBoundingClientRect();
    const targetRect = target.getBoundingClientRect();
    const associated = targetRect.left >= dialogRect.right - 32 &&
      targetRect.left - dialogRect.right <= 360 && targetRect.top <= dialogRect.top + 180;
    const point = associated && ownedClickPoint(target);
    return point
      ? { ok: true, hit_owned: true, shortcode: identity.shortcode, ...point, label: cleanText(target.getAttribute && target.getAttribute('aria-label') || '', 200) }
      : { ok: false, status: associated ? 'post_close_control_obscured' : 'post_close_control_unowned' };
  }

  function outboundUrl(link) {
    try {
      const url = new URL(link.href || link.getAttribute('href'), location.href);
      if (url.hostname.toLowerCase() === 'l.instagram.com') {
        const target = url.searchParams.get('u');
        if (target && /^https?:\/\//i.test(target)) return target;
      }
      return url.href;
    } catch (_) {
      return '';
    }
  }

  function isExternalProfileLink(href) {
    if (!href) return false;
    try {
      const host = new URL(href).hostname.toLowerCase();
      return host && host !== 'instagram.com' && !host.endsWith('.instagram.com') &&
        host !== 'threads.com' && !host.endsWith('.threads.com') &&
        host !== 'facebook.com' && !host.endsWith('.facebook.com');
    } catch (_) {
      return false;
    }
  }

  function externalProfileUrl(root) {
    if (!root || !root.querySelectorAll) return '';
    for (const link of root.querySelectorAll('a[href]')) {
      const href = outboundUrl(link);
      if (isExternalProfileLink(href)) return href;
    }
    return '';
  }

  function profileDetail() {
    const username = profileUsername(location.href);
    const state = pageState();
    if (!username) {
      return { ok: false, status: 'not_profile', url: location.href, page_state: state };
    }
    const description = metaContent('description') || metaContent('og:description');
    const title = metaContent('og:title') || document.title || '';
    const nameMatch = title.match(/^(.+?)\s*\(@([A-Za-z0-9._]+)\)/);
    const bioMatch = description.match(/["“]([\s\S]+)["”]\s*$/);
    const header = document.querySelector('header');
    const main = document.querySelector('main');
    const fromHeader = profileStats(cleanText(header, 4000));
    const fromMeta = profileStats(description);
    const stats = {
      followers: fromHeader.followers ?? fromMeta.followers,
      following: fromHeader.following ?? fromMeta.following,
      post_count: fromHeader.post_count ?? fromMeta.post_count,
    };
    const external = externalProfileUrl(header) || externalProfileUrl(main);
    const visiblePosts = profilePosts({ limit: 100 });
    const contentAvailable = hasProfileContent() && !!(title || description || visiblePosts.length);
    const stableFor = searchResultStability(visiblePosts.length, `profile-grid:${username}`);
    const gridReady = stableFor >= 800 && (visiblePosts.length > 0 || stats.post_count === 0);
    return {
      ok: contentAvailable && gridReady && !state.challenge_required && !state.rate_limited,
      status: !contentAvailable ? (state.login_required ? 'login_required' : 'unhydrated') : gridReady ? 'profile' : 'hydrating',
      id: username,
      username,
      url: canonicalPageUrl(),
      display_name: cleanText(nameMatch && nameMatch[1] || '', 500),
      bio: cleanText(bioMatch && bioMatch[1] || '', 5000),
      followers: stats.followers,
      following: stats.following,
      post_count: stats.post_count,
      avatar_url: metaContent('og:image'),
      external_url: external,
      visible_post_count: visiblePosts.length,
      login_gate_present: state.login_gate_present,
    };
  }

  function quotedCaption(description) {
    const match = cleanText(description, 30000).match(/[:：]\s*["“]([\s\S]*)["”]\.?\s*$/);
    return cleanText(match && match[1] || '', 20000);
  }

  function postAuthor(description, canonical) {
    const fromPath = (() => {
      try {
        const parts = new URL(canonical).pathname.split('/').filter(Boolean);
        return parts.length >= 3 && /^(p|reel)$/i.test(parts[1]) ? parts[0] : '';
      } catch (_) {
        return '';
      }
    })();
    if (fromPath) return fromPath;
    const match = description.match(/-\s*([A-Za-z0-9._]+)\s*(?:,|，|\bon\b)/i);
    return match ? match[1] : '';
  }

  function postMedia() {
    const output = [];
    const seen = new Map();
    const activeIdentity = activePostIdentity();
    const dialog = activePostDialog(activeIdentity);
    const root = dialog || document.querySelector('main') || document;
    const containers = [root, ...Array.from(root.querySelectorAll('article'))];
    const container = containers.find((candidate) => Array.from(candidate.querySelectorAll(
      'a[href*="/p/"], a[href*="/reel/"]',
    )).some((link) => {
      const identity = postIdentity(link.href || link.getAttribute('href'));
      return identity && activeIdentity && identity.shortcode === activeIdentity.shortcode;
    })) || (containers.length === 1 ? containers[0] : null);
    function mediaKey(raw) {
      try {
        const url = new URL(raw);
        return `${url.hostname}${url.pathname}`;
      } catch (_) {
        return '';
      }
    }
    function append(type, rawUrl, rawPoster, alt) {
      const url = /^https:\/\//i.test(rawUrl) ? rawUrl : '';
      const poster = /^https:\/\//i.test(rawPoster) ? rawPoster : '';
      const keys = [mediaKey(url), mediaKey(poster)].filter(Boolean);
      if (!keys.length) return;
      const duplicate = keys.map((key) => seen.get(key)).find((index) => index !== undefined);
      const item = { type, url, poster_url: poster, alt: cleanText(alt || '', 3000) };
      if (duplicate !== undefined) {
        if (type === 'video' && output[duplicate].type === 'image') {
          output[duplicate] = item;
          keys.forEach((key) => seen.set(key, duplicate));
        }
        return;
      }
      const index = output.push(item) - 1;
      keys.forEach((key) => seen.set(key, index));
    }
    if (container) {
      for (const media of container.querySelectorAll('video, video source[src], img[src]')) {
        const alt = cleanText(media.alt || '', 3000);
        if (media.tagName === 'IMG' && /(profile picture|头像|foto del perfil|photo de profil)/i.test(alt)) continue;
        if (media.tagName === 'IMG' && (/\.gif(?:\?|$)/i.test(media.src) || /\/t51\.\d+-19\//i.test(media.src))) continue;
        if (media.closest('a[href]') && !media.closest('a[href*="/p/"], a[href*="/reel/"]')) continue;
        const linkedPost = media.closest('a[href*="/p/"], a[href*="/reel/"]');
        const linkedIdentity = linkedPost && postIdentity(linkedPost.href);
        if (linkedIdentity && activeIdentity && linkedIdentity.shortcode !== activeIdentity.shortcode) continue;
        const video = media.tagName === 'VIDEO' ? media : media.closest('video');
        const source = video && video.querySelector('source[src]');
        const rawUrl = media.currentSrc || media.src || video && (video.currentSrc || video.src) || source && source.src || '';
        const rawPoster = video && video.poster || '';
        append(video ? 'video' : 'image', rawUrl, rawPoster, alt);
        if (output.length >= 20) break;
      }
    }
    if (!output.some((item) => item.type === 'video')) {
      const candidates = [metaContent('og:video'), metaContent('og:video:secure_url')];
      for (const script of document.querySelectorAll('script[type="application/json"], script:not([src])')) {
        const source = script.textContent || '';
        if (!source.includes('video_url')) continue;
        for (const match of source.matchAll(/"video_url"\s*:\s*"((?:\\.|[^"\\])+)"/g)) {
          try { candidates.push(JSON.parse(`"${match[1]}"`)); } catch (_) {}
          if (candidates.length >= 20) break;
        }
        if (candidates.length >= 20) break;
      }
      try {
        for (const entry of performance.getEntriesByType('resource').slice().reverse()) {
          const url = String(entry.name || '');
          if (/^https:\/\//i.test(url) && /(?:\.mp4(?:\?|$)|\/t16\/|cdninstagram\.com\/.*video)/i.test(url)) {
            candidates.push(url);
          }
          if (candidates.length >= 30) break;
        }
      } catch (_) {}
      const poster = output.find((item) => item.type === 'image');
      for (const candidate of candidates) {
        if (!/^https:\/\//i.test(candidate || '')) continue;
        append('video', candidate, poster && poster.url || '', metaContent('og:title'));
        break;
      }
    }
    const ogImage = metaContent('og:image');
    const metadataIdentity = postIdentity(metaContent('og:url'));
    if (ogImage && output.length < 20 && metadataIdentity && activeIdentity &&
      metadataIdentity.shortcode === activeIdentity.shortcode) {
      append('image', ogImage, '', metaContent('og:title'));
    }
    return output;
  }

  function postPublishedAt(root) {
    const identity = activePostIdentity();
    let fallback = '';
    for (const time of (root || document).querySelectorAll('time[datetime]')) {
      if (time.closest('a[href*="/c/"]')) continue;
      const date = time.dateTime || time.getAttribute('datetime') || '';
      const link = time.closest('a[href]');
      const linked = link && postIdentity(link.href);
      // The caption row can carry a different timestamp. The date linked to
      // this post's permalink is the publication date shown in both layouts.
      if (linked && identity && linked.shortcode === identity.shortcode) return date;
      if (!fallback) fallback = date;
    }
    return fallback;
  }

  function commentRows(limit) {
    const flat = [];
    const seen = new Set();
    const root = activePostDialog(activePostIdentity()) || document.querySelector('main') || document;
    for (const time of root.querySelectorAll('a[href*="/c/"] time[datetime]')) {
      const commentLink = time.closest('a[href*="/c/"]');
      const url = instagramUrl(commentLink && commentLink.href);
      const idMatch = url.match(/\/c\/(\d+)\/?$/);
      const id = idMatch && idMatch[1] || '';
      if (!id || seen.has(id)) continue;
      let node = time.parentElement;
      let row = null;
      let authorLink = null;
      for (let depth = 0; node && depth < 8; depth += 1, node = node.parentElement) {
        authorLink = Array.from(node.querySelectorAll('a[href]')).find((link) => profileUsername(link.href));
        if (!authorLink) continue;
        const author = profileUsername(authorLink.href);
        const lines = cleanText(node, 12000).split('\n').filter(Boolean);
        const payload = lines.filter((line) => line !== author && line !== cleanText(time, 200));
        if (payload.length && payload.join('\n').length > 0) {
          row = node;
          break;
        }
      }
      if (!row || !authorLink) continue;
      const author = profileUsername(authorLink.href);
      const relative = cleanText(time, 200);
      const lines = cleanText(row, 12000).split('\n').filter(Boolean);
      const text = cleanText(lines.filter((line) => line !== author && line !== relative).join('\n'), 10000);
      if (!text) continue;
      let actionRoot = row;
      for (let i = 0; actionRoot && i < 3; i += 1) actionRoot = actionRoot.parentElement;
      const actionText = cleanText(actionRoot, 3000);
      const likes = metricBeforeLabel(actionText, 'likes?|次赞');
      seen.add(id);
      let listDepth = 0;
      for (let parent = row.parentElement; parent && parent !== document.body; parent = parent.parentElement) {
        if (parent.matches('ul, ol, [role="list"]')) listDepth += 1;
      }
      const left = row.getBoundingClientRect ? Math.round(row.getBoundingClientRect().left / 12) : 0;
      flat.push({
        id,
        url,
        author: {
          username: author,
          url: instagramUrl(authorLink.href),
        },
        text,
        published_at: time.dateTime || time.getAttribute('datetime') || '',
        published_label: relative,
        likes,
        replies: [],
        __level: listDepth * 1000 + left,
      });
      if (flat.length >= limit) break;
    }
    const output = [];
    const stack = [];
    for (const item of flat) {
      while (stack.length && stack[stack.length - 1].__level >= item.__level) stack.pop();
      if (stack.length) stack[stack.length - 1].replies.push(item);
      else output.push(item);
      stack.push(item);
    }
    const clean = (item) => {
      delete item.__level;
      item.replies.forEach(clean);
      return item;
    };
    return output.map(clean);
  }

  function commentCount(items) {
    return items.reduce((count, item) => count + 1 + commentCount(item.replies || []), 0);
  }

  function comments(arg) {
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 25)));
    return postIdentity(location.href) ? commentRows(limit) : [];
  }

  // Write-action helpers never mutate the page. They expose the current
  // rendered editor/button geometry and exact read-back state; CDP owns the
  // trusted pointer/keyboard events and the one-shot commit policy.
  function commentRoot(arg) {
    const identity = activePostIdentity();
    const expected = cleanText(arg && (arg.shortcode || arg.id) || '', 200);
    if (!identity || !expected || identity.shortcode !== expected) return null;
    return activePostWriteRoot(identity);
  }

  function commentEditor(arg) {
    const root = commentRoot(arg);
    if (!root) return null;
    const editors = Array.from(root.querySelectorAll(
      'textarea, [contenteditable="true"], [role="textbox"]',
    )).filter((editor) => visible(editor) && !editor.disabled && editor.getAttribute('aria-disabled') !== 'true' && !editor.readOnly);
    return editors.find((editor) => /(add a comment|comment|添加评论|发表评论|评论)/i.test(
      `${editor.placeholder || ''} ${editor.getAttribute('aria-label') || ''}`,
    )) || null;
  }

  function commentEditorTarget(arg) {
    const editor = commentEditor(arg);
    if (!editor) return { ok: false, status: 'comment_editor_not_found' };
    const point = ownedClickPoint(editor);
    if (!point) return { ok: false, status: 'comment_editor_obscured' };
    return { ok: true, status: 'comment_editor_ready', shortcode: activePostIdentity().shortcode, hit_owned: true, ...point };
  }

  function commentDraftState(arg) {
    const editor = commentEditor(arg);
    if (!editor) return { ok: false, status: 'comment_editor_not_found', value: '' };
    const active = document.activeElement;
    return {
      ok: true,
      status: 'comment_editor_ready',
      shortcode: activePostIdentity().shortcode,
      focused: active === editor || editor.contains?.(active),
      value: editableText(editor, 10000),
    };
  }

  function commentSubmitTarget(arg) {
    const root = commentRoot(arg);
    const editor = commentEditor(arg);
    if (!root || !editor) return { ok: false, status: 'comment_editor_not_found' };
    const scopes = [];
    for (let node = editor.parentElement, depth = 0; node && node !== root && depth < 7; node = node.parentElement, depth += 1) scopes.push(node);
    scopes.push(root);
    const controls = scopes.flatMap((scope) => Array.from(scope.querySelectorAll('button, [role="button"]')))
      .filter((node) => visible(node) && /^(post|publish|send|发布|发送|发表)$/i.test(cleanText(node, 100)))
      .sort((a, b) => {
        const ar = a.getBoundingClientRect();
        const br = b.getBoundingClientRect();
        return ar.width * ar.height - br.width * br.height;
      });
    const control = controls[0];
    if (!control || !inViewport(control)) return { ok: false, status: 'comment_submit_not_found' };
    const point = ownedClickPoint(control);
    const owned = !!point;
    const disabled = !!control.disabled || control.getAttribute('aria-disabled') === 'true'
      || /disabled/.test(String(control.className || ''));
    return {
      ok: !disabled && owned,
      status: disabled ? 'comment_submit_disabled' : owned ? 'comment_submit_ready' : 'comment_submit_obscured',
      shortcode: activePostIdentity().shortcode,
      text: cleanText(control, 100),
      disabled,
      hit_owned: owned,
      ...(point || elementCenter(control)),
    };
  }

  function renderedCommentState(arg) {
    const expected = cleanText(arg && arg.text || '', 10000);
    if (!expected) return { ok: false, status: 'invalid_comment_text', visible: false, count: 0 };
    const root = commentRoot(arg);
    if (!root) return { ok: false, status: 'wrong_post', visible: false, count: 0 };
    const exact = Array.from(root.querySelectorAll('span, div, p')).filter((node) => {
      if (!visible(node) || node.closest?.('textarea, [contenteditable="true"], [role="textbox"]')) return false;
      return cleanText(node, 10000) === expected;
    });
    const matches = exact.filter((node) => !Array.from(node.querySelectorAll?.('span, div, p') || [])
      .some((child) => child !== node && visible(child) && cleanText(child, 10000) === expected));
    const inView = matches.filter(inViewport);
    return {
      ok: matches.length > 0,
      status: matches.length ? 'comment_visible' : 'comment_not_visible',
      visible: matches.length > 0,
      in_viewport: inView.length > 0,
      count: matches.length,
      in_viewport_count: inView.length,
    };
  }

  async function scrollComments() {
    if (!postIdentity(location.href)) {
      return { ok: false, status: 'not_post', url: location.href };
    }
    const before = commentCount(commentRows(100));
    const root = document.querySelector('[role="dialog"]') || document.querySelector('main') || document;
    const controls = Array.from(root.querySelectorAll('button, [role="button"]'));
    const commentsPattern = /(?:view|load)\s+(?:all\s+\d+|more|previous)\s+comments?|查看(?:全部|更多)?\s*\d*\s*条?评论/i;
    const repliesPattern = /(?:view|load)\s+(?:all\s+)?(?:\d+\s+)?repl(?:y|ies)|查看(?:全部|更多)?\s*\d*\s*条?回复/i;
    let clicked = 0;
    for (const control of controls) {
      if (!visible(control) || control.disabled) continue;
      const label = cleanText(`${control.innerText || ''} ${control.getAttribute && control.getAttribute('aria-label') || ''}`, 500);
      if (!commentsPattern.test(label) && !repliesPattern.test(label)) continue;
      control.click();
      clicked += 1;
      if (clicked >= 6) break;
    }
    const times = Array.from(root.querySelectorAll('a[href*="/c/"] time[datetime]'));
    const last = times[times.length - 1];
    if (last && last.scrollIntoView) last.scrollIntoView({ block: 'end', behavior: 'auto' });
    await new Promise((resolve) => setTimeout(resolve, 450));
    const after = commentCount(commentRows(100));
    return {
      ok: true,
      url: location.href,
      before,
      after,
      clicked,
      grew: after > before,
      at_end: clicked === 0 && after === before,
    };
  }

  function overlayArticle() {
    const dialog = document.querySelector('[role="dialog"]');
    if (!dialog) return null;
    return dialog.querySelector('article') || dialog;
  }

  const LIKE_ICON = 'svg[aria-label="Like" i], svg[aria-label="Unlike" i], svg[aria-label="赞"], svg[aria-label="取消赞"]';
  const COMMENT_ICON = 'svg[aria-label="Comment" i], svg[aria-label="评论"]';

  function postMetric(raw, source) {
    const text = cleanText(raw || '', 100);
    const match = text.match(/^([\d.,]+\s*[KMB万亿]?)(?:\s*(?:likes?|comments?|次赞|条评论|赞|评论))?$/i);
    return match ? {
      value: parseMetric(match[1]),
      source,
      approximate: /[KMB万亿]/i.test(match[1]),
    } : { value: null, source: 'unavailable', approximate: false };
  }

  function metricBesideIcon(bar, selector) {
    let node = bar.querySelector(selector);
    // Modern IG places the count beside the icon wrapper, not inside it.
    for (; node && node !== bar; node = node.parentElement) {
      const sibling = node.nextElementSibling;
      if (!sibling || sibling.querySelector('svg')) continue;
      const metric = postMetric(cleanText(sibling, 100), 'visible');
      if (metric.value !== null) return metric;
    }
    return postMetric('');
  }

  function postEngagement(root, description) {
    let bar = null;
    // A comment's heart has no adjacent post Comment control. Never scan all
    // "N likes" strings: that also matches the likes on individual comments.
    for (const icon of root.querySelectorAll(COMMENT_ICON)) {
      for (let node = icon.parentElement; node && node !== root; node = node.parentElement) {
        if (node.querySelector('a[href*="/c/"]')) break;
        if (node.querySelector(LIKE_ICON)) { bar = node; break; }
      }
      if (bar) break;
    }
    let likes = bar ? metricBesideIcon(bar, LIKE_ICON) : postMetric('');
    let comments = bar ? metricBesideIcon(bar, COMMENT_ICON) : postMetric('');
    // Older layouts render the post likes immediately after the action section.
    const section = bar && bar.closest('section');
    const likesRegion = section && section.nextElementSibling;
    if (likes.value === null && likesRegion && !likesRegion.querySelector('a[href*="/c/"]')) {
      const label = cleanText(likesRegion, 100);
      if (/^\s*[\d.,]+\s*[KMB万亿]?\s*(?:likes?|次赞|赞)\s*$/i.test(label)) {
        likes = postMetric(label, 'visible');
      } else if (/^liked by[\s\S]+and others$/i.test(label)) {
        likes = { value: null, source: 'hidden', approximate: false };
      }
    }
    for (const [key, labels] of [['likes', 'likes?|次赞|赞'], ['comments', 'comments?|条评论|评论']]) {
      const current = key === 'likes' ? likes : comments;
      if (current.value !== null || current.source === 'hidden') continue;
      const match = description.match(new RegExp(`([\\d.,]+\\s*[KMB万亿]?)\\s*(?:${labels})`, 'i'));
      const metric = postMetric(match && match[1] || '', 'metadata');
      if (key === 'likes') likes = metric;
      else comments = metric;
    }
    return {
      likes: likes.value,
      comments: comments.value,
      provenance: { likes, comments },
    };
  }

  function postRoot() {
    const identity = activePostIdentity();
    const dialog = activePostDialog(identity);
    if (dialog) return dialog;
    const overlay = overlayArticle();
    if (overlay) return overlay;
    const main = document.querySelector('main') || document;
    const articles = Array.from(main.querySelectorAll('article'));
    return articles.find((article) => Array.from(article.querySelectorAll('a[href]')).some((link) => {
      const linked = postIdentity(link.href);
      return linked && identity && linked.shortcode === identity.shortcode;
    })) || (articles.length === 1 ? articles[0] : main);
  }

  function visiblePostAuthor(root) {
    const time = Array.from(root.querySelectorAll('time[datetime]'))
      .find((node) => !node.closest('a[href*="/c/"]'));
    if (!time) return '';
    // Only profile links before the post's own date may identify its author;
    // links in the caption and commenters below it cannot supply a fallback.
    return Array.from(root.querySelectorAll('a[href]')).filter((link) =>
      link.compareDocumentPosition(time) & 4,
    ).map((link) => profileUsername(link.href)).find(Boolean) || '';
  }

  function overlayVideoUrl(article) {
    const video = article.querySelector('video');
    if (!video) return '';
    const source = video.querySelector('source[src]');
    const candidates = [
      video.getAttribute('src'),
      source && source.getAttribute('src'),
      video.currentSrc,
      video.src,
      source && source.src,
    ];
    for (const candidate of candidates) {
      if (/^https:\/\//i.test(candidate || '')) return candidate;
    }
    return '';
  }

  function commentState() {
    const overlay = overlayArticle();
    if (!overlay || !postIdentity(location.href)) {
      return { ok: false, status: 'not_overlay', count: 0, empty: false };
    }
    const text = cleanText(overlay, 6000);
    return {
      ok: true,
      count: commentCount(commentRows(100)),
      empty: /no comments yet|还没有评论|暂无评论/i.test(text),
      video_url: overlayVideoUrl(overlay),
      has_video: !!overlay.querySelector('video'),
    };
  }

  function overlayMedia(article) {
    const output = [];
    for (const node of article.querySelectorAll('video, img[src]')) {
      if (node.tagName === 'IMG' && /(profile picture|头像)/i.test(node.alt || '')) continue;
      const video = node.tagName === 'VIDEO' ? node : null;
      const url = video ? (video.currentSrc || video.src || '') : (node.currentSrc || node.src || '');
      if (!/^https:\/\//i.test(url) && !(video && video.poster)) continue;
      output.push({
        type: video ? 'video' : 'image',
        url: /^https:\/\//i.test(url) ? url : '',
        poster_url: video && /^https:\/\//i.test(video.poster || '') ? video.poster : '',
        alt: cleanText(node.alt || '', 3000),
      });
      if (output.length >= 20) break;
    }
    return output;
  }

  function postDetail() {
    const identity = activePostIdentity();
    const candidateUrl = canonicalPageUrl();
    const candidateIdentity = postIdentity(candidateUrl);
    const canonical = identity && candidateIdentity && identity.shortcode === candidateIdentity.shortcode
      ? candidateUrl
      : instagramUrl(location.href);
    const state = pageState();
    if (!identity) {
      return { ok: false, status: 'not_post', url: location.href, page_state: state };
    }
    const root = postRoot();
    const overlay = overlayArticle();
    const metadataIdentity = postIdentity(metaContent('og:url'));
    const metadataMatches = !overlay && (!metadataIdentity || metadataIdentity.shortcode === identity.shortcode);
    const description = metadataMatches ? metaContent('description') || metaContent('og:description') : '';
    const heading = root.querySelector('h1')
      || (root.querySelectorAll && Array.from(root.querySelectorAll('h1'))[0]);
    const caption = cleanText(heading, 20000) || quotedCaption(description);
    const author = visiblePostAuthor(root)
      || (root.querySelectorAll
        ? Array.from(root.querySelectorAll('a[href]')).map((link) => profileUsername(link.href)).find(Boolean)
        : '')
      || postAuthor(description, canonical);
    const media = overlay ? overlayMedia(root) : postMedia();
    const videoUrl = overlayVideoUrl(root) || (media.find((item) => item.type === 'video') || {}).url || '';
    const kind = videoUrl || root.querySelector('video') ? 'reel' : identity.kind;
    const engagement = postEngagement(root, description);
    const visibleComments = commentRows(100);
    const publishedAt = postPublishedAt(root);
    const missingFields = [];
    if (!author) missingFields.push('author');
    if (!publishedAt) missingFields.push('published_at');
    if (engagement.likes === null) missingFields.push('likes');
    if (engagement.comments === null) missingFields.push('comment_count');
    const contentAvailable = !!(caption || author || media.length);
    return {
      ok: contentAvailable && !state.challenge_required && !state.rate_limited,
      status: contentAvailable ? kind : state.login_required ? 'login_required' : 'unhydrated',
      complete: missingFields.length === 0,
      missing_fields: missingFields,
      id: identity.shortcode,
      shortcode: identity.shortcode,
      kind,
      url: `https://www.instagram.com/p/${identity.shortcode}/`,
      author: {
        username: author,
        url: author ? instagramUrl(`/${author}/`) : '',
      },
      caption,
      published_at: publishedAt,
      video_url: videoUrl,
      media,
      engagement: {
        likes: engagement.likes,
        comments: engagement.comments,
        provenance: engagement.provenance,
        visible_comments: commentCount(visibleComments),
      },
      login_gate_present: state.login_gate_present,
    };
  }

  function pageState() {
    const bodyLength = cleanText(document.body, 200000).length;
    const challenge = challengeRequired();
    const limited = rateLimited();
    const login = loginRoute();
    const loginGate = loginGatePresent();
    const searchCount = searchSurfaceActive() ? searchResultLinks().length : 0;
    const contentAvailable = hasPostContent() || hasProfileContent() || searchCount > 0;
    const hydrated = document.readyState !== 'loading' && (bodyLength > 20 || challenge || limited);
    return {
      ok: !challenge && !limited && !login && !loginGate,
      site: 'instagram',
      url: location.href,
      canonical_url: canonicalPageUrl(),
      title: document.title || '',
      page_type: pageType(),
      ready_state: document.readyState,
      body_text_len: bodyLength,
      authenticated: authenticated(),
      login_required: login || loginGate,
      login_gate_present: loginGate,
      challenge_required: challenge,
      rate_limited: limited,
      content_available: contentAvailable,
      result_count: searchCount,
      search_query: searchSurfaceActive() ? currentSearchQuery() : '',
      profile_username: profileUsername(location.href),
      hydrated,
      blank_or_throttled: document.readyState === 'loading' || (bodyLength < 20 && !challenge && !limited),
    };
  }

  function elementCenter(node) {
    const rect = node.getBoundingClientRect();
    return {
      x: Math.round(rect.left + rect.width / 2),
      y: Math.round(rect.top + rect.height / 2),
      width: Math.round(rect.width),
      height: Math.round(rect.height),
    };
  }

  function clickResult(arg) {
    const id = cleanText(arg && (arg.id || arg.shortcode) || '', 80);
    const link = searchResultLinks().find((candidate) => {
      const identity = postIdentity(candidate.href || candidate.getAttribute('href'));
      return identity && identity.shortcode === id;
    });
    if (!link) return { ok: false, error: 'card_not_found', id };
    link.scrollIntoView({ block: 'center', inline: 'center' });
    const video = link.querySelector('video');
    const image = link.querySelector('img');
    const target = (video && visible(video) && video) || (image && visible(image) && image) || link;
    const rect = target.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return { ok: false, error: 'card_zero_sized', id };
    return {
      ok: true,
      id,
      target: target === link ? 'link' : target.tagName.toLowerCase(),
      ...elementCenter(target),
    };
  }

  function closeOverlay() {
    const dialog = document.querySelector('[role="dialog"]');
    const root = dialog || document;
    const controls = Array.from(root.querySelectorAll('button, [role="button"], svg[aria-label], [aria-label]'));
    for (const control of controls) {
      const label = cleanText(`${control.getAttribute && control.getAttribute('aria-label') || ''} ${control.innerText || ''}`, 80);
      if (!/^(close|关闭|cerrar|fermer)$/i.test(label)) continue;
      const node = control.closest('button, [role="button"]') || control;
      if (!visible(node)) continue;
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;
      return { ok: true, label, ...elementCenter(node) };
    }
    return { ok: false, error: 'close_button_not_found', dialog: !!dialog };
  }

  window.SocaiInstagramPageScripts = Object.freeze({
    pageState,
    searchState,
    setSearchQuery,
    openSearch,
    accountSuggestions,
    searchResults,
    clickResult,
    closeOverlay,
    commentState,
    scrollResults,
    scrollPosts,
    profileDetail,
    profilePosts,
    postCardTarget,
    postOpenState,
    closePostTarget,
    postDetail,
    comments,
    scrollComments,
    commentEditorTarget,
    commentDraftState,
    commentSubmitTarget,
    renderedCommentState,
  });
})();
