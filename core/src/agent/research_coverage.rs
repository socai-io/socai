//! Structured completion protocol for coverage-guided research runs.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::llm::ToolSchema;
use crate::agent::note_store::load_notes;
use crate::agent::research::{ResearchBrief, ResearchPriority};
use crate::agent::run_state::RunState;

pub const RESEARCH_COVERAGE_PROTOCOL_VERSION: &str = "research-coverage-v1";
pub const SUBMIT_RESEARCH_COMPLETION_TOOL: &str = "submit_research_completion";
pub const FORCED_FINALIZATION_VERSION: &str = "forced-finalization-v1";
pub const DEFAULT_MAX_COMPLETION_ATTEMPTS: u32 = 3;
pub const DEFAULT_MAX_COVERAGE_PROTOCOL_RETRIES: u32 = 1;
pub const DEFAULT_MAX_FORCED_WRITER_ATTEMPTS: u32 = 2;
pub const DEFAULT_MAX_RESEARCH_RECOVERY_ROUNDS: u32 = 2;

const MAX_EVIDENCE_REFS: usize = 12;
const MAX_GAPS: usize = 12;
const MAX_LIMITATIONS: usize = 12;
const MAX_FIELD_CHARS: usize = 1_000;
const MAX_FINAL_ANSWER_CHARS: usize = 50_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinalizationTrigger {
    MaxSteps,
    ProtocolRetriesExhausted,
    CompletionAttemptsExhausted,
    ResearchBudgetUnavailable,
}

impl FinalizationTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaxSteps => "max_steps",
            Self::ProtocolRetriesExhausted => "protocol_retries_exhausted",
            Self::CompletionAttemptsExhausted => "completion_attempts_exhausted",
            Self::ResearchBudgetUnavailable => "research_budget_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinalAnswerSource {
    StructuredCompletion,
    SchemaSalvage,
    VisibleTextSalvage,
    ForcedWriter,
    TruncatedWriterSalvage,
}

impl FinalAnswerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StructuredCompletion => "structured_completion",
            Self::SchemaSalvage => "schema_salvage",
            Self::VisibleTextSalvage => "visible_text_salvage",
            Self::ForcedWriter => "forced_writer",
            Self::TruncatedWriterSalvage => "truncated_writer_salvage",
        }
    }
}

/// Forced finalization is part of the default workflow. The explicit off switch
/// exists only to reproduce the pre-fix behavior during controlled regressions.
pub fn forced_finalization_enabled() -> bool {
    !std::env::var("SOCAI_FORCED_FINALIZATION")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "no"
            )
        })
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
    pub material_gap: String,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchCompletionSubmission {
    pub subquestions: Vec<SubquestionCoverage>,
    pub hard_constraints_satisfied: bool,
    pub stop_conditions_satisfied: bool,
    pub unresolved_gaps: Vec<String>,
    pub limitations: Vec<String>,
    pub final_answer: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionGateDecision {
    Accept,
    ResearchMore,
    ReviseOnly,
    FinishWithLimitations,
}

impl CompletionGateDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::ResearchMore => "research_more",
            Self::ReviseOnly => "revise_only",
            Self::FinishWithLimitations => "finish_with_limitations",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionGateResult {
    pub decision: CompletionGateDecision,
    pub reasons: Vec<String>,
    pub required_uncovered: usize,
    pub required_blocked: usize,
}

impl CompletionGateResult {
    pub fn accepted(&self) -> bool {
        matches!(
            self.decision,
            CompletionGateDecision::Accept | CompletionGateDecision::FinishWithLimitations
        )
    }

    pub fn tool_result_value(
        &self,
        brief: &ResearchBrief,
        submission: &ResearchCompletionSubmission,
    ) -> Value {
        let required_gaps: Vec<Value> = submission
            .subquestions
            .iter()
            .zip(&brief.subquestions)
            .filter(|(coverage, question)| {
                question.priority == ResearchPriority::Required
                    && matches!(
                        coverage.status,
                        SubquestionCoverageStatus::Partial | SubquestionCoverageStatus::Missing
                    )
            })
            .map(|(coverage, _)| {
                json!({
                    "id": coverage.id,
                    "status": coverage.status,
                    "gap": coverage.material_gap,
                    "next_action": coverage.next_action,
                })
            })
            .collect();
        json!({
            "accepted": self.accepted(),
            "action": self.decision,
            "reasons": self.reasons,
            "required_gaps": required_gaps,
            "unresolved_gaps": submission.unresolved_gaps,
            "instruction": match self.decision {
                CompletionGateDecision::ResearchMore =>
                    "Continue only on the listed material gaps, then submit completion again.",
                CompletionGateDecision::ReviseOnly =>
                    "Use the evidence already gathered to revise the deliverable, then submit completion again without external research tools.",
                CompletionGateDecision::Accept | CompletionGateDecision::FinishWithLimitations =>
                    "The completion submission was accepted.",
            },
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceLocatorCatalog {
    locators: BTreeSet<String>,
}

impl EvidenceLocatorCatalog {
    pub fn from_run(
        run_dir: &Path,
        run_state: &RunState,
        tool_locators: &BTreeSet<String>,
    ) -> Self {
        let mut locators = tool_locators.clone();
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
                locators.insert(format!("artifact:{path}"));
            }
        }
        Self { locators }
    }

    fn contains(&self, locator: &str) -> bool {
        self.locators.contains(locator)
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
        for (index, (coverage, expected)) in self
            .subquestions
            .iter_mut()
            .zip(&brief.subquestions)
            .enumerate()
        {
            coverage.normalize_and_validate(index + 1, &expected.id, evidence)?;
        }
        normalize_list(&mut self.unresolved_gaps, "unresolved_gaps", MAX_GAPS)?;
        normalize_list(&mut self.limitations, "limitations", MAX_LIMITATIONS)?;
        normalize_required(
            &mut self.final_answer,
            "final_answer",
            MAX_FINAL_ANSWER_CHARS,
        )?;

        let required_blocked =
            brief
                .subquestions
                .iter()
                .zip(&self.subquestions)
                .any(|(question, coverage)| {
                    question.priority == ResearchPriority::Required
                        && coverage.status == SubquestionCoverageStatus::Blocked
                });
        if required_blocked && self.limitations.is_empty() {
            anyhow::bail!("blocked required subquestions require user-facing limitations");
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
        normalize_optional(&mut self.material_gap, "subquestion.material_gap")?;
        normalize_optional(&mut self.next_action, "subquestion.next_action")?;

        for locator in &self.evidence_refs {
            if !is_supported_locator(locator) {
                anyhow::bail!(
                    "subquestion {expected_id} has unsupported evidence locator '{locator}'"
                );
            }
            if !evidence.contains(locator) {
                anyhow::bail!(
                    "subquestion {expected_id} cites evidence locator not found in this run: {locator}"
                );
            }
        }
        match self.status {
            SubquestionCoverageStatus::Covered => {
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
                if self.material_gap.is_empty() {
                    anyhow::bail!(
                        "{} subquestion {expected_id} requires material_gap",
                        status_name(self.status)
                    );
                }
                if self.next_action.is_empty() {
                    anyhow::bail!(
                        "{} subquestion {expected_id} requires next_action",
                        status_name(self.status)
                    );
                }
            }
            SubquestionCoverageStatus::Blocked => {
                if self.material_gap.is_empty() {
                    anyhow::bail!("blocked subquestion {expected_id} requires material_gap");
                }
            }
            SubquestionCoverageStatus::Pending => unreachable!(),
        }
        Ok(())
    }
}

pub fn evaluate_completion(
    brief: &ResearchBrief,
    submission: &ResearchCompletionSubmission,
    budget_exhausted: bool,
) -> CompletionGateResult {
    let mut required_uncovered = 0usize;
    let mut required_blocked = 0usize;
    let mut reasons = Vec::new();

    for (question, coverage) in brief.subquestions.iter().zip(&submission.subquestions) {
        if question.priority != ResearchPriority::Required {
            continue;
        }
        match coverage.status {
            SubquestionCoverageStatus::Partial | SubquestionCoverageStatus::Missing => {
                required_uncovered += 1;
                reasons.push(format!(
                    "{} remains {}: {}",
                    question.id,
                    status_name(coverage.status),
                    coverage.material_gap
                ));
            }
            SubquestionCoverageStatus::Blocked => {
                required_blocked += 1;
                reasons.push(format!(
                    "{} is blocked: {}",
                    question.id, coverage.material_gap
                ));
            }
            SubquestionCoverageStatus::Covered => {}
            SubquestionCoverageStatus::Pending => {
                required_uncovered += 1;
                reasons.push(format!("{} remains pending", question.id));
            }
        }
    }

    let decision = if required_uncovered > 0 {
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ResearchMore
        }
    } else if required_blocked > 0 {
        CompletionGateDecision::FinishWithLimitations
    } else if !submission.unresolved_gaps.is_empty() {
        reasons.extend(
            submission
                .unresolved_gaps
                .iter()
                .map(|gap| format!("unresolved material gap: {gap}")),
        );
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ResearchMore
        }
    } else if !submission.hard_constraints_satisfied || !submission.stop_conditions_satisfied {
        if !submission.hard_constraints_satisfied {
            reasons.push("one or more hard constraints are not satisfied".to_string());
        }
        if !submission.stop_conditions_satisfied {
            reasons.push("one or more brief stop conditions are not satisfied".to_string());
        }
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ReviseOnly
        }
    } else {
        CompletionGateDecision::Accept
    };

    CompletionGateResult {
        decision,
        reasons,
        required_uncovered,
        required_blocked,
    }
}

pub fn initial_coverage_state(brief: &ResearchBrief) -> Value {
    json!({
        "protocol_version": RESEARCH_COVERAGE_PROTOCOL_VERSION,
        "status": "pending",
        "subquestions": brief.subquestions.iter().map(|question| json!({
            "id": question.id,
            "status": "pending",
            "evidence_refs": [],
            "support_summary": "",
            "material_gap": "",
            "next_action": "",
        })).collect::<Vec<_>>(),
    })
}

pub fn research_completion_tool_schema() -> ToolSchema {
    ToolSchema {
        name: SUBMIT_RESEARCH_COMPLETION_TOOL.to_string(),
        description: "Submit the complete coverage checkpoint and final user-facing answer for this research run. Call this tool alone only when you are ready to finish. Report every brief subquestion exactly once. A covered item must cite evidence locators actually produced in this run, preferably note:<note_id> or artifact:<relative_path>; tool:<step>:<sequence> is also accepted. Partial or missing required items will return material gaps for further research when budget remains. Blocked items require explicit user-facing limitations. This tool does not conduct research.".to_string(),
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
                            "material_gap": { "type": "string" },
                            "next_action": { "type": "string" }
                        },
                        "required": [
                            "id", "status", "evidence_refs", "support_summary",
                            "material_gap", "next_action"
                        ]
                    }
                },
                "hard_constraints_satisfied": { "type": "boolean" },
                "stop_conditions_satisfied": { "type": "boolean" },
                "unresolved_gaps": {
                    "type": "array",
                    "maxItems": MAX_GAPS,
                    "items": { "type": "string" }
                },
                "limitations": {
                    "type": "array",
                    "maxItems": MAX_LIMITATIONS,
                    "items": { "type": "string" }
                },
                "final_answer": { "type": "string" }
            },
            "required": [
                "subquestions", "hard_constraints_satisfied",
                "stop_conditions_satisfied", "unresolved_gaps", "limitations",
                "final_answer"
            ]
        }),
    }
}

pub fn completion_protocol_correction_prompt() -> &'static str {
    "Your previous response attempted to finish without one valid submit_research_completion call, so it has not been accepted as the final answer. In coverage-guided research mode, finish by calling submit_research_completion exactly once and do not combine it with another tool. Report every brief subquestion and include the complete user-facing answer in final_answer. If evidence is still materially missing and research tools are available, continue researching instead."
}

pub fn budget_exhausted_completion_prompt(max_steps: u32) -> String {
    format!(
        "You have reached the maximum of {max_steps} research steps. The research phase is over and no additional action is allowed. Do not call shell, browser, filesystem, Xiaohongshu, or any other tool. Your only permitted action is exactly one submit_research_completion call using only the evidence already present in the conversation. If a fact is missing, mark the corresponding required subquestion blocked rather than trying to retrieve or inspect anything else. State the concrete reason in material_gap, add a clear user-facing disclosure to limitations, keep unsupported claims out of final_answer, and produce the most useful complete answer possible within those limits."
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
    !value.is_empty() && value != "The research run ended without a schema-valid final answer."
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

fn is_supported_locator(locator: &str) -> bool {
    locator.starts_with("note:") || locator.starts_with("artifact:") || locator.starts_with("tool:")
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn budget_prompt_forbids_all_follow_on_tools() {
        let prompt = budget_exhausted_completion_prompt(30);
        assert!(prompt.contains("Do not call shell, browser, filesystem, Xiaohongshu"));
        assert!(prompt.contains("exactly one submit_research_completion"));
        assert!(!prompt.contains("Do not call external research tools"));
    }

    #[test]
    fn salvage_prefers_embedded_final_answer() {
        let input = json!({"final_answer": "  supported answer  "});
        let visible = vec!["fallback prose".to_string()];
        let (answer, source) =
            salvage_completion_text(Some(&input), &visible).expect("answer should be salvaged");
        assert_eq!(answer, "supported answer");
        assert_eq!(source, FinalAnswerSource::SchemaSalvage);
    }

    #[test]
    fn salvage_uses_visible_text_when_schema_has_no_answer() {
        let input = json!({"subquestions": []});
        let visible = vec!["first".to_string(), "second".to_string()];
        let (answer, source) =
            salvage_completion_text(Some(&input), &visible).expect("text should be salvaged");
        assert_eq!(answer, "first\nsecond");
        assert_eq!(source, FinalAnswerSource::VisibleTextSalvage);
    }

    #[test]
    fn legacy_placeholder_is_not_a_usable_answer() {
        let placeholder = "The research run ended without a schema-valid final answer.";
        assert!(!is_usable_final_answer(placeholder));
        assert!(salvage_completion_text(None, &[placeholder.to_string()]).is_none());
    }

    #[test]
    fn writer_retry_prompt_requires_plain_final_text() {
        let prompt = forced_final_writer_prompt(2);
        assert!(prompt.contains("No tools, no JSON, no planning"));
        assert!(prompt.contains("complete user-facing answer"));
    }
}
