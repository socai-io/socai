#!/usr/bin/env node
// Node.js >= 22. Search, redirects and article reads all run in real Chrome.
// HTTP fetch below is restricted to the local Chrome debugging endpoint.
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';

const { values: opts, positionals } = parseArgs({ allowPositionals: true, options: {
  limit: { type: 'string', default: '1' },
  out: { type: 'string', default: 'sogou-weixin-output' },
  timeout: { type: 'string', default: '45' },
  cdp: { type: 'string' },
  chrome: { type: 'string' },
  help: { type: 'boolean' },
} });
if (opts.help || !positionals.length) {
  console.log('Usage: node scripts/sogou-weixin-chrome.mjs "关键词" [--limit 3] [--out /tmp/weixin] [--timeout 45] [--cdp http://127.0.0.1:9222] [--chrome /path/to/chrome]');
  process.exit(opts.help ? 0 : 1);
}
const limit = Number(opts.limit), timeout = Number(opts.timeout) * 1000;
if (!Number.isInteger(limit) || limit < 1 || limit > 10 || !Number.isFinite(timeout) || timeout < 1000) {
  throw new Error('--limit must be 1–10; --timeout must be at least 1 second');
}
const query = positionals.join(' '), out = resolve(opts.out);
const pause = ms => new Promise(r => setTimeout(r, ms));
let chrome, profile, socket, endpoint, targetId;

async function localRequest(path, method = 'GET') {
  const response = await fetch(`${endpoint}${path}`, { method, signal: AbortSignal.timeout(5000) });
  if (!response.ok) throw new Error(`Chrome endpoint: HTTP ${response.status}`);
  return response.json();
}
async function connect(url) {
  socket = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Chrome connection timed out')), 10000);
    socket.onopen = () => { clearTimeout(timer); resolve(); };
    socket.onerror = () => { clearTimeout(timer); reject(new Error('Chrome WebSocket failed')); };
  });
  let id = 0;
  const pending = new Map();
  socket.onmessage = event => {
    const message = JSON.parse(event.data), request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id); clearTimeout(request.timer);
    if (message.error) request.reject(new Error(message.error.message));
    else request.resolve(message.result);
  };
  socket.onclose = () => {
    for (const p of pending.values()) { clearTimeout(p.timer); p.reject(new Error('Chrome disconnected')); }
    pending.clear();
  };
  return (method, params = {}) => new Promise((resolve, reject) => {
    const key = ++id;
    const timer = setTimeout(() => { pending.delete(key); reject(new Error(`${method} timed out`)); }, 15000);
    pending.set(key, { resolve, reject, timer });
    socket.send(JSON.stringify({ id: key, method, params }));
  });
}

// Selectors verified against live Sogou search and WeChat article pages.
function snapshot() {
  const text = selector => document.querySelector(selector)?.innerText?.trim() || '';
  return {
    url: location.href, title: text('#activity-name') || document.title,
    author: text('#js_name'), publishedAt: text('#publish_time'),
    body: text('#js_content'), pageText: document.body?.innerText || '',
    html: document.documentElement.outerHTML,
    links: [...document.querySelectorAll('.news-list h3 a')]
      .map(a => ({ title: a.innerText.trim(), url: a.href }))
      .filter(a => {
        const u = new URL(a.url);
        return u.protocol === 'https:' &&
          ((u.hostname === 'weixin.sogou.com' && u.pathname === '/link') || u.hostname === 'mp.weixin.qq.com');
      }),
  };
}
function isArticle(page) {
  const url = new URL(page.url);
  return url.hostname === 'mp.weixin.qq.com' && /^\/s(?:\/|$)/.test(url.pathname) && !!page.body;
}

try {
  await mkdir(out, { recursive: true });
  if (opts.cdp) {
    const url = new URL(opts.cdp);
    if (url.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)) {
      throw new Error('--cdp must be a local HTTP Chrome endpoint');
    }
    endpoint = url.origin;
  } else {
    profile = await mkdtemp(join(tmpdir(), 'sogou-weixin-chrome-'));
    const executable = opts.chrome || process.env.CHROME_PATH || (process.platform === 'darwin'
      ? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
      : process.platform === 'win32'
        ? 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe' : 'google-chrome');
    chrome = spawn(executable, [`--user-data-dir=${profile}`, '--remote-debugging-port=0',
      '--remote-debugging-address=127.0.0.1', '--no-first-run', '--no-default-browser-check', 'about:blank'], { stdio: 'ignore' });
    let launchError;
    chrome.on('error', error => { launchError = error; });
    const deadline = Date.now() + 15000;
    while (!endpoint && Date.now() < deadline) {
      if (launchError) throw launchError;
      if (chrome.exitCode !== null) throw new Error(`Chrome exited: ${chrome.exitCode}`);
      try {
        const port = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0];
        if (/^\d+$/.test(port)) endpoint = `http://127.0.0.1:${port}`;
      } catch { /* Chrome has not written its port yet. */ }
      if (!endpoint) await pause(200);
    }
    if (!endpoint) throw new Error('Chrome did not start within 15 seconds');
  }
  const target = await localRequest('/json/new?about:blank', 'PUT');
  targetId = target.id;
  let call = await connect(target.webSocketDebuggerUrl);
  const evaluate = async expression => {
    const result = await call('Runtime.evaluate', { expression, returnByValue: true });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
    return result.result.value;
  };
  const read = () => evaluate(`(${snapshot.toString()})()`);
  async function navigate(url, ready, referrer) {
    const result = await call('Page.navigate', { url, ...(referrer ? { referrer } : {}) });
    if (result.errorText) throw new Error(result.errorText);
    const deadline = Date.now() + timeout;
    let last, previousBody, stable = 0;
    while (Date.now() < deadline) {
      await pause(750);
      try { last = await read(); } catch (error) {
        if (/context|navigat/i.test(error.message)) continue;
        throw error;
      }
      if (ready(last)) {
        // Require consecutive identical article text snapshots before saving.
        if (!last.body) return last;
        stable = last.body === previousBody ? stable + 1 : 0;
        previousBody = last.body;
        if (stable >= 2) return last;
      }
    }
    if (!last) throw new Error('No browser document available');
    return last;
  }
  const searchUrl = `https://weixin.sogou.com/weixin?${new URLSearchParams({ type: '2', query })}`;
  console.error(`Search in Chrome: ${query}`);
  const search = await navigate(searchUrl, page => page.links.length > 0);
  await writeFile(join(out, 'search.html'), search.html);
  await writeFile(join(out, 'search.txt'), search.pageText);
  const links = [...new Map(search.links.map(a => [a.url, a])).values()].slice(0, limit);
  const report = { query, searchUrl, capturedAt: new Date().toISOString(), requested: limit, articles: [] };
  if (!links.length) report.error = 'No search results: inspect search.html/search.txt for a challenge, empty results or changed markup.';
  for (const [index, link] of links.entries()) {
    console.error(`Read in Chrome (${index + 1}/${links.length}): ${link.title}`);
    const prefix = `article-${String(index + 1).padStart(2, '0')}`;
    let article;
    try {
      // A fresh tab avoids old-page unload handlers and stale article content.
      socket.close();
      await fetch(`${endpoint}/json/close/${targetId}`, { signal: AbortSignal.timeout(3000) });
      const next = await localRequest('/json/new?about:blank', 'PUT');
      targetId = next.id;
      call = await connect(next.webSocketDebuggerUrl);
      for (const suffix of ['.txt', '.html', '.page.txt']) {
        await rm(join(out, `${prefix}${suffix}`), { force: true });
      }
      const page = await navigate(link.url, isArticle, searchUrl);
      await writeFile(join(out, `${prefix}.html`), page.html);
      await writeFile(join(out, `${prefix}.page.txt`), page.pageText);
      const ok = isArticle(page);
      if (ok) await writeFile(join(out, `${prefix}.txt`), page.body);
      article = { searchTitle: link.title, searchLink: link.url, url: page.url, title: page.title,
        author: page.author, publishedAt: page.publishedAt, status: ok ? 'ok' : 'unavailable',
        bodyChars: ok ? [...page.body].length : 0, body: ok ? page.body : '',
        ...(ok ? {} : { error: 'No WeChat article body found. Inspect saved page for verification, removal or redirect failure.' }) };
    } catch (error) {
      article = { searchTitle: link.title, searchLink: link.url, status: 'error', error: error.message };
      try {
        const page = await read();
        article.url = page.url;
        await writeFile(join(out, `${prefix}.html`), page.html);
        await writeFile(join(out, `${prefix}.page.txt`), page.pageText);
      } catch { /* Retain the original error if Chrome is no longer reachable. */ }
    }
    report.articles.push(article);
    await writeFile(join(out, 'results.json'), JSON.stringify(report, null, 2));
  }
  await writeFile(join(out, 'results.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
  if (!report.articles.length || report.articles.some(a => a.status !== 'ok')) process.exitCode = 2;
} catch (error) {
  console.error(error.message); process.exitCode = 1;
} finally {
  if (targetId) {
    // This closes only our new tab, including when attaching to existing Chrome.
    try { await fetch(`${endpoint}/json/close/${targetId}`, { signal: AbortSignal.timeout(3000) }); } catch {}
  }
  socket?.close();
  if (chrome && chrome.exitCode === null) {
    chrome.kill('SIGTERM');
    for (let i = 0; i < 30 && chrome.exitCode === null; i++) await pause(100);
  }
  if (profile && chrome?.exitCode !== null) await rm(profile, { recursive: true, force: true, maxRetries: 3 }).catch(() => {});
}
