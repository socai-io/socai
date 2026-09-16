//! Structured completion protocol for coverage-guided research runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::llm::ToolSchema;
use crate::agent::note_store::load_notes;
use crate::agent::research::{ResearchBrief, ResearchPriority};
use crate::agent::run_state::RunState;

pub const RESEARCH_COVERAGE_PROTOCOL_VERSION: &str = "research-coverage-v2.1";
pub const SUBMIT_RESEARCH_COMPLETION_TOOL: &str = "submit_research_completion";
pub const DEFAULT_MAX_COMPLETION_ATTEMPTS: u32 = 3;
pub const DEFAULT_MAX_COVERAGE_PROTOCOL_RETRIES: u32 = 1;
pub const DEFAULT_MAX_FORCED_WRITER_ATTEMPTS: u32 = 2;
pub const DEFAULT_MAX_RESEARCH_RECOVERY_ROUNDS: u32 = 2;

const MAX_EVIDENCE_REFS: usize = 12;
const MAX_UNMET_REQUIREMENTS: usize = 4;
const MAX_FIELD_CHARS: usize = 1_000;
const MAX_FINAL_ANSWER_CHARS: usize = 50_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinalAnswerSource {
    StructuredCompletion,
    SchemaSalvage,
    VisibleTextSalvage,
    ForcedWriter,
    TruncatedWriterSalvage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubquestionCoverageStatus {
    Pending,
    Covered,
    Partial,
    Missing,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubquestionCoverage {
    pub id: String,
    pub status: SubquestionCoverageStatus,
    pub evidence_refs: Vec<String>,
    pub support_summary: String,
    #[serde(default)]
    pub gap: Option<ResearchGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchCompletionSubmission {
    pub subquestions: Vec<SubquestionCoverage>,
    pub hard_constraints_satisfied: bool,
    pub stop_conditions_satisfied: bool,
    pub unmet_requirements: Vec<String>,
    pub final_answer: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchGapKind {
    Material,
    SearchScarcity,
    CapabilityBlocked,
    SourceUnavailable,
    Limitation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchGap {
    pub kind: ResearchGapKind,
    pub description: String,
    #[serde(default)]
    pub next_action: String,
    #[serde(default)]
    pub blocks_hard_constraint: Option<bool>,
    #[serde(default)]
    pub required_source_type: String,
    #[serde(default)]
    pub unblock_requirement: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionGateDecision {
    Accept,
    ResearchMore,
    LastMileResearch,
    ReviseOnly,
    FinishWithLimitations,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionGateResult {
    pub decision: CompletionGateDecision,
}

impl CompletionGateResult {
    pub fn tool_result_value(
        &self,
        brief: &ResearchBrief,
        submission: &ResearchCompletionSubmission,
    ) -> Value {
        let requested_kind = match self.decision {
            CompletionGateDecision::ResearchMore => Some(ResearchGapKind::Material),
            CompletionGateDecision::LastMileResearch => Some(ResearchGapKind::SearchScarcity),
            _ => None,
        };
        let required_gaps: Vec<Value> = submission
            .subquestions
            .iter()
            .zip(&brief.subquestions)
            .filter(|(coverage, question)| {
                question.priority == ResearchPriority::Required
                    && coverage
                        .gap
                        .as_ref()
                        .is_some_and(|gap| requested_kind == Some(gap.kind))
            })
            .filter_map(|(coverage, _)| {
                let gap = coverage.gap.as_ref()?;
                Some(json!({
                    "id": coverage.id,
                    "kind": gap.kind,
                    "description": gap.description,
                    "next_action": gap.next_action,
                }))
            })
            .collect();
        json!({
            "action": self.decision,
            "required_gaps": required_gaps,
            "instruction": match self.decision {
                CompletionGateDecision::ResearchMore =>
                    "Continue only on the listed material gaps, then submit completion again.",
                CompletionGateDecision::LastMileResearch =>
                    "Make at most two precise, non-duplicate tool calls for the listed scarcity gaps, then submit completion again.",
                CompletionGateDecision::ReviseOnly =>
                    "Resolve unmet_requirements using gathered evidence, then submit completion again without research tools.",
                CompletionGateDecision::Accept | CompletionGateDecision::FinishWithLimitations =>
                    "The completion submission was accepted.",
            },
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceLocatorCatalog {
    locators: BTreeSet<String>,
    aliases: BTreeMap<String, String>,
    run_dir: PathBuf,
}

impl EvidenceLocatorCatalog {
    pub fn from_run(
        run_dir: &Path,
        run_state: &RunState,
        evidence_ids: &BTreeMap<String, String>,
    ) -> Self {
        let mut locators: BTreeSet<String> = evidence_ids.values().cloned().collect();
        let mut aliases = evidence_ids.clone();
        for note in load_notes(run_dir) {
            if let Some(note_id) = note.get("note_id").and_then(Value::as_str) {
                let note_id = note_id.trim();
                if !note_id.is_empty() {
                    locators.insert(format!("note:{note_id}"));
                }
            }
        }
        for artifact in run_state.artifact_records() {
            let path = artifact.path.trim();
            if !path.is_empty() {
                let canonical = format!("artifact:{path}");
                locators.insert(canonical.clone());
                aliases.insert(
                    format!("artifact:{}", run_dir.join(path).display()),
                    canonical,
                );
            }
        }
        Self {
            locators,
            aliases,
            run_dir: run_dir.to_path_buf(),
        }
    }

    fn canonicalize(&self, locator: &str) -> Option<String> {
        let locator = locator.trim();
        if let Some(canonical) = self.aliases.get(locator) {
            return Some(canonical.clone());
        }
        if let Some(evidence_id) = normalize_evidence_id(locator) {
            if let Some(canonical) = self.aliases.get(&evidence_id) {
                return Some(canonical.clone());
            }
        }
        if self.locators.contains(locator) {
            return Some(locator.to_string());
        }
        if let Some(canonical) = canonical_tool_locator(locator) {
            return self.locators.contains(&canonical).then_some(canonical);
        }
        self.canonicalize_artifact(locator)
    }

    fn canonicalize_artifact(&self, locator: &str) -> Option<String> {
        let raw = locator.strip_prefix("artifact:")?.trim();
        if raw.is_empty() {
            return None;
        }
        let path = Path::new(raw);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.run_dir.join(path)
        };
        let canonical_run = std::fs::canonicalize(&self.run_dir).ok()?;
        let canonical_candidate = std::fs::canonicalize(candidate).ok()?;
        let relative = canonical_candidate.strip_prefix(canonical_run).ok()?;
        Some(format!("artifact:{}", relative.display()))
    }
}

impl ResearchCompletionSubmission {
    pub fn from_tool_input(
        input: Value,
        brief: &ResearchBrief,
        evidence: &EvidenceLocatorCatalog,
    ) -> anyhow::Result<Self> {
        let mut submission: Self = serde_json::from_value(input)
            .map_err(|error| anyhow::anyhow!("invalid research completion payload: {error}"))?;
        submission.normalize_and_validate(brief, evidence)?;
        Ok(submission)
    }

    fn normalize_and_validate(
        &mut self,
        brief: &ResearchBrief,
        evidence: &EvidenceLocatorCatalog,
    ) -> anyhow::Result<()> {
        if self.subquestions.len() != brief.subquestions.len() {
            anyhow::bail!(
                "completion must report all {} brief subquestions exactly once (got {})",
                brief.subquestions.len(),
                self.subquestions.len()
            );
        }
        let expected_positions: BTreeMap<String, usize> = brief
            .subquestions
            .iter()
            .enumerate()
            .map(|(index, question)| (question.id.clone(), index))
            .collect();
        let mut submitted_ids = BTreeSet::new();
        for coverage in &mut self.subquestions {
            normalize_required(&mut coverage.id, "subquestion.id", 8)?;
            if !expected_positions.contains_key(&coverage.id) {
                anyhow::bail!("completion contains unknown subquestion id {}", coverage.id);
            }
            if !submitted_ids.insert(coverage.id.clone()) {
                anyhow::bail!(
                    "completion contains duplicate subquestion id {}",
                    coverage.id
                );
            }
        }
        self.subquestions
            .sort_by_key(|coverage| expected_positions[&coverage.id]);
        for (index, (coverage, expected)) in self
            .subquestions
            .iter_mut()
            .zip(&brief.subquestions)
            .enumerate()
        {
            coverage.normalize_and_validate(index + 1, &expected.id, evidence)?;
        }
        normalize_list(
            &mut self.unmet_requirements,
            "unmet_requirements",
            MAX_UNMET_REQUIREMENTS,
        )?;
        normalize_required(
            &mut self.final_answer,
            "final_answer",
            MAX_FINAL_ANSWER_CHARS,
        )?;
        if !is_usable_final_answer(&self.final_answer) {
            anyhow::bail!("final_answer is a placeholder, process update, or too short to be a usable research deliverable");
        }
        if self.hard_constraints_satisfied && self.stop_conditions_satisfied {
            if !self.unmet_requirements.is_empty() {
                anyhow::bail!(
                    "unmet_requirements must be empty when hard constraints and stop conditions are satisfied"
                );
            }
        } else if self.unmet_requirements.is_empty() {
            anyhow::bail!(
                "unmet_requirements must describe what remains when a hard constraint or stop condition is not satisfied"
            );
        }
        if self.subquestions.iter().any(|coverage| {
            coverage.gap.as_ref().is_some_and(|gap| {
                is_blocked_gap_kind(gap.kind) && gap.blocks_hard_constraint == Some(true)
            })
        }) && self.hard_constraints_satisfied
        {
            anyhow::bail!(
                "hard_constraints_satisfied must be false when a blocked gap declares blocks_hard_constraint=true"
            );
        }
        Ok(())
    }
}

impl SubquestionCoverage {
    fn normalize_and_validate(
        &mut self,
        position: usize,
        expected_id: &str,
        evidence: &EvidenceLocatorCatalog,
    ) -> anyhow::Result<()> {
        normalize_required(&mut self.id, "subquestion.id", 8)?;
        if self.id != expected_id {
            anyhow::bail!(
                "completion subquestion {position} id must be {expected_id} (got {})",
                self.id
            );
        }
        if self.status == SubquestionCoverageStatus::Pending {
            anyhow::bail!("completion subquestion {expected_id} must not remain pending");
        }
        normalize_list(
            &mut self.evidence_refs,
            "subquestion.evidence_refs",
            MAX_EVIDENCE_REFS,
        )?;
        normalize_optional(&mut self.support_summary, "subquestion.support_summary")?;
        if let Some(gap) = &mut self.gap {
            gap.normalize_and_validate()?;
        }

        for locator in &mut self.evidence_refs {
            let submitted = locator.clone();
            *locator = evidence.canonicalize(&submitted).ok_or_else(|| {
                anyhow::anyhow!(
                    "subquestion {expected_id} cites evidence locator not found in this run: {submitted}"
                )
            })?;
        }
        match self.status {
            SubquestionCoverageStatus::Covered => {
                if self.gap.is_some() {
                    anyhow::bail!("covered subquestion {expected_id} must not include gap");
                }
                if self.evidence_refs.is_empty() {
                    anyhow::bail!(
                        "covered subquestion {expected_id} requires at least one evidence locator"
                    );
                }
                if self.support_summary.is_empty() {
                    anyhow::bail!("covered subquestion {expected_id} requires support_summary");
                }
            }
            SubquestionCoverageStatus::Partial | SubquestionCoverageStatus::Missing => {
                let gap = self.gap.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "{} subquestion {expected_id} requires gap",
                        status_name(self.status)
                    )
                })?;
                if is_blocked_gap_kind(gap.kind) {
                    anyhow::bail!(
                        "{} subquestion {expected_id} cannot use a blocked gap kind; use status=blocked",
                        status_name(self.status)
                    );
                }
            }
            SubquestionCoverageStatus::Blocked => {
                let gap = self.gap.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("blocked subquestion {expected_id} requires gap")
                })?;
                if !is_blocked_gap_kind(gap.kind) {
                    anyhow::bail!(
                        "blocked subquestion {expected_id} must use gap.kind=capability_blocked or source_unavailable"
                    );
                }
            }
            SubquestionCoverageStatus::Pending => unreachable!(),
        }
        Ok(())
    }
}

impl ResearchGap {
    fn normalize_and_validate(&mut self) -> anyhow::Result<()> {
        normalize_required(&mut self.description, "gap.description", MAX_FIELD_CHARS)?;
        normalize_optional(&mut self.next_action, "gap.next_action")?;
        normalize_optional(&mut self.required_source_type, "gap.required_source_type")?;
        normalize_optional(&mut self.unblock_requirement, "gap.unblock_requirement")?;
        if matches!(
            self.kind,
            ResearchGapKind::Material | ResearchGapKind::SearchScarcity
        ) && self.next_action.is_empty()
        {
            anyhow::bail!("material/search_scarcity gaps require a concrete next_action");
        }
        if is_blocked_gap_kind(self.kind) {
            if self.blocks_hard_constraint.is_none() {
                anyhow::bail!(
                    "{} gaps require blocks_hard_constraint",
                    gap_kind_name(self.kind)
                );
            }
            for (name, value) in [
                ("required_source_type", self.required_source_type.as_str()),
                ("unblock_requirement", self.unblock_requirement.as_str()),
            ] {
                if value.is_empty() {
                    anyhow::bail!("{} gaps require non-empty {name}", gap_kind_name(self.kind));
                }
            }
        }
        Ok(())
    }
}

pub fn evaluate_completion(
    brief: &ResearchBrief,
    submission: &ResearchCompletionSubmission,
    budget_exhausted: bool,
    last_mile_available: bool,
) -> CompletionGateResult {
    let mut required_material = 0usize;
    let mut required_scarce = 0usize;
    let mut required_blocked = 0usize;
    let mut required_limited = 0usize;

    for (question, coverage) in brief.subquestions.iter().zip(&submission.subquestions) {
        if question.priority != ResearchPriority::Required {
            continue;
        }
        match coverage.status {
            SubquestionCoverageStatus::Partial | SubquestionCoverageStatus::Missing => {
                match coverage.gap.as_ref().map(|gap| gap.kind) {
                    Some(ResearchGapKind::Material) | None => required_material += 1,
                    Some(ResearchGapKind::SearchScarcity) => required_scarce += 1,
                    Some(ResearchGapKind::CapabilityBlocked)
                    | Some(ResearchGapKind::SourceUnavailable) => required_blocked += 1,
                    Some(ResearchGapKind::Limitation) => required_limited += 1,
                }
            }
            SubquestionCoverageStatus::Blocked => required_blocked += 1,
            SubquestionCoverageStatus::Covered => {}
            SubquestionCoverageStatus::Pending => required_material += 1,
        }
    }

    let decision = if required_material > 0 {
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ResearchMore
        }
    } else if required_scarce > 0 {
        if budget_exhausted || !last_mile_available {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::LastMileResearch
        }
    } else if required_blocked > 0 || required_limited > 0 {
        CompletionGateDecision::FinishWithLimitations
    } else if !submission.hard_constraints_satisfied || !submission.stop_conditions_satisfied {
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ReviseOnly
        }
    } else {
        CompletionGateDecision::Accept
    };

    CompletionGateResult { decision }
}

pub fn initial_coverage_state(brief: &ResearchBrief) -> Value {
    json!({
        "schema_version": 1,
        "protocol_version": RESEARCH_COVERAGE_PROTOCOL_VERSION,
        "status": "pending",
        "decision": null,
        "subquestions": brief.subquestions.iter().map(|question| json!({
            "id": question.id,
            "status": "pending",
            "evidence_refs": [],
            "support_summary": "",
            "gap": null,
        })).collect::<Vec<_>>(),
        "hard_constraints_satisfied": null,
        "stop_conditions_satisfied": null,
        "unmet_requirements": [],
    })
}

pub fn coverage_checkpoint(
    submission: &ResearchCompletionSubmission,
    decision: CompletionGateDecision,
) -> Value {
    json!({
        "schema_version": 1,
        "protocol_version": RESEARCH_COVERAGE_PROTOCOL_VERSION,
        "status": "evaluated",
        "decision": decision,
        "subquestions": submission.subquestions,
        "hard_constraints_satisfied": submission.hard_constraints_satisfied,
        "stop_conditions_satisfied": submission.stop_conditions_satisfied,
        "unmet_requirements": submission.unmet_requirements,
    })
}

pub fn research_completion_tool_schema() -> ToolSchema {
    ToolSchema {
        name: SUBMIT_RESEARCH_COMPLETION_TOOL.to_string(),
        description: "Submit one final coverage checkpoint and the complete user-facing answer. Call this tool alone. Report every brief subquestion exactly once. Covered items require current-run E-IDs and no gap. Non-covered items require one nested gap: material/search_scarcity need next_action; capability_blocked/source_unavailable need blocks_hard_constraint, required_source_type, and unblock_requirement; limitation needs only a description. A failed keyword search or transient browser/platform error is not a blocked gap. Use unmet_requirements only for global hard-constraint or delivery issues not owned by one subquestion.".to_string(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "subquestions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "id": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["covered", "partial", "missing", "blocked"]
                            },
                            "evidence_refs": {
                                "type": "array",
                                "maxItems": MAX_EVIDENCE_REFS,
                                "items": { "type": "string" }
                            },
                            "support_summary": { "type": "string" },
                            "gap": {
                                "type": ["object", "null"],
                                "additionalProperties": false,
                                "properties": {
                                    "kind": {
                                        "type": "string",
                                        "enum": [
                                            "material", "search_scarcity", "capability_blocked",
                                            "source_unavailable", "limitation"
                                        ]
                                    },
                                    "description": { "type": "string" },
                                    "next_action": { "type": "string" },
                                    "blocks_hard_constraint": { "type": "boolean" },
                                    "required_source_type": { "type": "string" },
                                    "unblock_requirement": { "type": "string" }
                                },
                                "required": ["kind", "description"]
                            }
                        },
                        "required": [
                            "id", "status", "evidence_refs", "support_summary", "gap"
                        ]
                    }
                },
                "hard_constraints_satisfied": { "type": "boolean" },
                "stop_conditions_satisfied": { "type": "boolean" },
                "unmet_requirements": {
                    "type": "array",
                    "maxItems": MAX_UNMET_REQUIREMENTS,
                    "items": { "type": "string" }
                },
                "final_answer": { "type": "string" }
            },
            "required": [
                "subquestions", "hard_constraints_satisfied",
                "stop_conditions_satisfied", "unmet_requirements", "final_answer"
            ]
        }),
    }
}

pub fn completion_protocol_correction_prompt() -> &'static str {
    "Your previous completion was invalid. Call submit_research_completion exactly once and alone. Report every brief subquestion; covered items need current-run E-IDs and gap=null. Non-covered items need one nested gap. Include unmet_requirements only when a hard constraint or stop condition is false, and include the complete final answer."
}

pub fn budget_exhausted_completion_prompt(max_steps: u32) -> String {
    format!(
        "Research ended at the maximum of {max_steps} steps. Do not call search, author_scan, get_notes, read_file, shell, or any other tool. Your only permitted action is exactly one submit_research_completion call using gathered evidence. Classify remaining gaps honestly, list global unmet requirements, and return the most useful supported final answer."
    )
}

pub fn forced_final_writer_system_prompt(
    brief: &ResearchBrief,
    extra_instructions: &str,
) -> anyhow::Result<String> {
    let rendered = serde_json::to_string_pretty(brief).map_err(anyhow::Error::from)?;
    let extra = extra_instructions.trim();
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("\n\nAdditional task instructions that still apply to the final answer:\n{extra}")
    };
    Ok(format!(
        "You are the final-answer writer for a completed socai research run.\n\n\
         The research phase has ended. You have no tools and must not request, describe, or simulate another tool action. Write the final answer now using only evidence already present in the supplied conversation and research brief.\n\n\
         Requirements:\n\
         1. Answer the user's original request directly and in the same language.\n\
         2. Preserve the requested deliverable and hard constraints.\n\
         3. Prefer concrete findings and evidence links already obtained in this run.\n\
         4. Do not invent missing facts or imply that an unverified constraint was met.\n\
         5. Clearly disclose material gaps caused by missing evidence or exhausted budget.\n\
         6. Produce only the complete user-facing answer in Markdown. Do not output JSON, coverage bookkeeping, planning commentary, or a tool call.\n\n\
         <research_brief protocol=\"research-brief-v1\">\n\
         {rendered}\n\
         </research_brief>{extra}"
    ))
}

pub fn forced_final_writer_prompt(attempt: u32) -> &'static str {
    if attempt <= 1 {
        "Research is finished. Write the best complete final answer now. Do not perform another action. Where the collected evidence is incomplete, give the useful supported portion and state the limitation explicitly."
    } else {
        "Your previous final-answer attempt was unusable. Return only a concise, complete user-facing answer now. No tools, no JSON, no planning, and no preamble. Prioritize covering the requested deliverable over detail; state missing evidence briefly."
    }
}

pub fn salvage_completion_text(
    input: Option<&Value>,
    visible_texts: &[String],
) -> Option<(String, FinalAnswerSource)> {
    if let Some(answer) = input
        .and_then(|value| value.get("final_answer"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| is_usable_final_answer(value))
    {
        return Some((answer.to_string(), FinalAnswerSource::SchemaSalvage));
    }
    let text = visible_texts.join("\n").trim().to_string();
    is_usable_final_answer(&text).then_some((text, FinalAnswerSource::VisibleTextSalvage))
}

pub fn is_usable_final_answer(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value == "The research run ended without a schema-valid final answer." {
        return false;
    }
    let compact: String = value
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric() || is_cjk(*ch))
        .collect();
    if compact.chars().count() < 24 {
        return false;
    }
    const PLACEHOLDERS: &[&str] = &[
        "占位",
        "占位符",
        "待补充",
        "待完善",
        "正在整理",
        "整理中",
        "稍后给出",
        "后续补充",
        "placeholder",
        "todo",
        "tbd",
    ];
    if PLACEHOLDERS
        .iter()
        .any(|placeholder| compact.starts_with(placeholder) && compact.chars().count() < 120)
    {
        return false;
    }
    const PROCESS_PREFIXES: &[&str] = &[
        "我将",
        "接下来我会",
        "下面我会",
        "让我先",
        "正在为你",
        "正在整理",
        "iwill",
        "letme",
        "workingon",
    ];
    !PROCESS_PREFIXES
        .iter()
        .any(|prefix| compact.starts_with(prefix) && compact.chars().count() < 120)
}

pub fn completion_tool_result_content(value: &Value) -> Vec<crate::agent::llm::ToolResultContent> {
    vec![crate::agent::llm::ToolResultContent::Text {
        text: value.to_string(),
    }]
}

pub fn answer_with_missing_limitations(final_answer: &str, limitations: &[String]) -> String {
    let missing: Vec<&str> = limitations
        .iter()
        .map(String::as_str)
        .filter(|limitation| !final_answer.contains(limitation))
        .collect();
    if missing.is_empty() {
        return final_answer.to_string();
    }
    let heading = if final_answer.chars().any(is_cjk) {
        "## 调研限制"
    } else {
        "## Research limitations"
    };
    let mut answer = final_answer.trim_end().to_string();
    answer.push_str("\n\n");
    answer.push_str(heading);
    answer.push('\n');
    for limitation in missing {
        answer.push_str("- ");
        answer.push_str(limitation);
        answer.push('\n');
    }
    answer.trim_end().to_string()
}

pub fn completion_limitations(
    submission: &ResearchCompletionSubmission,
    decision: CompletionGateDecision,
) -> Vec<String> {
    let mut limitations = submission.unmet_requirements.clone();
    if decision != CompletionGateDecision::FinishWithLimitations {
        return limitations;
    }
    for question in &submission.subquestions {
        if let Some(gap) = &question.gap {
            let limitation = format!("{}: {}", question.id, gap.description);
            if !limitations.contains(&limitation) {
                limitations.push(limitation);
            }
            if is_blocked_gap_kind(gap.kind) {
                let disclosure = if submission.final_answer.chars().any(is_cjk) {
                    format!(
                        "{} 缺少来源：{}；解除条件：{}",
                        question.id, gap.required_source_type, gap.unblock_requirement
                    )
                } else {
                    format!(
                        "{} missing source: {}; unblock requirement: {}",
                        question.id, gap.required_source_type, gap.unblock_requirement
                    )
                };
                if !limitations.contains(&disclosure) {
                    limitations.push(disclosure);
                }
            }
        }
    }
    limitations
}

fn normalize_evidence_id(locator: &str) -> Option<String> {
    let locator = locator.trim();
    let digits = locator
        .strip_prefix('E')
        .or_else(|| locator.strip_prefix('e'))?;
    (!digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| format!("E{digits}"))
}

fn canonical_tool_locator(locator: &str) -> Option<String> {
    let raw = locator.strip_prefix("tool:")?.trim();
    let raw = raw.strip_prefix("step-").unwrap_or(raw);
    let normalized = raw.replace(":call-", ":").replace("-call-", ":");
    let mut parts = normalized.split(':');
    let step = parts.next()?.parse::<u32>().ok()?;
    let sequence = parts.next()?.parse::<u32>().ok()?;
    parts
        .next()
        .is_none()
        .then(|| format!("tool:{step}:{sequence}"))
}

fn is_blocked_gap_kind(kind: ResearchGapKind) -> bool {
    matches!(
        kind,
        ResearchGapKind::CapabilityBlocked | ResearchGapKind::SourceUnavailable
    )
}

fn gap_kind_name(kind: ResearchGapKind) -> &'static str {
    match kind {
        ResearchGapKind::Material => "material",
        ResearchGapKind::SearchScarcity => "search_scarcity",
        ResearchGapKind::CapabilityBlocked => "capability_blocked",
        ResearchGapKind::SourceUnavailable => "source_unavailable",
        ResearchGapKind::Limitation => "limitation",
    }
}

fn status_name(status: SubquestionCoverageStatus) -> &'static str {
    match status {
        SubquestionCoverageStatus::Pending => "pending",
        SubquestionCoverageStatus::Covered => "covered",
        SubquestionCoverageStatus::Partial => "partial",
        SubquestionCoverageStatus::Missing => "missing",
        SubquestionCoverageStatus::Blocked => "blocked",
    }
}

fn normalize_required(value: &mut String, name: &str, max_chars: usize) -> anyhow::Result<()> {
    *value = value.trim().to_string();
    if value.is_empty() {
        anyhow::bail!("{name} must be non-empty");
    }
    if value.chars().count() > max_chars {
        anyhow::bail!("{name} exceeds {max_chars} characters");
    }
    Ok(())
}

fn normalize_optional(value: &mut String, name: &str) -> anyhow::Result<()> {
    *value = value.trim().to_string();
    if value.chars().count() > MAX_FIELD_CHARS {
        anyhow::bail!("{name} exceeds {MAX_FIELD_CHARS} characters");
    }
    Ok(())
}

fn normalize_list(values: &mut Vec<String>, name: &str, max_items: usize) -> anyhow::Result<()> {
    if values.len() > max_items {
        anyhow::bail!("{name} accepts at most {max_items} items");
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values.drain(..) {
        let value = value.trim().to_string();
        if value.is_empty() {
            anyhow::bail!("{name} must not contain empty items");
        }
        if value.chars().count() > MAX_FIELD_CHARS {
            anyhow::bail!("{name} item exceeds {MAX_FIELD_CHARS} characters");
        }
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    *values = normalized;
    Ok(())
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}
