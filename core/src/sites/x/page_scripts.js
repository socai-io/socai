(function () {
  const RESERVED = new Set([
    'compose', 'explore', 'home', 'i', 'intent', 'login', 'messages', 'notifications',
    'search', 'settings', 'share', 'signup',
  ]);

  function cleanText(value, maxLength) {
    const raw = typeof value === 'string'
      ? value
      : value && (value.innerText || value.textContent) || '';
    return String(raw).replace(/\u00a0/g, ' ').replace(/[ \t]+/g, ' ')
      .replace(/\s*\n\s*/g, '\n').replace(/\n{3,}/g, '\n\n').trim()
      .slice(0, Math.max(0, Number(maxLength || 12000)));
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

  function firstVisible(root, selectors) {
    if (!root || !root.querySelectorAll) return null;
    for (const selector of selectors) {
      const match = Array.from(root.querySelectorAll(selector)).find(visible);
      if (match) return match;
    }
    return null;
  }

  function elementCenter(node) {
    const rect = node.getBoundingClientRect();
    return {
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2,
      width: rect.width,
      height: rect.height,
    };
  }

  function hitOwned(node, point) {
    const hit = document.elementFromPoint && document.elementFromPoint(point.x, point.y);
    return !!hit && (hit === node || node.contains?.(hit) || hit.closest?.('a,button,[role="button"]') === node);
  }

  function ownedPoint(node) {
    if (!node || !inViewport(node)) return null;
    const rect = node.getBoundingClientRect();
    const insetX = Math.min(8, rect.width / 4);
    const insetY = Math.min(6, rect.height / 4);
    const points = [
      elementCenter(node),
      { x: rect.left + insetX, y: rect.top + rect.height / 2 },
      { x: rect.right - insetX, y: rect.top + rect.height / 2 },
      { x: rect.left + rect.width / 2, y: rect.top + insetY },
      { x: rect.left + rect.width / 2, y: rect.bottom - insetY },
    ];
    return points.find((point) => hitOwned(node, point)) || null;
  }

  function replyTargets(article) {
    if (!article || !article.querySelectorAll) return [];
    const postBody = article.querySelector('[data-testid="tweetText"]');
    const quotedPost = article.querySelector('[data-testid="quoteTweet"]');
    const contentRoots = [postBody, quotedPost].filter(Boolean);
    const raw = Array.from(article.querySelectorAll('span, div[dir="ltr"]'))
      .filter((node) => visible(node) && !contentRoots.some((content) =>
        content === node || content.contains?.(node) || node.contains?.(content)))
      .map((node) => cleanText(node, 500))
      .filter(Boolean)
      .join('\n');
    const labels = /(?:replying\s+to|in\s+reply\s+to|en\s+respuesta\s+a|respondendo\s+a|r[ée]ponse\s+[àa]|antwort\s+an|in\s+risposta\s+a|返信先|正在回复|回覆|回复|答复|回应)\s*[:：]?\s*@([A-Za-z0-9_]{1,15})/ig;
    return Array.from(raw.matchAll(labels), (match) => match[1].toLowerCase());
  }

  function currentUsername() {
    const profileLinks = Array.from(document.querySelectorAll(
      'a[data-testid="AppTabBar_Profile_Link"][href], [data-testid="SideNav_AccountSwitcher_Button"] a[href]',
    ));
    for (const link of profileLinks) {
      const username = profileUsername(link.href || link.getAttribute('href'));
      if (username) return username;
    }
    const switcher = firstVisible(document, ['[data-testid="SideNav_AccountSwitcher_Button"]']);
    const handle = cleanText(switcher, 1000).match(/@([A-Za-z0-9_]{1,15})/);
    return handle ? handle[1].toLowerCase() : '';
  }

  function xUrl(raw) {
    if (!raw) return '';
    try {
      const url = new URL(raw, location.href);
      const host = url.hostname.toLowerCase();
      if (url.protocol !== 'https:' || url.username || url.password || !(
        host === 'x.com' || host.endsWith('.x.com') ||
        host === 'twitter.com' || host.endsWith('.twitter.com')
      )) return '';
      url.hostname = 'x.com';
      url.hash = '';
      for (const key of Array.from(url.searchParams.keys())) {
        if (!['s', 't'].includes(key)) url.searchParams.delete(key);
      }
      return url.href;
    } catch (_) {
      return '';
    }
  }

  function statusIdentity(raw) {
    try {
      const canonical = xUrl(raw);
      if (!canonical) return null;
      const parts = new URL(canonical).pathname.split('/').filter(Boolean);
      if (parts.length === 3 && parts[1].toLowerCase() === 'status' && /^\d+$/.test(parts[2])) {
        const username = parts[0].toLowerCase();
        return /^[a-z0-9_]{1,15}$/i.test(username) ? { username, id: parts[2] } : null;
      }
      if (parts.length === 4 && parts[0].toLowerCase() === 'i' &&
        parts[1].toLowerCase() === 'web' && parts[2].toLowerCase() === 'status' && /^\d+$/.test(parts[3])) {
        return { username: 'i', id: parts[3] };
      }
      return null;
    } catch (_) {
      return null;
    }
  }

  function profileUsername(raw) {
    try {
      const parts = new URL(raw, location.href).pathname.split('/').filter(Boolean);
      if (parts.length !== 1) return '';
      const value = parts[0].toLowerCase();
      return /^[a-z0-9_]{1,15}$/i.test(value) && !RESERVED.has(value) ? value : '';
    } catch (_) {
      return '';
    }
  }

  function laterSnowflake(candidate, parent) {
    const child = String(candidate || '');
    const root = String(parent || '');
    if (!/^\d+$/.test(child) || !/^\d+$/.test(root) || child === root) return false;
    return child.length !== root.length ? child.length > root.length : child > root;
  }

  const PROFILE_TABS = {
    with_replies: 'replies',
    replies: 'replies',
    reposts: 'reposts',
    highlights: 'highlights',
    articles: 'articles',
    media: 'media',
    likes: 'likes',
  };

  function profileRoute(raw) {
    try {
      const parts = new URL(raw || location.href, location.href).pathname.split('/').filter(Boolean);
      if (!parts.length || parts.length > 2) return null;
      const username = parts[0].toLowerCase();
      if (!/^[a-z0-9_]{1,15}$/i.test(username) || RESERVED.has(username)) return null;
      if (parts.length === 1) return { username, tab: 'posts' };
      const tab = PROFILE_TABS[parts[1].toLowerCase()];
      return tab ? { username, tab } : null;
    } catch (_) {
      return null;
    }
  }

  function primaryColumn() {
    return document.querySelector('[data-testid="primaryColumn"]') || document;
  }

  function searchFilterFromHref(raw) {
    try {
      const value = new URL(raw || location.href, location.href).searchParams.get('f') || '';
      if (!value) return 'top';
      if (value === 'live') return 'latest';
      if (value === 'user') return 'people';
      if (value === 'media') return 'media';
      if (value === 'list') return 'lists';
      return '';
    } catch (_) {
      return '';
    }
  }

  function activeSearchFilter() {
    const selected = firstVisible(document, ['a[role="tab"][aria-selected="true"]'])
      || document.querySelector('a[role="tab"][aria-selected="true"]');
    const fromTab = selected && searchFilterFromHref(selected.getAttribute('href') || selected.href);
    return fromTab || searchFilterFromHref(location.href);
  }

  function loginRequired() {
    if (/^\/i\/(?:flow\/login|jf\/onboarding\/web)/i.test(location.pathname)) return true;
    return !!firstVisible(document, [
      'input[autocomplete="username"]',
      'input[name="text"]',
      'input[type="password"]',
    ]);
  }

  function challengeRequired() {
    if (/^\/account\/access|^\/i\/flow\/consent/i.test(location.pathname)) return true;
    return !!firstVisible(document, [
      'iframe[title*="captcha" i]',
      '[data-testid*="challenge" i]',
      '[data-testid*="arkose" i]',
    ]);
  }

  function rateLimited() {
    const text = cleanText(document.body, 4000);
    return /rate limit exceeded|try again later|too many requests|请求过于频繁|稍后再试/i.test(text)
      && tweetArticles().length === 0;
  }

  function pageType() {
    const path = location.pathname;
    if (loginRequired()) return 'login';
    if (challengeRequired()) return 'challenge';
    if (statusIdentity(location.href)) return 'post';
    if (/^\/search(?:\/|$)/i.test(path)) return 'search';
    if (profileRoute(location.href)) return 'profile';
    if (/^\/(?:home|explore)(?:\/|$)/i.test(path) || path === '/') return 'feed';
    return 'unknown';
  }

  function tweetArticles(root) {
    const scope = root || document;
    return Array.from(scope.querySelectorAll('article[data-testid="tweet"], article'))
      .filter((article) => article.querySelector('a[href*="/status/"]'));
  }

  function articleIdentity(article) {
    if (!article) return null;
    const links = Array.from(article.querySelectorAll('a[href*="/status/"]'));
    const timed = links.find((link) => link.querySelector('time[datetime]'));
    return statusIdentity((timed || links[0]) && ((timed || links[0]).href || (timed || links[0]).getAttribute('href')));
  }

  function metric(raw) {
    const text = cleanText(raw || '', 160).replace(/,/g, '');
    const match = text.match(/([\d.]+)\s*([KMB万亿]?)/i);
    if (!match) return null;
    let value = Number(match[1]);
    if (!Number.isFinite(value)) return null;
    const unit = match[2].toUpperCase();
    if (unit === 'K') value *= 1e3;
    else if (unit === 'M') value *= 1e6;
    else if (unit === 'B') value *= 1e9;
    else if (unit === '万') value *= 1e4;
    else if (unit === '亿') value *= 1e8;
    return Math.round(value);
  }

  function buttonMetric(article, selectors) {
    const node = article.querySelector(selectors);
    if (!node) return null;
    return metric(`${node.getAttribute('aria-label') || ''} ${cleanText(node, 120)}`);
  }

  function parseAuthor(article, identity) {
    const root = article.querySelector('[data-testid="User-Name"]');
    const text = cleanText(root, 1000);
    const handle = text.match(/@([A-Za-z0-9_]{1,15})/);
    const links = root ? Array.from(root.querySelectorAll('a[href]')) : [];
    const profile = links.map((link) => ({ link, username: profileUsername(link.href) }))
      .find((entry) => entry.username);
    const username = profile && profile.username || handle && handle[1].toLowerCase() || identity.username;
    const lines = text.split('\n').filter(Boolean);
    return {
      username,
      display_name: lines.find((line) => !line.startsWith('@')) || '',
      url: username ? xUrl(`https://x.com/${username}`) : '',
    };
  }

  function parseMedia(article) {
    const output = [];
    const seen = new Set();
    function append(type, url, poster, alt) {
      if (!url && !poster) return;
      const key = `${type}:${url || poster}`;
      if (seen.has(key)) return;
      seen.add(key);
      output.push({ type, url: url || '', poster_url: poster || '', alt: cleanText(alt || '', 2000) });
    }
    for (const image of article.querySelectorAll('[data-testid="tweetPhoto"] img[src]')) {
      append('image', image.currentSrc || image.src || '', '', image.alt || image.getAttribute('alt'));
    }
    for (const video of article.querySelectorAll('video')) {
      const source = video.querySelector && video.querySelector('source[src]');
      append('video', video.currentSrc || video.src || source && source.src || '', video.poster || '', '');
    }
    return output.slice(0, 20);
  }

  function parseTweet(article, position) {
    const identity = articleIdentity(article);
    if (!identity) return null;
    const textNode = article.querySelector('[data-testid="tweetText"]');
    const time = article.querySelector('a[href*="/status/"] time[datetime]') || article.querySelector('time[datetime]');
    const quote = article.querySelector('[data-testid="quoteTweet"]');
    return {
      ok: true,
      id: identity.id,
      url: xUrl(`https://x.com/${identity.username}/status/${identity.id}`),
      author: parseAuthor(article, identity),
      text: cleanText(textNode, 30000),
      published_at: time && (time.dateTime || time.getAttribute('datetime')) || '',
      media: parseMedia(article),
      metrics: {
        replies: buttonMetric(article, '[data-testid="reply"]'),
        reposts: buttonMetric(article, '[data-testid="retweet"], [data-testid="unretweet"]'),
        likes: buttonMetric(article, '[data-testid="like"], [data-testid="unlike"]'),
        bookmarks: buttonMetric(article, '[data-testid="bookmark"], [data-testid="removeBookmark"]'),
        views: buttonMetric(article, 'a[href$="/analytics"]'),
      },
      is_reply: replyTargets(article).length > 0,
      quoted_post: quote ? cleanText(quote, 10000) : '',
      position: Number(position || 0),
    };
  }

  function pageState() {
    const login = loginRequired();
    const challenge = challengeRequired();
    const limited = rateLimited();
    const authenticated = !!currentUsername();
    const count = tweetArticles().length;
    const primary = document.querySelector('[data-testid="primaryColumn"]');
    const hydrated = document.readyState !== 'loading' && (count > 0 || !!primary || login || challenge || limited);
    const status = login ? 'login_required' : challenge ? 'challenge_required'
      : limited ? 'rate_limited' : hydrated ? 'ready' : 'unhydrated';
    return {
      ok: hydrated && !login && !challenge && !limited,
      status,
      site: 'x',
      url: location.href,
      page_type: pageType(),
      ready_state: document.readyState,
      authenticated,
      login_required: login,
      challenge_required: challenge,
      rate_limited: limited,
      hydrated,
      result_count: count,
    };
  }

  function mediaStatusLinks(root) {
    const seen = new Set();
    const links = [];
    for (const link of (root || primaryColumn()).querySelectorAll('a[href*="/status/"]')) {
      const identity = mediaIdentity(link.href || link.getAttribute('href'));
      if (!identity || seen.has(identity.id)) continue;
      seen.add(identity.id);
      links.push({ link, identity });
    }
    return links;
  }

  function mediaIdentity(raw) {
    try {
      const parts = new URL(raw, location.href).pathname.split('/').filter(Boolean);
      const index = parts.findIndex((part) => part.toLowerCase() === 'status');
      const id = parts[index + 1] || '';
      const username = parts[index - 1] || '';
      if (index <= 0 || !/^\d+$/.test(id) || !/^[A-Za-z0-9_]{1,15}$/.test(username)) return null;
      const kind = (parts[index + 2] || '').toLowerCase();
      return { username: username.toLowerCase(), id, kind: kind === 'video' ? 'video' : kind === 'photo' ? 'image' : 'post' };
    } catch (_) {
      return null;
    }
  }

  function searchResultCount(filter) {
    const root = primaryColumn();
    if (filter === 'people') return root.querySelectorAll('[data-testid="UserCell"]').length;
    if (filter === 'lists') return root.querySelectorAll('[data-testid="listCell"]').length;
    if (filter === 'media') return mediaStatusLinks(root).length;
    return tweetArticles(root).length;
  }

  function sourceSurfaceState() {
    const state = pageState();
    const resultCount = state.page_type === 'search'
      ? searchResultCount(activeSearchFilter())
      : state.result_count;
    const postIds = tweetArticles(primaryColumn())
      .map((article) => articleIdentity(article)?.id)
      .filter(Boolean);
    const documentHeight = Math.max(
      Number(document.documentElement?.scrollHeight || 0),
      Number(document.body?.scrollHeight || 0),
    );
    const scrollY = Math.max(0, Math.round(window.scrollY || 0));
    const viewportHeight = Math.max(0, Math.round(window.innerHeight || 0));
    return {
      ...state,
      search_query: cleanText(new URL(location.href).searchParams.get('q') || '', 1000),
      profile_username: profileRoute(location.href)?.username || '',
      result_count: resultCount,
      post_ids: Array.from(new Set(postIds)),
      scroll_y: scrollY,
      viewport_height: viewportHeight,
      document_height: documentHeight,
      at_end: viewportHeight + scrollY >= documentHeight - 8,
    };
  }

  function searchState(arg) {
    const state = pageState();
    const query = cleanText(new URL(location.href).searchParams.get('q') || '', 1000);
    const expected = cleanText(arg && arg.query || '', 1000);
    const expectedFilter = cleanText(arg && arg.filter || 'top', 40).toLowerCase() || 'top';
    const filter = activeSearchFilter();
    const onSearch = /^\/search(?:\/|$)/i.test(location.pathname);
    const queryOk = !expected || query.toLowerCase() === expected.toLowerCase();
    const filterOk = filter === expectedFilter;
    const count = searchResultCount(expectedFilter);
    const ready = onSearch && queryOk && filterOk && count > 0;
    let status = state.status;
    if (state.ok && !onSearch) status = 'not_search';
    else if (state.ok && !queryOk) status = 'query_mismatch';
    else if (state.ok && !filterOk) status = 'filter_mismatch';
    else if (state.ok && count > 0) status = 'results';
    else if (state.ok) status = 'unhydrated';
    return {
      ...state,
      ok: state.ok && ready,
      status,
      query,
      filter,
      result_count: count,
      empty: false,
    };
  }

  // Write-action helpers are deliberately inspection-only. Rust/CDP owns the
  // trusted pointer/keyboard events; these helpers only return current geometry
  // and rendered state so prepare/commit can fail closed on layout changes.
  function searchInputTarget() {
    const input = firstVisible(document, [
      'input[data-testid="SearchBox_Search_Input"]',
      'input[placeholder*="Search" i]',
      'input[aria-label*="Search" i]',
      'input[placeholder*="搜索"]',
      'input[aria-label*="搜索"]',
    ]);
    if (!input || !inViewport(input)) return { ok: false, status: 'search_input_not_found' };
    const point = ownedPoint(input);
    return point
      ? { ok: true, status: 'search_input_ready', hit_owned: true, value: String(input.value || ''), ...point }
      : { ok: false, status: 'search_input_obscured', hit_owned: false, value: String(input.value || '') };
  }

  function postLinkTarget(arg) {
    const expected = cleanText(arg && (arg.id || arg.post_id) || '', 100);
    if (!/^\d+$/.test(expected)) return { ok: false, status: 'invalid_post_id' };
    for (const article of tweetArticles()) {
      if (articleIdentity(article)?.id !== expected) continue;
      const links = Array.from(article.querySelectorAll('a[href*="/status/"]'));
      const link = links.find((candidate) => candidate.querySelector('time[datetime]')) || links[0];
      if (!link) return { ok: false, status: 'post_link_not_found', id: expected };
      if (!visible(link) || !inViewport(link)) return { ok: false, status: 'post_link_not_visible', id: expected };
      const point = ownedPoint(link);
      const geometry = point || elementCenter(link);
      const blocker = !point && document.elementFromPoint?.(geometry.x, geometry.y);
      return {
        ok: !!point,
        status: point ? 'post_link_ready' : 'post_link_obscured',
        id: expected,
        url: xUrl(link.href || link.getAttribute('href')),
        hit_owned: !!point,
        blocker: blocker ? {
          tag: String(blocker.tagName || '').toLowerCase(),
          test_id: blocker.getAttribute?.('data-testid') || '',
          role: blocker.getAttribute?.('role') || '',
          text: cleanText(blocker, 80),
        } : null,
        ...geometry,
      };
    }
    return { ok: false, status: 'post_not_found', id: expected };
  }

  function replyContext(arg) {
    const expected = cleanText(arg && (arg.post_id || arg.id) || '', 100);
    if (!/^\d+$/.test(expected)) return { error: 'invalid_post_id' };
    const active = statusIdentity(location.href);
    if (!active || active.id !== expected) return { error: active ? 'wrong_post' : 'not_post' };
    const article = activePostArticle();
    const region = article && article.closest(
      '[aria-label*="conversation" i], [aria-label*="对话"], section[role="region"], [role="region"]',
    );
    if (!article || !region) return { error: 'conversation_not_found' };
    return { active, article, region };
  }

  function replyEditor(arg) {
    const context = replyContext(arg);
    if (context.error) return { context, editor: null };
    const editors = Array.from(document.querySelectorAll(
      '[data-testid="tweetTextarea_0"][contenteditable="true"], [role="textbox"][contenteditable="true"]',
    )).filter((editor) => visible(editor) && inViewport(editor) && context.region.contains?.(editor) &&
      !editor.closest?.('[role="dialog"]') && editor.getAttribute('aria-disabled') !== 'true');
    return { context, editor: editors.length === 1 ? editors[0] : null, count: editors.length };
  }

  function replyEditorTarget(arg) {
    const found = replyEditor(arg);
    if (found.context.error) return { ok: false, status: found.context.error };
    if (!found.editor) return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found' };
    const point = ownedPoint(found.editor);
    return point
      ? { ok: true, status: 'reply_editor_ready', post_id: found.context.active.id, hit_owned: true, ...point }
      : { ok: false, status: 'reply_editor_obscured', post_id: found.context.active.id, hit_owned: false };
  }

  function replyDraftState(arg) {
    const found = replyEditor(arg);
    if (found.context.error) return { ok: false, status: found.context.error, value: '' };
    const editor = found.editor;
    if (!editor) return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found', value: '' };
    const active = document.activeElement;
    return {
      ok: true,
      status: 'reply_editor_ready',
      post_id: found.context.active.id,
      focused: active === editor || editor.contains?.(active),
      value: editableText(editor, 10000),
    };
  }

  function replySubmitTarget(arg) {
    const found = replyEditor(arg);
    if (found.context.error) return { ok: false, status: found.context.error };
    const editor = found.editor;
    if (!editor) return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found' };
    const roots = [];
    // The inline Reply control sits about twenty ancestors above the Draft.js
    // editor, still inside the conversation region.
    for (let node = editor.parentElement, depth = 0;
      node && node !== found.context.region && depth < 40;
      node = node.parentElement, depth += 1) roots.push(node);
    let button = null;
    for (const root of roots) {
      button = Array.from(root.querySelectorAll?.(
        '[data-testid="tweetButtonInline"], [data-testid="tweetButton"]',
      ) || []).find((candidate) => {
        const control = candidate.closest?.('button, [role="button"]') || candidate;
        const testId = candidate.getAttribute?.('data-testid') || control.getAttribute?.('data-testid') || '';
        return visible(control) && /^(?:tweetButtonInline|tweetButton)$/.test(testId);
      });
      if (button) button = button.closest?.('button, [role="button"]') || button;
      if (button) break;
    }
    if (!button || !inViewport(button)) return { ok: false, status: 'reply_submit_not_found' };
    const point = ownedPoint(button);
    const geometry = point || elementCenter(button);
    const disabled = !!button.disabled || button.getAttribute('aria-disabled') === 'true';
    const owned = !!point;
    return {
      ok: !disabled && owned,
      status: disabled ? 'reply_submit_disabled' : owned ? 'reply_submit_ready' : 'reply_submit_obscured',
      post_id: found.context.active.id,
      text: cleanText(button, 100),
      disabled,
      hit_owned: owned,
      ...geometry,
    };
  }

  function listTweets(arg, root) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    const seen = new Set();
    for (const article of tweetArticles(root)) {
      if (viewportOnly && !inViewport(article)) continue;
      const item = parseTweet(article, output.length + 1);
      if (!item || seen.has(item.id)) continue;
      seen.add(item.id);
      output.push(item);
      if (output.length >= limit) break;
    }
    return output;
  }

  function nestedListPath(value, depth) {
    if (!value || depth > 5) return '';
    if (typeof value === 'string') {
      const match = value.match(/\/i\/lists\/\d+/);
      return match ? match[0] : '';
    }
    if (typeof value !== 'object') return '';
    for (const key of Object.keys(value)) {
      if (key === 'children') continue;
      const found = nestedListPath(value[key], depth + 1);
      if (found) return found;
    }
    return '';
  }

  // List rows are role=link controls without an href attribute. The destination
  // /i/lists/<id> is on the row's React props, which is what the click opens.
  function listPath(node) {
    if (!node) return '';
    const propKey = Object.keys(node).find((name) => name.startsWith('__reactProps'));
    const direct = nestedListPath(propKey && node[propKey], 0);
    if (direct) return direct;
    const fiberKey = Object.keys(node).find((name) => name.startsWith('__reactFiber'));
    let current = fiberKey && node[fiberKey];
    for (let depth = 0; current && depth < 12; depth += 1, current = current.return) {
      const found = nestedListPath(current.memoizedProps, 0);
      if (found) return found;
    }
    return '';
  }

  function parseUserCell(cell, position) {
    const username = Array.from(cell.querySelectorAll('a[href]'))
      .map((link) => profileUsername(link.href || link.getAttribute('href')))
      .find(Boolean);
    if (!username) return null;
    const lines = cleanText(cell, 2000).split('\n').map((line) => line.trim()).filter(Boolean)
      .filter((line) => !/^follow(?:ing)?$/i.test(line));
    const handle = `@${username}`;
    const displayName = lines.find((line) => line.toLowerCase() !== handle && !line.startsWith('@')) || '';
    const bio = lines.filter((line) => line !== displayName && line.toLowerCase() !== handle).join('\n');
    return {
      username,
      display_name: displayName,
      bio,
      url: xUrl(`https://x.com/${username}`),
      position: Number(position || 0),
    };
  }

  function parseListCell(cell, position) {
    const match = listPath(cell).match(/^\/i\/lists\/(\d+)$/);
    if (!match) return null;
    const nameNode = Array.from(cell.querySelectorAll('span')).find((span) => !span.closest('a'));
    const text = cleanText(cell, 1000);
    const members = text.match(/([\d.,]+)\s*([KMB万亿])?\s*(?:members\b|成员)/i);
    const owner = Array.from(cell.querySelectorAll('a[href]'))
      .map((link) => profileUsername(link.href || link.getAttribute('href')))
      .find(Boolean) || '';
    return {
      id: match[1],
      name: cleanText(nameNode, 200),
      members: members ? metric(`${members[1]}${members[2] || ''}`) : null,
      owner_username: owner,
      url: xUrl(`https://x.com/i/lists/${match[1]}`),
      position: Number(position || 0),
    };
  }

  function collectCells(arg, selector, parse) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    const seen = new Set();
    for (const cell of primaryColumn().querySelectorAll(selector)) {
      if (viewportOnly && !inViewport(cell)) continue;
      const item = parse(cell, output.length + 1);
      if (!item) continue;
      const key = item.id || item.username || item.url;
      if (!key || seen.has(key)) continue;
      seen.add(key);
      output.push(item);
      if (output.length >= limit) break;
    }
    return output;
  }

  function searchMedia(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    for (const entry of mediaStatusLinks(primaryColumn())) {
      if (viewportOnly && !inViewport(entry.link)) continue;
      output.push({
        id: entry.identity.id,
        url: xUrl(`https://x.com/${entry.identity.username}/status/${entry.identity.id}`),
        author: { username: entry.identity.username, url: xUrl(`https://x.com/${entry.identity.username}`) },
        media_type: entry.identity.kind,
        label: cleanText(entry.link, 80),
        position: output.length + 1,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function searchResults(arg) { return listTweets(arg, primaryColumn()); }
  function searchPeople(arg) { return collectCells(arg, '[data-testid="UserCell"]', parseUserCell); }
  function searchLists(arg) { return collectCells(arg, '[data-testid="listCell"]', parseListCell); }
  function profilePosts(arg) { return listTweets(arg, primaryColumn()); }

  function scrollList(arg) {
    const input = arg || {};
    const before = window.scrollY;
    const beforeCount = tweetArticles().length;
    const delta = input.to_top ? -before : input.nudge_up
      ? -Math.max(240, Math.floor(window.innerHeight * 0.35))
      : Math.max(520, Math.floor(window.innerHeight * 0.82));
    window.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return {
      ok: pageState().ok,
      before,
      after: window.scrollY,
      before_count: beforeCount,
      result_count: tweetArticles().length,
      at_end: window.innerHeight + window.scrollY >= document.documentElement?.scrollHeight - 8,
    };
  }

  function scrollResults(arg) { return scrollList(arg); }
  function scrollPosts(arg) { return scrollList(arg); }

  function profileDetail() {
    const route = profileRoute(location.href);
    const state = pageState();
    if (!route) return { ok: false, status: 'not_profile', url: location.href, page_state: state };
    const username = route.username;
    const primary = document.querySelector('[data-testid="primaryColumn"]') || document.querySelector('main') || document;
    const nameRoot = primary.querySelector('[data-testid="UserName"]');
    const description = primary.querySelector('[data-testid="UserDescription"]');
    const text = cleanText(nameRoot, 1000);
    const lines = text.split('\n').filter(Boolean);
    const linkText = (suffix) => {
      const link = Array.from(primary.querySelectorAll('a[href]')).find((node) => {
        try {
          const parts = new URL(node.getAttribute('href') || node.href, location.href).pathname.split('/').filter(Boolean);
          return parts.length === 2 && parts[0].toLowerCase() === username && parts[1].toLowerCase() === suffix;
        } catch (_) {
          return false;
        }
      });
      return cleanText(link, 200);
    };
    const header = headerFollow(username);
    const postsReady = tweetArticles(primary).length;
    const timelineEmpty = postsReady === 0 && /No (?:posts|reposts|replies|likes|media)|hasn.?t (?:posted|reposted|liked)|还没有|没有转发|没有回复|没有喜欢/i.test(cleanText(primary, 4000));
    const contentReady = !!(nameRoot && (postsReady > 0 || timelineEmpty));
    return {
      ok: state.ok && contentReady,
      status: !state.ok ? state.status : contentReady ? 'profile' : 'unhydrated',
      id: username,
      username,
      tab: route.tab,
      display_name: lines.find((line) => !line.startsWith('@')) || '',
      bio: cleanText(description, 5000),
      followers: metric(linkText('followers')),
      following: metric(linkText('following')),
      viewer_following: header.ok ? header.following : null,
      url: xUrl(location.href),
      visible_post_count: tweetArticles(primary).length,
      page_state: state,
    };
  }

  function likeTarget(arg) {
    const expected = cleanText(arg && (arg.post_id || arg.id) || '', 100);
    if (!/^\d+$/.test(expected)) return { ok: false, status: 'invalid_post_id', liked: false };
    const active = statusIdentity(location.href);
    if (!active || active.id !== expected) return { ok: false, status: active ? 'wrong_post' : 'not_post', liked: false };
    const article = activePostArticle();
    if (!article) return { ok: false, status: 'post_unavailable', post_id: expected, liked: false };
    const buttons = Array.from(article.querySelectorAll('[data-testid="like"], [data-testid="unlike"]'))
      .filter((button) => article.contains(button) && !button.closest('[data-testid="quoteTweet"]') && visible(button));
    if (buttons.length !== 1) {
      return { ok: false, status: buttons.length ? 'ambiguous_like_button' : 'like_button_not_found', post_id: expected, liked: false };
    }
    const button = buttons[0];
    const liked = button.getAttribute('data-testid') === 'unlike';
    const point = ownedPoint(button);
    const actor = currentUsername();
    return point
      ? { ok: true, status: liked ? 'liked' : 'like_ready', post_id: expected, liked, actor, hit_owned: true, text: cleanText(button, 80), ...point }
      : { ok: false, status: 'like_button_obscured', post_id: expected, liked, actor, hit_owned: false };
  }

  function headerFollow(username) {
    const primary = document.querySelector('[data-testid="primaryColumn"]') || document;
    const buttons = Array.from(primary.querySelectorAll('button[data-testid$="-follow"], button[data-testid$="-unfollow"]'))
      .filter((button) => {
        const testId = button.getAttribute('data-testid') || '';
        const aria = (button.getAttribute('aria-label') || '').toLowerCase();
        return /^\d+-(?:follow|unfollow)$/.test(testId)
          && !button.closest('[data-testid="UserCell"]')
          && aria.includes(`@${username}`)
          && visible(button);
      });
    if (buttons.length !== 1) {
      return {
        ok: false,
        status: buttons.length ? 'ambiguous_follow_button' : 'follow_button_not_found',
        username,
        following: false,
        button: null,
      };
    }
    const button = buttons[0];
    const following = (button.getAttribute('data-testid') || '').endsWith('-unfollow');
    return {
      ok: true,
      status: following ? 'following' : 'follow_ready',
      username,
      following,
      button,
    };
  }

  function followTarget(arg) {
    const username = cleanText(arg && (arg.username || arg.profile) || '', 40).replace(/^@/, '').toLowerCase();
    const route = profileRoute(location.href);
    if (!/^[a-z0-9_]{1,15}$/.test(username)) return { ok: false, status: 'invalid_username', following: false };
    if (!route || route.username !== username) return { ok: false, status: route ? 'wrong_profile' : 'not_profile', following: false };
    const header = headerFollow(username);
    if (!header.ok) return { ok: false, status: header.status, username, following: false };
    const point = ownedPoint(header.button);
    const actor = currentUsername();
    return point
      ? { ok: true, status: header.status, username, following: header.following, actor, hit_owned: true, text: cleanText(header.button, 80), ...point }
      : { ok: false, status: 'follow_button_obscured', username, following: header.following, actor, hit_owned: false };
  }

  function activePostArticle() {
    const active = statusIdentity(location.href);
    if (!active) return null;
    return tweetArticles().find((article) => articleIdentity(article)?.id === active.id) || null;
  }

  function postOpenState(arg) {
    const expected = cleanText(arg && (arg.id || arg.post_id) || '', 100);
    const state = pageState();
    const active = statusIdentity(location.href);
    const matches = !!active && /^\d+$/.test(expected) && active.id === expected;
    const article = matches ? activePostArticle() : null;
    const detail = article ? postDetail() : null;
    const authorRoot = article && firstVisible(article, ['[data-testid="User-Name"]']);
    const authorText = cleanText(authorRoot, 1000);
    const authorHandle = authorText.match(/@([A-Za-z0-9_]{1,15})/);
    const authorProfile = authorRoot && Array.from(authorRoot.querySelectorAll('a[href]'))
      .map((link) => profileUsername(link.href || link.getAttribute('href')))
      .find(Boolean);
    // Readiness must be proven by the visible author DOM. parseAuthor also
    // accepts the status URL as a best-effort list fallback, which is not
    // sufficient evidence that a just-opened post has hydrated.
    const author = authorProfile || authorHandle && authorHandle[1].toLowerCase() || '';
    const publishedAt = cleanText(detail && detail.published_at || '', 200);
    const hasContent = !!cleanText(detail && detail.text || '', 12000)
      || (Array.isArray(detail && detail.media) && detail.media.length > 0);
    const ready = !!detail && detail.ok === true && detail.id === expected
      && !!author && !!publishedAt && hasContent;
    let status = 'post_not_open';
    if (!/^\d+$/.test(expected)) status = 'invalid_post_id';
    else if (active && !matches) status = 'wrong_post';
    else if (matches && !ready) status = 'post_unhydrated';
    else if (ready) status = 'post_open';
    return {
      ok: ready && state.ok,
      status: state.ok ? status : state.status,
      expected_post_id: expected,
      post_id: active && active.id || '',
      url: location.href,
      content_ready: ready,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
    };
  }

  function postDetail() {
    const state = pageState();
    const active = statusIdentity(location.href);
    if (!active) return { ok: false, status: 'not_post', url: location.href, page_state: state };
    const article = activePostArticle();
    if (!article) return { ok: false, status: state.status === 'ready' ? 'unhydrated' : state.status, id: active.id, url: xUrl(location.href), page_state: state };
    const result = parseTweet(article, 1);
    return { ...result, status: 'post', page_state: state };
  }

  function comments(arg) {
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 25)));
    const active = statusIdentity(location.href);
    if (!active) return [];
    const allArticles = tweetArticles();
    const rootArticle = allArticles.find((article) => articleIdentity(article)?.id === active.id);
    const region = rootArticle && rootArticle.closest(
      '[aria-label*="conversation" i], [aria-label*="对话"], section[role="region"], [role="region"]',
    );
    if (!region) return [];
    const articles = tweetArticles(region);
    const rootIndex = articles.findIndex((article) => articleIdentity(article)?.id === active.id);
    if (rootIndex < 0) return [];
    const root = parseTweet(articles[rootIndex], 1);
    const rootHandle = root && root.author && root.author.username;
    if (!rootHandle) return [];
    const visibleReplyCount = root && root.metrics && root.metrics.replies;
    const inferredReplyBudget = Number.isFinite(visibleReplyCount)
      ? Math.max(0, Number(visibleReplyCount))
      : 0;
    const output = [];
    for (const article of articles.slice(rootIndex + 1)) {
      const replying = replyTargets(article);
      const explicitlyTargetsRoot = replying.includes(rootHandle);
      if (replying.length > 0 && !explicitlyTargetsRoot) continue;
      // Current X omits "Replying to" for direct root replies, including
      // same-author thread continuations. Retain those only while the root's
      // rendered reply metric proves that replies exist and bounds how many
      // unlabeled conversation rows may be classified. Explicit relationships
      // remain authoritative even when the metric is absent or stale.
      if (!explicitlyTargetsRoot && output.length >= inferredReplyBudget) continue;
      const item = parseTweet(article, output.length + 1);
      // Promoted rows do not expose a canonical timestamp/status link, so they
      // cannot satisfy this evidence contract even when interleaved between
      // the root and its replies.
      if (!item || !item.published_at || !laterSnowflake(item.id, active.id) ||
        output.some((entry) => entry.id === item.id)) continue;
      item.is_reply = true;
      item.replying_to = explicitlyTargetsRoot ? replying : [rootHandle];
      item.relationship_evidence = explicitlyTargetsRoot
        ? 'explicit_reply_target'
        : 'conversation_region_with_root_reply_budget';
      output.push(item);
      if (output.length >= limit) break;
    }
    return output;
  }

  function renderedReplyState(arg) {
    const expected = cleanText(arg && arg.text || '', 10000);
    const expectedPostId = cleanText(arg && (arg.post_id || arg.id) || '', 100);
    const active = statusIdentity(location.href);
    if (!active || !/^\d+$/.test(expectedPostId) || active.id !== expectedPostId || !expected) {
      const status = !active ? 'not_post' : !/^\d+$/.test(expectedPostId) ? 'invalid_post_id'
        : active.id !== expectedPostId ? 'wrong_post' : 'empty_text';
      return { ok: false, status, visible: false, count: 0, ids: [] };
    }
    const rootArticle = activePostArticle();
    const region = rootArticle && rootArticle.closest(
      '[aria-label*="conversation" i], [aria-label*="对话"], section[role="region"], [role="region"]',
    );
    const root = rootArticle && parseTweet(rootArticle, 1);
    const rootHandle = root && root.author && root.author.username;
    const signedInAs = currentUsername();
    if (!region || !rootHandle || !signedInAs) {
      return { ok: false, status: !signedInAs ? 'current_user_unknown' : 'conversation_not_found', visible: false, count: 0, ids: [] };
    }
    const matches = [];
    const candidates = [];
    const conversationArticles = tweetArticles(region);
    const rootIndex = conversationArticles.indexOf(rootArticle);
    for (const article of conversationArticles) {
      const identity = articleIdentity(article);
      const text = cleanText(article.querySelector('[data-testid="tweetText"]'), 10000);
      if (!identity || identity.id === active.id || text !== expected) continue;
      const parsed = parseTweet(article, 1);
      const replyingTo = replyTargets(article);
      const inConversation = region.contains?.(article) === true;
      const isVisible = visible(article);
      const position = conversationArticles.indexOf(article);
      // X omits "Replying to" for some direct root replies. In that case the
      // conversation timeline order is the rendered relationship signal. If an
      // explicit target exists, it must name the active post's author.
      const related = replyingTo.length > 0
        ? replyingTo.includes(rootHandle)
        : rootIndex >= 0 && position > rootIndex;
      candidates.push({
        id: identity.id,
        author: parsed && parsed.author.username || '',
        replying_to: replyingTo,
        in_conversation: inConversation,
        visible: isVisible,
        related,
      });
      if (inConversation && isVisible && related &&
        parsed && parsed.author.username === signedInAs) matches.push(identity.id);
    }
    return {
      ok: true,
      status: matches.length ? 'visible' : 'not_visible',
      visible: matches.length > 0,
      count: matches.length,
      ids: matches,
      exact_candidates: candidates,
      author: signedInAs,
      post_id: active.id,
    };
  }

  function normalizeLabel(value) {
    return cleanText(value, 80).toLowerCase().replace(/[\s_-]+/g, '');
  }

  function timelineSurface() {
    const path = location.pathname;
    if (/^\/(?:home|explore)(?:\/|$)/i.test(path) || path === '/') return 'feed';
    if (/^\/search(?:\/|$)/i.test(path)) return 'search';
    if (profileRoute(location.href)) return 'profile';
    return '';
  }

  function timelineState() {
    const state = pageState();
    const surface = timelineSurface();
    return {
      ok: !!surface && state.ok,
      status: surface ? state.status : 'not_timeline',
      surface,
      url: location.href,
      path: location.pathname,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
    };
  }

  function homeTabs() {
    return Array.from(primaryColumn().querySelectorAll('[role="tab"]')).filter((tab) => visible(tab));
  }

  function feedState(arg) {
    const state = pageState();
    const onHome = /^\/home(?:\/|$)/i.test(location.pathname);
    const expected = normalizeLabel(arg && arg.tab || '');
    const tabs = homeTabs();
    const selected = tabs.find((tab) => tab.getAttribute('aria-selected') === 'true');
    const tab = selected ? cleanText(selected, 80) : '';
    const articles = onHome ? tweetArticles(primaryColumn()) : [];
    const first = articles.length ? articleIdentity(articles[0]) : null;
    const tabMatches = !expected || normalizeLabel(tab) === expected;
    let status = 'results';
    if (!onHome) status = 'not_home';
    else if (!state.ok) status = state.status;
    else if (!tab) status = 'tab_not_found';
    else if (!tabMatches) status = 'tab_mismatch';
    else if (!articles.length) status = 'unhydrated';
    return {
      ok: status === 'results',
      status,
      tab,
      tabs: tabs.map((item) => cleanText(item, 80)),
      first_id: first ? first.id : '',
      result_count: articles.length,
      url: location.href,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
    };
  }

  function feedTabTarget(arg) {
    const expected = normalizeLabel(arg && arg.tab || '');
    if (!expected) return { ok: false, status: 'invalid_tab' };
    if (!/^\/home(?:\/|$)/i.test(location.pathname)) return { ok: false, status: 'not_home', url: location.href };
    const matches = homeTabs().filter((tab) => normalizeLabel(tab) === expected);
    if (matches.length !== 1) {
      return {
        ok: false,
        status: matches.length ? 'ambiguous_tab' : 'tab_not_found',
        tabs: homeTabs().map((tab) => cleanText(tab, 80)),
      };
    }
    const tab = matches[0];
    const point = ownedPoint(tab);
    return point
      ? {
        ok: true,
        status: 'tab_ready',
        tab: cleanText(tab, 80),
        selected: tab.getAttribute('aria-selected') === 'true',
        hit_owned: true,
        ...point,
      }
      : { ok: false, status: 'tab_obscured', tab: cleanText(tab, 80), hit_owned: false };
  }

  function feedPosts(arg) { return listTweets(arg, primaryColumn()); }

  function streamArticles() {
    return tweetArticles(primaryColumn()).filter((article) => !article.closest?.('[role="dialog"]'));
  }

  function streamArticle(postId) {
    const matches = streamArticles().filter((article) => articleIdentity(article)?.id === postId);
    return matches.length === 1 ? matches[0] : null;
  }

  function articleReplyButton(article) {
    return Array.from(article.querySelectorAll('[data-testid="reply"]'))
      .find((button) => !button.closest?.('[data-testid="quoteTweet"]')) || null;
  }

  function streamReplyTarget(arg) {
    const postId = cleanText(arg && (arg.post_id || arg.id) || '', 40);
    if (!/^\d+$/.test(postId)) return { ok: false, status: 'invalid_post_id' };
    const surface = timelineSurface();
    if (!surface) return { ok: false, status: 'not_timeline', url: location.href };
    const article = streamArticle(postId);
    if (!article) return { ok: false, status: 'post_not_found', post_id: postId, surface };
    const parsed = parseTweet(article, 1);
    const button = articleReplyButton(article);
    if (!button) return { ok: false, status: 'reply_button_not_found', post_id: postId, surface };
    if (!inViewport(button)) return { ok: false, status: 'reply_button_not_visible', post_id: postId, surface };
    const point = ownedPoint(button);
    return point && parsed
      ? {
        ok: true,
        status: 'reply_ready',
        post_id: postId,
        surface,
        username: parsed.author.username,
        text: parsed.text,
        published_at: parsed.published_at,
        actor: currentUsername(),
        hit_owned: true,
        ...point,
      }
      : { ok: false, status: 'reply_button_obscured', post_id: postId, surface, hit_owned: false };
  }

  function visibleDialogs() {
    return Array.from(document.querySelectorAll('[role="dialog"]')).filter((dialog) => {
      const rect = dialog.getBoundingClientRect();
      return visible(dialog) && rect.width > 40 && rect.height > 40;
    });
  }

  function overlayDialog(arg) {
    const dialogs = visibleDialogs();
    if (dialogs.length !== 1) {
      return { error: dialogs.length ? 'ambiguous_reply_overlay' : 'reply_overlay_not_found' };
    }
    const dialog = dialogs[0];
    const article = dialog.querySelector('[data-testid="tweet"]');
    const username = article ? parseAuthor(article, { username: '', id: '' }).username : '';
    const text = cleanText(article && article.querySelector('[data-testid="tweetText"]'), 30000);
    const time = article && article.querySelector('time[datetime]');
    const published = time && (time.dateTime || time.getAttribute('datetime')) || '';
    const expectedUser = cleanText(arg && arg.username || '', 40).toLowerCase();
    const expectedText = cleanText(arg && arg.text || '', 30000);
    const expectedTime = cleanText(arg && arg.published_at || '', 80);
    if (!article || !expectedUser || username !== expectedUser) return { error: 'wrong_reply_target' };
    if (expectedTime && published && published !== expectedTime) return { error: 'wrong_reply_target' };
    if (!text || (expectedText && text !== expectedText && !expectedText.startsWith(text))) {
      return { error: 'wrong_reply_target' };
    }
    return { dialog, username, text, published_at: published };
  }

  function overlayEditor(arg) {
    const found = overlayDialog(arg);
    if (found.error) return found;
    const editors = Array.from(found.dialog.querySelectorAll(
      '[data-testid="tweetTextarea_0"][contenteditable="true"]',
    )).filter((editor) => visible(editor) && inViewport(editor));
    return { ...found, editor: editors.length === 1 ? editors[0] : null, count: editors.length };
  }

  function overlayReplyEditorTarget(arg) {
    const found = overlayEditor(arg);
    if (found.error) return { ok: false, status: found.error };
    if (!found.editor) {
      return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found' };
    }
    const point = ownedPoint(found.editor);
    return point
      ? { ok: true, status: 'reply_editor_ready', username: found.username, hit_owned: true, ...point }
      : { ok: false, status: 'reply_editor_obscured', username: found.username, hit_owned: false };
  }

  function overlayReplyDraftState(arg) {
    const found = overlayEditor(arg);
    if (found.error) return { ok: false, status: found.error, value: '' };
    if (!found.editor) {
      return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found', value: '' };
    }
    const active = document.activeElement;
    return {
      ok: true,
      status: 'reply_editor_ready',
      username: found.username,
      focused: active === found.editor || found.editor.contains?.(active),
      value: editableText(found.editor, 10000),
    };
  }

  function overlayReplySubmitTarget(arg) {
    const found = overlayEditor(arg);
    if (found.error) return { ok: false, status: found.error };
    if (!found.editor) {
      return { ok: false, status: found.count > 1 ? 'ambiguous_reply_editor' : 'reply_editor_not_found' };
    }
    const buttons = Array.from(found.dialog.querySelectorAll('[data-testid="tweetButton"], [data-testid="tweetButtonInline"]'))
      .map((node) => node.closest?.('button, [role="button"]') || node)
      .filter((button) => visible(button) && inViewport(button));
    const unique = [];
    for (const button of buttons) if (!unique.includes(button)) unique.push(button);
    if (unique.length !== 1) {
      return { ok: false, status: unique.length ? 'ambiguous_reply_submit' : 'reply_submit_not_found' };
    }
    const button = unique[0];
    const point = ownedPoint(button);
    const disabled = !!button.disabled || button.getAttribute('aria-disabled') === 'true';
    return {
      ok: !disabled && !!point,
      status: disabled ? 'reply_submit_disabled' : point ? 'reply_submit_ready' : 'reply_submit_obscured',
      username: found.username,
      text: cleanText(button, 40),
      disabled,
      hit_owned: !!point,
      ...(point || {}),
    };
  }

  function overlayCloseTarget() {
    const dialogs = visibleDialogs();
    if (dialogs.length !== 1) {
      return { ok: false, status: dialogs.length ? 'ambiguous_reply_overlay' : 'reply_overlay_not_found' };
    }
    const button = dialogs[0].querySelector('[data-testid="app-bar-close"]');
    if (!button || !inViewport(button)) return { ok: false, status: 'overlay_close_not_found' };
    const point = ownedPoint(button);
    return point
      ? { ok: true, status: 'overlay_close_ready', hit_owned: true, ...point }
      : { ok: false, status: 'overlay_close_obscured', hit_owned: false };
  }

  function overlayClosed(arg) {
    const expectedPath = cleanText(arg && arg.path || '', 200);
    const dialogs = visibleDialogs().length;
    const path = location.pathname;
    const graduated = /graduated-access/i.test(path + location.search);
    const compose = /\/compose\/(?:post|reply)/i.test(path);
    const back = !!expectedPath && path === expectedPath;
    let status = 'returned';
    if (graduated) status = 'graduated_access';
    else if (dialogs || compose) status = 'overlay_open';
    else if (!back) status = 'left_timeline';
    return {
      ok: status === 'returned',
      status,
      url: location.href,
      path,
      dialogs,
    };
  }

  function streamNameLinks(username) {
    return Array.from(primaryColumn().querySelectorAll('[data-testid="User-Name"] a[href]')).filter((link) => {
      if (profileUsername(link.href || link.getAttribute('href')) !== username) return false;
      return !link.closest?.('[role="dialog"]') && !link.closest?.('[data-testid="HoverCard"]');
    });
  }

  function streamNameTarget(arg) {
    const username = cleanText(arg && arg.username || '', 40).toLowerCase();
    if (!/^[a-z0-9_]{1,15}$/.test(username)) return { ok: false, status: 'invalid_username' };
    if (!timelineSurface()) return { ok: false, status: 'not_timeline', url: location.href };
    const links = streamNameLinks(username);
    if (!links.length) return { ok: false, status: 'name_not_found', username };
    const visibleLink = links.find((link) => inViewport(link));
    if (!visibleLink) return { ok: false, status: 'name_not_visible', username };
    const point = ownedPoint(visibleLink);
    return point
      ? { ok: true, status: 'name_ready', username, hit_owned: true, ...point }
      : { ok: false, status: 'name_obscured', username, hit_owned: false };
  }

  function visibleHoverCard() {
    const cards = Array.from(document.querySelectorAll('[data-testid="HoverCard"]')).filter((card) => {
      const rect = card.getBoundingClientRect();
      return visible(card) && rect.width > 40 && rect.height > 40;
    });
    return cards.length === 1 ? cards[0] : { count: cards.length };
  }

  function hoverCount(card, username, kind) {
    const link = Array.from(card.querySelectorAll('a[href]')).find((node) => {
      try {
        const parts = new URL(node.getAttribute('href') || node.href, location.href).pathname.split('/').filter(Boolean);
        if ((parts[0] || '').toLowerCase() !== username) return false;
        const leaf = (parts[1] || '').toLowerCase();
        return kind === 'following' ? leaf === 'following' : leaf === 'followers' || leaf === 'verified_followers';
      } catch (_) {
        return false;
      }
    });
    return link ? metric(link) : null;
  }

  function hoverBio(card, username, displayName) {
    const described = card.querySelector('[data-testid="UserDescription"], [data-testid="UserBio"]');
    const direct = cleanText(described, 1000);
    if (direct) return direct;
    const skipped = new Set([
      'follow', 'following', 'profile summary',
      displayName.toLowerCase(), `@${username}`, username,
    ]);
    return cleanText(card, 4000).split('\n').map((line) => line.trim()).filter((line) => {
      const lower = line.toLowerCase();
      if (!line || skipped.has(lower)) return false;
      if (/^followed by /i.test(line) || /^click to follow /i.test(line)) return false;
      if (/^[\d,.]+$/.test(line)) return false;
      if (/followers|following/i.test(line) && /\d/.test(line)) return false;
      return true;
    }).slice(0, 4).join('\n');
  }

  function hoverCardState(arg) {
    const username = cleanText(arg && arg.username || '', 40).toLowerCase();
    const card = visibleHoverCard();
    if (!card || card.count !== undefined) {
      return { ok: false, status: card && card.count ? 'ambiguous_hover_card' : 'hover_card_not_found', username };
    }
    const name = Array.from(card.querySelectorAll('a[href]')).find((link) => {
      const handle = profileUsername(link.href || link.getAttribute('href'));
      return handle === username && cleanText(link, 80) && !cleanText(link, 80).startsWith('@');
    });
    const handle = Array.from(card.querySelectorAll('a[href]')).some((link) => (
      profileUsername(link.href || link.getAttribute('href')) === username
    ));
    if (!handle) return { ok: false, status: 'wrong_hover_card', username };
    const displayName = name ? cleanText(name, 80) : '';
    const buttons = Array.from(card.querySelectorAll('button')).filter((button) => (
      /^\d+-(?:follow|unfollow)$/.test(button.getAttribute('data-testid') || '')
    ));
    const button = buttons.length === 1 ? buttons[0] : null;
    const aria = button ? (button.getAttribute('aria-label') || '') : '';
    const following = !!button && (button.getAttribute('data-testid') || '').endsWith('-unfollow');
    return {
      ok: true,
      status: 'hover_card_ready',
      username,
      display_name: displayName,
      bio: hoverBio(card, username, displayName),
      following_count: hoverCount(card, username, 'following'),
      followers_count: hoverCount(card, username, 'followers'),
      following,
      follow_status: !button ? (buttons.length ? 'ambiguous_follow_button' : 'follow_button_not_found')
        : (aria.toLowerCase().includes(`@${username}`) ? (following ? 'following' : 'follow_ready') : 'wrong_follow_button'),
      actor: currentUsername(),
    };
  }

  function hoverFollowTarget(arg) {
    const cardState = hoverCardState(arg);
    if (!cardState.ok) return cardState;
    if (cardState.follow_status !== 'following' && cardState.follow_status !== 'follow_ready') {
      return { ok: false, status: cardState.follow_status, username: cardState.username, following: false };
    }
    const card = visibleHoverCard();
    const button = Array.from(card.querySelectorAll('button')).find((node) => (
      /^\d+-(?:follow|unfollow)$/.test(node.getAttribute('data-testid') || '')
    ));
    const point = button && ownedPoint(button);
    const following = (button.getAttribute('data-testid') || '').endsWith('-unfollow');
    return point
      ? {
        ok: true,
        status: following ? 'following' : 'follow_ready',
        username: cardState.username,
        following,
        actor: cardState.actor,
        hit_owned: true,
        text: cleanText(button, 40),
        ...point,
      }
      : { ok: false, status: 'follow_button_obscured', username: cardState.username, following, hit_owned: false };
  }

  function homeLinkTarget() {
    const home = document.querySelector('[data-testid="AppTabBar_Home_Link"]');
    const point = home && ownedPoint(home);
    return point
      ? { ok: true, status: 'home_link_ready', hit_owned: true, ...point }
      : { ok: false, status: 'home_link_not_found', hit_owned: false };
  }

  function navLinkTarget(arg) {
    const name = cleanText(arg && arg.name || '', 40).toLowerCase();
    const testid = name === 'notifications' ? 'AppTabBar_Notifications_Link'
      : name === 'profile' ? 'AppTabBar_Profile_Link'
        : name === 'home' ? 'AppTabBar_Home_Link' : '';
    if (!testid) return { ok: false, status: 'invalid_nav', name, hit_owned: false };
    const link = document.querySelector(`[data-testid="${testid}"]`);
    const username = name === 'profile' && link
      ? profileUsername(link.href || link.getAttribute('href')) : '';
    const point = link && ownedPoint(link);
    return point
      ? { ok: true, status: 'nav_ready', name, username, hit_owned: true, ...point }
      : { ok: false, status: 'nav_not_found', name, username, hit_owned: false };
  }

  function notificationArticles() {
    return Array.from(primaryColumn().querySelectorAll(
      'article[data-testid="tweet"], article[data-testid="notification"]',
    ));
  }

  function avatarUsernames(article) {
    const names = [];
    for (const node of article.querySelectorAll('[data-testid^="UserAvatar-Container-"]')) {
      const username = String(node.getAttribute('data-testid') || '')
        .slice('UserAvatar-Container-'.length).toLowerCase();
      if (/^[a-z0-9_]{1,15}$/.test(username) && !names.includes(username)) names.push(username);
    }
    return names.slice(0, 30);
  }

  function notificationKind(article) {
    const testid = article.getAttribute('data-testid') || '';
    const text = cleanText(article, 800);
    if (testid === 'notification' || !article.querySelector('a[href*="/status/"]')) {
      if (/followed you/i.test(text)) return 'follow';
      if (/\bliked\b/i.test(text)) return 'like';
      return 'other';
    }
    return replyTargets(article).length ? 'reply' : 'mention';
  }

  function notificationsTabs() {
    if (!/^\/notifications\/?$/i.test(location.pathname)) return [];
    return Array.from(primaryColumn().querySelectorAll('[role="tab"]')).filter((tab) => visible(tab));
  }

  function notificationsState(arg) {
    const state = pageState();
    const onPage = /^\/notifications\/?$/i.test(location.pathname);
    const expected = normalizeLabel(arg && arg.tab || '');
    const tabs = notificationsTabs();
    const selected = tabs.find((tab) => tab.getAttribute('aria-selected') === 'true');
    const tab = selected ? cleanText(selected, 40) : '';
    const count = onPage ? notificationArticles().length : 0;
    let status = 'results';
    if (!onPage) status = 'not_notifications';
    else if (!state.ok) status = state.status;
    else if (!tab) status = 'tab_not_found';
    else if (expected && normalizeLabel(tab) !== expected) status = 'tab_mismatch';
    return {
      ok: status === 'results',
      status,
      tab,
      tabs: tabs.map((item) => cleanText(item, 40)),
      result_count: count,
      actor: currentUsername(),
      url: location.href,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
    };
  }

  function notificationsTabTarget(arg) {
    const expected = normalizeLabel(arg && arg.tab || '');
    if (!expected) return { ok: false, status: 'invalid_tab' };
    if (!/^\/notifications\/?$/i.test(location.pathname)) {
      return { ok: false, status: 'not_notifications', url: location.href };
    }
    const matches = notificationsTabs().filter((tab) => normalizeLabel(tab) === expected);
    if (matches.length !== 1) {
      return {
        ok: false,
        status: matches.length ? 'ambiguous_tab' : 'tab_not_found',
        tabs: notificationsTabs().map((tab) => cleanText(tab, 40)),
      };
    }
    const tab = matches[0];
    const point = ownedPoint(tab);
    return point
      ? {
        ok: true,
        status: 'tab_ready',
        tab: cleanText(tab, 40),
        selected: tab.getAttribute('aria-selected') === 'true',
        hit_owned: true,
        ...point,
      }
      : { ok: false, status: 'tab_obscured', tab: cleanText(tab, 40), hit_owned: false };
  }

  function notificationItems(arg) {
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 20)));
    const actor = currentUsername();
    const output = [];
    const seen = new Set();
    let position = 0;
    for (const article of notificationArticles()) {
      position += 1;
      const kind = notificationKind(article);
      const accounts = avatarUsernames(article);
      const time = article.querySelector('time[datetime]');
      const published = time && (time.dateTime || time.getAttribute('datetime')) || '';
      const tweet = kind === 'reply' || kind === 'mention' ? parseTweet(article, position) : null;
      const id = tweet && tweet.id
        ? tweet.id
        : `${kind}:${published}:${accounts.join(',')}`;
      if (!id || id.endsWith(':') || seen.has(id)) continue;
      seen.add(id);
      const liked = !!article.querySelector('[data-testid="unlike"]');
      if (tweet) {
        output.push({
          ...tweet,
          kind,
          liked,
          replying_to: replyTargets(article),
          accounts,
          actor,
          position,
        });
      } else {
        const username = accounts[0] || '';
        output.push({
          ok: true,
          kind,
          id,
          url: '',
          author: {
            username,
            display_name: '',
            url: username ? xUrl(`https://x.com/${username}`) : '',
          },
          text: cleanText(article, 1000),
          published_at: published,
          liked,
          replying_to: [],
          accounts,
          actor,
          position,
        });
      }
      if (output.length >= limit) break;
    }
    return output;
  }

  function followersUsername() {
    const parts = location.pathname.split('/').filter(Boolean);
    if (parts.length !== 2 || parts[1].toLowerCase() !== 'followers') return '';
    const username = parts[0].toLowerCase();
    return /^[a-z0-9_]{1,15}$/.test(username) && !RESERVED.has(username) ? username : '';
  }

  function followersState() {
    const state = pageState();
    const username = followersUsername();
    const count = username
      ? primaryColumn().querySelectorAll('[data-testid="UserCell"]').length : 0;
    let status = 'results';
    if (!username) status = 'not_followers';
    else if (!state.ok) status = state.status;
    else if (!count) status = 'unhydrated';
    return {
      ok: status === 'results',
      status,
      username,
      result_count: count,
      actor: currentUsername(),
      url: location.href,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
    };
  }

  function followerPath(link, actor) {
    try {
      const parts = new URL(link.href || link.getAttribute('href'), location.href)
        .pathname.split('/').filter(Boolean);
      if (parts.length !== 2 || parts[0].toLowerCase() !== actor) return '';
      return parts[1].toLowerCase();
    } catch (_) {
      return '';
    }
  }

  function followersLinkTarget() {
    const actor = currentUsername();
    if (!actor) return { ok: false, status: 'actor_not_found', hit_owned: false };
    const links = Array.from(document.querySelectorAll('a[href]')).filter((link) => {
      if (!visible(link) || link.getAttribute('role') === 'tab') return false;
      if (!/\bfollowers\b|粉丝/i.test(cleanText(link, 40))) return false;
      return /followers$/.test(followerPath(link, actor));
    });
    const link = links[0];
    const point = link && ownedPoint(link);
    return point
      ? { ok: true, status: 'followers_link_ready', username: actor, hit_owned: true, ...point }
      : { ok: false, status: 'followers_link_not_found', username: actor, hit_owned: false };
  }

  function followersTabTarget() {
    const actor = currentUsername();
    if (!actor) return { ok: false, status: 'actor_not_found', hit_owned: false };
    const matches = Array.from(primaryColumn().querySelectorAll('a[role="tab"][href]'))
      .filter((tab) => visible(tab) && followerPath(tab, actor) === 'followers');
    if (matches.length !== 1) {
      return { ok: false, status: matches.length ? 'ambiguous_tab' : 'tab_not_found', username: actor };
    }
    const tab = matches[0];
    const point = ownedPoint(tab);
    return point
      ? {
        ok: true,
        status: 'tab_ready',
        username: actor,
        selected: tab.getAttribute('aria-selected') === 'true',
        hit_owned: true,
        ...point,
      }
      : { ok: false, status: 'tab_obscured', username: actor, hit_owned: false };
  }

  function followBackCandidates(arg) {
    const limit = Math.min(100, Math.max(1, Number(arg && arg.limit || 20)));
    if (!followersUsername()) return [];
    const output = [];
    const seen = new Set();
    for (const cell of primaryColumn().querySelectorAll('[data-testid="UserCell"]')) {
      const text = cleanText(cell, 2000);
      const handle = text.match(/@([A-Za-z0-9_]{1,15})/);
      const username = handle ? handle[1].toLowerCase() : '';
      if (!username || seen.has(username) || RESERVED.has(username)) continue;
      const button = Array.from(cell.querySelectorAll('button')).find((node) => (
        /^\d+-(?:follow|unfollow)$/.test(node.getAttribute('data-testid') || '')
      ));
      if (!button) continue;
      const following = (button.getAttribute('data-testid') || '').endsWith('-unfollow');
      const label = cleanText(button, 40);
      if (following || !/follow back|回关/i.test(label)) continue;
      seen.add(username);
      const lines = text.split('\n').map((line) => line.trim()).filter(Boolean);
      output.push({
        id: username,
        username,
        display_name: lines.find((line) => !line.startsWith('@') && !/follow/i.test(line)) || '',
        bio: cleanText(cell.querySelector('[data-testid="UserDescription"]'), 500),
        follows_you: /follows you|关注了你/i.test(text),
        following: false,
        position: output.length + 1,
      });
      if (output.length >= limit) break;
    }
    return output;
  }

  function hoverDismissTarget() {
    const home = homeLinkTarget();
    if (!home.ok) return { ok: false, status: 'dismiss_point_not_found', hit_owned: false };
    return { ...home, status: 'dismiss_ready' };
  }

  function hoverCardPresence() {
    const card = visibleHoverCard();
    const open = !!card && card.count === undefined;
    const count = open ? 1 : card && card.count || 0;
    return { ok: count === 0, status: count ? 'hover_card_open' : 'hover_card_closed', count };
  }

  function homeInlineRoot() {
    if (!/^\/home\/?$/i.test(location.pathname)) return { error: 'not_home' };
    const primary = document.querySelector('[data-testid="primaryColumn"]');
    if (!primary) return { error: 'composer_not_found' };
    return { primary };
  }

  function homeInlineEditors(primary) {
    return Array.from(primary.querySelectorAll(
      '[data-testid="tweetTextarea_0"][contenteditable="true"]',
    )).filter((editor) => visible(editor) && !editor.closest?.('[role="dialog"]')
      && editor.getAttribute('aria-disabled') !== 'true');
  }

  function homeInlineButton(primary) {
    const buttons = Array.from(primary.querySelectorAll('[data-testid="tweetButtonInline"]'))
      .map((node) => node.closest?.('button, [role="button"]') || node)
      .filter((button) => visible(button) && !button.closest?.('[role="dialog"]'));
    const unique = [];
    for (const button of buttons) if (!unique.includes(button)) unique.push(button);
    return unique.length === 1 ? unique[0] : null;
  }

  function postComposerTarget() {
    const root = homeInlineRoot();
    const actor = currentUsername();
    if (root.error) return { ok: false, status: root.error, actor };
    const editors = homeInlineEditors(root.primary);
    if (editors.length !== 1) {
      return { ok: false, status: editors.length ? 'ambiguous_composer' : 'composer_not_found', actor };
    }
    const point = ownedPoint(editors[0]);
    return point
      ? { ok: true, status: 'post_editor_ready', actor, hit_owned: true, ...point }
      : { ok: false, status: 'post_editor_obscured', actor, hit_owned: false };
  }

  function postDraftState() {
    const root = homeInlineRoot();
    const actor = currentUsername();
    if (root.error) return { ok: false, status: root.error, actor, value: '' };
    const editors = homeInlineEditors(root.primary);
    if (editors.length !== 1) {
      return { ok: false, status: editors.length ? 'ambiguous_composer' : 'composer_not_found', actor, value: '' };
    }
    const editor = editors[0];
    const active = document.activeElement;
    return {
      ok: true,
      status: 'post_editor_ready',
      actor,
      focused: active === editor || editor.contains?.(active),
      value: editableText(editor, 10000),
    };
  }

  function postSubmitTarget() {
    const root = homeInlineRoot();
    const actor = currentUsername();
    if (root.error) return { ok: false, status: root.error, actor };
    const button = homeInlineButton(root.primary);
    if (!button) return { ok: false, status: 'post_submit_not_found', actor };
    const point = ownedPoint(button);
    const disabled = !!button.disabled || button.getAttribute('aria-disabled') === 'true';
    return {
      ok: !disabled && !!point,
      status: disabled ? 'post_submit_disabled' : point ? 'post_submit_ready' : 'post_submit_obscured',
      actor,
      text: cleanText(button, 40),
      disabled,
      hit_owned: !!point,
      ...(point || {}),
    };
  }

  function postCloseTarget() {
    return { ok: false, status: 'post_close_not_found', hit_owned: false };
  }

  function renderedPostState(arg) {
    const text = cleanText(arg && arg.text || '', 10000);
    const actor = currentUsername();
    const state = pageState();
    const path = location.pathname + location.search;
    if (/graduated-access/i.test(path)) {
      return { ok: false, status: 'graduated_access', actor, ids: [], url: location.href, composer_open: false };
    }
    if (!actor) return { ok: false, status: 'current_user_unknown', actor: '', ids: [], url: location.href };
    const ids = [];
    for (const article of tweetArticles()) {
      const parsed = parseTweet(article, ids.length + 1);
      if (!parsed || parsed.author.username !== actor || parsed.text.trim() !== text) continue;
      if (!ids.includes(parsed.id)) ids.push(parsed.id);
    }
    const composer = homeInlineRoot();
    return {
      ok: state.ok || ids.length > 0,
      status: ids.length ? 'visible' : state.ok ? 'ready' : state.status,
      actor,
      ids,
      count: ids.length,
      composer_open: !composer.error,
      url: location.href,
    };
  }

  async function scrollComments() {
    if (!statusIdentity(location.href)) return { ok: false, status: 'not_post', url: location.href };
    const before = comments({ limit: 100 }).length;
    const beforeY = window.scrollY;
    const delta = Math.max(520, Math.floor(window.innerHeight * 0.8));
    window.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    await new Promise((resolve) => setTimeout(resolve, 900));
    const after = comments({ limit: 100 }).length;
    const state = pageState();
    return {
      ok: state.ok,
      status: state.status,
      before,
      after,
      grew: after > before,
      at_end: after === before && window.scrollY === beforeY,
      login_required: state.login_required,
      challenge_required: state.challenge_required,
      rate_limited: state.rate_limited,
      url: location.href,
    };
  }

  window.SocaiXPageScripts = Object.freeze({
    pageState,
    sourceSurfaceState,
    searchState,
    searchInputTarget,
    searchResults,
    searchPeople,
    searchLists,
    searchMedia,
    postLinkTarget,
    scrollResults,
    profileDetail,
    profilePosts,
    scrollPosts,
    postOpenState,
    postDetail,
    comments,
    renderedReplyState,
    scrollComments,
    replyEditorTarget,
    replyDraftState,
    replySubmitTarget,
    likeTarget,
    followTarget,
    timelineState,
    feedState,
    feedTabTarget,
    feedPosts,
    streamReplyTarget,
    overlayReplyEditorTarget,
    overlayReplyDraftState,
    overlayReplySubmitTarget,
    overlayCloseTarget,
    overlayClosed,
    postComposerTarget,
    postDraftState,
    postSubmitTarget,
    postCloseTarget,
    renderedPostState,
    streamNameTarget,
    hoverCardState,
    hoverFollowTarget,
    homeLinkTarget,
    navLinkTarget,
    notificationsState,
    notificationsTabTarget,
    notificationItems,
    followersState,
    followersLinkTarget,
    followersTabTarget,
    followBackCandidates,
    hoverDismissTarget,
    hoverCardPresence,
  });
})();
