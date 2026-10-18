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
    if (!text('#js_content') || !text('#activity-name')) return failure('article_not_ready');
    return { ok: true };
  }
  function articleDetail() {
    const state = articleState();
    if (!state.ok) return state;
    const url = new URL(location.href);
    const textContent = text('#js_content');
    // Prefer the publisher's stable article identity when the page exposes it.
    const biz = typeof window.biz === 'string' ? window.biz : url.searchParams.get('__biz');
    const mid = window.mid || url.searchParams.get('mid');
    const idx = window.idx || url.searchParams.get('idx');
    // Signed Sogou destinations may omit sn. Preserve the observed working URL;
    // never manufacture a public article link from an incomplete parameter set.
    return { ...(biz && mid && idx ? { article_id: `${biz}:${mid}:${idx}` } : {}),
      url: url.href, title: text('#activity-name'),
      author: text('#js_name'), published_at: text('#publish_time'), text: textContent,
      text_chars: [...textContent].length };
  }
  window.SocaiWeixinPageTools = { searchState, searchResults, articleState, articleDetail };
})();
