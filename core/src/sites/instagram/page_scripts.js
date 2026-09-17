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
      const match = new URL(raw, location.href).pathname.match(POST_PATH);
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

  function searchResults(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    for (const link of searchResultLinks()) {
      if (viewportOnly && !inViewport(link)) continue;
      const url = instagramUrl(link.href || link.getAttribute('href'));
      const kind = resultKind(url);
      const identity = postIdentity(url);
      const username = profileUsername(url);
      let id = identity && identity.shortcode || username;
      if (!id) {
        const parts = new URL(url).pathname.split('/').filter(Boolean);
        id = parts[parts.length - 1] || url;
      }
      const card = link.closest('li, article, [role="listitem"]') || link;
      const lines = cleanText(card, 2000).split('\n').filter(Boolean);
      const image = link.querySelector('img[src]') || card.querySelector('img[src]');
      const imageAlt = cleanText(image && image.alt || '', 1000);
      const title = cleanText(link, 500).split('\n').filter(Boolean)[0] || lines[0] || imageAlt || id;
      const subtitle = lines.filter((line) => line !== title).slice(0, 3).join('\n');
      output.push({
        kind,
        id,
        url,
        title,
        subtitle,
        thumbnail_url: image && (image.currentSrc || image.src) || '',
        media_description: imageAlt,
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
    const hydrated = document.readyState !== 'loading' && (resultCount > 0 || empty || login || challenge || limited);
    let status = 'unhydrated';
    if (login) status = 'login_required';
    else if (challenge) status = 'challenge_required';
    else if (limited) status = 'rate_limited';
    else if (!validRoute) status = 'not_search_surface';
    else if (!queryMatches) status = 'query_mismatch';
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

  function scrollResults(arg) {
    const input = arg || {};
    const before = window.scrollY;
    const delta = input.to_top ? -before : input.nudge_up
      ? -Math.max(240, Math.floor(window.innerHeight * 0.35))
      : Math.max(520, Math.floor(window.innerHeight * 0.82));
    window.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return {
      ok: searchSurfaceActive() && !loginRoute() && !challengeRequired() && !rateLimited(),
      before,
      after: window.scrollY,
      result_count: searchResultLinks().length,
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
    const main = document.querySelector('main');
    const stats = profileStats(`${cleanText(main, 12000)}\n${description}`);
    const external = main && Array.from(main.querySelectorAll('a[href]')).map((link) => {
      try {
        const url = new URL(link.href, location.href);
        if (url.hostname.toLowerCase() === 'l.instagram.com') {
          const target = url.searchParams.get('u');
          if (target && /^https?:\/\//i.test(target)) return target;
        }
        return url.href;
      } catch (_) {
        return '';
      }
    }).find((href) => {
      if (!href) return false;
      try {
        const host = new URL(href).hostname.toLowerCase();
        return host && host !== 'instagram.com' && !host.endsWith('.instagram.com') &&
          host !== 'threads.com' && !host.endsWith('.threads.com');
      } catch (_) {
        return false;
      }
    }) || '';
    const visiblePosts = profilePosts({ limit: 100 });
    const contentAvailable = hasProfileContent() && !!(title || description || visiblePosts.length);
    return {
      ok: contentAvailable && !state.challenge_required && !state.rate_limited,
      status: contentAvailable ? 'profile' : state.login_required ? 'login_required' : 'unhydrated',
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
    const main = document.querySelector('main') || document;
    const activeIdentity = activePostIdentity();
    const containers = Array.from(main.querySelectorAll('article, [role="dialog"]'));
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
      for (const media of container.querySelectorAll('video, img[src]')) {
        const alt = cleanText(media.alt || '', 3000);
        if (media.tagName === 'IMG' && /(profile picture|头像|foto del perfil|photo de profil)/i.test(alt)) continue;
        if (media.tagName === 'IMG' && (/\.gif(?:\?|$)/i.test(media.src) || /\/t51\.\d+-19\//i.test(media.src))) continue;
        if (media.closest('a[href]') && !media.closest('a[href*="/p/"], a[href*="/reel/"]')) continue;
        const linkedPost = media.closest('a[href*="/p/"], a[href*="/reel/"]');
        const linkedIdentity = linkedPost && postIdentity(linkedPost.href);
        if (linkedIdentity && activeIdentity && linkedIdentity.shortcode !== activeIdentity.shortcode) continue;
        const rawUrl = media.currentSrc || media.src || '';
        const rawPoster = media.tagName === 'VIDEO' ? media.poster || '' : '';
        append(media.tagName === 'VIDEO' ? 'video' : 'image', rawUrl, rawPoster, alt);
        if (output.length >= 20) break;
      }
    }
    const ogImage = metaContent('og:image');
    const metadataIdentity = postIdentity(metaContent('og:url'));
    if (ogImage && output.length < 20 && metadataIdentity && activeIdentity &&
      metadataIdentity.shortcode === activeIdentity.shortcode) {
      append('image', ogImage, '', metaContent('og:title'));
    }
    if (activeIdentity && activeIdentity.kind === 'reel' && !output.some((item) => item.type === 'video')) {
      const cover = output.find((item) => item.type === 'image');
      if (cover) {
        cover.type = 'video';
        cover.poster_url = cover.url;
        cover.url = '';
      }
    }
    return output;
  }

  function postPublishedAt() {
    for (const time of document.querySelectorAll('main time[datetime], time[datetime]')) {
      const commentLink = time.closest('a[href*="/c/"]');
      if (!commentLink) return time.dateTime || time.getAttribute('datetime') || '';
    }
    return '';
  }

  function engagementFromDescription(description) {
    return {
      likes: metricBeforeLabel(description, 'likes?|次赞|赞'),
      comments: metricBeforeLabel(description, 'comments?|条评论|评论'),
    };
  }

  function commentRows(limit) {
    const output = [];
    const seen = new Set();
    for (const time of document.querySelectorAll('a[href*="/c/"] time[datetime]')) {
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
      output.push({
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
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function comments(arg) {
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 25)));
    return postIdentity(location.href) ? commentRows(limit) : [];
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
    const description = metaContent('description') || metaContent('og:description');
    const caption = quotedCaption(description || metaContent('og:title'));
    const author = postAuthor(description, canonical);
    const media = postMedia();
    const engagement = engagementFromDescription(description);
    const visibleComments = commentRows(100);
    const contentAvailable = hasPostContent() && !!(caption || author || media.length);
    return {
      ok: contentAvailable && !state.challenge_required && !state.rate_limited,
      status: contentAvailable ? identity.kind : state.login_required ? 'login_required' : 'unhydrated',
      id: identity.shortcode,
      shortcode: identity.shortcode,
      kind: identity.kind,
      url: canonical,
      author: {
        username: author,
        url: author ? instagramUrl(`/${author}/`) : '',
      },
      caption,
      published_at: postPublishedAt(),
      media,
      engagement: {
        likes: engagement.likes,
        comments: engagement.comments,
        visible_comments: visibleComments.length,
      },
      login_gate_present: state.login_gate_present,
    };
  }

  function pageState() {
    const bodyLength = cleanText(document.body, 200000).length;
    const challenge = challengeRequired();
    const limited = rateLimited();
    const login = loginRoute();
    const searchCount = searchSurfaceActive() ? searchResultLinks().length : 0;
    const contentAvailable = hasPostContent() || hasProfileContent() || searchCount > 0;
    const hydrated = document.readyState !== 'loading' && (bodyLength > 20 || challenge || limited);
    return {
      ok: !challenge && !limited && !login,
      site: 'instagram',
      url: location.href,
      canonical_url: canonicalPageUrl(),
      title: document.title || '',
      page_type: pageType(),
      ready_state: document.readyState,
      body_text_len: bodyLength,
      authenticated: authenticated(),
      login_required: login,
      login_gate_present: loginGatePresent(),
      challenge_required: challenge,
      rate_limited: limited,
      content_available: contentAvailable,
      result_count: searchCount,
      hydrated,
      blank_or_throttled: document.readyState === 'loading' || (bodyLength < 20 && !challenge && !limited),
    };
  }

  window.SocaiInstagramPageScripts = Object.freeze({
    pageState,
    searchState,
    setSearchQuery,
    searchResults,
    scrollResults,
    profileDetail,
    profilePosts,
    postDetail,
    comments,
  });
})();
