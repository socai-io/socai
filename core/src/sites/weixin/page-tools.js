(() => {
  const text = (selector, root = document) => root.querySelector(selector)?.innerText?.trim() || '';
  const failure = reason => ({ ok: false, reason, url: location.href });
  function gate() {
    const route = location.pathname;
    if (/antispider|captcha|verify/i.test(route) || document.querySelector('#seccodeImage, #tcaptcha_iframe')) return failure('challenge_required');
    // Only inspect the page shell when there is no article body: article prose
    // mentioning verification/rate limits must not trigger a false gate.
    if (!text('#js_content')) {
      const body = document.body?.innerText || '';
      if (/访问过于频繁|操作频繁|too many requests/i.test(body)) return failure('rate_limited');
      if (/环境异常|完成验证|请输入验证码|安全验证/.test(body)) return failure('challenge_required');
      if (/该内容已被发布者删除|此内容因违规无法查看|内容已删除|该内容暂时无法查看|链接已过期/.test(body)) return failure('article_unavailable');
    }
    return null;
  }
  function searchState({ query, page } = {}) {
    const blocked = gate();
    if (blocked) return blocked;
    const url = new URL(location.href);
    if (url.hostname !== 'weixin.sogou.com' || url.pathname !== '/weixin' || url.searchParams.get('type') !== '2') return failure('wrong_search_route');
    if (query !== undefined && url.searchParams.get('query') !== query) return failure('query_mismatch');
    if (page !== undefined && Number(url.searchParams.get('page') || 1) !== page) return failure('page_mismatch');
    if (document.querySelector('.news-list h3 a')) return { ok: true };
    const body = document.body?.innerText || '';
    if (/没有找到|未找到|暂无相关|找不到和/.test(body)) return { ok: true, empty: true };
    return failure('search_not_ready');
  }
  function searchResults(args = {}) {
    const state = searchState(args);
    if (!state.ok) return state;
    const url = new URL(location.href);
    const results = [...document.querySelectorAll('.news-list > li')].flatMap(li => {
      const anchor = li.querySelector('h3 a');
      if (!anchor) return [];
      const link = new URL(anchor.href);
      if (link.protocol !== 'https:' || !((link.hostname === 'weixin.sogou.com' && link.pathname === '/link') || link.hostname === 'mp.weixin.qq.com')) return [];
      return [{ title: anchor.innerText.trim(), url: link.href, snippet: text('.txt-info', li),
        author: text('.s-p .all-time-y2', li), published_at: text('.s-p .s2', li) }];
    }).slice(0, Math.max(1, Math.min(10, args.limit || 10)));
    const next = [...document.querySelectorAll('a')].find(a => a.innerText.trim() === '下一页');
    return { query: url.searchParams.get('query'), page: Number(url.searchParams.get('page') || 1),
      results, has_more: !!next };
  }
  function articleState() {
    const blocked = gate();
    if (blocked) return blocked;
    if (location.hostname === 'weixin.sogou.com' && location.pathname === '/link') return failure('redirect_pending');
    if (location.hostname !== 'mp.weixin.qq.com' || !/^\/s(?:\/|$)/.test(location.pathname)) return failure('wrong_article_route');
    if (!text('#js_content') && text('.weui-msg__title') === '该公众号已迁移') {
      const link = document.querySelector('#js_access_msg');
      const rect = link?.getBoundingClientRect();
      if (link?.innerText?.trim() === '访问文章' && rect?.width > 0 && rect?.height > 0) {
        try {
          const url = new URL(link.href);
          if (['http:', 'https:'].includes(url.protocol) && url.hostname === 'mp.weixin.qq.com'
              && !url.username && !url.password && !url.port && /^\/s(?:\/|$)/.test(url.pathname)) {
            // The real migration button can contain HTTP. Open its exact
            // destination over HTTPS; never retrieve article HTML over HTTP.
            url.protocol = 'https:';
            return { ...failure('account_migrated'), migration_url: url.href };
          }
        } catch { /* Keep an unsafe or malformed destination out of the host. */ }
      }
      return failure('account_migration_unavailable');
    }
    if (!text('#js_content') || !text('#activity-name')) return failure('article_not_ready');
    return { ok: true };
  }
  function imageUrl(value) {
    try {
      const url = new URL(value || '', location.href);
      if (!['http:', 'https:'].includes(url.protocol) || !/(^|\.)(qpic\.cn|qlogo\.cn)$/.test(url.hostname)) return '';
      url.protocol = 'https:';
      url.hash = '';
      return url.href;
    } catch { return ''; }
  }
  function articleBlocks() {
    const blocks = [], images = [], indexes = new Map();
    let pending = '';
    const flush = () => {
      const value = pending.replace(/[ \t\r\n]+/g, ' ').trim();
      if (value) blocks.push({ type: 'text', text: value });
      pending = '';
    };
    const walk = node => {
      if (node.nodeType === Node.TEXT_NODE) { pending += node.textContent; return; }
      if (node.nodeType !== Node.ELEMENT_NODE) return;
      if (['SCRIPT', 'STYLE', 'NOSCRIPT'].includes(node.tagName) || node.hidden || node.style.display === 'none') return;
      if (node.tagName === 'IMG') {
        const url = imageUrl(node.getAttribute('data-src') || node.currentSrc || node.getAttribute('src'));
        if (!url) return;
        flush();
        if (!indexes.has(url)) {
          indexes.set(url, images.length);
          images.push({ url, alt: node.alt || '', width: Number(node.getAttribute('data-w')) || node.naturalWidth || 0,
            height: (Number(node.getAttribute('data-w')) * Number(node.getAttribute('data-ratio'))) || node.naturalHeight || 0 });
        }
        blocks.push({ type: 'image', image_index: indexes.get(url) });
        return;
      }
      const boundary = /^(P|DIV|SECTION|H[1-6]|LI|BLOCKQUOTE|TR|BR)$/.test(node.tagName);
      if (boundary) flush();
      for (const child of node.childNodes) walk(child);
      if (node.tagName === 'TD' || node.tagName === 'TH') pending += ' ';
      if (boundary) flush();
    };
    const root = document.querySelector('#js_content');
    if (root) walk(root);
    flush();
    return { blocks, images };
  }
  function articleDetail() {
    const state = articleState();
    if (!state.ok) return state;
    const url = new URL(location.href);
    const textContent = text('#js_content');
    const rich = articleBlocks();
    const avatar = imageUrl(window.cgiDataNew?.round_head_img);
    // Prefer the publisher's stable article identity when the page exposes it.
    const biz = typeof window.biz === 'string' ? window.biz : url.searchParams.get('__biz');
    const mid = window.mid || url.searchParams.get('mid');
    const idx = window.idx || url.searchParams.get('idx');
    // Signed Sogou destinations may omit sn. Preserve the observed working URL;
    // never manufacture a public article link from an incomplete parameter set.
    return { ...(biz && mid && idx ? { article_id: `${biz}:${mid}:${idx}` } : {}),
      url: url.href, title: text('#activity-name'),
      author: text('#js_name'), author_avatar: avatar, blocks: rich.blocks, images: rich.images, published_at: text('#publish_time'), text: textContent,
      text_chars: [...textContent].length };
  }
  window.SocaiWeixinPageTools = { searchState, searchResults, articleState, articleDetail };
})();
