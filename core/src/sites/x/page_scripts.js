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
    if (profileUsername(location.href)) return 'profile';
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
      login_required: login,
      challenge_required: challenge,
      rate_limited: limited,
      hydrated,
      result_count: count,
    };
  }

  function sourceSurfaceState() {
    const state = pageState();
    const documentHeight = Math.max(
      Number(document.documentElement?.scrollHeight || 0),
      Number(document.body?.scrollHeight || 0),
    );
    const scrollY = Math.max(0, Math.round(window.scrollY || 0));
    const viewportHeight = Math.max(0, Math.round(window.innerHeight || 0));
    return {
      ...state,
      search_query: cleanText(new URL(location.href).searchParams.get('q') || '', 1000),
      profile_username: profileUsername(location.href),
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
    const onSearch = /^\/search(?:\/|$)/i.test(location.pathname);
    const count = tweetArticles().length;
    return {
      ...state,
      ok: state.ok && onSearch && (!expected || query.toLowerCase() === expected.toLowerCase()) && count > 0,
      status: !state.ok ? state.status : !onSearch ? 'not_search' : count > 0 ? 'results' : 'unhydrated',
      query,
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
    for (let node = editor.parentElement, depth = 0;
      node && node !== found.context.region && depth < 8;
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

  function listTweets(arg) {
    const input = arg || {};
    const limit = Math.min(100, Math.max(1, Number(input.limit || 25)));
    const viewportOnly = !!input.viewport_only;
    const output = [];
    const seen = new Set();
    for (const article of tweetArticles()) {
      if (viewportOnly && !inViewport(article)) continue;
      const item = parseTweet(article, output.length + 1);
      if (!item || seen.has(item.id)) continue;
      seen.add(item.id);
      output.push(item);
      if (output.length >= limit) break;
    }
    return output;
  }

  function searchResults(arg) { return listTweets(arg); }
  function profilePosts(arg) { return listTweets(arg); }

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
    const username = profileUsername(location.href);
    const state = pageState();
    if (!username) return { ok: false, status: 'not_profile', url: location.href, page_state: state };
    const primary = document.querySelector('[data-testid="primaryColumn"]') || document.querySelector('main') || document;
    const nameRoot = primary.querySelector('[data-testid="UserName"]');
    const description = primary.querySelector('[data-testid="UserDescription"]');
    const text = cleanText(nameRoot, 1000);
    const lines = text.split('\n').filter(Boolean);
    const linkText = (suffix) => {
      const link = primary.querySelector(`a[href="/${username}/${suffix}"], a[href$="/${suffix}"]`);
      return cleanText(link, 200);
    };
    const contentReady = !!(nameRoot || tweetArticles().length);
    return {
      ok: state.ok && contentReady,
      status: !state.ok ? state.status : contentReady ? 'profile' : 'unhydrated',
      id: username,
      username,
      display_name: lines.find((line) => !line.startsWith('@')) || '',
      bio: cleanText(description, 5000),
      followers: metric(linkText('followers')),
      following: metric(linkText('following')),
      url: xUrl(location.href),
      visible_post_count: tweetArticles().length,
      page_state: state,
    };
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
    const output = [];
    for (const article of articles.slice(rootIndex + 1)) {
      const replying = replyTargets(article);
      // Generic reads fail closed when X omits the relationship label. The
      // broad conversation region can also contain recommendations, so list
      // position alone is not enough authority to classify a direct reply.
      if (!replying.includes(rootHandle)) continue;
      const item = parseTweet(article, output.length + 1);
      if (!item || item.id === active.id || output.some((entry) => entry.id === item.id)) continue;
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
  });
})();
