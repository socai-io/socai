(function () {
  const browserAction = typeof socaiAction === "function" ? socaiAction : async (perform) => perform();
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

  function center(el) {
    const rect = el.getBoundingClientRect();
    return {
      x: Math.max(1, Math.min(window.innerWidth - 1, rect.left + rect.width / 2)),
      y: Math.max(1, Math.min(window.innerHeight - 1, rect.top + rect.height / 2)),
      w: rect.width,
      h: rect.height,
    };
  }

  function hasUsefulBody() {
    return text(document.body).length > 20 || document.querySelectorAll('input, textarea, [contenteditable="true"], a[href], video').length > 0;
  }

  function loginBlocked(hasContent) {
    const bodyText = text(document.body);
    const dialog = firstVisible([
      '[role="dialog"]',
      '#login-panel-new',
      '[id*="login-panel"]',
      '[class*="login-modal"]',
      '[class*="loginModal"]',
      '[class*="login-panel"]',
      '[class*="loginPanel"]',
    ]);
    const dialogText = text(dialog);
    const explicitGate = /登录后(?:查看|继续|浏览|评论)|请先登录|登录即可/.test(bodyText);
    const visibleGate = !!dialog && /登录|验证码|扫码|手机号/.test(dialogText);
    return !hasContent && (explicitGate || visibleGate);
  }

  function challengeRequired() {
    if (/验证码|安全验证|访问验证/.test(document.title || '')) return true;
    return !!firstVisible([
      '#captcha_container',
      '#captcha-verify-container',
      '[class*="captcha"]',
      '[class*="Captcha"]',
      '[class*="verify-container"]',
      'iframe[src*="/verifycenter/"]',
      'iframe[src*="captcha"]',
    ]);
  }

  function pageState() {
    const bodyText = text(document.body);
    const signedIn = !!document.querySelector(
      'a[href*="/user/self"] img, [data-e2e="live-avatar"] img'
    );
    const inputs = Array.from(document.querySelectorAll('input, textarea, [contenteditable="true"], [role="searchbox"]'))
      .filter(visible)
      .slice(0, 8)
      .map((el) => {
        const rect = center(el);
        return {
          tag: el.tagName.toLowerCase(),
          role: el.getAttribute('role') || '',
          placeholder: el.getAttribute('placeholder') || '',
          aria_label: el.getAttribute('aria-label') || '',
          text: text(el).slice(0, 80),
          x: rect.x,
          y: rect.y,
          w: rect.w,
          h: rect.h,
        };
      });
    const hasSearchInput = inputs.some((input) =>
      /searchbox/i.test(input.role) ||
      /搜索|search/i.test(`${input.placeholder} ${input.aria_label}`)
    );
    const hasPublicContent = hasSearchInput || !!firstVisible([
      '[data-aweme-id]',
      '[id^="waterfall_item_"]',
      'a[href*="/video/"]',
      'video',
    ]);
    const blankOrThrottled = document.readyState === 'loading' || !hasUsefulBody();
    return {
      ok: true,
      site: 'dy',
      url: location.href,
      title: document.title || '',
      ready_state: document.readyState,
      body_text_len: bodyText.length,
      signed_in: signedIn,
      blank_or_throttled: blankOrThrottled,
      login_required: loginBlocked(hasPublicContent),
      challenge_required: challengeRequired(),
      search_inputs: inputs,
    };
  }

  const SEARCH_INPUT_SELECTORS = [
      '[data-e2e="searchbar-input"]',
      'input[placeholder*="搜索"]',
      'textarea[placeholder*="搜索"]',
      '[contenteditable="true"][data-e2e*="search"]',
      '[role="searchbox"]',
  ];

  function findSearchInput() {
    for (const selector of SEARCH_INPUT_SELECTORS) {
      const input = Array.from(document.querySelectorAll(selector)).find((candidate) => {
        if (!visible(candidate) || candidate.getAttribute('aria-hidden') === 'true') return false;
        return parseFloat(window.getComputedStyle(candidate).opacity || '1') >= 0.1;
      });
      if (input) return input;
    }
    return null;
  }

  function searchInput() {
    const input = findSearchInput();
    if (!input) {
      return { ok: false, error: 'search_input_not_found', state: pageState() };
    }
    const submit = document.querySelector('[data-e2e="searchbar-button"]') ||
      Array.from(document.querySelectorAll('button')).find((btn) => visible(btn) && /搜索/.test(text(btn)));
    return {
      ok: true,
      input: center(input),
      submit: submit && visible(submit) ? center(submit) : null,
      placeholder: input.getAttribute('placeholder') || '',
      value: input.value || text(input),
    };
  }

  function selectSearchInput() {
    const input = findSearchInput();
    if (!input) return { ok: false, error: 'search_input_not_found' };
    input.focus();
    let value = '';
    if (input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement) {
      value = String(input.value || '');
      input.setSelectionRange(0, value.length);
    } else if (input.isContentEditable) {
      value = String(input.textContent || '');
      const range = document.createRange();
      range.selectNodeContents(input);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
    } else {
      return { ok: false, error: 'unsupported_search_input' };
    }
    return { ok: true, value };
  }

  function setNativeValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const descriptor = Object.getOwnPropertyDescriptor(proto, 'value');
    if (descriptor && descriptor.set) {
      descriptor.set.call(el, value);
    } else {
      el.value = value;
    }
  }

  function setSearchInput(arg) {
    const query = String((arg && arg.query) || '').trim();
    const loc = searchInput();
    if (!loc.ok) return loc;
    // Resolve the same visible DOM input directly. A full-page login overlay
    // can make elementFromPoint return the overlay even while the header
    // search input remains present and focusable underneath it.
    const target = findSearchInput();
    if (!target) {
      return { ok: false, error: 'search_input_target_missing', loc };
    }
    target.focus();
    if (target.isContentEditable) {
      target.textContent = query;
    } else {
      setNativeValue(target, query);
    }
    target.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: query }));
    target.dispatchEvent(new Event('change', { bubbles: true }));
    return { ok: true, query, value: target.value || text(target), loc: searchInput() };
  }

  function cardNodes(root) {
    const scope = root && root.querySelectorAll ? root : document;
    const nodes = Array.from(scope.querySelectorAll(
      '.search-result-card, [id^="waterfall_item_"], [data-aweme-id], a[href*="/video/"], [href*="/video/"]'
    ));
    const cards = [];
    const seen = new Set();
    for (const node of nodes) {
      const card = node.closest('[id^="waterfall_item_"]') ||
        node.closest('.search-result-card') ||
        node.closest('[data-aweme-id]') ||
        node.closest('a[href*="/video/"], [href*="/video/"]') ||
        node;
      if (!card || seen.has(card)) continue;
      seen.add(card);
      if (!visible(card)) continue;
      cards.push(card);
    }
    return cards;
  }

  function videoIdFromUrl(url) {
    const match = String(url || '').match(/\/(?:video|note|share\/video)\/([^/?#]+)/);
    return match ? match[1] : '';
  }

  function authorIdFromUrl(url) {
    const match = String(url || '').match(/\/user\/([^/?#]+)/);
    return match ? match[1] : '';
  }

  function metaContent(property) {
    const node = document.querySelector(`meta[property="${property}"], meta[name="${property}"]`);
    return (node && node.getAttribute('content') || '').trim();
  }

  function breadcrumbAuthor() {
    const canonical = document.querySelector('link[rel="canonical"]');
    const currentVideoId = videoIdFromUrl(location.href) || videoIdFromUrl(canonical && canonical.href);
    if (!currentVideoId) return { name: '', id: '', url: '' };
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
        const entries = Array.isArray(documentData.itemListElement) ? documentData.itemListElement : [];
        const matchesVideo = entries.some((entry) => videoIdFromUrl(entry && entry.item) === currentVideoId);
        if (!matchesVideo) continue;
        const author = entries.find((entry) => authorIdFromUrl(entry && entry.item));
        if (!author) continue;
        const url = normUrl(author.item || '');
        return { name: String(author.name || '').trim(), id: authorIdFromUrl(url), url };
      }
    }
    return { name: '', id: '', url: '' };
  }

  function firstVisibleWithin(root, selectors) {
    const scope = root && root.querySelectorAll ? root : document;
    for (const selector of selectors) {
      const node = Array.from(scope.querySelectorAll(selector)).find(visible);
      if (node) return node;
    }
    return null;
  }

  function firstVisible(selectors) {
    return firstVisibleWithin(document, selectors);
  }

  function mainVisibleVideo(expectedId) {
    const videos = Array.from(document.querySelectorAll('video')).filter(visible);
    if (!videos.length) return null;
    const exact = videos.find((video) => {
      const owner = video.closest('[data-aweme-id], [id*="waterfall_item_"], [data-e2e*="video"]');
      const ownerRawId = (owner && owner.id) || '';
      const waterfallId = (ownerRawId.match(/waterfall_item_(\d+)/) || [])[1] || '';
      const ownerId = (owner && owner.getAttribute('data-aweme-id')) || waterfallId || videoIdFromUrl(ownerRawId);
      return expectedId && ownerId === expectedId;
    });
    if (exact) return exact;
    return videos.length === 1 ? videos[0] : null;
  }

  function durationSeconds(raw) {
    const value = String(raw || '').trim();
    const parts = value.split(':').map((part) => Number(part));
    if (!parts.length || parts.some((part) => !Number.isFinite(part))) return 0;
    return parts.reduce((total, part) => total * 60 + part, 0);
  }

  // The action bar that carries the count hooks is display:none outside the
  // immersive player, so it is read without a visibility filter, and only
  // inside the work's own player: a page-wide lookup can read another work.
  // A zero count is drawn as the bare label (收藏, 分享); only a number is a value.
  function statCount(player, selector) {
    const value = text(player && player.querySelector(selector));
    return /^\d[\d.,]*(?:万|亿|[wWkKmMbB])?\+?$/.test(value) ? value : '';
  }

  // The detail page prints 发布时间 in the browser's own time zone, so the same
  // work shows a different calendar day on a remote browser. Report the instant
  // in Beijing time, the zone Douyin dates are quoted in.
  function publishTime(videoId) {
    const raw = text(firstVisible(['[data-e2e="detail-video-publish-time"]']))
      .replace(/^发布时间\s*[:：]\s*/, '');
    const parts = raw.match(/^(\d{4})-(\d{1,2})-(\d{1,2})\s+(\d{1,2}):(\d{2})$/);
    if (!parts) return raw;
    const [year, month, day, hour, minute] = parts.slice(1).map(Number);
    const local = new Date(year, month - 1, day, hour, minute);
    let instant = local.getTime();
    if (Number.isNaN(instant)) return raw;
    // In the hour a clock falls back, a wall time names two instants and Date
    // takes the first. A work is created before it is published, so the
    // creation second in its id rules the first one out when it is too early.
    const fallBack = (new Date(instant + 3 * 3600 * 1000).getTimezoneOffset() - local.getTimezoneOffset()) * 60 * 1000;
    const second = new Date(instant + fallBack);
    if (fallBack > 0 && second.getHours() === hour && second.getMinutes() === minute) {
      const created = /^\d+$/.test(videoId) ? Number(BigInt(videoId) >> 32n) * 1000 : NaN;
      if (!(created < instant + fallBack + 60 * 1000)) return raw;
      if (created >= instant + 60 * 1000) instant += fallBack;
    }
    return `${new Date(instant + 8 * 3600 * 1000).toISOString().slice(0, 19)}+08:00`;
  }

  // The share-card meta is the only node that names this work's cover. Player
  // <img> nodes are avatars and next-video thumbnails.
  function shareCardCover(videoId) {
    const frameId = (metaContent('lark:url:video_iframe_url').match(/\/light\/([^/?#]+)/) || [])[1] || '';
    return videoId && frameId === videoId ? metaContent('lark:url:video_cover_image_url') : '';
  }

  function allowedMediaUrl(raw) {
    const value = String(raw || '').trim();
    if (!value || /^(data|blob):/i.test(value)) return '';
    try {
      const url = new URL(value, location.href);
      if (url.protocol !== 'https:' || url.username || url.password || url.port) return '';
      if (url.pathname.toLowerCase().endsWith('.m3u8')) return '';
      const host = url.hostname.toLowerCase();
      const sitePage = host === 'douyin.com' || host === 'www.douyin.com' || host === 'm.douyin.com';
      if (sitePage && !/mime_type=video|\.mp4(?:[?#]|$)|\/aweme\/v1\/play|\/video\/tos\//i.test(`${url.pathname}${url.search}`)) {
        return '';
      }
      const suffixes = [
        'douyinvod.com', 'douyinpic.com', 'douyin.com', 'byteimg.com',
        'zjcdn.com', 'bytecdn.cn', 'snssdk.com', 'pstatp.com', 'volccdn.com',
      ];
      return suffixes.some((suffix) => host === suffix || host.endsWith(`.${suffix}`)) ? url.href : '';
    } catch (_) {
      return '';
    }
  }

  function collectVideoInfo(video, cover) {
    const candidates = [];
    const push = (raw, source, kind) => {
      const url = allowedMediaUrl(raw || '');
      if (!url || candidates.some((item) => item.url === url)) return;
      candidates.push({ url, source, kind: kind || 'video' });
    };
    if (video) {
      push(video.currentSrc, 'video.currentSrc');
      push(video.src, 'video.src');
      for (const source of video.querySelectorAll('source')) {
        push(source.src || source.getAttribute('src'), 'source');
      }
    }
    // Douyin feeds the player through MediaSource, so the DOM commonly only
    // exposes a blob: URL. The corresponding signed CDN request remains in
    // the page's resource timing buffer and can be reused by the downloader.
    for (const entry of performance.getEntriesByType('resource')) {
      const raw = String(entry && entry.name || '');
      let url;
      try {
        url = new URL(raw, location.href);
      } catch (_) {
        continue;
      }
      const host = url.hostname.toLowerCase();
      const path = `${url.pathname}${url.search}`.toLowerCase();
      const videoHost = host === 'douyinvod.com' || host.endsWith('.douyinvod.com') ||
        host === 'zjcdn.com' || host.endsWith('.zjcdn.com') ||
        host === 'volccdn.com' || host.endsWith('.volccdn.com');
      const videoPath = /(?:\.mp4(?:$|\?)|\/video\/tos\/|mime_type=video|media_type=4)/.test(path);
      const kind = path.includes('media-audio') ? 'audio' : 'video';
      if (videoHost && videoPath) push(url.href, `performance.${entry.initiatorType || 'resource'}`, kind);
    }
    const directVideo = candidates.find((item) => item.kind === 'video' && !item.source.startsWith('performance.'));
    const directAudio = candidates.find((item) => item.kind === 'audio' && !item.source.startsWith('performance.'));
    const resolved = directVideo || [...candidates].reverse().find((item) => item.kind === 'video');
    const audio = directAudio || [...candidates].reverse().find((item) => item.kind === 'audio');
    return {
      url: resolved ? resolved.url : '',
      resolved_url: resolved ? resolved.url : '',
      audio_url: audio ? audio.url : '',
      poster_url: cover,
      source_urls: candidates.filter((item) => item.kind === 'video').map((item) => item.url),
      candidates,
    };
  }

  function httpsCover(img) {
    const found = [];
    const push = (raw) => {
      const value = String(raw || '').trim();
      if (!value || /^(data|blob):/i.test(value)) return;
      try {
        const url = new URL(value, location.href);
        if (url.protocol === 'https:' && !found.includes(url.href)) found.push(url.href);
      } catch (_) {}
    };
    const nodes = img ? [img, ...Array.from((img.parentElement || img).querySelectorAll('img, source'))] : [];
    for (const node of nodes) {
      push(node.currentSrc);
      push(node.getAttribute('src'));
      push(node.getAttribute('data-src'));
      for (const part of (node.getAttribute('srcset') || '').split(',')) {
        push(part.trim().split(/\s+/)[0]);
      }
    }
    return found[0] || '';
  }

  function videoCards(arg) {
    const limit = Math.max(1, Number((arg && arg.limit) || 30));
    const root = arg && arg.root && arg.root.querySelectorAll ? arg.root : document;
    const expectedAuthorId = String((arg && arg.author_id) || '').trim();
    const viewportOnly = !!(arg && arg.viewport_only);
    const cards = [];
    const seen = new Set();
    for (const card of cardNodes(root)) {
      if (viewportOnly && !inViewport(card)) continue;
      const linkNode = card.matches('a[href*="/video/"], [href*="/video/"]')
        ? card
        : card.querySelector('a[href*="/video/"], [href*="/video/"]');
      const isLive = !!card.querySelector('a[href*="live.douyin.com"]') || /直播中|直播间/.test(text(card));
      const hasVideoSignal = !!card.querySelector('.videoImage, [class*="videoImage"]') ||
        !!card.querySelector('[class*="duration"], .cxEIO6RG') ||
        !!linkNode ||
        !!card.getAttribute('data-aweme-id');
      if (isLive || !hasVideoSignal) continue;
      const rawHref = (linkNode && (linkNode.href || linkNode.getAttribute('href'))) ||
        card.getAttribute('href') ||
        '';
      const idMatch = (card.id || '').match(/^waterfall_item_(\d+)/);
      const videoId = card.getAttribute('data-aweme-id') || videoIdFromUrl(rawHref) || (idMatch ? idMatch[1] : '');
      const url = normUrl(rawHref || (videoId ? `/video/${videoId}` : ''));
      if (!videoId && !url) continue;
      const key = videoId || url;
      if (seen.has(key)) continue;
      seen.add(key);

      const img = card.querySelector('img');
      const allText = text(card);
      const videoRegion = card.querySelector('.videoImage, [class*="videoImage"]') || card;
      const leafTextNodes = Array.from(card.querySelectorAll('div, span, p')).filter((node) => {
        return node.children.length === 0 && text(node).length > 0;
      });
      const durationNode = leafTextNodes.find((node) => {
        return videoRegion.contains(node) && /^\d{1,2}:\d{2}(?::\d{2})?$/.test(text(node));
      });
      const countNode = leafTextNodes.find((node) => {
        return videoRegion.contains(node) && node !== durationNode && /^\d+(?:\.\d+)?(?:万|w|W|k|K)?$/.test(text(node));
      });
      const structuralTitle = leafTextNodes
        .filter((node) => !videoRegion.contains(node))
        .map((node) => ({ node, value: text(node) }))
        .filter(({ value }) => value.length > 4 && !/^[@·]|^\d{4}年\d{1,2}月\d{1,2}日$/.test(value))
        .sort((a, b) => b.value.length - a.value.length)[0];
      const preciseTitleNode = card.querySelector('.BjLsdJMi, [data-e2e="search-card-desc"]');
      const genericTitleNode = card.querySelector('.RBpYLmIg, .trjxC5lo, [class*="title"], [class*="desc"], [title], [aria-label]');
      const titleCandidate = (value) => {
        const candidate = String(value || '').trim();
        if (!candidate || /^@/.test(candidate) || /^\d+(?:\.\d+)?(?:万|w|W|k|K)?$/.test(candidate)) return '';
        if (/^@?.+?(?:\s*·|\s+)\d{4}年\d{1,2}月\d{1,2}日/.test(candidate)) return '';
        return candidate;
      };
      let title = titleCandidate(text(preciseTitleNode)) ||
        titleCandidate(img && img.alt) ||
        titleCandidate(text(genericTitleNode)) ||
        titleCandidate(structuralTitle && structuralTitle.value);
      if (!title) {
        const lines = allText.split(/\s{2,}|(?=@)/).map((line) => line.trim()).filter(Boolean);
        title = lines.find((line) => !line.startsWith('@')) || allText;
      }
      const atAuthor = Array.from(card.querySelectorAll('span'))
        .filter((node) => node.children.length === 0)
        .map((node) => text(node).match(/^@(.+?)(?:\s*·|\s+\d{4}年|$)/))
        .find(Boolean);
      const author = text(card.querySelector('.WldPmwm5, [data-e2e="search-card-author"], .lGzJpEad, .j5CaTxWe')) ||
        (atAuthor ? atAuthor[1].trim() : '') ||
        ((allText.match(/@(.+?)(?:\s*·|\s+\d{4}年)/) || [])[1] || '');
      const authorLink = card.querySelector('a[href*="/user/"]');
      const authorUrl = normUrl((authorLink && authorLink.href) || '');
      const cardAuthorId = authorIdFromUrl(authorUrl);
      if (expectedAuthorId && cardAuthorId && cardAuthorId !== expectedAuthorId) continue;
      const likeNode = card.querySelector('.GiEcbsyC span, [data-e2e="search-card-like-count"]') || countNode;
      const duration = text(card.querySelector('[class*="duration"], time, [data-e2e="search-card-duration"]')) || text(durationNode);
      cards.push({
        video_id: videoId,
        url,
        title: title.trim(),
        author,
        author_id: cardAuthorId || expectedAuthorId,
        author_url: authorUrl || (expectedAuthorId ? normUrl(`/user/${expectedAuthorId}`) : ''),
        likes: text(likeNode),
        comments: '',
        shares: '',
        views: '',
        cover_url: httpsCover(img),
        video_url: allowedMediaUrl((card.querySelector('video') && (card.querySelector('video').currentSrc || card.querySelector('video').src)) || ''),
        duration_seconds: durationSeconds(duration),
        position: cards.length,
      });
      if (cards.length >= limit) break;
    }
    return cards;
  }

  function videoState() {
    const canonical = document.querySelector('link[rel="canonical"]');
    const id = videoIdFromUrl(location.href) || videoIdFromUrl(canonical && canonical.href);
    const bodyText = text(document.body);
    const video = mainVisibleVideo(id);
    const videoSource = video && (video.currentSrc || video.src || (video.querySelector('source') && video.querySelector('source').src)) || '';
    const hasVideo = !!video && (!!videoSource || video.readyState > 0);
    const detailNode = firstVisible([
      '[data-e2e="detail-video-info"]',
      '[data-e2e="video-desc"]',
      '[data-e2e="video-author-name"]',
      '[class*="video-desc"]',
    ]);
    const detailText = text(detailNode) || metaContent('og:description');
    const hasDetail = detailText.length > 0 || (hasVideo && metaContent('og:title').length > 0);
    const structuredAuthor = breadcrumbAuthor();
    const hasAuthor = !!structuredAuthor.id || !!firstVisible([
      '[data-e2e="user-info"] a[href*="/user/"]',
      '[data-e2e="video-author-name"] a[href*="/user/"]',
      '[data-click-from="click_icon"] a[href*="/user/"]',
    ]);
    // Douyin can paint the description and a short placeholder player before
    // the author block and playable source arrive. Returning at that point
    // produces an apparently successful but mostly empty entity. Wait until
    // at least one stable identity/media signal is present.
    const contentReady = hasDetail && (hasAuthor || !!videoSource);
    return {
      ok: !!id && contentReady,
      site: 'dy',
      state: id ? 'video_detail' : 'other',
      video_id: id,
      url: location.href,
      ready_state: document.readyState,
      login_required: loginBlocked(hasVideo),
      challenge_required: challengeRequired(),
      unavailable: /作品不存在|视频不见了|内容已删除|暂时无法播放/.test(bodyText),
      has_video: hasVideo,
      has_author: hasAuthor,
      has_video_source: !!videoSource,
    };
  }

  function videoDetail() {
    const state = videoState();
    if (!state.ok) {
      return { ok: false, reason: 'not_video_detail', state };
    }
    const video = mainVisibleVideo(state.video_id);
    if (!video && state.has_video) {
      return { ok: false, reason: 'ambiguous_video_player', state };
    }
    const detailRoot = firstVisible([
      '[data-e2e="detail-video-info"]',
      '[class*="video-detail-container"]',
    ]);
    const detailPage = detailRoot && detailRoot.closest('.detailPage, [class*="detailPage"]');
    const authorScope = detailPage || detailRoot || document;
    const structuredAuthor = breadcrumbAuthor();
    const explicitAuthorLink = firstVisibleWithin(authorScope, [
      '[data-e2e="user-info"] a[href*="/user/"]',
      '[data-e2e="video-author-name"] a[href*="/user/"]',
      '[data-click-from="click_icon"] a[href*="/user/"]',
    ]);
    const genericAuthorLink = Array.from(authorScope.querySelectorAll('a[href*="/user/"]'))
      .filter(visible)
      .find((link) => {
        const id = authorIdFromUrl(link.href || link.getAttribute('href'));
        if (!id || id === 'self') return false;
        if (link.closest('[data-e2e="comment-item"], [data-comment-id], [class*="comment-item-info-wrap"]')) return false;
        return !!link.querySelector('[data-e2e="live-avatar"]') || !!link.querySelector('img[alt]') || text(link).length > 0;
      });
    const explicitAuthorUrl = normUrl((explicitAuthorLink && explicitAuthorLink.href) || '');
    const authorUrl = explicitAuthorUrl || structuredAuthor.url || normUrl((genericAuthorLink && genericAuthorLink.href) || '');
    const authorTextLink = authorUrl ? Array.from(authorScope.querySelectorAll('a[href*="/user/"]'))
      .find((link) => normUrl(link.href || link.getAttribute('href')) === authorUrl && text(link).length > 0) : null;
    const authorLink = explicitAuthorLink || authorTextLink || genericAuthorLink;
    const authorAvatar = authorLink && authorLink.querySelector('img[alt]');
    const descNode = firstVisible([
      '[data-e2e="video-desc"]',
      '[data-e2e="detail-video-info"] [data-click-from="title"]',
      '[data-e2e="detail-video-info"] h1',
      '[class*="video-desc"]',
      '[class*="VideoDetail"] h1',
      'main h1',
    ]);
    const description = text(descNode) || metaContent('og:description') || metaContent('description');
    const title = metaContent('og:title') || description || document.title || '';
    if (video && video.paused && video.src && video.src.startsWith('blob:')) {
      video.play().catch(() => {});
    }
    const cover = normUrl((video && video.poster) || shareCardCover(state.video_id));
    const media = collectVideoInfo(video, cover);
    const actionBar = video && video.closest('[data-e2e="player-container"]');
    const hashtagSet = new Set();
    for (const match of description.matchAll(/#([^#\s]+)/g)) hashtagSet.add(match[1]);
    for (const link of (detailRoot || document).querySelectorAll('a[href*="/search/"]')) {
      const value = text(link).replace(/^#/, '').trim();
      if (value) hashtagSet.add(value);
    }
    return {
      entity_type: 'video',
      platform: 'douyin',
      video_id: state.video_id,
      url: location.href,
      title,
      description,
      hashtags: Array.from(hashtagSet).slice(0, 30),
      created_at: publishTime(state.video_id),
      author: text(authorLink) || text(authorTextLink) || (authorAvatar && authorAvatar.alt || '') || structuredAuthor.name ||
        text(firstVisible(['[data-e2e="video-author-name"]', '[class*="author-name"]'])),
      author_id: authorIdFromUrl(authorUrl),
      author_url: authorUrl,
      likes: statCount(actionBar, '[data-e2e="video-player-digg"]'),
      comments_count: statCount(actionBar, '[data-e2e="feed-comment-icon"]'),
      shares: statCount(actionBar, '[data-e2e="video-player-share"]'),
      favorites: statCount(actionBar, '[data-e2e="video-player-collect"]'),
      views: '',
      duration_seconds: video ? Math.round(Number(video.duration) || 0) : 0,
      cover_url: cover,
      video: media,
      top_comments: [],
    };
  }

  function commentNodes() {
    const wrapped = Array.from(document.querySelectorAll('[class*="comment-item-info-wrap"]'))
      .map((node) => node.parentElement)
      .filter((node) => node && visible(node));
    if (wrapped.length) return Array.from(new Set(wrapped));
    const selectors = [
      '[data-e2e="comment-item"]',
      '[data-comment-id]',
    ];
    const found = [];
    const seen = new Set();
    for (const selector of selectors) {
      for (const node of document.querySelectorAll(selector)) {
        if (!visible(node) || seen.has(node)) continue;
        seen.add(node);
        found.push(node);
      }
    }
    return found;
  }

  function commentEntity(node) {
    const authorLink = node.querySelector('a[href*="/user/"]');
    const authorUrl = normUrl((authorLink && authorLink.href) || '');
    const contentNode = node.querySelector('[data-e2e="comment-level-1"], [class*="comment-content"], [class*="commentContent"]') ||
      Array.from(node.children).find((child) => {
        if (child.matches('[class*="comment-item-info-wrap"], [class*="comment-item-stats-container"]') ||
            child.querySelector('[class*="comment-item-info-wrap"], [class*="comment-item-stats-container"]')) return false;
        const value = text(child);
        return value.length > 0 && value !== '...' && !/^\d+$/.test(value) &&
          !/^(?:刚刚|\d+\s*(?:分钟|小时|天|月|年)前)(?:·.+)?$/.test(value);
      });
    const tooltip = node.querySelector('[id^="tooltip_"]');
    const timeNode = node.querySelector('time, [class*="time"]') ||
      Array.from(node.children).find((child) => /(?:刚刚|分钟前|小时前|天前|月前|年前)(?:·.+)?$/.test(text(child)));
    return {
      comment_id: node.getAttribute('data-comment-id') || node.id || ((tooltip && tooltip.id || '').replace(/^tooltip_/, '')),
      author: (text(authorLink) || text(node.querySelector('[class*="author"], [class*="name"]'))).replace(/\s*作者(?:赞过|回复过)?\s*$/, ''),
      author_id: authorIdFromUrl(authorUrl),
      author_url: authorUrl,
      text: text(contentNode),
      likes: text(node.querySelector('[data-e2e="comment-like-count"], [class*="like-count"], [class*="likeCount"], [class*="comment-item-stats-container"] p span')),
      time: text(timeNode),
      replies: [],
    };
  }

  function commentLevel(node) {
    let depth = 0;
    for (let parent = node.parentElement; parent && parent !== document.body; parent = parent.parentElement) {
      if (parent.matches('ul, ol, [role="list"], [class*="reply"], [class*="Reply"]')) depth += 1;
    }
    const left = node.getBoundingClientRect ? Math.round(node.getBoundingClientRect().left / 12) : 0;
    return depth * 1000 + left;
  }

  function nestCommentEntries(entries) {
    const roots = [];
    const stack = [];
    for (const entry of entries) {
      while (stack.length && stack[stack.length - 1].level >= entry.level) stack.pop();
      if (stack.length) stack[stack.length - 1].item.replies.push(entry.item);
      else roots.push(entry.item);
      stack.push(entry);
    }
    return roots;
  }

  function commentTreeCount(items) {
    return items.reduce((count, item) => count + 1 + commentTreeCount(item.replies || []), 0);
  }

  function comments(arg) {
    const limit = Math.max(0, Number((arg && arg.limit) || 20));
    const entries = [];
    const seen = new Set();
    for (const node of commentNodes()) {
      const item = commentEntity(node);
      const key = item.comment_id || `${item.author}\n${item.text}`;
      if (!item.text || seen.has(key)) continue;
      seen.add(key);
      entries.push({ item, level: commentLevel(node) });
      if (entries.length >= limit) break;
    }
    return nestCommentEntries(entries);
  }

  async function scrollComments() {
    const root = firstVisible([
      '[data-e2e="comment-list"]',
      '[class*="comment-list"]',
      '[class*="commentList"]',
    ]) || document;
    const before = commentTreeCount(comments({ limit: 999 }));
    const expandPattern = /(?:view|show|load|展开|查看)(?:\s+all|\s+more|更多|全部)?\s*(?:\d+\s*)?(?:repl(?:y|ies)|回复|评论)/i;
    let clicked = 0;
    for (const control of root.querySelectorAll('button, [role="button"]')) {
      if (!visible(control) || control.disabled) continue;
      const label = `${text(control)} ${control.getAttribute('aria-label') || ''}`.trim();
      if (!expandPattern.test(label)) continue;
      await browserAction(() => control.click());
      clicked += 1;
      if (clicked >= 6) break;
    }
    let scrollable = root;
    for (let node = root; node && node !== document.body; node = node.parentElement) {
      if (node.scrollHeight > node.clientHeight + 8) {
        scrollable = node;
        break;
      }
    }
    if (!scrollable || scrollable === document) scrollable = document.scrollingElement || document.documentElement;
    const beforeY = scrollable.scrollTop || window.scrollY;
    const step = Math.max(360, Math.floor((scrollable.clientHeight || window.innerHeight) * 0.8));
    await browserAction(() => {
      if (typeof scrollable.scrollBy === 'function') {
        scrollable.scrollBy({ top: step, left: 0, behavior: 'auto' });
      } else {
        scrollable.scrollTop = beforeY + step;
      }
    });
    await new Promise((resolve) => setTimeout(resolve, 500));
    const after = commentTreeCount(comments({ limit: 999 }));
    const y = scrollable.scrollTop || window.scrollY;
    const atEnd = y + (scrollable.clientHeight || window.innerHeight) >= scrollable.scrollHeight - 8;
    return { ok: true, before, after, clicked, grew: after > before, y, at_end: clicked === 0 && atEnd };
  }

  // The works tab shows an in-place message with a 刷新 control ("服务异常，
  // 重新刷新拉取数据") in place of cards when the list request comes back as
  // HTTP 200 with no body, which is how Douyin turns a session away.
  function authorPostsFailure() {
    const grid = document.querySelector('[data-e2e="user-post-list"]');
    if (!grid || cardNodes(grid).length) return null;
    const control = Array.from(grid.querySelectorAll('span'))
      .find((node) => node.children.length === 0 && text(node) === '刷新');
    return control ? { control, message: text(control.parentElement) } : null;
  }

  async function refreshAuthorPosts() {
    const failure = authorPostsFailure();
    if (!failure) return { ok: false, error: 'refresh_control_not_found' };
    // The login panel covers the grid in a logged-out session, so a pointer
    // click at the control's coordinates would land on the panel instead.
    await browserAction(() => failure.control.click());
    return { ok: true };
  }

  // A logged-out session mounts the login panel and keeps a 登录 button in the
  // page header; both are gone once an account is signed in.
  function loggedOut() {
    if (document.querySelector('#login-panel-new')) return true;
    return Array.from(document.querySelectorAll('header button'))
      .some((button) => /^(登录|Log ?in)$/i.test(text(button)));
  }

  function authorState() {
    const authorId = authorIdFromUrl(location.href);
    const bodyText = text(document.body);
    const nameNode = firstVisible([
      '[data-e2e="user-title"]',
      '[data-e2e="user-info"] h1',
      '[data-e2e="user-detail"] h1',
      '[class*="nickname"]',
    ]);
    const metaName = metaContent('og:title').replace(/的抖音|\s*-\s*抖音$/, '').trim();
    const displayName = text(nameNode) || metaName;
    const hasProfileEvidence = !!nameNode || !!document.querySelector(
      '[data-e2e="user-bio"], [data-e2e="user-post-list"], [data-e2e="user-info"], [data-e2e="user-detail"], [class*="user-info"], [class*="userInfo"]'
    );
    const hasProfile = displayName.length > 0 && hasProfileEvidence;
    const postsFailure = authorPostsFailure();
    return {
      ok: !!authorId && hasProfile,
      site: 'dy',
      state: authorId ? 'author_profile' : 'other',
      author_id: authorId,
      url: location.href,
      ready_state: document.readyState,
      login_required: loginBlocked(hasProfile),
      challenge_required: challengeRequired(),
      unavailable: /用户不存在|账号已注销|页面不存在/.test(bodyText),
      logged_out: loggedOut(),
      posts_error: postsFailure ? postsFailure.message : '',
    };
  }

  // Douyin draws emoji as <img alt="♌"> nodes, which innerText leaves out.
  function textWithEmoji(node, skip) {
    const walk = (current) => Array.from(current.childNodes).map((child) => {
      if (child === skip) return '';
      if (child.nodeType === 3) return child.nodeValue || '';
      if (child.nodeType !== 1) return '';
      return child.tagName === 'IMG' ? (child.getAttribute('alt') || '') : walk(child);
    }).join('');
    return node ? walk(node).replace(/\s+/g, ' ').trim() : '';
  }

  // The profile route is server-rendered from a `self.__pace_f.push([1,
  // "<id>:<json>"])` chunk that holds the author record. It is the one place
  // on the page with the whole bio: the header joins the lines, cuts the text
  // at 30 characters, and only shows the rest on hover.
  function embeddedAuthorBio(authorId) {
    const find = (value, depth) => {
      if (!value || typeof value !== 'object' || depth > 8) return null;
      if (value.secUid === authorId) return value;
      for (const child of Object.values(value)) {
        const found = find(child, depth + 1);
        if (found) return found;
      }
      return null;
    };
    for (const script of document.querySelectorAll('script')) {
      const source = script.textContent || '';
      if (!authorId || !source.startsWith('self.__pace_f.push(') || !source.includes(authorId)) continue;
      try {
        const chunk = JSON.parse(source.slice(source.indexOf('(') + 1, source.lastIndexOf(')')))[1];
        const record = find(JSON.parse(chunk.slice(chunk.indexOf(':') + 1)), 0);
        if (record) {
          // Flight string encoding: "$undefined" is no value, "$$…" is text
          // that starts with "$", and any other "$…" points at another chunk.
          const desc = typeof record.desc === 'string' ? record.desc : '';
          if (desc === '$undefined') return '';
          if (/^\$[^$]/.test(desc)) return null;
          return desc.replace(/^\$\$/, '$').replace(/\s+/g, ' ').trim();
        }
      } catch (_) {}
    }
    return null;
  }

  // Rows of [data-e2e="user-info"] in page order: name (plus the 认证 badge),
  // counters, the 抖音号 / IP属地 / gender-age line, an optional
  // 最新作品发布时间 line, the bio when the author wrote one, then entry cards.
  // Their class names are hashed, so each row is recognized by what it holds.
  function authorHeader(authorId) {
    const root = firstVisible(['[data-e2e="user-detail"]']) || document;
    const info = root.querySelector('[data-e2e="user-info"]');
    const rows = info ? Array.from(info.children) : [];
    const metaIndex = rows.findIndex((row) => /^抖音号[:：]/.test(text(row)));
    const metaLeaves = metaIndex < 0 ? [] : Array.from(rows[metaIndex].querySelectorAll('span'))
      .filter((node) => node.children.length === 0);
    const metaValue = (pattern) => {
      const leaf = metaLeaves.find((node) => pattern.test(text(node)));
      return leaf ? text(leaf).match(pattern)[1].trim() : '';
    };
    // The tag reads "36岁", or 男 / 女 when the age is hidden. A set gender
    // adds an icon, and only the female glyph is drawn through a mask
    // (id "woman_svg__a").
    const tagLeaf = metaLeaves.find((node) => /^(\d{1,3}岁|男|女)$/.test(text(node)));
    const tag = text(tagLeaf);
    const icon = tagLeaf ? tagLeaf.parentElement.querySelector('svg') : null;
    const female = tag === '女' || !!(icon && icon.querySelector('[id^="woman_svg"]'));

    let bio = embeddedAuthorBio(authorId);
    if (bio === null) {
      // Without the record, read the row the header shows. Entry cards take
      // the same slot when there is no bio, and they carry icons.
      const next = metaIndex < 0 ? [] : rows.slice(metaIndex + 1).filter((row) => row.tagName !== 'P');
      const bioRow = next.length && !next[0].querySelector('svg') ? next[0] : null;
      const more = bioRow && Array.from(bioRow.querySelectorAll('span'))
        .find((node) => node.children.length === 0 && /^(更多|登录后查看更多)$/.test(text(node)));
      // A cut bio ends in "..." and is followed by a 更多 control.
      const shown = textWithEmoji(bioRow, more && more.parentElement);
      bio = /(\.\.\.|…)$/.test(shown) ? shown : textWithEmoji(bioRow);
    }

    const avatar = info && Array.from(info.parentElement.querySelectorAll('img'))
      .find((img) => !info.contains(img) && /头像$/.test(img.getAttribute('alt') || ''));
    return {
      handle: metaValue(/^抖音号[:：]\s*(\S+)/),
      ip_location: metaValue(/^IP属地[:：]\s*(.+)$/),
      gender: female ? 'female' : (tag === '男' || icon) ? 'male' : '',
      age: (tag.match(/^(\d{1,3})岁$/) || [])[1] || '',
      bio,
      avatar_url: httpsCover(avatar),
      verified: !!(info && info.querySelector('[data-e2e="badge-role-name"]')),
    };
  }

  function profileStat(label, selector) {
    const node = selector && firstVisible([selector]);
    if (node) {
      const value = text(node).replace(label, '').trim();
      if (value) return value;
    }
    const body = text(document.body);
    const before = body.match(new RegExp(`([0-9.,]+(?:万|w|W|k|K)?)\\s*${label}`));
    if (before) return before[1];
    const after = body.match(new RegExp(`${label}\\s*([0-9.,]+(?:万|w|W|k|K)?)`));
    return after ? after[1] : '';
  }

  function authorProfile(arg) {
    const state = authorState();
    if (!state.ok) return { ok: false, reason: 'not_author_profile', state };
    const limit = Math.max(1, Number((arg && arg.limit) || 20));
    const nameNode = firstVisible([
      '[data-e2e="user-title"]',
      '[data-e2e="user-info"] h1',
      '[data-e2e="user-detail"] h1',
      '[class*="nickname"]',
    ]);
    const displayName = text(nameNode) || metaContent('og:title').replace(/的抖音| - 抖音$/, '').trim();
    const header = authorHeader(state.author_id);
    const postGrid = firstVisible([
      '[data-e2e="user-post-list"]',
      '[class*="user-post-list"]',
      '[class*="userPostList"]',
      '[class*="profile"] [class*="waterfall"]',
    ]);
    const cards = postGrid ? videoCards({
      limit,
      root: postGrid,
      author_id: state.author_id,
      viewport_only: !!(arg && arg.viewport_only),
    }) : [];
    for (const card of cards) {
      if (!card.author) card.author = displayName;
    }
    return {
      entity_type: 'author',
      platform: 'douyin',
      author_id: state.author_id,
      display_name: displayName,
      handle: header.handle,
      url: location.href,
      avatar_url: header.avatar_url,
      bio: header.bio,
      ip_location: header.ip_location,
      gender: header.gender,
      age: header.age,
      verified: header.verified,
      followers: profileStat('粉丝', '[data-e2e="user-info-fans"]'),
      following: profileStat('关注', '[data-e2e="user-info-follow"]'),
      likes: profileStat('获赞', '[data-e2e="user-info-like"]'),
      video_count: profileStat('作品'),
      video_cards: cards,
    };
  }

  function searchState(arg) {
    const query = String((arg && arg.query) || '').trim();
    const cards = videoCards({ limit: 3 });
    const bodyText = text(document.body);
    return {
      ok: true,
      url: location.href,
      title: document.title || '',
      ready_state: document.readyState,
      query,
      query_visible: query ? bodyText.includes(query) || decodeURIComponent(location.href).includes(query) : false,
      query_in_url: query ? decodeURIComponent(location.href).includes(query) : false,
      card_count: cards.length,
      blank_or_throttled: document.readyState === 'loading' || !hasUsefulBody(),
      login_required: loginBlocked(cards.length > 0),
      challenge_required: challengeRequired(),
      has_no_results: /暂无|没有找到|无结果|换个词/.test(bodyText),
    };
  }

  function scrollFeed(arg) {
    const down = !(arg && arg.nudge_up);
    const delta = down ? Math.floor(window.innerHeight * 0.85) : -Math.floor(window.innerHeight * 0.35);
    const candidates = Array.from(document.querySelectorAll('.route-scroll-container, [class*="scroll"], main, body, html'));
    const scrollable = candidates.find((el) => {
      if (!visible(el) && el !== document.body && el !== document.documentElement) return false;
      return el.scrollHeight > el.clientHeight + 20;
    }) || document.scrollingElement || document.documentElement;
    if (arg && arg.to_top) {
      scrollable.scrollTo({ top: 0, left: 0, behavior: 'instant' });
      return { ok: true, delta: 0, y: scrollable.scrollTop || window.scrollY, card_count: videoCards({ limit: 999 }).length };
    }
    scrollable.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return { ok: true, delta, y: scrollable.scrollTop || window.scrollY, card_count: videoCards({ limit: 999 }).length };
  }

  window.SocaiDouyinPageScripts = {
    pageState,
    searchInput,
    selectSearchInput,
    setSearchInput,
    searchState,
    videoCards,
    scrollFeed,
    videoState,
    videoDetail,
    comments,
    scrollComments,
    authorState,
    authorProfile,
    refreshAuthorPosts,
  };
})();
