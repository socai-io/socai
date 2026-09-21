//! Per-run research ledger and structural completion review. These records are
//! model-authored claims, never runtime verification of source truth.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::compaction::truncate;
use super::extensions::{AgentExtension, FinishDecision, RequestPhase, ToolFeedback};
use super::tool::{Tool, ToolContext, ToolResult};

const RESEARCH_CORE: &str = include_str!("skills/research/CORE.md");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadStatus {
    Pending,
    Investigating,
    #[serde(alias = "qualified")]
    Promising,
    Excluded,
    Deferred,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadKind {
    Person,
    Project,
    Topic,
    Query,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchEvidence {
    pub source_url: String,
    pub observation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchLead {
    pub id: String,
    pub kind: LeadKind,
    pub title: String,
    pub rationale: String,
    pub priority: Priority,
    pub status: LeadStatus,
    pub next_action: String,
    pub findings: String,
    pub questions: Vec<String>,
    pub evidence: Vec<ResearchEvidence>,
}

impl ResearchLead {
    fn open(&self) -> bool {
        matches!(self.status, LeadStatus::Pending | LeadStatus::Investigating)
    }

    fn validate(&self) -> Result<()> {
        text_field(&self.id, "lead.id", 160, true)?;
        text_field(&self.title, "lead.title", 300, true)?;
        text_field(&self.rationale, "lead.rationale", 800, true)?;
        text_field(&self.next_action, "lead.next_action", 800, self.open())?;
        text_field(&self.findings, "lead.findings", 2000, !self.open())?;
        string_list(&self.questions, "lead.questions", 12, 500)?;
        if self.evidence.len() > 12 {
            bail!("lead.evidence supports at most 12 entries");
        }
        for evidence in &self.evidence {
            text_field(&evidence.source_url, "evidence.source_url", 2000, true)?;
            let url = reqwest::Url::parse(&evidence.source_url)
                .context("evidence.source_url must be an original source URL, not a claim")?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                bail!("evidence.source_url must be an http(s) source URL");
            }
            text_field(&evidence.observation, "evidence.observation", 1000, true)?;
        }
        if self.status == LeadStatus::Promising && self.evidence.is_empty() {
            bail!("promising lead {} needs a discovery source", self.id);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Satisfied,
    Saturated,
    BudgetExhausted,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchReview {
    pub reason: StopReason,
    pub coverage: String,
    pub remaining_gaps: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchFocus {
    #[default]
    Explore,
    Verify,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ResearchState {
    /// Free-form discovery plan; unknown facts are not unfinished actions.
    pub exploration: String,
    pub branch_notes: Vec<String>,
    pub focus: ResearchFocus,
    pub objective: String,
    pub criteria: Vec<String>,
    pub leads: BTreeMap<String, ResearchLead>,
    pub review: Option<ResearchReview>,
    #[serde(skip)]
    pub artifact_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchUpdate {
    exploration: Option<String>,
    focus: Option<ResearchFocus>,
    objective: Option<String>,
    criteria: Option<Vec<String>>,
    #[serde(default)]
    leads: Vec<ResearchLead>,
    review: Option<ResearchReview>,
}

impl ResearchState {
    fn apply(&mut self, update: ResearchUpdate) -> Result<()> {
        if let Some(exploration) = update.exploration {
            text_field(&exploration, "exploration", 12000, false)?;
            self.exploration = exploration;
        }
        if let Some(focus) = update.focus {
            self.focus = focus;
        }
        if let Some(objective) = update.objective {
            text_field(&objective, "objective", 1500, true)?;
            self.objective = objective;
        }
        if let Some(criteria) = update.criteria {
            string_list(&criteria, "criteria", 12, 500)?;
            if criteria.is_empty() {
                bail!("criteria must describe at least one deliverable");
            }
            self.criteria = criteria;
        }
        if self.objective.is_empty() || self.criteria.is_empty() {
            bail!("initialize research with an objective and non-empty criteria");
        }
        if update.leads.len() > 20 {
            bail!("update at most 20 leads per call");
        }
        for lead in update.leads {
            lead.validate()?;
            self.leads.insert(lead.id.clone(), lead);
        }
        // Bound working state without silently evicting evidence or open leads.
        if self.leads.len() > 100 {
            bail!("research ledger holds at most 100 leads; consolidate the scope or deliver a partial result");
        }
        self.review = None;
        if let Some(review) = update.review {
            text_field(&review.coverage, "review.coverage", 2000, true)?;
            string_list(&review.remaining_gaps, "review.remaining_gaps", 20, 800)?;
            if self.focus == ResearchFocus::Verify
                && matches!(review.reason, StopReason::Satisfied | StopReason::Saturated)
                && self
                    .leads
                    .values()
                    .any(|lead| lead.open() && lead.priority == Priority::High)
            {
                bail!("verification focus still has high-priority open questions; investigate, defer with reasons, or report partial coverage");
            }
            let has_gaps = self.leads.values().any(|lead| {
                lead.open() || lead.status == LeadStatus::Deferred || !lead.questions.is_empty()
            });
            if review.remaining_gaps.is_empty()
                && (has_gaps
                    || matches!(
                        review.reason,
                        StopReason::BudgetExhausted | StopReason::Blocked
                    ))
            {
                bail!("review.remaining_gaps must acknowledge unresolved leads/questions or the partial-outcome limitation");
            }
            self.review = Some(review);
        }
        Ok(())
    }

    pub fn partial_reason(&self) -> Option<String> {
        self.review.as_ref().and_then(|review| {
            matches!(
                review.reason,
                StopReason::BudgetExhausted | StopReason::Blocked
            )
            .then(|| format!("Research is partial: {}", review.coverage))
        })
    }

    /// This is intentionally compact; the complete ledger stays in the artifact.
    pub fn context(&self, tool_free: bool) -> String {
        let mut leads: Vec<&ResearchLead> = self.leads.values().collect();
        leads.sort_by_key(|lead| (!lead.open(), lead.priority, &lead.id));
        let compact: Vec<Value> = leads.iter().take(12).map(|lead| json!({
            "id": lead.id,
            "title": truncate(&lead.title, 100),
            "status": lead.status,
            "priority": lead.priority,
            "next_action": truncate(&lead.next_action, 200),
            "findings": truncate(&lead.findings, 200),
            "sources": lead.evidence.iter().take(2).map(|e| &e.source_url).collect::<Vec<_>>(),
            "questions": lead.questions.iter().take(2).map(|q| truncate(q, 120)).collect::<Vec<_>>(),
        })).collect();
        let value = json!({
            "focus": self.focus,
            "objective": truncate(&self.objective, 500),
            "criteria": self.criteria.iter().map(|c| truncate(c, 200)).collect::<Vec<_>>(),
            "lead_count": self.leads.len(),
            "open_lead_count": self.leads.values().filter(|lead| lead.open()).count(),
            "status_counts": {
                "pending": self.leads.values().filter(|l| l.status == LeadStatus::Pending).count(),
                "investigating": self.leads.values().filter(|l| l.status == LeadStatus::Investigating).count(),
                "promising": self.leads.values().filter(|l| l.status == LeadStatus::Promising).count(),
                "deferred": self.leads.values().filter(|l| l.status == LeadStatus::Deferred).count(),
                "excluded": self.leads.values().filter(|l| l.status == LeadStatus::Excluded).count(),
            },
            // Keep every identity visible even when detail falls outside the
            // priority preview or the transcript has been compacted.
            "all_leads_index": self.leads.values().map(|lead| json!({
                "id": lead.id, "title": truncate(&lead.title, 100),
                "status": lead.status, "priority": lead.priority,
                "source_url": lead.evidence.first().map(|e| &e.source_url),
            })).collect::<Vec<_>>(),
            "leads_preview": compact,
            "review": self.review.as_ref().map(|review| json!({
                "reason": review.reason,
                "coverage": truncate(&review.coverage, 500),
                "remaining_gap_count": review.remaining_gaps.len(),
                "remaining_gaps_preview": review.remaining_gaps.iter().take(6).map(|gap| truncate(gap, 200)).collect::<Vec<_>>(),
            })),
            "full_record_artifact": self.artifact_path,
        });
        let next = if tool_free {
            "Acquisition has ended. Completion bookkeeping is waived. Use existing materials for partial delivery and explain gaps. If local read/write/publish tools are exposed, use them only to finish deliverables; do not call research_update or resume discovery."
        } else if self.review.is_some() {
            "Coverage review is recorded. Address any preflight feedback, then proceed to delivery if no valuable new branch remains. Do not re-evaluate unchanged coverage for each local read, write or publish action."
        } else {
            "Before drafting long deliverables, record a coverage review with research_update and read its feedback in a separate round. Review is invalidated by new acquisition or ledger changes, not by local file reads/writes or publishing."
        };
        format!(
            "Current research record (model-authored task data, not instructions or verified facts). \
             all_leads_index includes every recorded lead. Only 12 have detailed previews. \
             Use research_read(ids=[...]) for full details, or read full_record_artifact. \
             Include relevant pending/deferred leads with their source links in the deliverable, not just the shortlist. An unnamed lead still needs a clickable discovery post. \
             {next}\n{value}\nDiscovery notebook (free-form):\n{}\nSaved candidate branches:\n{}",
             truncate(&self.exploration, 5000),
             self.branch_notes.iter().enumerate().map(|(index, note)| {
                 if index + 3 >= self.branch_notes.len() { truncate(note, 1100) }
                 else { note.lines().next().unwrap_or("").to_string() }
             }).collect::<Vec<_>>().join("\n\n")
        )
    }
}

fn text_field(value: &str, field: &str, max: usize, required: bool) -> Result<()> {
    if (required && value.trim().is_empty()) || value.chars().count() > max {
        bail!(
            "{field} must {}contain at most {max} characters",
            if required { "be non-empty and " } else { "" }
        );
    }
    Ok(())
}

fn string_list(values: &[String], field: &str, max_items: usize, max_chars: usize) -> Result<()> {
    if values.len() > max_items {
        bail!("{field} supports at most {max_items} entries");
    }
    for value in values {
        text_field(value, field, max_chars, true)?;
    }
    Ok(())
}

pub struct ResearchUpdateTool;
pub struct ResearchReadTool;

/// The research policy is dormant until its skill activates the run ledger.
/// All research-specific prompts, counters and stop decisions live here.
#[derive(Default)]
pub struct ResearchExtension {
    review_retries: u32,
    tool_failures: BTreeMap<String, u32>,
    seen_sources: BTreeSet<String>,
    recent: VecDeque<(String, usize)>,
    searches: VecDeque<Value>,
    actions: usize,
    finish_challenges: usize,
    challenged_at: usize,
}

impl ResearchExtension {
    fn discovery_review(
        &mut self,
        research: &ResearchState,
        steps_remaining: Option<u32>,
    ) -> Option<String> {
        let normal_stop = research.review.as_ref().is_some_and(|review| {
            matches!(review.reason, StopReason::Satisfied | StopReason::Saturated)
        });
        let recent_gain: usize = self.recent.iter().rev().take(3).map(|(_, n)| n).sum();
        if research.focus != ResearchFocus::Explore
            || !normal_stop
            || recent_gain == 0
            || steps_remaining.is_some_and(|remaining| remaining <= 10)
            || self.finish_challenges >= 3
            || (self.finish_challenges > 0 && self.actions < self.challenged_at + 3)
        {
            return None;
        }
        self.finish_challenges += 1;
        self.challenged_at = self.actions;
        Some(format!(
            "Coverage preflight, before long-form delivery: the last three acquisition actions added {recent_gain} source IDs. \
             This is a novelty signal, not a search quota. Consult the current plan and branch summaries. \
             If valuable discovery paths remain and budget/access permit, pursue a small useful batch before drafting the report. \
             If additions are substantively repetitive or no worthwhile path remains, briefly explain why in research_update(exploration/review) and proceed to delivery. \
             Do not write a full report just to request this review. Do not re-evaluate unchanged candidates or require identity verification."
        ))
    }
}

impl AgentExtension for ResearchExtension {
    fn execution_ceiling(&self, ctx: &ToolContext, configured: u32) -> u32 {
        // Explicit non-default budgets remain authoritative for every host.
        if configured == super::r#loop::DEFAULT_MAX_STEPS
            && ctx.run_state.as_ref().is_some_and(|s| s.research_active())
        {
            120
        } else {
            configured
        }
    }
    fn request_context(&self, ctx: &ToolContext, phase: RequestPhase) -> Option<String> {
        let research = ctx.run_state.as_ref()?.research()?;
        let context = match phase {
            RequestPhase::Summary | RequestPhase::LocalDelivery => research.context(true),
            RequestPhase::Action { step, max_steps } => format!(
                "Runtime research checkpoint. Step {step} of {max_steps}.\n{}\n{}",
                match research.focus {
                    ResearchFocus::Explore => "Discovery focus: follow the research operating principles above. Use the full skill only for additional strategy/tool details, not to recover the core rules after compaction.",
                    ResearchFocus::Verify => "Verification focus: answer the user's explicit fact-checking questions, preserve contradictions and limits. Use research_read for full earlier findings; reload the full skill only if needed.",
                },
                format!(
                    "{}\nRecent search queries and filters (observed tool calls): {}\n\
                     Do not revisit a parked identity question without a new clue. If returning to a searched entity, explain the expected new angle or filter; synonyms alone are not new coverage.\n\
                     Observed discovery actions: {}; recent actions/new source IDs: {:?}. \
                     New sources are not necessarily new useful candidates. While exploring, compare useful additions after small batches and update the short current plan. \
                     Follow valuable remaining branches before drafting; once coverage is reviewed, focus on delivery rather than repeating the assessment. \
                     A high ceiling is permission to explore, not a quota. Do not mistake unfamiliar identities for diligence blockers.",
                    research.context(false), serde_json::to_string(&self.searches).unwrap_or_default(),
                    self.actions, self.recent,
                )
            ),
        };
        Some(format!("{RESEARCH_CORE}\n\n{context}"))
    }

    fn before_tool(&mut self, ctx: &ToolContext, name: &str) {
        // Invalidate on acquisition, not ordinary local delivery. In particular,
        // reading/writing a report must not make a valid coverage review disappear.
        // Shell is also used for local citation indexes and report assembly. Its
        // invocation alone is not evidence of new acquisition; the model must
        // update coverage if it actually collects external material through it.
        if !matches!(
            name,
            "read_skill"
                | "research_read"
                | "read_saved_notes"
                | "read_file"
                | "write_file"
                | "shell"
                | "publish_artifact"
                | "record_skill_learning"
        ) {
            if let Some(state) = &ctx.run_state {
                state.invalidate_research_review();
            }
        }
    }

    fn after_tool(
        &mut self,
        ctx: &ToolContext,
        name: &str,
        result: &ToolResult,
        error: Option<&str>,
    ) -> ToolFeedback {
        let mut feedback = ToolFeedback::default();
        if name == "research_update" && error.is_none() && !result.failed() {
            if let Some(research) = ctx.run_state.as_ref().and_then(|s| s.research()) {
                if let Some(note) = self.discovery_review(&research, None) {
                    feedback.notes.push(note);
                } else if research.review.is_some() {
                    feedback.notes.push("Coverage review recorded. Proceed to delivery if no valuable new branch remains. File reads/writes and publishing preserve this review; do not re-plan or rewrite the full roster for those mechanical actions. New acquisition or ledger edits require a fresh review.".into());
                }
            }
        }
        if ctx.run_state.as_ref().is_some_and(|s| s.research_active())
            && matches!(name, "search" | "author_scan" | "get_notes" | "explore")
        {
            let mut ids = ctx.search_note_ids();
            ids.extend(
                super::note_store::load_notes(&ctx.run_dir)
                    .iter()
                    .filter_map(|n| n["note_id"].as_str().map(str::to_string)),
            );
            let added = ids
                .into_iter()
                .filter(|id| self.seen_sources.insert(id.clone()))
                .count();
            if name == "search" {
                if let Ok(bytes) = std::fs::read(ctx.output_dir().join("tool.json")) {
                    if let Ok(call) = serde_json::from_slice::<Value>(&bytes) {
                        self.searches.push_back(json!({
                            "query": call["input"]["query"], "filters": call["input"]["filters"],
                            "new_source_ids": added, "error": error.is_some(),
                        }));
                        if self.searches.len() > 40 {
                            self.searches.pop_front();
                        }
                        if let Ok(bytes) = serde_json::to_vec_pretty(&self.searches) {
                            let _ = std::fs::write(
                                ctx.run_dir.join("research-search-history.json"),
                                bytes,
                            );
                        }
                    }
                }
            }
            let mut action_count = 1;
            if name == "explore" {
                if let Ok(bytes) = std::fs::read(ctx.output_dir().join("exploration-calls.json")) {
                    if let Ok(calls) = serde_json::from_slice::<Vec<Value>>(&bytes) {
                        action_count = calls
                            .iter()
                            .filter(|c| {
                                matches!(
                                    c["tool"].as_str(),
                                    Some("search" | "author_scan" | "get_notes")
                                )
                            })
                            .count();
                        for call in calls.iter().filter(|c| c["tool"] == "search") {
                            self.searches.push_back(json!({"query":call["input"]["query"],"filters":call["input"]["filters"],"branch":call["branch"],"error":call["failed"]}));
                        }
                        while self.searches.len() > 40 {
                            self.searches.pop_front();
                        }
                        if let Ok(bytes) = serde_json::to_vec_pretty(&self.searches) {
                            let _ = std::fs::write(
                                ctx.run_dir.join("research-search-history.json"),
                                bytes,
                            );
                        }
                    }
                }
            }
            self.actions += action_count;
            self.recent.push_back((name.into(), added));
            if self.recent.len() > 5 {
                self.recent.pop_front();
            }
            let progress = format!("# Discovery progress\n\nActions: {}\nRecent action/new source counts: {:?}\nThese count source IDs, not useful companies.\n\n{}", self.actions, self.recent, ctx.run_state.as_ref().and_then(|s| s.research()).map(|r| r.context(false)).unwrap_or_default());
            if let Err(error) = std::fs::write(ctx.run_dir.join("research-progress.md"), progress) {
                tracing::warn!(%error, "failed to save discovery progress");
            }
            if self.actions % 4 == 0 {
                feedback.notes.push("Discovery batch checkpoint: briefly save productive directions, new branches and the next useful actions in research_update(exploration=free text). Compare actual new people/background, not just the number of searches. If a direction repeats, try an adjacent angle, purposeful filter or candidate history.".into());
            }
        }
        if !ctx
            .run_state
            .as_ref()
            .is_some_and(|state| state.research_active())
            || matches!(
                name,
                "read_skill"
                    | "research_update"
                    | "research_read"
                    | "publish_artifact"
                    | "record_skill_learning"
            )
        {
            return feedback;
        }
        // Inspect the tool's trusted outcome, never keywords in page text.
        let failures = self.tool_failures.entry(name.to_string()).or_default();
        if result.failed() || error.is_some() {
            *failures += 1;
            feedback.notes.push(format!(
                "[Research action failed ({failures} unresolved failures for {name}). \
                 This is not evidence of absent candidates. Load self-healing before retrying \
                 the failed action; changing keywords does not repair browser access. If this \
                 source is optional, park the branch and continue other useful discovery.]"
            ));
            if *failures >= 3 {
                feedback.notes.push(format!(
                    "{name} has repeatedly failed. Park this source/branch and continue useful \
                     discovery elsewhere; a failed optional source does not end the whole task. \
                     Stop as blocked only if the task's primary sources are unavailable."
                ));
            }
        } else {
            *failures = 0;
        }
        feedback
    }

    fn before_finish(&mut self, ctx: &ToolContext, steps_remaining: u32) -> FinishDecision {
        let research = ctx.run_state.as_ref().and_then(|state| state.research());
        if let Some(r) = &research {
            if let Some(note) = self.discovery_review(r, Some(steps_remaining)) {
                return FinishDecision::Continue(note);
            }
        }
        if !research.is_some_and(|research| research.review.is_none()) {
            return FinishDecision::Allow;
        }
        self.review_retries += 1;
        if self.review_retries <= 2 && steps_remaining > 0 {
            FinishDecision::Continue(
                "Research completion review is missing or stale. Use research_update to record \
                 actual coverage, remaining gaps and a stopping reason before answering. Investigate \
                 uncovered directions if valuable and budget permits. In explore focus, include \
                 unresolved leads in the results instead of requiring diligence on every lead. \
                 When genuinely budget-limited record an honest budget_exhausted \
                 or blocked partial review. Do not fabricate evidence or defer everything just to \
                 pass this structural check.".into()
            )
        } else {
            FinishDecision::Partial("Research ended without a valid completion review.".into())
        }
    }

    fn execution_limit_reason(&self, ctx: &ToolContext) -> Option<String> {
        ctx.run_state.as_ref()?.research_active().then(|| {
            "Research reached the execution budget; remaining leads may be uninvestigated.".into()
        })
    }

    fn partial_reason(&self, ctx: &ToolContext) -> Option<String> {
        ctx.run_state.as_ref()?.research()?.partial_reason()
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResearchRead {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ResearchReadTool {
    fn available_in_local_delivery(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "research_read"
    }

    fn description(&self) -> &str {
        "Read complete leads from the current research ledger after context compaction or before \
         writing the full candidate roster. Optional ids filter; omitted ids select all leads. \
         Returns paginated findings, questions, original source URLs and next actions. \
         This reads saved task data without re-browsing or invalidating completion review."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "ids": {"type": "array", "maxItems": 20, "items": {"type": "string"}},
                "offset": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 5, "default": 3}
            },
            "additionalProperties": false
        })
    }

    fn is_available(&self, ctx: &ToolContext) -> bool {
        ctx.run_state
            .as_ref()
            .is_some_and(|state| state.research_active())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let input: ResearchRead = serde_json::from_value(input)?;
        let limit = input.limit.unwrap_or(3);
        if !(1..=5).contains(&limit) || input.ids.len() > 20 {
            bail!("research_read supports 1..5 leads per page and at most 20 selected ids");
        }
        let research = ctx
            .run_state
            .as_ref()
            .and_then(|state| state.research())
            .context("load read_skill(name=research) first")?;
        for id in &input.ids {
            if !research.leads.contains_key(id) {
                bail!("unknown research lead {id:?}; use all_leads_index for saved ids");
            }
        }
        let selected: Vec<_> = research
            .leads
            .values()
            .filter(|lead| input.ids.is_empty() || input.ids.contains(&lead.id))
            .collect();
        let page: Vec<_> = selected.iter().skip(input.offset).take(limit).collect();
        let next = input.offset.saturating_add(page.len());
        Ok(ToolResult::text(serde_json::to_string(&json!({
            "focus": research.focus,
            "objective": research.objective,
            "total_selected": selected.len(),
            "offset": input.offset,
            "next_offset": (next < selected.len()).then_some(next),
            "leads": page,
            "full_record_artifact": research.artifact_path,
            "note": "Saved model-authored findings, not certification. Include relevant unresolved leads in the result."
        }))?))
    }
}

#[async_trait]
impl Tool for ResearchUpdateTool {
    fn name(&self) -> &str {
        "research_update"
    }

    fn description(&self) -> &str {
        "Maintain the current run's research objective, criteria, and leads after loading the research skill. \
         Upsert only changed leads as complete entries by stable id; omitted leads are retained. Avoid resending unchanged entries. Record findings, counterevidence, \
         questions, sources and next actions. Explore is the default: retain useful uncertain leads, \
         and use promising for worthwhile follow-up, not verified/investable. Before drafting long deliverables, submit a review with actual coverage, \
         remaining gaps and stopping reason. This checks structure, not source truth."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "focus": {"type": "string", "enum": ["explore", "verify"], "description": "Default explore for broad social discovery/sourcing. Use verify only when the user explicitly requests fact-checking or due diligence."},
                "exploration": {"type":"string", "maxLength":12000, "description":"Free-form discovery notebook: useful directions tried, recent new people/background, promising next branches and why; park low-value directions with reasons. Replace with the current plan. Unknown facts alone are not a discovery backlog. No required format."},
                "objective": {"type": "string", "maxLength": 1500},
                "criteria": {"type": "array", "minItems": 1, "maxItems": 12, "items": {"type": "string", "maxLength": 500}},
                "leads": {"type": "array", "maxItems": 20, "items": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string", "maxLength": 160},
                        "kind": {"type": "string", "enum": ["person", "project", "topic", "query"]},
                        "title": {"type": "string", "maxLength": 300},
                        "rationale": {"type": "string", "maxLength": 800},
                        "priority": {"type": "string", "enum": ["high", "medium", "low"]},
                        "status": {"type": "string", "enum": ["pending", "investigating", "promising", "excluded", "deferred"]},
                        "next_action": {"type": "string", "maxLength": 800},
                        "findings": {"type": "string", "maxLength": 2000},
                        "questions": {"type": "array", "maxItems": 12, "items": {"type": "string", "maxLength": 500}},
                        "evidence": {"type": "array", "maxItems": 12, "items": {
                            "type": "object",
                            "properties": {
                                "source_url": {"type": "string", "maxLength": 2000, "description": "Exact original http(s) URL returned by a source tool; preserve required URL tokens. Do not put a claim here."},
                                "observation": {"type": "string", "maxLength": 1000, "description": "What this source supports; distinguish self-report, third-party report, and directly observed facts."}
                            },
                            "required": ["source_url", "observation"],
                            "additionalProperties": false
                        }}
                    },
                    "required": ["id", "kind", "title", "rationale", "priority", "status", "next_action", "findings", "questions", "evidence"],
                    "additionalProperties": false
                }},
                "review": {
                    "type": "object",
                    "properties": {
                        "reason": {"type": "string", "enum": ["satisfied", "saturated", "budget_exhausted", "blocked"]},
                        "coverage": {"type": "string", "maxLength": 2000},
                        "remaining_gaps": {"type": "array", "maxItems": 20, "items": {"type": "string", "maxLength": 800}}
                    },
                    "required": ["reason", "coverage", "remaining_gaps"],
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        })
    }

    fn is_available(&self, ctx: &ToolContext) -> bool {
        ctx.run_state
            .as_ref()
            .is_some_and(|state| state.research_active())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let state = ctx
            .run_state
            .as_ref()
            .context("research requires an agent run")?;
        let mut research = state
            .research()
            .context("load read_skill(name=research) first")?;
        let update: ResearchUpdate = serde_json::from_value(input)?;
        research.apply(update)?;
        let path = ctx.write_json_artifact(
            "research",
            &serde_json::to_value(&research)?,
            "artifacts",
            self.name(),
            "research",
            "Research leads and completion review",
            json!({}),
        )?;
        research.artifact_path = Some(ctx.run_dir.join(path).to_string_lossy().into_owned());
        let result = serde_json::to_string(&json!({
            "saved": true,
            "lead_count": research.leads.len(),
            "review": research.review,
            "full_record_artifact": research.artifact_path,
            "note": "Update saved. Omitted leads are retained. The next runtime checkpoint contains the current index; use research_read for full details."
        }))?;
        state.set_research(research);
        Ok(ToolResult::text(result))
    }
}
