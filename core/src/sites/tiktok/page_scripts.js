(function () {
  function text(node) {
    return (node && (node.innerText || node.textContent) || '').replace(/\s+/g, ' ').trim();
  }

  function normUrl(value) {
    if (!value) return '';
    try {
      return new URL(value, location.href).href;
    } catch (_) {
      return String(value || '');
    }
  }

  function visible(el) {
    if (!el || !el.getBoundingClientRect) return false;
    const rect = el.getBoundingClientRect();
    const style = window.getComputedStyle(el);
    return rect.width > 0 && rect.height > 0 && style.visibility !== 'hidden' && style.display !== 'none';
  }

  function inViewport(el) {
    if (!visible(el)) return false;
    const rect = el.getBoundingClientRect();
    return rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
  }

  function firstVisible(selectors) {
    for (const selector of selectors) {
      const node = Array.from(document.querySelectorAll(selector)).find(visible);
      if (node) return node;
    }
    return null;
  }

  function metaContent(property) {
    const node = document.querySelector(`meta[property="${property}"], meta[name="${property}"]`);
    return (node && node.getAttribute('content') || '').trim();
  }

  function videoIdFromUrl(url) {
    const match = String(url || '').match(/\/(?:video|player\/v1)\/([^/?#]+)/);
    return match ? match[1] : '';
  }

  function handleFromUrl(url) {
    const match = String(url || '').match(/\/@([^/?#]+)/);
    if (!match) return '';
    try {
      return decodeURIComponent(match[1]);
    } catch (_) {
      return '';
    }
  }

  function breadcrumbAuthor() {
    const videoId = videoIdFromUrl(location.href);
    if (!videoId) return { name: '', handle: '', url: '' };
    for (const script of document.querySelectorAll('script[type="application/ld+json"]')) {
      let data;
      try {
        data = JSON.parse(script.textContent || '');
      } catch (_) {
        continue;
      }
      const documents = Array.isArray(data) ? data : [data];
      for (const documentData of documents) {
        if (!documentData || documentData['@type'] !== 'BreadcrumbList') continue;
        const items = Array.isArray(documentData.itemListElement) ? documentData.itemListElement : [];
        const videoItem = items.find((entry) => {
          const item = entry && entry.item || {};
          return videoIdFromUrl(item['@id'] || item.url || '') === videoId;
        });
        if (!videoItem) continue;
        const authorEntry = items.find((entry) => Number(entry && entry.position) === 2);
        const item = authorEntry && authorEntry.item || {};
        const url = normUrl(item['@id'] || item.url || '');
        const handle = handleFromUrl(url);
        if (!handle) continue;
        const name = String(item.name || authorEntry.name || '')
          .replace(/\s*\(@[^)]+\)\s*\|\s*TikTok.*$/i, '')
          .trim();
        return { name, handle, url: normUrl(`/@${handle}`) };
      }
    }
    return { name: '', handle: '', url: '' };
  }

  function durationSeconds(raw) {
    const value = String(raw || '').trim();
    const parts = value.split(':').map((part) => Number(part));
    if (!parts.length || parts.some((part) => !Number.isFinite(part))) return 0;
    return parts.reduce((total, part) => total * 60 + part, 0);
  }

  function parseJsonScript(selector) {
    const node = document.querySelector(selector);
    if (!node) return null;
    try {
      return JSON.parse(node.textContent || '{}');
    } catch (_) {
      return null;
    }
  }

  function initialVideo(videoId) {
    const sigi = parseJsonScript('#SIGI_STATE');
    const sigiItem = sigi && sigi.ItemModule && sigi.ItemModule[videoId];
    if (sigiItem) return sigiItem;
    const universal = parseJsonScript('#__UNIVERSAL_DATA_FOR_REHYDRATION__');
    const scope = universal && universal.__DEFAULT_SCOPE__ || {};
    const detail = scope['webapp.video-detail'] || {};
    const item = detail.itemInfo && detail.itemInfo.itemStruct;
    return item && String(item.id || '') === String(videoId || '') ? item : null;
  }

  function initialUser(handle) {
    const universal = parseJsonScript('#__UNIVERSAL_DATA_FOR_REHYDRATION__');
    const scope = universal && universal.__DEFAULT_SCOPE__ || {};
    const detail = scope['webapp.user-detail'] || {};
    const info = detail.userInfo || {};
    const user = info.user || {};
    if (user.uniqueId && handle && String(user.uniqueId).toLowerCase() === String(handle).toLowerCase()) {
      return info;
    }
    return {};
  }

  function challengeRequired() {
    const bodyText = text(document.body);
    const challenge = firstVisible([
      '#captcha-verify-container',
      '.verify-captcha',
      '[class*="captcha"]',
      '[data-e2e*="captcha"]',
    ]);
    return !!challenge ||
      /drag the slider to fit the puzzle|complete the puzzle to continue|安全验证|拖动滑块|验证码/i.test(bodyText);
  }

  function loginBlocked(hasContent) {
    const bodyText = text(document.body);
    const dialog = firstVisible([
      '[role="dialog"]',
      '[data-e2e="login-modal"]',
      '[class*="login-modal"]',
      '[class*="LoginModal"]',
    ]);
    const explicitGate = /log in to (?:continue|watch|view)|sign in to (?:continue|watch|view)|登录后(?:查看|继续|浏览)/i.test(bodyText);
    const visibleGate = !!dialog && /log in|sign up|phone|email|登录|注册/i.test(text(dialog));
    return !hasContent && (explicitGate || visibleGate);
  }

  function hasUsefulBody() {
    return text(document.body).length > 20 || document.querySelectorAll('a[href], video, input, main').length > 0;
  }

  function pageState() {
    const bodyText = text(document.body);
    const hasVideo = !!document.querySelector('video, [data-e2e="browse-video"]');
    const hasCards = cardNodes().length > 0;
    return {
      ok: true,
      site: 'tiktok',
      url: location.href,
      title: document.title || '',
      ready_state: document.readyState,
      body_text_len: bodyText.length,
      blank_or_throttled: document.readyState === 'loading' || !hasUsefulBody(),
      login_required: loginBlocked(hasVideo || hasCards),
      challenge_required: challengeRequired(),
      has_video: hasVideo,
      card_count: hasCards ? cardNodes().length : 0,
    };
  }

  function cardNodes(root) {
    const scope = root && root.querySelectorAll ? root : document;
    const nodes = Array.from(scope.querySelectorAll(
      '[data-e2e="search-card-video"], [data-e2e="user-post-item"], [data-e2e="recommend-list-item-container"], a[href*="/video/"]'
    ));
    const cards = [];
    const seen = new Set();
    for (const node of nodes) {
      const link = node.matches('a[href*="/video/"]') ? node : node.querySelector('a[href*="/video/"]');
      const card = node.closest('[data-e2e="search-card-video"], [data-e2e="user-post-item"], [data-e2e="recommend-list-item-container"]') || link || node;
      if (!card || !link || !visible(card) || seen.has(card)) continue;
      seen.add(card);
      cards.push(card);
    }
    return cards;
  }

  function videoCards(arg) {
    const limit = Math.max(1, Number(arg && arg.limit || 30));
    const root = arg && arg.root && arg.root.querySelectorAll ? arg.root : document;
    const expectedHandle = String(arg && arg.author_id || '').replace(/^@/, '').trim().toLowerCase();
    const viewportOnly = !!(arg && arg.viewport_only);
    const cards = [];
    const seen = new Set();
    for (const card of cardNodes(root)) {
      if (viewportOnly && !inViewport(card)) continue;
      const link = card.matches('a[href*="/video/"]') ? card : card.querySelector('a[href*="/video/"]');
      const url = normUrl(link && (link.href || link.getAttribute('href')) || '');
      const videoId = videoIdFromUrl(url);
      if (!videoId || seen.has(videoId)) continue;
      seen.add(videoId);
      const img = card.querySelector('img');
      const allText = text(card);
      const authorLink = Array.from(card.querySelectorAll('a[href^="/@"], a[href*="tiktok.com/@"]'))
        .find((link) => !/\/video\//.test(link.getAttribute('href') || '')) || null;
      const authorUrl = normUrl(authorLink && (authorLink.href || authorLink.getAttribute('href')) || '');
      const handle = handleFromUrl(authorUrl || url);
      const profileUrl = handle ? normUrl(`/@${handle}`) : '';
      if (expectedHandle && handle.toLowerCase() !== expectedHandle) continue;
      const desc = text(card.querySelector('[data-e2e="search-card-desc"], [data-e2e="video-desc"], [class*="Desc"]'));
      const duration = text(card.querySelector('[data-e2e="video-duration"], time, [class*="Duration"]'));
      cards.push({
        video_id: videoId,
        url,
        title: desc || img && img.alt || allText,
        author: text(authorLink) || handle,
        author_id: handle,
        author_url: profileUrl,
        likes: text(card.querySelector('[data-e2e="like-count"], [data-e2e="search-card-like-container"]')),
        comments: text(card.querySelector('[data-e2e="comment-count"]')),
        shares: text(card.querySelector('[data-e2e="share-count"]')),
        views: text(card.querySelector('[data-e2e="video-views"], [class*="VideoViews"]')),
        cover_url: normUrl(img && (img.currentSrc || img.src) || ''),
        duration_seconds: durationSeconds(duration),
        position: cards.length,
      });
      if (cards.length >= limit) break;
    }
    return cards;
  }

  function allowedMediaUrl(raw) {
    try {
      const url = new URL(raw, location.href);
      if (url.protocol !== 'https:' || url.username || url.password || url.port) return '';
      if (url.pathname.toLowerCase().endsWith('.m3u8')) return '';
      const host = url.hostname.toLowerCase();
      const canonicalPage = /^\/@[^/]+\/video\/\d+\/?$/.test(url.pathname) ||
        /^\/player\/v1\/\d+\/?$/.test(url.pathname);
      if ((host === 'tiktok.com' || host.endsWith('.tiktok.com')) && canonicalPage) return '';
      const suffixes = [
        'tiktokcdn.com', 'tiktokcdn-us.com', 'tiktokv.com', 'tiktok.com',
        'byteoversea.com', 'ibytedtos.com', 'muscdn.com', 'akamaized.net',
      ];
      return suffixes.some((suffix) => host === suffix || host.endsWith(`.${suffix}`)) ? url.href : '';
    } catch (_) {
      return '';
    }
  }

  function collectVideoInfo(video, stateVideo, cover) {
    const candidates = [];
    const push = (raw, source) => {
      const url = allowedMediaUrl(raw || '');
      if (!url || candidates.some((item) => item.url === url)) return;
      candidates.push({ url, source });
    };
    const stateMedia = stateVideo && stateVideo.video || {};
    push(stateMedia.playAddr, 'initial_state.playAddr');
    push(stateMedia.downloadAddr, 'initial_state.downloadAddr');
    if (Array.isArray(stateMedia.bitrateInfo)) {
      for (const item of stateMedia.bitrateInfo) {
        const play = item && item.PlayAddr || {};
        push(play.UrlList && play.UrlList[0], 'initial_state.bitrate');
      }
    }
    if (video) {
      push(video.currentSrc, 'video.currentSrc');
      push(video.src, 'video.src');
      for (const source of video.querySelectorAll('source')) push(source.src || source.getAttribute('src'), 'source');
    }
    for (const entry of performance.getEntriesByType('resource')) {
      if (/mime_type=video_mp4|\.mp4(?:[?#]|$)|\/video\/tos\//i.test(entry.name || '')) {
        push(entry.name, 'performance.resource');
      }
    }
    push(metaContent('og:video'), 'meta.og:video');
    push(metaContent('og:video:url'), 'meta.og:video:url');
    const resolved = candidates.find((item) => /^https?:/.test(item.url) && !item.url.startsWith('blob:'));
    return {
      url: candidates[0] ? candidates[0].url : '',
      resolved_url: resolved ? resolved.url : '',
      poster_url: cover,
      source_urls: candidates.map((item) => item.url),
      candidates,
    };
  }

  function videoState() {
    const videoId = videoIdFromUrl(location.href);
    const stateVideo = initialVideo(videoId);
    const visibleVideos = Array.from(document.querySelectorAll('video')).filter(visible);
    const video = visibleVideos.length === 1 ? visibleVideos[0] : null;
    const videoSource = video && (video.currentSrc || video.src || (video.querySelector('source') && video.querySelector('source').src)) || '';
    const canvasPlayer = firstVisible([
      '[role="region"][aria-roledescription="video player"]',
      '[aria-label="TikTok video player"]',
    ]);
    const hasVideo = !!stateVideo || (!!video && (!!videoSource || video.readyState > 0)) || !!canvasPlayer;
    const detailNode = firstVisible([
      '[data-e2e="browse-video-desc"]',
      '[data-e2e="browse-username"]',
      '[data-e2e="video-desc"]',
    ]);
    const detailText = text(detailNode) || metaContent('og:description');
    const canonicalVideoLink = videoId && document.querySelector(`a[href*="/video/${videoId}"]`);
    const hasDetail = !!stateVideo || (hasVideo && (detailText.length > 0 || !!canonicalVideoLink));
    const unavailableNode = firstVisible([
      '[data-e2e="video-unavailable"]',
      '[data-e2e="browse-video-error"]',
      '[data-e2e="video-error"]',
      'main [role="status"]',
      'main [role="alert"]',
      'main h1',
      'main h2',
    ]);
    const unavailableText = text(unavailableNode);
    return {
      ok: !!videoId && hasDetail,
      site: 'tiktok',
      state: videoId ? 'video_detail' : 'other',
      video_id: videoId,
      url: location.href,
      ready_state: document.readyState,
      login_required: loginBlocked(hasVideo),
      challenge_required: challengeRequired(),
      unavailable: /video currently unavailable|couldn't find this video|video has been removed|not available in your country|视频不可用|已删除/i.test(unavailableText),
      has_video: hasVideo,
    };
  }

  function createdAt(value) {
    const seconds = Number(value || 0);
    if (!Number.isFinite(seconds) || seconds <= 0) return '';
    try {
      return new Date(seconds * 1000).toISOString();
    } catch (_) {
      return '';
    }
  }

  function videoDetail() {
    const state = videoState();
    if (!state.ok) return { ok: false, reason: 'not_video_detail', state };
    const stateVideo = initialVideo(state.video_id) || {};
    const stats = stateVideo.stats || {};
    const stateAuthor = stateVideo.author || {};
    const visibleVideos = Array.from(document.querySelectorAll('video')).filter(visible);
    const video = visibleVideos.length === 1 ? visibleVideos[0] : null;
    const authorLink = firstVisible([
      'a[aria-label*="profile on TikTok"][href*="/@"]',
      '[data-e2e="browse-username"] a[href*="/@"]',
      'a[data-e2e="browse-user-avatar"]',
      'a[href*="/@"]',
    ]);
    const authorUrl = normUrl(authorLink && (authorLink.href || authorLink.getAttribute('href')) || '');
    const structuredAuthor = breadcrumbAuthor();
    const handle = stateAuthor.uniqueId || (typeof stateAuthor === 'string' ? stateAuthor : '') ||
      structuredAuthor.handle || handleFromUrl(authorUrl);
    const canonicalAuthorUrl = handle ? normUrl(`/@${handle}`) : '';
    const descNode = firstVisible(['[data-e2e="browse-video-desc"]', '[data-e2e="video-desc"]', '[class*="DivVideoInfo"]']);
    const description = stateVideo.desc || text(descNode) || metaContent('og:description') || metaContent('description');
    const playerCover = firstVisible(['img[alt="Video Cover"]', '[aria-label="TikTok video player"] img']);
    const cover = normUrl(stateVideo.video && (stateVideo.video.cover || stateVideo.video.originCover || stateVideo.video.dynamicCover) || video && video.poster || playerCover && (playerCover.currentSrc || playerCover.src) || metaContent('og:image'));
    const seek = firstVisible(['[role="slider"][aria-label="Seek video"]']);
    const seekDuration = seek && (seek.getAttribute('aria-valuetext') || '').split(/\s+of\s+/i).pop() || '';
    const hashtags = Array.from(new Set([
      ...(stateVideo.textExtra || []).map((item) => item && item.hashtagName || '').filter(Boolean),
      ...Array.from(description.matchAll(/#([^#\s]+)/g)).map((match) => match[1]),
    ])).slice(0, 30);
    return {
      entity_type: 'video',
      platform: 'tiktok',
      video_id: state.video_id,
      url: location.href,
      title: metaContent('og:title') || description || document.title || '',
      description,
      hashtags,
      created_at: createdAt(stateVideo.createTime) || text(firstVisible(['[data-e2e="browser-nickname"] time', 'time'])),
      author: stateAuthor.nickname || structuredAuthor.name || text(authorLink && authorLink.querySelector('.nickname')) || text(authorLink) || handle,
      author_id: handle,
      author_internal_id: stateAuthor.id || stateVideo.authorId || '',
      author_url: canonicalAuthorUrl,
      likes: String(stats.diggCount || text(firstVisible(['[data-e2e="like-count"]', 'a[aria-label^="Like this post on TikTok"]'])) || ''),
      comments_count: String(stats.commentCount ?? (text(firstVisible(['[data-e2e="comment-count"]', 'a[aria-label^="Comment this post on TikTok"]'])) || '')),
      shares: String(stats.shareCount || text(firstVisible(['[data-e2e="share-count"]', 'a[aria-label^="Share this post on TikTok"]'])) || ''),
      favorites: String(stats.collectCount || text(firstVisible(['[data-e2e="collect-count"]', '[data-e2e="bookmark-count"]'])) || ''),
      views: String(stats.playCount || ''),
      duration_seconds: Math.round(Number(stateVideo.video && stateVideo.video.duration || video && video.duration || durationSeconds(seekDuration) || 0)),
      cover_url: cover,
      video: collectVideoInfo(video, stateVideo, cover),
      top_comments: [],
    };
  }

  function playerPlayButton() {
    const button = firstVisible([
      'button[aria-label="Play video"]',
      'button[aria-label="Play"]',
    ]);
    if (!button) return { found: false };
    const rect = button.getBoundingClientRect();
    return {
      found: rect.width > 0 && rect.height > 0,
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2,
    };
  }

  function commentActivation() {
    const comments = commentNodes();
    if (comments.length > 0) {
      return { ready: true, count: comments.length, action: '' };
    }

    const buttons = Array.from(document.querySelectorAll('button, [role="button"]')).filter(visible);
    const guide = buttons.find((button) => /^(got it|understood|i understand|知道了|我知道了)$/i.test(text(button)));
    if (guide) {
      const rect = guide.getBoundingClientRect();
      return {
        ready: false,
        count: 0,
        action: 'dismiss_guide',
        found: rect.width > 0 && rect.height > 0,
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2,
      };
    }

    const panel = firstVisible([
      '[data-e2e="comment-list"]',
      '[class*="DivCommentListContainer"]',
      '[class*="DivCommentMain"]',
    ]);
    if (panel) {
      return { ready: true, count: 0, action: '' };
    }

    const target = firstVisible([
      '[data-e2e="comment-icon"]',
      '[aria-label^="Read or add comments"]',
      '[aria-label*=" comments"]',
    ]);
    if (!target) {
      return { ready: false, count: 0, action: '', found: false };
    }
    const rect = target.getBoundingClientRect();
    return {
      ready: false,
      count: 0,
      action: 'open_comments',
      found: rect.width > 0 && rect.height > 0,
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2,
    };
  }

  function commentNodes() {
    const selectors = [
      '[data-e2e="comment-level-1"]',
      '[data-e2e="comment-item"]',
      '[class*="DivCommentItemContainer"]',
      '[class*="CommentItem"]',
    ];
    const found = [];
    const seen = new Set();
    for (const selector of selectors) {
      for (const node of document.querySelectorAll(selector)) {
        const container = node.closest('[data-e2e="comment-item"], [class*="DivCommentItemContainer"], [class*="CommentItem"]') || node;
        if (!visible(container) || seen.has(container)) continue;
        seen.add(container);
        found.push(container);
      }
    }
    return found;
  }

  function comments(arg) {
    const limit = Math.max(0, Number(arg && arg.limit || 20));
    const items = [];
    const seen = new Set();
    for (const node of commentNodes()) {
      const authorLink = node.querySelector('a[href*="/@"]');
      const authorUrl = normUrl(authorLink && (authorLink.href || authorLink.getAttribute('href')) || '');
      const subContent = node.querySelector('[class*="DivCommentSubContentWrapper"]');
      let content = node.matches('[data-e2e="comment-level-1"], [data-e2e="comment-content"]')
        ? node
        : node.querySelector('[data-e2e="comment-level-1"]') ||
          node.querySelector('[data-e2e="comment-content"]') ||
          node.querySelector('[class*="CommentText"]');
      if (!content) {
        const username = node.querySelector('[data-e2e="comment-username-1"]');
        content = Array.from(node.querySelectorAll('p')).find((candidate) => {
          return text(candidate) &&
            !(username && username.contains(candidate)) &&
            !(subContent && subContent.contains(candidate)) &&
            !candidate.closest('a[href*="/@"]');
        });
      }
      const timeNode = node.querySelector('time') || subContent && subContent.querySelector('span');
      const likeNode = node.querySelector('[data-e2e="comment-like-count"], [class*="LikeCount"], [class*="DivLikeContainer"]');
      const likeControl = node.querySelector('[class*="DivLikeContainer"][aria-label], [aria-label^="Like video"], [aria-label*=" likes"]');
      const likeLabel = likeControl && likeControl.getAttribute('aria-label') || '';
      const likeMatch = likeLabel.match(/([\d.,]+(?:[KMB])?)\s+likes?/i);
      const item = {
        comment_id: node.getAttribute('data-comment-id') || node.id || '',
        author: text(node.querySelector('[data-e2e="comment-username-1"]')) || text(authorLink) || handleFromUrl(authorUrl),
        author_id: handleFromUrl(authorUrl),
        author_url: authorUrl,
        text: text(content),
        likes: text(likeNode) || (likeMatch ? likeMatch[1] : ''),
        time: text(timeNode),
        replies: [],
      };
      const key = item.comment_id || `${item.author}\n${item.text}`;
      if (!item.text || seen.has(key)) continue;
      seen.add(key);
      items.push(item);
      if (items.length >= limit) break;
    }
    return items;
  }

  function scrollComments() {
    const container = firstVisible([
      '[data-e2e="comment-list"]',
      '[class*="DivCommentListContainer"]',
      '[class*="CommentList"]',
    ]);
    const scrollable = container || document.scrollingElement || document.documentElement;
    scrollable.scrollBy({ top: Math.floor(window.innerHeight * 0.75), left: 0, behavior: 'instant' });
    return { ok: true, y: scrollable.scrollTop || window.scrollY, count: comments({ limit: 999 }).length };
  }

  function profileStat(selectors, fallback) {
    const node = firstVisible(selectors);
    if (node) return text(node);
    const bodyText = text(document.body);
    const match = bodyText.match(fallback);
    return match ? match[1] : '';
  }

  function authorState() {
    const handle = handleFromUrl(location.href);
    const stateUser = initialUser(handle);
    const titleNode = firstVisible(['[data-e2e="user-title"]']);
    const subtitleNode = firstVisible(['[data-e2e="user-subtitle"]']);
    const subtitleHandle = text(subtitleNode).replace(/^@/, '').trim();
    const hasDomIdentity = text(titleNode).length > 0
      && subtitleHandle.length > 0
      && subtitleHandle.toLowerCase() === String(handle || '').toLowerCase();
    const hasProfile = !!stateUser.user || hasDomIdentity;
    const bodyText = text(document.body);
    return {
      ok: !!handle && hasProfile,
      site: 'tiktok',
      state: handle ? 'author_profile' : 'other',
      author_id: handle,
      author_internal_id: stateUser.user && stateUser.user.id || '',
      handle,
      url: location.href,
      ready_state: document.readyState,
      login_required: loginBlocked(hasProfile),
      challenge_required: challengeRequired(),
      unavailable: !hasProfile && /couldn't find this account|account not found|page not available|找不到此账号/i.test(bodyText),
    };
  }

  function authorProfile(arg) {
    const state = authorState();
    if (!state.ok) return { ok: false, reason: 'not_author_profile', state };
    const info = initialUser(state.handle);
    const user = info.user || {};
    const stats = info.stats || {};
    const postGrid = firstVisible([
      '[data-e2e="user-post-item-list"]',
      '[data-e2e="user-post-list"]',
      'main',
    ]);
    return {
      entity_type: 'author',
      platform: 'tiktok',
      author_id: user.uniqueId || state.handle,
      author_internal_id: user.id || state.author_internal_id,
      display_name: user.nickname || text(firstVisible(['[data-e2e="user-title"]', 'main h1'])),
      handle: user.uniqueId || state.handle,
      url: location.href,
      bio: user.signature || text(firstVisible(['[data-e2e="user-bio"]', '[class*="ShareDesc"]'])) || metaContent('description'),
      verified: !!user.verified || !!document.querySelector('[data-e2e="verified-badge"], [class*="VerifiedBadge"]'),
      followers: String(stats.followerCount || profileStat(['[data-e2e="followers-count"]'], /([0-9.,]+[KMB]?)\s+Followers/i) || ''),
      following: String(stats.followingCount || profileStat(['[data-e2e="following-count"]'], /([0-9.,]+[KMB]?)\s+Following/i) || ''),
      likes: String(stats.heartCount || stats.heart || profileStat(['[data-e2e="likes-count"]'], /([0-9.,]+[KMB]?)\s+Likes/i) || ''),
      video_count: String(stats.videoCount || ''),
      video_cards: postGrid ? videoCards({
        limit: Math.max(1, Number(arg && arg.limit || 20)),
        root: postGrid,
        author_id: user.uniqueId || state.handle,
        viewport_only: !!(arg && arg.viewport_only),
      }) : [],
    };
  }

  function searchState(arg) {
    const query = String(arg && arg.query || '').trim();
    const cards = videoCards({ limit: 3 });
    const bodyText = text(document.body);
    const emptyState = firstVisible([
      '[data-e2e="search-no-results"]',
      '[class*="SearchNoResult"]',
      '[class*="NoResult"]',
      '[class*="EmptyState"]',
    ]);
    const errorTitle = firstVisible([
      '[data-e2e="search-error-title"]',
      '[class*="SearchError"] h2',
    ]);
    const errorDescription = firstVisible([
      '[data-e2e="search-error-desc"]',
      '[class*="SearchError"] p',
    ]);
    const errorTitleText = text(errorTitle);
    const errorDescriptionText = text(errorDescription);
    const explicitSearchError = !!errorTitle && errorTitle.matches('[data-e2e="search-error-title"]');
    const recognizedSearchError = /something went wrong|couldn['’]t load|try again|server error|搜索失败|加载失败/i.test(
      `${errorTitleText} ${errorDescriptionText}`
    );
    const searchError = cards.length === 0 && errorTitleText && (explicitSearchError || recognizedSearchError) ? {
      title: errorTitleText,
      description: errorDescriptionText,
    } : null;
    return {
      ok: true,
      site: 'tiktok',
      url: location.href,
      title: document.title || '',
      ready_state: document.readyState,
      query,
      query_visible: query ? bodyText.toLowerCase().includes(query.toLowerCase()) || decodeURIComponent(location.href).toLowerCase().includes(query.toLowerCase()) : false,
      card_count: cards.length,
      blank_or_throttled: document.readyState === 'loading' || !hasUsefulBody(),
      login_required: loginBlocked(cards.length > 0),
      challenge_required: challengeRequired(),
      reason: searchError ? 'search_unavailable' : '',
      search_error: searchError,
      has_no_results: cards.length === 0
        && !!emptyState
        && /no results found|couldn't find|try another search|暂无结果|没有找到/i.test(text(emptyState)),
    };
  }

  function scrollFeed(arg) {
    const down = !(arg && arg.nudge_up);
    const delta = down ? Math.floor(window.innerHeight * 0.85) : -Math.floor(window.innerHeight * 0.35);
    const candidates = Array.from(document.querySelectorAll('[class*="Scroll"], main, body, html'));
    if (arg && arg.to_top) {
      const resetTargets = Array.from(new Set([
        document.scrollingElement,
        document.documentElement,
        document.body,
        ...candidates,
      ].filter(Boolean)));
      window.scrollTo(0, 0);
      for (const target of resetTargets) {
        target.scrollTop = 0;
        target.scrollLeft = 0;
      }
      return { ok: true, delta: 0, y: window.scrollY, card_count: videoCards({ limit: 999 }).length };
    }
    const scrollable = candidates.find((el) => {
      if (!visible(el) && el !== document.body && el !== document.documentElement) return false;
      return el.scrollHeight > el.clientHeight + 20;
    }) || document.scrollingElement || document.documentElement;
    scrollable.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return { ok: true, delta, y: scrollable.scrollTop || window.scrollY, card_count: videoCards({ limit: 999 }).length };
  }

  window.SocaiTikTokPageScripts = {
    pageState,
    searchState,
    videoCards,
    scrollFeed,
    videoState,
    videoDetail,
    playerPlayButton,
    commentActivation,
    comments,
    scrollComments,
    authorState,
    authorProfile,
  };
})();
