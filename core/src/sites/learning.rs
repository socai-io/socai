//! Runtime discovery and loading for site learning packages.
//!
//! Site metadata and page-context tools live in `manifest.json`, following the
//! domain-skill model used by browser-use and ego-lite. Rust adapters remain a
//! separate implementation detail because native code cannot be loaded from a
//! learning directory at runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::agent::{SharedTool, Tool, ToolContext, ToolResult};
use crate::cdp::PageSession;

const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_RESOURCE_BYTES: usize = 1024 * 1024;
const VALUE_TYPES: &[&str] = &["string", "number", "integer", "boolean", "array", "object"];

pub(crate) struct EmbeddedSiteSkillFile {
    pub path: &'static str,
    pub contents: &'static str,
}

pub(crate) struct EmbeddedSiteSkill {
    pub id: &'static str,
    pub manifest: &'static str,
    pub files: &'static [EmbeddedSiteSkillFile],
}

include!(concat!(env!("OUT_DIR"), "/site_skill_assets.rs"));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSkillManifest {
    pub id: String,
    pub name: String,
    pub domains: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub browser_tools: BTreeMap<String, BrowserToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserToolDefinition {
    pub description: String,
    pub path: String,
    /// Bundle expression that exposes the callable table, for example
    /// `window.SocaiDouyinPageScripts`. Omit both `binding` and `callable`
    /// when `path` itself contains an anonymous async function.
    #[serde(default)]
    pub binding: Option<String>,
    #[serde(default)]
    pub callable: Option<String>,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub returns: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSkillContext {
    pub id: String,
    pub name: String,
    pub domains: Vec<String>,
    pub knowledge: Vec<SiteKnowledgeNote>,
    pub browser_tools: BTreeMap<String, BrowserToolDefinition>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteKnowledgeNote {
    pub path: String,
    pub content: String,
}

enum SkillSource {
    Embedded(&'static EmbeddedSiteSkill),
    Local(PathBuf),
}

struct LoadedSiteSkill {
    manifest: SiteSkillManifest,
    source: SkillSource,
}

/// User-editable site-skill directory. A complete local package with the same
/// id overrides its bundled counterpart without recompiling socai.
pub fn site_skills_root() -> PathBuf {
    if let Some(path) = non_empty_env("SOCAI_SITE_SKILLS_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = non_empty_env("SOCAI_HOME") {
        return PathBuf::from(path).join("site-skills");
    }
    dirs::home_dir()
        .map(|home| home.join(".socai/site-skills"))
        .unwrap_or_else(|| PathBuf::from(".socai/site-skills"))
}

/// List valid bundled skills plus local additions. A local package shadows a
/// bundled package with the same id.
pub fn available_site_skills() -> Result<Vec<SiteSkillManifest>> {
    let mut manifests = BTreeMap::new();
    for package in BUILTIN_SITE_SKILLS {
        let manifest = parse_manifest(package.manifest, &format!("bundled skill {}", package.id))?;
        manifests.insert(manifest.id.clone(), manifest);
    }

    let root = site_skills_root();
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.filter_map(std::result::Result::ok) {
            let Some(directory_id) = entry.file_name().to_str().map(str::to_string) else {
                tracing::warn!(path = %entry.path().display(), "skipping non-UTF-8 site skill directory");
                continue;
            };
            if let Err(error) = validate_site_id(&directory_id) {
                tracing::warn!(path = %entry.path().display(), %error, "skipping invalid site skill directory");
                continue;
            }
            match load_local_package(&root, &directory_id) {
                Ok(Some((manifest, _))) => {
                    manifests.insert(manifest.id.clone(), manifest);
                }
                Ok(None) => {}
                Err(error) => {
                    manifests.remove(&directory_id);
                    tracing::warn!(site = directory_id, %error, "skipping invalid local site skill");
                }
            }
        }
    }
    Ok(manifests.into_values().collect())
}

/// Return only skills whose manifest domain patterns match the supplied URL.
pub fn site_skills_for_url(url: &str) -> Result<Vec<SiteSkillManifest>> {
    let hostname = url_hostname(url);
    if hostname.is_empty() {
        return Ok(Vec::new());
    }
    Ok(available_site_skills()?
        .into_iter()
        .filter(|skill| {
            skill
                .domains
                .iter()
                .any(|pattern| domain_matches(&hostname, pattern))
        })
        .collect())
}

/// Load notes and exact browser-tool schemas for one site on demand.
pub fn load_site_skill_context(site_id: &str) -> Result<SiteSkillContext> {
    let skill = find_site_skill(site_id)?;
    let mut knowledge = Vec::new();
    for path in &skill.manifest.notes {
        let content = skill.read_resource(path)?;
        knowledge.push(SiteKnowledgeNote {
            path: path.clone(),
            content,
        });
    }
    Ok(SiteSkillContext {
        id: skill.manifest.id,
        name: skill.manifest.name,
        domains: skill.manifest.domains,
        knowledge,
        browser_tools: skill.manifest.browser_tools,
    })
}

/// Compose a native host preamble with the selected site's notes. Invalid
/// local packages fail closed for browser execution but do not prevent the host
/// agent itself from starting.
pub fn site_agent_instructions(site_id: &str, extra: &str) -> String {
    let knowledge = match load_site_skill_context(site_id) {
        Ok(context) => context
            .knowledge
            .into_iter()
            .map(|note| note.content)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Err(error) => {
            tracing::warn!(site = site_id, %error, "failed to load site skill notes");
            String::new()
        }
    };
    join_instructions(extra, &knowledge)
}

/// Generic browser-use-style tools. They make manifest-only packages usable
/// by an agent without adding a Rust adapter or a CLI command table.
pub fn site_learning_tools(page: Arc<PageSession>) -> Vec<SharedTool> {
    build_site_learning_tools(page, None)
}

/// Desktop source selection restricts navigation and execution independently
/// of model instructions. Other hosts retain unrestricted site discovery.
pub fn scoped_site_learning_tools(page: Arc<PageSession>, sites: &[String]) -> Vec<SharedTool> {
    build_site_learning_tools(page, Some(Arc::new(sites.iter().cloned().collect())))
}

fn build_site_learning_tools(
    page: Arc<PageSession>,
    allowed_sites: Option<Arc<BTreeSet<String>>>,
) -> Vec<SharedTool> {
    vec![
        Arc::new(NavigateSiteTool {
            page: page.clone(),
            allowed_sites: allowed_sites.clone(),
        }),
        Arc::new(ReadSiteSkillsTool {
            page: page.clone(),
            allowed_sites: allowed_sites.clone(),
        }),
        Arc::new(RunSiteBrowserTool {
            page,
            allowed_sites,
        }),
    ]
}

fn check_selected_site(allowed: &Option<Arc<BTreeSet<String>>>, site: &str) -> Result<()> {
    if allowed.as_ref().is_some_and(|sites| !sites.contains(site)) {
        anyhow::bail!("platform {site} is not selected for this task; ask the user to change the source selection");
    }
    Ok(())
}

fn check_selected_context(allowed: &Option<Arc<BTreeSet<String>>>, context: &Value) -> Result<()> {
    if let Some(skills) = context["skills"].as_array() {
        for skill in skills {
            if let Some(site) = skill["id"].as_str() {
                check_selected_site(allowed, site)?;
            }
        }
    }
    Ok(())
}

struct NavigateSiteTool {
    page: Arc<PageSession>,
    allowed_sites: Option<Arc<BTreeSet<String>>>,
}

#[async_trait]
impl Tool for NavigateSiteTool {
    fn name(&self) -> &str {
        "navigate_site"
    }

    fn description(&self) -> &str {
        "Navigate the current browser tab to an HTTPS URL covered by an installed site-skill \
         manifest. The matching skill notes and browser-tool schemas are returned after the \
         final page loads, so read them before using platform-specific browser tools."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "HTTPS page URL on a domain declared by an installed site skill." }
            },
            "required": ["url"]
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("navigate_site requires a non-empty `url`")?;
        validate_navigation_url(url)?;
        let skills = site_skills_for_url(url)?;
        for skill in &skills {
            check_selected_site(&self.allowed_sites, &skill.id)?;
        }
        if skills.is_empty() {
            anyhow::bail!("no installed site skill matches navigation URL: {url}");
        }
        self.page.navigate_with_timeout(url, 60.0).await?;
        let result = current_site_skill_contexts(&self.page).await?;
        check_selected_context(&self.allowed_sites, &result)?;
        if result["skills"].as_array().is_none_or(Vec::is_empty) {
            anyhow::bail!(
                "navigation left the declared site-skill domains: {}",
                result["url"]
            );
        }
        Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
    }
}

struct ReadSiteSkillsTool {
    page: Arc<PageSession>,
    allowed_sites: Option<Arc<BTreeSet<String>>>,
}

#[async_trait]
impl Tool for ReadSiteSkillsTool {
    fn name(&self) -> &str {
        "read_site_skills"
    }

    fn description(&self) -> &str {
        "Discover site skills for the current browser page by its real hostname, then read their \
         durable notes and exact browser-tool schemas. Call this after a page changes outside \
         navigate_site or whenever the active domain is uncertain."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolResult> {
        let result = current_site_skill_contexts(&self.page).await?;
        check_selected_context(&self.allowed_sites, &result)?;
        Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
    }
}

struct RunSiteBrowserTool {
    page: Arc<PageSession>,
    allowed_sites: Option<Arc<BTreeSet<String>>>,
}

#[async_trait]
impl Tool for RunSiteBrowserTool {
    fn name(&self) -> &str {
        "run_site_browser_tool"
    }

    fn description(&self) -> &str {
        "Run one browser-context tool declared by a site skill previously returned by \
         read_site_skills or navigate_site. The current page must still match that skill's \
         domains, and arguments/results are checked against its manifest schema. List and \
         comment extractors automatically scroll, deduplicate, and accumulate up to `limit`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "site_id": { "type": "string", "description": "Site-skill id returned by read_site_skills." },
                "tool_name": { "type": "string", "description": "Browser-tool name declared by that skill." },
                "args": { "type": "object", "description": "Arguments matching the declared browser-tool schema." }
            },
            "required": ["site_id", "tool_name"]
        })
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let site_id = required_tool_string(&input, "site_id")?;
        check_selected_site(&self.allowed_sites, site_id)?;
        let tool_name = required_tool_string(&input, "tool_name")?;
        let args = input.get("args");
        if args.is_some_and(|value| !value.is_object()) {
            anyhow::bail!("run_site_browser_tool `args` must be an object");
        }
        let mut result =
            run_site_browser_tool_collecting(&self.page, site_id, tool_name, args).await?;
        crate::sites::post_archive::save_site_media(
            &self.page,
            ctx,
            site_id,
            tool_name,
            &mut result,
        )
        .await;
        let page_url = self
            .page
            .page_info()
            .await
            .ok()
            .and_then(|info| info.get("url").and_then(Value::as_str).map(str::to_string));
        crate::sites::post_archive::persist_site_tool_result(
            ctx,
            site_id,
            tool_name,
            &result,
            page_url.as_deref(),
        );
        Ok(ToolResult::text(serde_json::to_string_pretty(&result)?))
    }
}

/// Collect lazy content lists up to the requested `limit`. The page-level
/// extractors remain platform-owned; this host loop only alternates extraction
/// and the platform's declared scroll action while retaining virtualized rows.
pub async fn run_site_browser_tool_collecting(
    page: &PageSession,
    site_id: &str,
    tool_name: &str,
    args: Option<&Value>,
) -> Result<Value> {
    let Some(scroll_tool) = pagination_scroll_tool(site_id, tool_name) else {
        return run_site_browser_tool(page, site_id, tool_name, args).await;
    };
    if args
        .and_then(|value| value.get("viewport_only"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return run_site_browser_tool(page, site_id, tool_name, args).await;
    }

    let target = args
        .and_then(|value| value.get("limit"))
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 100) as usize;
    let mut collection_args = args.cloned().unwrap_or_else(|| json!({}));
    collection_args["limit"] = json!(target);
    if collection_args.get("viewport_only").is_some() {
        collection_args["viewport_only"] = Value::Bool(false);
    }

    let mut collected = Vec::new();
    let mut indexes = BTreeMap::new();
    let mut stalls = 0usize;
    let mut at_end = false;
    for _ in 0..40 {
        let batch = run_site_browser_tool(page, site_id, tool_name, Some(&collection_args)).await?;
        let items = batch.as_array().with_context(|| {
            format!("paginated browser tool {site_id}.{tool_name} did not return an array")
        })?;
        let before = collected.len();
        merge_paginated_items(&mut collected, &mut indexes, items);
        if collected.len() >= target {
            break;
        }
        stalls = if collected.len() == before {
            stalls + 1
        } else {
            0
        };
        if stalls >= 4 || (at_end && stalls >= 2) {
            break;
        }

        let scroll = run_site_browser_tool(page, site_id, scroll_tool, None).await?;
        at_end = scroll
            .get("at_end")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        tokio::time::sleep(Duration::from_millis(if tool_name == "comments" {
            700
        } else {
            900
        }))
        .await;
    }
    collected.truncate(target);
    Ok(Value::Array(collected))
}

fn pagination_scroll_tool(site_id: &str, tool_name: &str) -> Option<&'static str> {
    match (site_id, tool_name) {
        ("linkedin" | "instagram" | "dy" | "tiktok" | "x", "comments") => Some("scrollComments"),
        ("linkedin" | "instagram" | "x", "searchResults") => Some("scrollResults"),
        ("instagram" | "x", "profilePosts") => Some("scrollPosts"),
        ("dy" | "tiktok", "videoCards") => Some("scrollFeed"),
        _ => None,
    }
}

fn merge_paginated_items(
    collected: &mut Vec<Value>,
    indexes: &mut BTreeMap<String, usize>,
    items: &[Value],
) {
    for item in items {
        let key = paginated_item_key(item);
        if let Some(index) = indexes.get(&key).copied() {
            // Keep evidence from every viewport. In particular, virtualized
            // comment rows can expose different reply subsets after a scroll.
            crate::sites::post_archive::merge_post_record(&mut collected[index], item);
        } else {
            indexes.insert(key, collected.len());
            collected.push(item.clone());
        }
    }
}

fn paginated_item_key(item: &Value) -> String {
    for field in [
        "comment_id",
        "post_id",
        "video_id",
        "shortcode",
        "id",
        "url",
    ] {
        if let Some(value) = item.get(field).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                return format!("{field}:{value}");
            }
        }
    }
    let author = item
        .get("author")
        .and_then(|value| match value {
            Value::String(value) => Some(value.as_str()),
            Value::Object(value) => value
                .get("username")
                .or_else(|| value.get("name"))
                .and_then(Value::as_str),
            _ => None,
        })
        .unwrap_or_default();
    let text = item
        .get("text")
        .or_else(|| item.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let time = item
        .get("published_at")
        .or_else(|| item.get("published_label"))
        .or_else(|| item.get("time"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !author.is_empty() || !text.is_empty() {
        return format!("content:{author}\n{text}\n{time}");
    }
    serde_json::to_string(item).unwrap_or_default()
}

async fn current_site_skill_contexts(page: &PageSession) -> Result<Value> {
    let info = page.page_info().await?;
    let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
    let mut contexts = Vec::new();
    for manifest in site_skills_for_url(url)? {
        contexts.push(load_site_skill_context(&manifest.id)?);
    }
    Ok(json!({ "url": url, "skills": contexts }))
}

fn required_tool_string<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("run_site_browser_tool requires a non-empty `{key}`"))
}

/// Resolve a manifest-declared browser tool, load its source from the selected
/// package, and execute it in the current page context.
pub async fn run_site_browser_tool(
    page: &PageSession,
    site_id: &str,
    tool_name: &str,
    args: Option<&Value>,
) -> Result<Value> {
    let skill = find_site_skill(site_id)?;
    let tool = skill
        .manifest
        .browser_tools
        .get(tool_name)
        .with_context(|| {
            format!("browser tool {tool_name:?} is not declared by site skill {site_id:?}")
        })?;
    ensure_page_matches_manifest(page, &skill.manifest).await?;
    validate_tool_arguments(tool_name, tool, args)?;
    let source = skill.read_resource(&tool.path)?;
    let expression = browser_tool_expression(site_id, tool_name, tool, &source, args)?;
    let result = page.evaluate_json(&expression).await?;
    validate_value_against_schema(
        &result,
        &tool.returns,
        &format!("browserTools.{tool_name}.returns"),
    )?;
    Ok(result)
}

impl LoadedSiteSkill {
    fn read_resource(&self, relative: &str) -> Result<String> {
        validate_relative_path(relative)?;
        match &self.source {
            SkillSource::Embedded(package) => package
                .files
                .iter()
                .find(|file| file.path == relative)
                .map(|file| file.contents.to_string())
                .with_context(|| {
                    format!(
                        "bundled site skill {:?} is missing resource {relative:?}",
                        self.manifest.id
                    )
                }),
            SkillSource::Local(root) => read_local_resource(root, relative),
        }
    }
}

fn find_site_skill(site_id: &str) -> Result<LoadedSiteSkill> {
    validate_site_id(site_id)?;
    if let Some((manifest, local_root)) = load_local_package(&site_skills_root(), site_id)? {
        return Ok(LoadedSiteSkill {
            manifest,
            source: SkillSource::Local(local_root),
        });
    }

    let package = BUILTIN_SITE_SKILLS
        .iter()
        .find(|package| package.id == site_id)
        .with_context(|| format!("site skill not found: {site_id:?}"))?;
    Ok(LoadedSiteSkill {
        manifest: parse_manifest(package.manifest, &format!("bundled skill {site_id}"))?,
        source: SkillSource::Embedded(package),
    })
}

fn load_local_package(
    configured_root: &Path,
    site_id: &str,
) -> Result<Option<(SiteSkillManifest, PathBuf)>> {
    validate_site_id(site_id)?;
    let package = configured_root.join(site_id);
    let manifest_path = package.join("manifest.json");
    let package_metadata = match fs::symlink_metadata(&package) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to stat site skill package {}", package.display())
            })
        }
    };
    if package_metadata.file_type().is_symlink() || !package_metadata.is_dir() {
        anyhow::bail!(
            "site skill package must be a real directory: {}",
            package.display()
        );
    }
    let manifest_metadata = match fs::symlink_metadata(&manifest_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to stat {}", manifest_path.display()))
        }
    };
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        anyhow::bail!(
            "site skill manifest must be a regular file: {}",
            manifest_path.display()
        );
    }

    let canonical_root = fs::canonicalize(configured_root).with_context(|| {
        format!(
            "failed to resolve site skills root {}",
            configured_root.display()
        )
    })?;
    let canonical_package = fs::canonicalize(&package)
        .with_context(|| format!("failed to resolve site skill package {}", package.display()))?;
    if !canonical_package.starts_with(&canonical_root) {
        anyhow::bail!(
            "site skill package escapes configured root: {}",
            package.display()
        );
    }
    let manifest = read_local_manifest(&canonical_package)?;
    if manifest.id != site_id {
        anyhow::bail!(
            "local site skill directory {site_id:?} declares id {:?}",
            manifest.id
        );
    }
    Ok(Some((manifest, canonical_package)))
}

fn read_local_manifest(root: &Path) -> Result<SiteSkillManifest> {
    let path = root.join("manifest.json");
    let content = read_bounded(&path, MAX_MANIFEST_BYTES)?;
    parse_manifest(&content, &path.display().to_string())
}

fn parse_manifest(content: &str, label: &str) -> Result<SiteSkillManifest> {
    if content.len() > MAX_MANIFEST_BYTES {
        anyhow::bail!("site skill manifest exceeds {MAX_MANIFEST_BYTES} bytes: {label}");
    }
    let manifest: SiteSkillManifest = serde_json::from_str(content)
        .with_context(|| format!("invalid site skill manifest: {label}"))?;
    validate_manifest(&manifest)
        .with_context(|| format!("invalid site skill manifest: {label}"))?;
    Ok(manifest)
}

fn validate_manifest(manifest: &SiteSkillManifest) -> Result<()> {
    validate_site_id(&manifest.id)?;
    if manifest.name.trim().is_empty() {
        anyhow::bail!("name must not be empty");
    }
    if manifest.domains.is_empty() {
        anyhow::bail!("domains must not be empty");
    }
    for domain in &manifest.domains {
        validate_domain_pattern(domain)?;
    }
    let mut resources = BTreeSet::new();
    for note in &manifest.notes {
        validate_relative_path(note)?;
        if !note.ends_with(".md") {
            anyhow::bail!("note must be a markdown file: {note:?}");
        }
        resources.insert(note);
    }
    for (name, tool) in &manifest.browser_tools {
        validate_tool_name(name)?;
        if tool.description.trim().is_empty() {
            anyhow::bail!("browserTools.{name}.description must not be empty");
        }
        validate_relative_path(&tool.path)?;
        resources.insert(&tool.path);
        validate_tool_schema(name, tool)?;
        match (&tool.binding, &tool.callable) {
            (None, None) => {}
            (Some(binding), Some(callable)) => {
                validate_js_path(binding, "binding")?;
                validate_js_identifier(callable, "callable")?;
            }
            _ => anyhow::bail!(
                "browserTools.{name} must declare both binding and callable, or neither"
            ),
        }
    }
    Ok(())
}

fn validate_tool_schema(name: &str, tool: &BrowserToolDefinition) -> Result<()> {
    let args = tool
        .args
        .as_object()
        .with_context(|| format!("browserTools.{name}.args must be an object"))?;
    for (arg_name, schema) in args {
        validate_tool_name(arg_name)?;
        let schema = schema
            .as_object()
            .with_context(|| format!("browserTools.{name}.args.{arg_name} must be an object"))?;
        validate_value_type(
            schema.get("type"),
            &format!("browserTools.{name}.args.{arg_name}.type"),
        )?;
        if !schema.get("required").is_some_and(Value::is_boolean) {
            anyhow::bail!("browserTools.{name}.args.{arg_name}.required must be a boolean");
        }
        if !schema
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            anyhow::bail!("browserTools.{name}.args.{arg_name}.description must not be empty");
        }
    }

    let returns = tool
        .returns
        .as_object()
        .with_context(|| format!("browserTools.{name}.returns must be an object"))?;
    validate_value_type(
        returns.get("type"),
        &format!("browserTools.{name}.returns.type"),
    )?;
    if !returns
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        anyhow::bail!("browserTools.{name}.returns.description must not be empty");
    }
    Ok(())
}

fn validate_tool_arguments(
    name: &str,
    tool: &BrowserToolDefinition,
    input: Option<&Value>,
) -> Result<()> {
    let declared = tool
        .args
        .as_object()
        .context("validated browser-tool args schema is not an object")?;
    let empty = Map::new();
    let supplied = match input {
        Some(value) => value
            .as_object()
            .with_context(|| format!("browser tool {name:?} arguments must be an object"))?,
        None => &empty,
    };

    for key in supplied.keys() {
        if !declared.contains_key(key) {
            anyhow::bail!("browser tool {name:?} received undeclared argument {key:?}");
        }
    }
    for (key, schema) in declared {
        let schema = schema
            .as_object()
            .context("validated browser-tool argument schema is not an object")?;
        let required = schema
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match supplied.get(key) {
            Some(value) => validate_value_against_schema(
                value,
                &Value::Object(schema.clone()),
                &format!("browserTools.{name}.args.{key}"),
            )?,
            None if required => {
                anyhow::bail!("browser tool {name:?} is missing required argument {key:?}")
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_value_against_schema(value: &Value, schema: &Value, label: &str) -> Result<()> {
    let expected = schema
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("{label}.type must be a string"))?;
    let valid = match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    if !valid {
        anyhow::bail!("{label} must be {expected}, got {}", json_type_name(value));
    }
    Ok(())
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

async fn ensure_page_matches_manifest(
    page: &PageSession,
    manifest: &SiteSkillManifest,
) -> Result<()> {
    let info = page.page_info().await?;
    let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
    let hostname = url_hostname(url);
    if hostname.is_empty()
        || !manifest
            .domains
            .iter()
            .any(|pattern| domain_matches(&hostname, pattern))
    {
        anyhow::bail!(
            "current page {url:?} is outside site skill {:?} domains",
            manifest.id
        );
    }
    Ok(())
}

fn validate_navigation_url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value).context("navigate_site URL is invalid")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        anyhow::bail!("navigate_site requires an HTTPS URL without credentials");
    }
    Ok(())
}

fn validate_value_type(value: Option<&Value>, label: &str) -> Result<()> {
    let value = value
        .and_then(Value::as_str)
        .with_context(|| format!("{label} must be a string"))?;
    if !VALUE_TYPES.contains(&value) {
        anyhow::bail!("{label} has unsupported value {value:?}");
    }
    Ok(())
}

fn browser_tool_expression(
    site_id: &str,
    tool_name: &str,
    tool: &BrowserToolDefinition,
    source: &str,
    args: Option<&Value>,
) -> Result<String> {
    let argument = serde_json::to_string(args.unwrap_or(&Value::Object(Map::new())))?;
    let label = serde_json::to_string(&format!("{site_id}/{tool_name}"))?;
    if let (Some(binding), Some(callable)) = (&tool.binding, &tool.callable) {
        let invocation = if args.is_some() {
            format!("__socaiBrowserTool({argument})")
        } else {
            "__socaiBrowserTool()".to_string()
        };
        return Ok(format!(
            "(async () => {{\n{source}\n// SOCAI_SITE_SKILL: {label}\n\
             const __socaiBrowserTools = {binding};\n\
             const __socaiBrowserTool = __socaiBrowserTools[{callable:?}];\n\
             if (typeof __socaiBrowserTool !== 'function') {{\n\
               throw new Error('Site browser tool is not callable: ' + {label});\n\
             }}\n\
             return await {invocation};\n\
             }})()"
        ));
    }
    Ok(format!(
        "(async () => {{\nconst __socaiBrowserTool = ({source});\n// SOCAI_SITE_SKILL: {label}\n\
         if (typeof __socaiBrowserTool !== 'function') {{\n\
           throw new Error('Site browser tool is not callable: ' + {label});\n\
         }}\n\
         return await __socaiBrowserTool({argument});\n\
         }})()"
    ))
}

fn read_local_resource(root: &Path, relative: &str) -> Result<String> {
    let canonical_root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve site skill root {}", root.display()))?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("failed to stat site skill resource {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("site skill resource must be a regular file: {relative:?}");
    }
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("failed to resolve site skill resource {}", path.display()))?;
    if !canonical_path.starts_with(&canonical_root) {
        anyhow::bail!("site skill resource escapes its package: {relative:?}");
    }
    read_bounded(&canonical_path, MAX_RESOURCE_BYTES)
}

fn read_bounded(path: &Path, limit: usize) -> Result<String> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    if metadata.len() > limit as u64 {
        anyhow::bail!(
            "site skill resource exceeds {limit} bytes: {}",
            path.display()
        );
    }
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

fn validate_site_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        anyhow::bail!("invalid site skill id: {value:?}");
    }
    Ok(())
}

fn validate_tool_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid browser tool name: {value:?}");
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        anyhow::bail!("site skill path must be a safe relative path: {value:?}");
    }
    Ok(())
}

fn validate_domain_pattern(value: &str) -> Result<()> {
    let domain = value.strip_prefix("*.").unwrap_or(value);
    if domain.is_empty()
        || domain.len() > 253
        || domain != domain.to_ascii_lowercase()
        || !domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        })
    {
        anyhow::bail!("invalid site skill domain pattern: {value:?}");
    }
    Ok(())
}

fn validate_js_path(value: &str, label: &str) -> Result<()> {
    if value.split('.').all(|segment| is_js_identifier(segment)) {
        return Ok(());
    }
    anyhow::bail!("invalid browser tool {label}: {value:?}")
}

fn validate_js_identifier(value: &str, label: &str) -> Result<()> {
    if is_js_identifier(value) {
        return Ok(());
    }
    anyhow::bail!("invalid browser tool {label}: {value:?}")
}

fn is_js_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(first, b'_' | b'$'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

fn url_hostname(value: &str) -> String {
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    reqwest::Url::parse(&candidate)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_string()
}

fn domain_matches(hostname: &str, pattern: &str) -> bool {
    let normalized = pattern.to_ascii_lowercase();
    match normalized.strip_prefix("*.") {
        Some(suffix) => hostname.ends_with(&format!(".{suffix}")),
        None => hostname == normalized,
    }
}

fn join_instructions(extra: &str, knowledge: &str) -> String {
    match (extra.trim(), knowledge.trim()) {
        ("", "") => String::new(),
        ("", knowledge) => knowledge.to_string(),
        (extra, "") => extra.to_string(),
        (extra, knowledge) => format!("{extra}\n\n{knowledge}"),
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
