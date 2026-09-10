//! Structured completion protocol for coverage-guided research runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    pub gap_kind: SubquestionGapKind,
    pub evidence_refs: Vec<String>,
    pub support_summary: String,
    pub material_gap: String,
    pub next_action: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubquestionGapKind {
    None,
    Material,
    SearchScarcity,
    CapabilityBlocked,
    SourceUnavailable,
    Limitation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchCompletionSubmission {
    pub subquestions: Vec<SubquestionCoverage>,
    pub hard_constraints_satisfied: bool,
    pub stop_conditions_satisfied: bool,
    pub gaps: Vec<ResearchGap>,
    pub limitations: Vec<String>,
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
    pub next_action: String,
    pub blocks_hard_constraint: Option<bool>,
    pub available_with_current_tools: Option<bool>,
    #[serde(default)]
    pub candidate_action: String,
    pub candidate_action_already_attempted: Option<bool>,
    #[serde(default)]
    pub blocked_reason: String,
    #[serde(default)]
    pub required_source_type: String,
    #[serde(default)]
    pub attempted_actions: Vec<String>,
    #[serde(default)]
    pub impact_on_deliverable: String,
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

impl CompletionGateDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::ResearchMore => "research_more",
            Self::LastMileResearch => "last_mile_research",
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
        let requested_kind = match self.decision {
            CompletionGateDecision::ResearchMore => Some(SubquestionGapKind::Material),
            CompletionGateDecision::LastMileResearch => Some(SubquestionGapKind::SearchScarcity),
            _ => None,
        };
        let required_gaps: Vec<Value> = submission
            .subquestions
            .iter()
            .zip(&brief.subquestions)
            .filter(|(coverage, question)| {
                question.priority == ResearchPriority::Required
                    && requested_kind.is_some_and(|kind| coverage.gap_kind == kind)
            })
            .map(|(coverage, _)| {
                json!({
                    "id": coverage.id,
                    "status": coverage.status,
                    "gap_kind": coverage.gap_kind,
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
            "gaps": submission.gaps,
            "instruction": match self.decision {
                CompletionGateDecision::ResearchMore =>
                    "Continue only on gaps whose kind is material, then submit completion again. Capability-blocked, source-unavailable, search-scarcity, and ordinary limitation gaps must not enter this general research path.",
                CompletionGateDecision::LastMileResearch =>
                    "This is the only last-mile recovery. Make at most two precise, non-duplicate tool calls targeting only the listed search-scarcity gaps. Do not repeat broad searches. After those results, submit completion again; no further scarcity recovery is available.",
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
        mut input: Value,
        brief: &ResearchBrief,
        evidence: &EvidenceLocatorCatalog,
    ) -> anyhow::Result<Self> {
        normalize_legacy_gap_input(&mut input)?;
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
        if self.gaps.len() > MAX_GAPS {
            anyhow::bail!("gaps accepts at most {MAX_GAPS} items");
        }
        for gap in &mut self.gaps {
            gap.normalize_and_validate()?;
        }
        normalize_list(&mut self.limitations, "limitations", MAX_LIMITATIONS)?;
        normalize_required(
            &mut self.final_answer,
            "final_answer",
            MAX_FINAL_ANSWER_CHARS,
        )?;
        if !is_usable_final_answer(&self.final_answer) {
            anyhow::bail!("final_answer is a placeholder, process update, or too short to be a usable research deliverable");
        }

        let required_blocked =
            brief
                .subquestions
                .iter()
                .zip(&self.subquestions)
                .any(|(question, coverage)| {
                    question.priority == ResearchPriority::Required
                        && is_blocked_subquestion_kind(coverage.gap_kind)
                });
        let has_blocked_gap = self.gaps.iter().any(|gap| is_blocked_gap_kind(gap.kind));
        if (required_blocked || has_blocked_gap) && self.limitations.is_empty() {
            anyhow::bail!("blocked required subquestions require user-facing limitations");
        }
        if has_blocked_gap
            && !self
                .subquestions
                .iter()
                .any(|coverage| is_blocked_subquestion_kind(coverage.gap_kind))
        {
            anyhow::bail!(
                "a capability_blocked/source_unavailable gap requires at least one related non-covered subquestion with the same blocked classification"
            );
        }
        if self
            .gaps
            .iter()
            .any(|gap| is_blocked_gap_kind(gap.kind) && gap.blocks_hard_constraint == Some(true))
            && self.hard_constraints_satisfied
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
        normalize_optional(&mut self.material_gap, "subquestion.material_gap")?;
        normalize_optional(&mut self.next_action, "subquestion.next_action")?;

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
                if self.gap_kind != SubquestionGapKind::None {
                    anyhow::bail!("covered subquestion {expected_id} must use gap_kind=none");
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
                if self.gap_kind == SubquestionGapKind::None {
                    anyhow::bail!(
                        "{} subquestion {expected_id} must classify its gap as material, search_scarcity, capability_blocked, source_unavailable, or limitation",
                        status_name(self.status)
                    );
                }
                if self.material_gap.is_empty() {
                    anyhow::bail!(
                        "{} subquestion {expected_id} requires material_gap",
                        status_name(self.status)
                    );
                }
                if matches!(
                    self.gap_kind,
                    SubquestionGapKind::Material | SubquestionGapKind::SearchScarcity
                ) && self.next_action.is_empty()
                {
                    anyhow::bail!(
                        "material/search_scarcity {} subquestion {expected_id} requires next_action",
                        status_name(self.status)
                    );
                }
            }
            SubquestionCoverageStatus::Blocked => {
                if !is_blocked_subquestion_kind(self.gap_kind) {
                    anyhow::bail!(
                        "blocked subquestion {expected_id} must use gap_kind=capability_blocked or source_unavailable"
                    );
                }
                if self.material_gap.is_empty() {
                    anyhow::bail!("blocked subquestion {expected_id} requires material_gap");
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
        normalize_optional(&mut self.candidate_action, "gap.candidate_action")?;
        normalize_optional(&mut self.blocked_reason, "gap.blocked_reason")?;
        normalize_optional(&mut self.required_source_type, "gap.required_source_type")?;
        normalize_list(
            &mut self.attempted_actions,
            "gap.attempted_actions",
            MAX_GAPS,
        )?;
        normalize_optional(&mut self.impact_on_deliverable, "gap.impact_on_deliverable")?;
        normalize_optional(&mut self.unblock_requirement, "gap.unblock_requirement")?;
        let blocks_hard_constraint = self
            .blocks_hard_constraint
            .ok_or_else(|| anyhow::anyhow!("every gap must answer blocks_hard_constraint"))?;
        let available_with_current_tools = self
            .available_with_current_tools
            .ok_or_else(|| anyhow::anyhow!("every gap must answer available_with_current_tools"))?;
        let candidate_action_already_attempted =
            self.candidate_action_already_attempted.ok_or_else(|| {
                anyhow::anyhow!("every gap must answer candidate_action_already_attempted")
            })?;

        if !available_with_current_tools && !is_blocked_gap_kind(self.kind) {
            anyhow::bail!(
                "a gap whose required source is unavailable with current tools must use capability_blocked or source_unavailable, not {}",
                gap_kind_name(self.kind)
            );
        }
        if is_blocked_gap_kind(self.kind) && available_with_current_tools {
            anyhow::bail!(
                "{} requires available_with_current_tools=false; a reachable source is not capability/source blocked",
                gap_kind_name(self.kind)
            );
        }
        if !self.candidate_action.is_empty() && !candidate_action_already_attempted {
            if !matches!(
                self.kind,
                ResearchGapKind::Material | ResearchGapKind::SearchScarcity
            ) {
                anyhow::bail!(
                    "a concrete, untried action available in the current run must be classified as material or search_scarcity, not {}",
                    gap_kind_name(self.kind)
                );
            }
            if self.next_action.is_empty() {
                anyhow::bail!("an untried candidate_action requires next_action");
            }
        }
        if self.kind == ResearchGapKind::SearchScarcity
            && (self.candidate_action.is_empty() || candidate_action_already_attempted)
        {
            anyhow::bail!(
                "search_scarcity requires one concrete candidate_action that has not already been attempted"
            );
        }
        if is_blocked_gap_kind(self.kind)
            && (!self.candidate_action.is_empty() || !candidate_action_already_attempted)
        {
            anyhow::bail!(
                "blocked gaps must have no available candidate action and candidate_action_already_attempted=true"
            );
        }
        if matches!(
            self.kind,
            ResearchGapKind::Material | ResearchGapKind::SearchScarcity
        ) && self.next_action.is_empty()
        {
            anyhow::bail!("material/search_scarcity gaps require a concrete next_action");
        }
        if self.kind == ResearchGapKind::SearchScarcity && self.attempted_actions.is_empty() {
            anyhow::bail!(
                "search_scarcity gaps require attempted_actions so last-mile queries can be checked for novelty"
            );
        }
        if is_blocked_gap_kind(self.kind) {
            for (name, value) in [
                ("blocked_reason", self.blocked_reason.as_str()),
                ("required_source_type", self.required_source_type.as_str()),
                ("impact_on_deliverable", self.impact_on_deliverable.as_str()),
                ("unblock_requirement", self.unblock_requirement.as_str()),
            ] {
                if value.is_empty() {
                    anyhow::bail!("{} gaps require non-empty {name}", gap_kind_name(self.kind));
                }
            }
            if self.attempted_actions.is_empty() {
                anyhow::bail!(
                    "{} gaps require attempted_actions explaining the capability/source check",
                    gap_kind_name(self.kind)
                );
            }
        }
        let _ = blocks_hard_constraint;
        Ok(())
    }
}

pub fn evaluate_completion(
    brief: &ResearchBrief,
    submission: &ResearchCompletionSubmission,
    budget_exhausted: bool,
    last_mile_available: bool,
) -> CompletionGateResult {
    let mut required_uncovered = 0usize;
    let mut required_scarce = 0usize;
    let mut required_blocked = 0usize;
    let mut required_limited = 0usize;
    let mut reasons = Vec::new();

    for (question, coverage) in brief.subquestions.iter().zip(&submission.subquestions) {
        if question.priority != ResearchPriority::Required {
            continue;
        }
        match coverage.status {
            SubquestionCoverageStatus::Partial | SubquestionCoverageStatus::Missing => {
                match coverage.gap_kind {
                    SubquestionGapKind::Material | SubquestionGapKind::None => {
                        required_uncovered += 1;
                        reasons.push(format!(
                            "{} remains materially {}: {}",
                            question.id,
                            status_name(coverage.status),
                            coverage.material_gap
                        ));
                    }
                    SubquestionGapKind::SearchScarcity => {
                        required_scarce += 1;
                        reasons.push(format!(
                            "{} has a searchable evidence scarcity: {}",
                            question.id, coverage.material_gap
                        ));
                    }
                    SubquestionGapKind::CapabilityBlocked
                    | SubquestionGapKind::SourceUnavailable => {
                        required_blocked += 1;
                        reasons.push(format!(
                            "{} is partially blocked: {}",
                            question.id, coverage.material_gap
                        ));
                    }
                    SubquestionGapKind::Limitation => {
                        required_limited += 1;
                        reasons.push(format!(
                            "{} has a non-material limitation: {}",
                            question.id, coverage.material_gap
                        ));
                    }
                }
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

    let has_material_gap = submission
        .gaps
        .iter()
        .any(|gap| gap.kind == ResearchGapKind::Material);
    let has_search_scarcity = submission
        .gaps
        .iter()
        .any(|gap| gap.kind == ResearchGapKind::SearchScarcity);
    let decision = if required_uncovered > 0 {
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ResearchMore
        }
    } else if has_material_gap {
        reasons.extend(
            submission
                .gaps
                .iter()
                .filter(|gap| gap.kind == ResearchGapKind::Material)
                .map(|gap| format!("unresolved material gap: {}", gap.description)),
        );
        if budget_exhausted {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::ResearchMore
        }
    } else if required_scarce > 0 || has_search_scarcity {
        reasons.extend(
            submission
                .gaps
                .iter()
                .filter(|gap| gap.kind == ResearchGapKind::SearchScarcity)
                .map(|gap| format!("search scarcity: {}", gap.description)),
        );
        if budget_exhausted || !last_mile_available {
            CompletionGateDecision::FinishWithLimitations
        } else {
            CompletionGateDecision::LastMileResearch
        }
    } else if required_blocked > 0
        || required_limited > 0
        || submission.gaps.iter().any(|gap| {
            matches!(
                gap.kind,
                ResearchGapKind::CapabilityBlocked
                    | ResearchGapKind::SourceUnavailable
                    | ResearchGapKind::Limitation
            )
        })
    {
        reasons.extend(submission.gaps.iter().filter_map(|gap| match gap.kind {
            ResearchGapKind::CapabilityBlocked => {
                Some(format!("capability-blocked gap: {}", gap.description))
            }
            ResearchGapKind::SourceUnavailable => {
                Some(format!("source-unavailable gap: {}", gap.description))
            }
            ResearchGapKind::Limitation => Some(format!("limitation: {}", gap.description)),
            ResearchGapKind::Material | ResearchGapKind::SearchScarcity => None,
        }));
        CompletionGateDecision::FinishWithLimitations
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
            "gap_kind": "none",
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
        description: "Submit the complete coverage checkpoint and final user-facing answer for this research run. Call this tool alone only when you are ready to finish. Report every brief subquestion exactly once. For covered items, cite the system-generated E1/E2 evidence IDs and set gap_kind=none. Classify every other subquestion and top-level gap: material means an ordinary available-tool action can still change the core answer; search_scarcity means one precise last-mile search may find a higher-match example; capability_blocked means the current tools cannot enter the required source; source_unavailable means the fact is not public (for example private messages, payments, or backend conversion); limitation means the gap does not change the core answer. Never use a blocked kind merely because current keywords found nothing, and never classify captcha, login, rate limit, or browser failures as a coverage gap. For every gap answer the blocked challenge: whether the exact required source is available_with_current_tools, whether a concrete candidate_action exists, and whether that action was already attempted. An unreachable required source cannot be limitation; an untried available action cannot be limitation. The legacy-named material_gap field must contain the concrete gap description for every non-covered subquestion. Material and search_scarcity require next_action. A capability_blocked/source_unavailable top-level gap must also fill blocked_reason, required_source_type, attempted_actions, impact_on_deliverable, unblock_requirement, and blocks_hard_constraint. This tool does not conduct research.".to_string(),
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
                            "gap_kind": {
                                "type": "string",
                                "enum": [
                                    "none", "material", "search_scarcity",
                                    "capability_blocked", "source_unavailable", "limitation"
                                ]
                            },
                            "evidence_refs": {
                                "type": "array",
                                "maxItems": MAX_EVIDENCE_REFS,
                                "items": { "type": "string" }
                            },
                            "support_summary": { "type": "string" },
                            "material_gap": {
                                "type": "string",
                                "description": "Concrete gap description. Must be non-empty for partial, missing, or blocked status; use an empty string only for covered status."
                            },
                            "next_action": { "type": "string" }
                        },
                        "required": [
                            "id", "status", "gap_kind", "evidence_refs", "support_summary",
                            "material_gap", "next_action"
                        ]
                    }
                },
                "hard_constraints_satisfied": { "type": "boolean" },
                "stop_conditions_satisfied": { "type": "boolean" },
                "gaps": {
                    "type": "array",
                    "maxItems": MAX_GAPS,
                    "items": {
                        "type": "object",
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
                            "available_with_current_tools": {
                                "type": "boolean",
                                "description": "Whether the exact required source/fact can be reached with tools available in this run. XHS reposts do not make a government website reachable."
                            },
                            "candidate_action": {
                                "type": "string",
                                "description": "One concrete non-duplicate action still available in this run, or an empty string if none exists."
                            },
                            "candidate_action_already_attempted": {
                                "type": "boolean",
                                "description": "False only when candidate_action is concrete and has not yet been attempted; true when there is no remaining candidate action."
                            },
                            "blocked_reason": { "type": "string" },
                            "required_source_type": { "type": "string" },
                            "attempted_actions": {
                                "type": "array",
                                "maxItems": MAX_GAPS,
                                "items": { "type": "string" },
                                "description": "Required for search_scarcity and blocked kinds. List prior queries/tool or source-capability checks so the next action can be verified as non-duplicate."
                            },
                            "impact_on_deliverable": { "type": "string" },
                            "unblock_requirement": { "type": "string" }
                        },
                        "required": [
                            "kind", "description", "next_action", "blocks_hard_constraint",
                            "available_with_current_tools", "candidate_action",
                            "candidate_action_already_attempted", "blocked_reason",
                            "required_source_type", "attempted_actions",
                            "impact_on_deliverable", "unblock_requirement"
                        ]
                    }
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
                "stop_conditions_satisfied", "gaps", "limitations",
                "final_answer"
            ]
        }),
    }
}

pub fn completion_protocol_correction_prompt() -> &'static str {
    "Your previous response did not produce one valid submit_research_completion call. Call it exactly once and alone. Cite supplied E1/E2 evidence IDs. Use gap_kind=none for covered items; otherwise classify gaps as material, search_scarcity, capability_blocked, source_unavailable, or limitation. For every gap answer available_with_current_tools, candidate_action, and candidate_action_already_attempted. An unreachable required source must be capability_blocked/source_unavailable; one concrete untried available action must be material/search_scarcity. Do not use blocked for a failed keyword search or a transient platform/browser error. Material and search_scarcity require a precise next_action. Blocked top-level gaps require auditable source/tool/impact/unblock fields, and blocks_hard_constraint=true requires hard_constraints_satisfied=false. Include the complete user-facing final answer."
}

pub fn budget_exhausted_completion_prompt(max_steps: u32) -> String {
    format!(
        "You have reached the maximum of {max_steps} research steps. The research phase is over and no additional action is allowed. Do not call shell, browser, filesystem, Xiaohongshu, or any other tool. Your only permitted action is exactly one submit_research_completion call using only the evidence already present in the conversation. A gap caused only by exhausted budget is a limitation, not capability_blocked. Use capability_blocked only when the available tools cannot enter the required source, and source_unavailable only for facts that are not public; include their audit fields. State concrete gaps, add clear user-facing limitations, keep unsupported claims out of final_answer, and produce the most useful complete answer possible."
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

fn normalize_legacy_gap_input(input: &mut Value) -> anyhow::Result<()> {
    let Value::Object(map) = input else {
        anyhow::bail!("research completion payload must be an object");
    };
    if let Some(Value::Array(subquestions)) = map.get_mut("subquestions") {
        for subquestion in subquestions {
            let Some(subquestion) = subquestion.as_object_mut() else {
                continue;
            };
            if subquestion.contains_key("gap_kind") {
                continue;
            }
            let inferred = match subquestion.get("status").and_then(Value::as_str) {
                Some("covered") | Some("pending") => "none",
                Some("blocked") => "capability_blocked",
                Some("partial") | Some("missing") => "material",
                _ => "material",
            };
            subquestion.insert("gap_kind".to_string(), json!(inferred));
        }
    }
    let legacy = map.remove("unresolved_gaps");
    if map.contains_key("gaps") {
        if legacy
            .as_ref()
            .is_some_and(|value| value.as_array().is_some_and(|items| !items.is_empty()))
        {
            anyhow::bail!("completion must use gaps, not both gaps and unresolved_gaps");
        }
        return Ok(());
    }
    let legacy = legacy.unwrap_or_else(|| json!([]));
    let items = legacy
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unresolved_gaps must be an array"))?;
    let mut gaps = Vec::with_capacity(items.len());
    for item in items {
        let description = item
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("legacy unresolved_gaps items must be strings"))?;
        gaps.push(json!({
            "kind": "search_scarcity",
            "description": description,
            "next_action": "Use at most two precise, non-duplicate current-tool queries targeting the missing attribute.",
            "blocks_hard_constraint": false,
            "available_with_current_tools": true,
            "candidate_action": "Use at most two precise, non-duplicate current-tool queries targeting the missing attribute.",
            "candidate_action_already_attempted": false,
            "blocked_reason": "",
            "required_source_type": "",
            "attempted_actions": ["legacy unresolved_gaps payload did not preserve the earlier query list"],
            "impact_on_deliverable": "",
            "unblock_requirement": ""
        }));
    }
    map.insert("gaps".to_string(), Value::Array(gaps));
    Ok(())
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

fn is_blocked_subquestion_kind(kind: SubquestionGapKind) -> bool {
    matches!(
        kind,
        SubquestionGapKind::CapabilityBlocked | SubquestionGapKind::SourceUnavailable
    )
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
