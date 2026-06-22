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

  function pageState() {
    const bodyText = text(document.body);
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
    const loginRequired = /登录|验证码|扫码|手机号/.test(bodyText) && /登录后|立即登录|扫码登录|验证码/.test(bodyText);
    const blankOrThrottled = document.readyState !== 'complete' || !hasUsefulBody();
    return {
      ok: true,
      site: 'dy',
      url: location.href,
      title: document.title || '',
      ready_state: document.readyState,
      body_text_len: bodyText.length,
      blank_or_throttled: blankOrThrottled,
      login_required: loginRequired,
      search_inputs: inputs,
    };
  }

  function searchInput() {
    const candidates = [
      '[data-e2e="searchbar-input"]',
      'input[placeholder*="搜索"]',
      'textarea[placeholder*="搜索"]',
      '[contenteditable="true"][data-e2e*="search"]',
      '[role="searchbox"]',
    ];
    const input = candidates.flatMap((selector) => Array.from(document.querySelectorAll(selector))).find(visible);
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
    const input = document.elementFromPoint(loc.input.x, loc.input.y);
    const target = input && (input.matches('input, textarea, [contenteditable="true"]')
      ? input
      : input.closest('input, textarea, [contenteditable="true"]'));
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

  function cardNodes() {
    const nodes = Array.from(document.querySelectorAll(
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
    const match = String(url || '').match(/\/(?:video|note)\/([^/?#]+)/);
    return match ? match[1] : '';
  }

  function videoCards(arg) {
    const limit = Math.max(1, Number((arg && arg.limit) || 30));
    const cards = [];
    const seen = new Set();
    for (const card of cardNodes()) {
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
      const titleNode = card.querySelector('[title], [aria-label]') ||
        card.querySelector('.RBpYLmIg, .trjxC5lo, [class*="title"], [class*="desc"]');
      let title = (img && img.alt) || text(titleNode) || '';
      if (!title) {
        const lines = allText.split(/\s{2,}|(?=@)/).map((line) => line.trim()).filter(Boolean);
        title = lines.find((line) => !line.startsWith('@')) || allText;
      }
      const author = text(card.querySelector('.lGzJpEad, .j5CaTxWe')) ||
        ((allText.match(/@([^\s·]+)/) || [])[1] || '');
      const likeNode = card.querySelector('.GiEcbsyC span');
      const countMatches = Array.from(allText.matchAll(/(\d+(?:\.\d+)?\s*(?:万|w|W|k|K)?)/g)).map((m) => m[1].replace(/\s+/g, ''));
      cards.push({
        video_id: videoId,
        url,
        title: title.trim(),
        author,
        author_url: normUrl((card.querySelector('a[href*="/user/"]') || {}).href || ''),
        likes: text(likeNode) || countMatches[0] || '',
        comments: '',
        shares: '',
        cover_url: normUrl((img && (img.currentSrc || img.src)) || ''),
        position: cards.length,
      });
      if (cards.length >= limit) break;
    }
    return cards;
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
      card_count: cards.length,
      blank_or_throttled: document.readyState !== 'complete' || !hasUsefulBody(),
      login_required: /登录后|扫码登录|验证码|立即登录/.test(bodyText),
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
    scrollable.scrollBy({ top: delta, left: 0, behavior: 'instant' });
    return { ok: true, delta, y: scrollable.scrollTop || window.scrollY, card_count: videoCards({ limit: 999 }).length };
  }

  window.SocaiDouyinPageScripts = {
    pageState,
    searchInput,
    setSearchInput,
    searchState,
    videoCards,
    scrollFeed,
  };
})();
