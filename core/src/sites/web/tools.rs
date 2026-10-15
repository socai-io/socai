use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::{Backend as LlmProvider, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;
use crate::sites::registry::SiteSpec;

pub const WEB_KNOWLEDGE: &str = include_str!("knowledge.md");

const READ_SCRIPT: &str = r#"
const maxChars = __MAX_CHARS__;
const maxLinks = __MAX_LINKS__;
const visible = (el) => {
  const style = getComputedStyle(el);
  const rect = el.getBoundingClientRect();
  return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
};
const clean = (value, limit = 240) => String(value || '').replace(/\s+/g, ' ').trim().slice(0, limit);
document.querySelectorAll('[data-socai-web-ref]').forEach((el) => el.removeAttribute('data-socai-web-ref'));
const candidates = Array.from(document.querySelectorAll('a[href], button, input, textarea, select, [role="button"], [contenteditable="true"]'))
  .filter(visible).slice(0, maxLinks);
const controls = candidates.map((el, index) => {
  const ref = `w${index + 1}`;
  el.setAttribute('data-socai-web-ref', ref);
  const tag = el.tagName.toLowerCase();
  const item = {
    ref,
    kind: tag === 'a' ? 'link' : (tag === 'input' || tag === 'textarea' || el.isContentEditable) ? 'input' : tag,
    text: clean(el.innerText || el.textContent || el.getAttribute('aria-label') || el.getAttribute('title') || el.getAttribute('placeholder')),
  };
  if (tag === 'a') item.url = el.href;
  if (tag === 'input' || tag === 'textarea') {
    item.input_type = el.getAttribute('type') || tag;
    item.placeholder = clean(el.getAttribute('placeholder'));
  }
  return item;
});
return {
  url: location.href,
  title: document.title,
  ready_state: document.readyState,
  scroll: { y: Math.round(scrollY), height: document.documentElement.scrollHeight, viewport: innerHeight },
  text: String(document.body?.innerText || '').replace(/\n{3,}/g, '\n\n').slice(0, maxChars),
  controls,
};
"#;

fn json_text(value: &Value) -> ToolResult {
    ToolResult::text(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

fn required_text<'a>(input: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required argument: {key}"))
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || octets[0] == 0
        || octets[0] == 100 && (64..=127).contains(&octets[1])
        || octets[0] == 169 && octets[1] == 254
        || octets[0] >= 224
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || ip.to_ipv4_mapped().is_some_and(is_blocked_ipv4)
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => is_blocked_ipv4(value),
        IpAddr::V6(value) => is_blocked_ipv6(value),
    }
}

async fn validate_public_url(raw: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|error| anyhow::anyhow!("invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("only public http and https URLs are supported");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("URLs containing credentials are not supported");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL must include a host"))?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host == "metadata.google.internal"
    {
        anyhow::bail!("local and internal hosts are blocked");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            anyhow::bail!("private and local network addresses are blocked");
        }
    } else {
        let port = url.port_or_known_default().unwrap_or(443);
        let resolved = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|error| anyhow::anyhow!("could not resolve URL host: {error}"))?;
        let mut saw_address = false;
        for address in resolved {
            saw_address = true;
            if is_blocked_ip(address.ip()) {
                anyhow::bail!("URL host resolves to a private or local network address");
            }
        }
        if !saw_address {
            anyhow::bail!("URL host did not resolve");
        }
    }
    Ok(url)
}

pub fn web_tools(page: Arc<PageSession>) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(NavigateTool { page: page.clone() }),
        Arc::new(ReadTool { page: page.clone() }),
        Arc::new(CollectLinksTool { page: page.clone() }),
        Arc::new(ClickTool { page: page.clone() }),
        Arc::new(TypeTool { page: page.clone() }),
        Arc::new(BackTool { page }),
    ]
}

pub async fn web_agent_tools(
    page: Arc<PageSession>,
    _llm_provider: Arc<dyn LlmProvider>,
) -> anyhow::Result<Vec<Arc<dyn Tool>>> {
    Ok(web_tools(page))
}

pub fn web_agent_instructions(extra: &str) -> String {
    let extra = extra.trim();
    if extra.is_empty() {
        WEB_KNOWLEDGE.trim().to_string()
    } else {
        format!("{extra}\n\n{}", WEB_KNOWLEDGE.trim())
    }
}

pub static WEB_SITE: SiteSpec = SiteSpec {
    id: "web",
    about: "Public Web browser research",
    home_url: "about:blank",
    agent_tools: |page, llm| Box::pin(web_agent_tools(page, llm)),
    default_agent_tools: None,
    agent_instructions: web_agent_instructions,
    default_agent_instructions: None,
    commands: &[],
};

struct CollectLinksTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for CollectLinksTool {
    fn name(&self) -> &str {
        "web_collect_links"
    }

    fn description(&self) -> &str {
        "Collect exact public link URLs from the current search or list page with nearby visible context. Use url_contains to narrow results (for example /abs/ on arXiv), select relevant records by title/context, then navigate only to returned URLs. When has_more is true, continue with offset=next_offset; never repeat unchanged arguments on the same page."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url_contains": {"type": "string", "description": "Optional case-insensitive substring required in each link URL"},
                "text_contains": {"type": "string", "description": "Optional case-insensitive substring required in link text or nearby context"},
                "max_links": {"type": "integer", "minimum": 1, "maximum": 40, "default": 20},
                "offset": {"type": "integer", "minimum": 0, "maximum": 100000, "default": 0, "description": "Continuation offset returned by a previous call"},
                "context_chars": {"type": "integer", "minimum": 80, "maximum": 400, "default": 240}
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let url_contains = input
            .get("url_contains")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let text_contains = input
            .get("text_contains")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if url_contains.chars().count() > 256 || text_contains.chars().count() > 256 {
            anyhow::bail!("link filter exceeds the 256 character limit");
        }
        let max_links = input
            .get("max_links")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .clamp(1, 40);
        let offset = input
            .get("offset")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(100_000);
        let context_chars = input
            .get("context_chars")
            .and_then(Value::as_u64)
            .unwrap_or(240)
            .clamp(80, 400);
        let script = format!(
            r#"
const urlNeedle = {url_contains};
const textNeedle = {text_contains};
const maxLinks = {max_links};
const offset = {offset};
const offsetLimit = 100000;
const contextChars = {context_chars};
const outputBudget = 24000;
const clean = (value, limit) => String(value || '').replace(/\s+/g, ' ').trim().slice(0, limit);
const visible = (element) => {{
  const style = getComputedStyle(element);
  const rect = element.getBoundingClientRect();
  return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
}};
const seen = new Set();
const links = [];
let matched = 0;
let outputChars = 0;
let hasMore = false;
for (const anchor of document.querySelectorAll('a[href]')) {{
  if (!visible(anchor)) continue;
  let parsed;
  try {{ parsed = new URL(anchor.href, location.href); }} catch {{ continue; }}
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) continue;
  if (parsed.href.length > 2048 || seen.has(parsed.href)) continue;
  if (urlNeedle && !parsed.href.toLocaleLowerCase().includes(urlNeedle.toLocaleLowerCase())) continue;
  const container = anchor.closest('article, li, .arxiv-result, .result, .search-result, [role="listitem"], tr') || anchor.parentElement;
  const text = clean(anchor.innerText || anchor.getAttribute('aria-label'), 180);
  const context = clean(container?.innerText, contextChars);
  if (textNeedle && !`${{text}} ${{context}}`.toLocaleLowerCase().includes(textNeedle.toLocaleLowerCase())) continue;
  seen.add(parsed.href);
  if (matched >= offsetLimit) break;
  if (matched++ < offset) continue;
  const item = {{url: parsed.href, text, context}};
  const itemChars = JSON.stringify(item).length;
  if (links.length >= maxLinks || outputChars + itemChars > outputBudget) {{ hasMore = true; break; }}
  links.push(item);
  outputChars += itemChars;
}}
return {{
  url: clean(location.href, 2048),
  title: clean(document.title, 180),
  links,
  has_more: hasMore,
  next_offset: hasMore ? offset + links.length : null,
}};
"#,
            url_contains = serde_json::to_string(&url_contains)?,
            text_contains = serde_json::to_string(&text_contains)?,
        );
        let mut result = self.page.evaluate_json(&script).await?;
        let candidates = result
            .get_mut("links")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .unwrap_or_default();
        let mut public_links = Vec::with_capacity(candidates.len());
        let mut public_origins: HashMap<(String, u16), bool> = HashMap::new();
        for candidate in candidates {
            let Some(url) = candidate.get("url").and_then(Value::as_str) else {
                continue;
            };
            let Ok(parsed) = reqwest::Url::parse(url) else {
                continue;
            };
            if !matches!(parsed.scheme(), "http" | "https")
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                continue;
            }
            let Some(host) = parsed.host_str() else {
                continue;
            };
            let origin = (
                host.trim_end_matches('.').to_ascii_lowercase(),
                parsed.port_or_known_default().unwrap_or(443),
            );
            let is_public = match public_origins.get(&origin) {
                Some(value) => *value,
                None => {
                    let value = validate_public_url(url).await.is_ok();
                    public_origins.insert(origin, value);
                    value
                }
            };
            if is_public {
                public_links.push(candidate);
            }
        }
        result["links"] = Value::Array(public_links);
        Ok(json_text(&result))
    }
}

struct NavigateTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for NavigateTool {
    fn name(&self) -> &str {
        "web_navigate"
    }

    fn description(&self) -> &str {
        "Navigate the real browser to one public http/https URL. Private-network, local, credential-bearing, and non-Web URLs are blocked."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"url": {"type": "string", "description": "Exact public URL to open"}},
            "required": ["url"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let raw = required_text(&input, "url")?;
        let url = validate_public_url(raw).await?;
        self.page.navigate_with_timeout(url.as_str(), 60.0).await?;
        Ok(json_text(&self.page.page_info().await?))
    }
}

struct ReadTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "web_read"
    }

    fn description(&self) -> &str {
        "Read the current real detail page as compact visible text plus referenced links and controls. Do not use this on an arXiv search page; call web_collect_links there. Input values and raw HTML are never returned."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max_chars": {"type": "integer", "minimum": 1000, "maximum": 30000, "default": 12000},
                "max_controls": {"type": "integer", "minimum": 1, "maximum": 200, "default": 80},
                "wait_ms": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 500},
                "scroll_by": {"type": "integer", "minimum": -5000, "maximum": 5000, "description": "Optional vertical pixels to scroll before reading"}
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let page_info = self.page.page_info().await?;
        let current_url = page_info.get("url").and_then(Value::as_str).unwrap_or("");
        if reqwest::Url::parse(current_url).is_ok_and(|url| {
            matches!(url.host_str(), Some("arxiv.org" | "www.arxiv.org"))
                && url.path().starts_with("/search")
        }) {
            anyhow::bail!(
                "arXiv search results require web_collect_links with url_contains '/abs/'"
            );
        }
        let max_chars = input
            .get("max_chars")
            .and_then(Value::as_u64)
            .unwrap_or(12_000)
            .clamp(1_000, 30_000);
        let max_links = input
            .get("max_controls")
            .and_then(Value::as_u64)
            .unwrap_or(80)
            .clamp(1, 200);
        let wait_ms = input
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .min(10_000);
        if let Some(delta) = input.get("scroll_by").and_then(Value::as_i64) {
            self.page.scroll(delta.clamp(-5_000, 5_000)).await?;
        }
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        let script = READ_SCRIPT
            .replace("__MAX_CHARS__", &max_chars.to_string())
            .replace("__MAX_LINKS__", &max_links.to_string());
        Ok(json_text(&self.page.evaluate_json(&script).await?))
    }
}

struct ClickTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for ClickTool {
    fn name(&self) -> &str {
        "web_click"
    }

    fn description(&self) -> &str {
        "Click a visible page control from the latest web_read, preferably by its ref. A text lookup is available when a page rerenders."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ref": {"type": "string", "description": "Control ref such as w12 from web_read"},
                "text": {"type": "string", "description": "Visible control text fallback"},
                "wait_ms": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 800}
            }
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let reference = input
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let text = input
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if reference.is_empty() && text.is_empty() {
            anyhow::bail!("provide ref or text");
        }
        let reference_json = serde_json::to_string(reference)?;
        let text_json = serde_json::to_string(text)?;
        let script = format!(
            r#"
const ref = {reference_json};
const wanted = {text_json}.toLowerCase();
const clean = (value) => String(value || '').replace(/\s+/g, ' ').trim();
let el = ref ? document.querySelector(`[data-socai-web-ref="${{CSS.escape(ref)}}"]`) : null;
if (!el && wanted) {{
  const nodes = Array.from(document.querySelectorAll('a[href], button, [role="button"]'));
  el = nodes.find((node) => clean(node.innerText || node.textContent || node.getAttribute('aria-label')).toLowerCase() === wanted)
    || nodes.find((node) => clean(node.innerText || node.textContent || node.getAttribute('aria-label')).toLowerCase().includes(wanted));
}}
if (!el) return {{ok: false, error: 'control not found; call web_read again'}};
el.scrollIntoView({{block: 'center', inline: 'center'}});
const rect = el.getBoundingClientRect();
return {{ok: true, x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, text: clean(el.innerText || el.textContent || el.getAttribute('aria-label')).slice(0, 160)}};
"#
        );
        let target = self.page.evaluate_json(&script).await?;
        if !target.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            anyhow::bail!(
                "{}",
                target
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("control not found")
            );
        }
        let x = target
            .get("x")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("control has no click position"))?;
        let y = target
            .get("y")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("control has no click position"))?;
        self.page.click(x, y).await?;
        let wait_ms = input
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(800)
            .min(10_000);
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        Ok(json_text(
            &json!({"ok": true, "clicked": target.get("text"), "page": self.page.page_info().await?}),
        ))
    }
}

struct TypeTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for TypeTool {
    fn name(&self) -> &str {
        "web_type"
    }

    fn description(&self) -> &str {
        "Type into a visible public search or filter field referenced by web_read. The typed text is not echoed in the tool result. Never use this for credentials or sensitive data."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ref": {"type": "string", "description": "Input ref from web_read"},
                "text": {"type": "string", "description": "Text to type"},
                "clear": {"type": "boolean", "default": true},
                "press_enter": {"type": "boolean", "default": false}
            },
            "required": ["ref", "text"]
        })
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let reference = required_text(&input, "ref")?;
        let text = input
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required argument: text"))?;
        if text.chars().count() > 2_000 {
            anyhow::bail!("text exceeds the 2000 character limit");
        }
        let clear = input.get("clear").and_then(Value::as_bool).unwrap_or(true);
        let reference_json = serde_json::to_string(reference)?;
        let script = format!(
            r#"
const ref = {reference_json};
const el = document.querySelector(`[data-socai-web-ref="${{CSS.escape(ref)}}"]`);
if (!el) return {{ok: false, error: 'input not found; call web_read again'}};
if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el.isContentEditable)) return {{ok: false, error: 'referenced control is not a text input'}};
el.scrollIntoView({{block: 'center'}}); el.focus();
if ({clear}) {{
  if (el.isContentEditable) el.textContent = '';
  else el.value = '';
  el.dispatchEvent(new InputEvent('input', {{bubbles: true, inputType: 'deleteContentBackward'}}));
}}
return {{ok: true, kind: el.tagName.toLowerCase()}};
"#
        );
        let focused = self.page.evaluate_json(&script).await?;
        if !focused.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            anyhow::bail!(
                "{}",
                focused
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("input not found")
            );
        }
        self.page.type_chars(text).await?;
        let pressed_enter = input
            .get("press_enter")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if pressed_enter {
            self.page.press_key("Enter").await?;
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        Ok(json_text(
            &json!({"ok": true, "typed": true, "pressed_enter": pressed_enter, "page": self.page.page_info().await?}),
        ))
    }
}

struct BackTool {
    page: Arc<PageSession>,
}

#[async_trait]
impl Tool for BackTool {
    fn name(&self) -> &str {
        "web_back"
    }

    fn description(&self) -> &str {
        "Go back one entry in the current browser tab and return the resulting page information."
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"wait_ms": {"type": "integer", "minimum": 0, "maximum": 10000, "default": 800}}})
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        self.page
            .evaluate_json("history.back(); return {ok: true};")
            .await?;
        let wait_ms = input
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(800)
            .min(10_000);
        if wait_ms > 0 {
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        Ok(json_text(&self.page.page_info().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_local_and_non_web_urls() {
        for value in [
            "file:///etc/passwd",
            "http://localhost:8080/",
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "https://user:secret@example.com/",
        ] {
            assert!(validate_public_url(value).await.is_err(), "{value}");
        }
    }

    #[tokio::test]
    async fn accepts_public_web_url() {
        assert_eq!(
            validate_public_url("https://8.8.8.8/example")
                .await
                .unwrap()
                .host_str(),
            Some("8.8.8.8")
        );
    }
}
