//! Small, incremental research updates for the desktop workbench.
use super::{Tool, ToolContext, ToolResult};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const RELATION_GUIDANCE: &str = include_str!("skills/research/RELATIONSHIPS.md");

#[derive(Default)]
pub struct ResearchBoardTool {
    lock: tokio::sync::Mutex<()>,
}

impl ResearchBoardTool {
    /// Observed tool activity, separate from model-authored findings.
    pub async fn record_operation(
        &self,
        ctx: &ToolContext,
        site: &str,
        tool: &str,
        input: &Value,
        failed: bool,
    ) -> Result<()> {
        let _guard = self.lock.lock().await;
        let path = ctx.run_dir.join("research-board.json");
        let mut board: Value = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| json!({"platforms":[],"leads":[],"relations":[]}));
        if !board["operations"].is_array() {
            board["operations"] = json!([]);
        }
        let action = input["tool_name"].as_str().unwrap_or(tool);
        let detail = ["query", "url", "author_id", "user_id"]
            .into_iter()
            .find_map(|k| input[k].as_str())
            .unwrap_or("");
        let rows = board["operations"]
            .as_array_mut()
            .context("Invalid operations")?;
        rows.push(json!({"platform":site,"tool":action,"detail":detail.chars().take(160).collect::<String>(),"failed":failed,"at":chrono::Utc::now().timestamp_millis()}));
        if rows.len() > 150 {
            rows.drain(..rows.len() - 150);
        }
        board["updated_at"] = json!(chrono::Utc::now().timestamp_millis());
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&board)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

#[async_trait]
impl Tool for ResearchBoardTool {
    fn available_in_local_delivery(&self) -> bool { true }
    fn name(&self) -> &str {
        "update_research_board"
    }
    fn description(&self) -> &str {
        "Update the user's live research workbench as useful discoveries arrive. Upsert concise leads and sourced relationships by stable id; do not repeat the full board. Include early/uncertain findings, not just verified candidates. Record platform focus and next steps. This is a progress surface, not a gate or final report. Only record observed work; never invent sources or completion."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "summary":{"type":"string","description":"One sentence: current working hypothesis or discovery, not private reasoning."},
            "next_steps":{"type":"array","maxItems":5,"items":{"type":"string"}},
            "platforms":{"type":"array","maxItems":10,"items":{"type":"object","properties":{
                "id":{"type":"string","description":"Platform id, e.g. xhs, x, weixin, podcast, github"},
                "status":{"type":"string","enum":["scouting","reading","following","synthesizing","blocked","done"]},
                "focus":{"type":"string","description":"Concrete current lead, action or access limitation"}
            },"required":["id","status","focus"]}},
            "leads":{"type":"array","maxItems":30,"items":{"type":"object","properties":{
                "id":{"type":"string","description":"Stable unique lead id; workers prefix with platform id"},
                "name":{"type":"string"},"platform":{"type":"string"},
                "signal":{"type":"string","description":"What is new or worth following"},
                "uncertainty":{"type":"string","description":"Missing details or qualification, not reason to discard"},
                "url":{"type":"string","description":"Exact observed source URL"},
                "stage":{"type":"string","enum":["signal","following","shortlist","context"]}
            },"required":["id","name","platform","signal","url","stage"]}},
            "relations":{"type":"array","maxItems":30,"items":{"type":"object","properties":{
                "id":{"type":"string"},"from":{"type":"string","description":"Lead id"},"to":{"type":"string","description":"Lead id"},
                "label":{"type":"string"},"url":{"type":"string","description":"Primary supporting source"},"uncertain":{"type":"boolean","description":"Use true with kind=hypothesis; fact and comparison use false. Conflicting values are conservatively stored as an uncertain hypothesis, never promoted to a fact."},
                "dimension":{"type":"string","enum":["identity","people","problem","audience","technology","ecosystem","business","stage"]},
                "kind":{"type":"string","enum":["fact","comparison","hypothesis"],"description":"fact: a source explicitly states the connection; comparison: attributes of both endpoints support the comparison; hypothesis: decisive evidence is missing (uncertain=true)."},
                "basis":{"type":"string","description":"Concrete connection or comparison; preserve source uncertainty. Never infer partnership from similarity."},
                "sources":{"type":"array","minItems":1,"maxItems":6,"items":{"type":"object","properties":{
                    "lead_id":{"type":"string"},"url":{"type":"string"},"excerpt":{"type":"string","description":"Optional literal source excerpt, not a paraphrase"}
                },"required":["lead_id","url"]}}
            },"required":["id","from","to","label","url","uncertain","dimension","kind","basis","sources"]}}
        }})
    }
    async fn call(&self, mut input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        anyhow::ensure!(
            input.is_object() && serde_json::to_vec(&input)?.len() <= 64000,
            "Provide a small board update (max 64 KB)"
        );
        for (key, fields, max) in [
            ("platforms", vec!["id", "status", "focus"], 10),
            (
                "leads",
                vec!["id", "name", "platform", "signal", "url", "stage"],
                30,
            ),
            ("relations", vec!["id", "from", "to", "label", "url"], 30),
        ] {
            if let Some(value) = input.get(key) {
                let items = value
                    .as_array()
                    .context("Board collections must be arrays")?;
                anyhow::ensure!(items.len() <= max, "Too many items in one update");
                for item in items {
                    for field in &fields {
                        anyhow::ensure!(
                            item[*field]
                                .as_str()
                                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 4000),
                            "Invalid {key}.{field}"
                        );
                    }
                    if let Some(url) = item.get("url") {
                        let parsed =
                            reqwest::Url::parse(url.as_str().context("Invalid source URL")?)?;
                        anyhow::ensure!(
                            matches!(parsed.scheme(), "http" | "https"),
                            "Source must be HTTP(S)"
                        );
                    }
                }
            }
        }
        for edge in input["relations"].as_array().into_iter().flatten() {
            anyhow::ensure!(edge["uncertain"].is_boolean(), "Relation uncertainty must be boolean");
            // Older callers can still send legacy relationships without metadata.
            if edge.get("kind").is_some() || edge.get("dimension").is_some() {
                anyhow::ensure!(matches!(edge["dimension"].as_str(), Some("identity" | "people" | "problem" | "audience" | "technology" | "ecosystem" | "business" | "stage")), "Invalid relationship dimension");
                let kind = edge["kind"].as_str().unwrap_or("");
                anyhow::ensure!(matches!(kind, "fact" | "comparison" | "hypothesis"), "Invalid relationship kind");
                anyhow::ensure!(edge["basis"].as_str().is_some_and(|s| !s.trim().is_empty() && s.len() <= 4000), "Relationship basis is required");
                let sources = edge["sources"].as_array().context("Relationship sources required")?;
                anyhow::ensure!(!sources.is_empty() && sources.len() <= 6, "Provide 1-6 relationship sources");
                for source in sources {
                    anyhow::ensure!(source["lead_id"] == edge["from"] || source["lead_id"] == edge["to"], "Source must reference an endpoint");
                    let url = reqwest::Url::parse(source["url"].as_str().context("Invalid relationship source")?)?;
                    anyhow::ensure!(matches!(url.scheme(), "http" | "https"), "Source must be HTTP(S)");
                    if let Some(excerpt) = source.get("excerpt") { anyhow::ensure!(excerpt.as_str().is_some_and(|s| s.len() <= 4000), "Invalid source excerpt"); }
                }
                if kind == "comparison" {
                    anyhow::ensure!(sources.iter().any(|s| s["lead_id"] == edge["from"]) && sources.iter().any(|s| s["lead_id"] == edge["to"]), "Comparisons require sources for both endpoints");
                }
            }
        }
        if let Some(steps) = input.get("next_steps") {
            anyhow::ensure!(
                steps.as_array().is_some_and(|a| a.len() <= 5
                    && a.iter()
                        .all(|v| v.as_str().is_some_and(|s| s.len() <= 2000))),
                "Invalid next_steps"
            );
        }
        if let Some(summary) = input.get("summary") {
            anyhow::ensure!(
                summary.as_str().is_some_and(|s| s.len() <= 4000),
                "Invalid summary"
            );
        }
        // Validate the submitted kind and its evidence BEFORE reconciling
        // uncertainty: a claimed comparison must still cite both endpoints,
        // even when the caller also marks it uncertain. Never repair a
        // contradiction by making an uncertain connection certain.
        let mut normalized_relations = Vec::new();
        for edge in input
            .get_mut("relations")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let conflicting = edge["kind"]
                .as_str()
                .is_some_and(|kind| edge["uncertain"] != json!(kind == "hypothesis"));
            if conflicting {
                normalized_relations.push(edge["id"].as_str().unwrap_or_default().to_owned());
                edge["kind"] = json!("hypothesis");
                edge["uncertain"] = json!(true);
            }
        }
        let _guard = self.lock.lock().await;
        let path = ctx.run_dir.join("research-board.json");
        let mut board: Value = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| json!({"platforms":[],"leads":[],"relations":[]}));
        for key in ["summary", "next_steps"] {
            if let Some(value) = input.get(key) {
                board[key] = value.clone();
            }
        }
        for key in ["platforms", "leads", "relations"] {
            if let Some(updates) = input[key].as_array() {
                let rows = board[key].as_array_mut().context("Invalid saved board")?;
                for item in updates {
                    if let Some(existing) = rows.iter_mut().find(|r| r["id"] == item["id"]) {
                        *existing = item.clone();
                    } else {
                        anyhow::ensure!(rows.len() < 1000, "Board full; update existing items");
                        rows.push(item.clone());
                    }
                }
            }
        }
        for edge in input["relations"].as_array().into_iter().flatten() {
            anyhow::ensure!(edge["from"] != edge["to"], "A relationship needs two distinct endpoints");
            for endpoint in ["from", "to"] {
                anyhow::ensure!(board["leads"].as_array().is_some_and(|rows| rows.iter().any(|r| r["id"] == edge[endpoint])), "Unknown relationship endpoint");
            }
        }
        board["updated_at"] = json!(chrono::Utc::now().timestamp_millis());
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&board)?)?;
        std::fs::rename(temporary, path)?;
        let mut message =
            "Research workbench updated. Continue discovery; this is not a completion check."
                .to_owned();
        if !normalized_relations.is_empty() {
            message.push_str(&format!(
                " Normalized conflicting kind/uncertain values to kind=hypothesis, uncertain=true for: {}. Review the evidence gap; no relationship was promoted to certainty.",
                normalized_relations.join(", ")
            ));
        }
        Ok(ToolResult::text(message))
    }
}
