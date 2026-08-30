//! Brief-guided research planning for the opt-in deep-research variant.

use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::llm::ToolSchema;

pub const RESEARCH_BRIEF_PROTOCOL_VERSION: &str = "research-brief-v1";
pub const SUBMIT_RESEARCH_BRIEF_TOOL: &str = "submit_research_brief";
pub const DEFAULT_RESEARCH_PLAN_MAX_TOKENS: u32 = 6_000;

const MAX_OBJECTIVE_CHARS: usize = 1_000;
const MAX_DELIVERABLE_CHARS: usize = 1_000;
const MAX_SCOPE_CHARS: usize = 500;
const MAX_SUBQUESTION_CHARS: usize = 1_000;
const MAX_LIST_ITEM_CHARS: usize = 500;
const MAX_CLARIFICATION_CHARS: usize = 500;
const MAX_SUBQUESTIONS: usize = 4;
const MAX_REQUIREMENTS_PER_QUESTION: usize = 8;
const MAX_HARD_CONSTRAINTS: usize = 8;
const MAX_ASSUMPTIONS: usize = 8;
const MAX_SEARCH_ANGLES: usize = 4;
const MAX_STOP_CONDITIONS: usize = 6;
const MAX_SUBJECTS: usize = 8;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    ReactiveV1,
    BriefGuidedResearchV1,
    BriefCoverageResearchV2,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReactiveV1 => "reactive_v1",
            Self::BriefGuidedResearchV1 => "brief_guided_research_v1",
            Self::BriefCoverageResearchV2 => "brief_coverage_research_v2",
        }
    }

    pub fn requires_research_brief(self) -> bool {
        self != Self::ReactiveV1
    }

    pub fn requires_research_coverage(self) -> bool {
        self == Self::BriefCoverageResearchV2
    }
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "reactive_v1" | "reactive" | "standard" => Ok(Self::ReactiveV1),
            "brief_guided_research_v1" | "deep_research" | "deep" => {
                Ok(Self::BriefGuidedResearchV1)
            }
            "brief_coverage_research_v2" | "deep_research_v2" | "deep_v2" => {
                Ok(Self::BriefCoverageResearchV2)
            }
            other => anyhow::bail!(
                "unknown agent mode '{other}' (expected reactive_v1, brief_guided_research_v1, or brief_coverage_research_v2)"
            ),
        }
    }
}

pub fn brief_guided_research_enabled() -> bool {
    env_flag("SOCAI_DEEP_RESEARCH_V1")
}

pub fn coverage_guided_research_enabled() -> bool {
    env_flag("SOCAI_RESEARCH_COVERAGE_V2")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        )
    })
}

pub fn agent_mode_from_env() -> anyhow::Result<AgentMode> {
    let Some(value) = std::env::var("SOCAI_AGENT_MODE").ok() else {
        return Ok(AgentMode::ReactiveV1);
    };
    let mode = AgentMode::from_str(&value)?;
    ensure_agent_mode_available(mode)?;
    Ok(mode)
}

pub fn ensure_agent_mode_available(mode: AgentMode) -> anyhow::Result<()> {
    if mode.requires_research_brief() && !brief_guided_research_enabled() {
        anyhow::bail!(
            "{} is unavailable; set SOCAI_DEEP_RESEARCH_V1=on to enable deep research",
            mode.as_str()
        );
    }
    if mode.requires_research_coverage() && !coverage_guided_research_enabled() {
        anyhow::bail!(
            "brief_coverage_research_v2 is unavailable; set SOCAI_RESEARCH_COVERAGE_V2=on to enable it"
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BriefDecision {
    Proceed,
    Clarify,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchPriority {
    Required,
    Optional,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchScope {
    pub time_range: String,
    pub location: String,
    pub subjects: Vec<String>,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchSubquestion {
    pub id: String,
    pub question: String,
    pub priority: ResearchPriority,
    pub evidence_requirements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchBrief {
    pub objective: String,
    pub deliverable: String,
    pub scope: ResearchScope,
    pub subquestions: Vec<ResearchSubquestion>,
    pub hard_constraints: Vec<String>,
    pub assumptions: Vec<String>,
    pub initial_search_angles: Vec<String>,
    pub stop_conditions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResearchBriefSubmission {
    pub decision: BriefDecision,
    #[serde(default)]
    pub clarifying_question: Option<String>,
    #[serde(default)]
    pub brief: Option<ResearchBrief>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchBriefEnvelope {
    pub schema_version: u32,
    pub protocol_version: String,
    pub decision: BriefDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarifying_question: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief: Option<ResearchBrief>,
}

impl ResearchBriefEnvelope {
    pub fn from_tool_input(input: Value) -> anyhow::Result<Self> {
        let mut submission: ResearchBriefSubmission = serde_json::from_value(input)
            .map_err(|error| anyhow::anyhow!("invalid research brief payload: {error}"))?;
        submission.normalize_and_validate()?;
        Ok(Self {
            schema_version: 1,
            protocol_version: RESEARCH_BRIEF_PROTOCOL_VERSION.to_string(),
            decision: submission.decision,
            clarifying_question: submission.clarifying_question,
            brief: submission.brief,
        })
    }

    pub fn clarification(&self) -> Option<&str> {
        (self.decision == BriefDecision::Clarify)
            .then_some(self.clarifying_question.as_deref())
            .flatten()
    }

    pub fn brief(&self) -> Option<&ResearchBrief> {
        (self.decision == BriefDecision::Proceed)
            .then_some(self.brief.as_ref())
            .flatten()
    }

    pub fn persist(&self, run_dir: &Path) -> std::io::Result<()> {
        let dir = run_dir.join("research");
        std::fs::create_dir_all(&dir)?;
        let value = serde_json::to_value(self).map_err(std::io::Error::other)?;
        write_json_atomic(&dir.join("brief.json"), &value)
    }
}

impl ResearchBriefSubmission {
    fn normalize_and_validate(&mut self) -> anyhow::Result<()> {
        match self.decision {
            BriefDecision::Clarify => {
                let question = self
                    .clarifying_question
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("clarify requires clarifying_question"))?;
                normalize_required(question, "clarifying_question", MAX_CLARIFICATION_CHARS)?;
                if self.brief.is_some() {
                    anyhow::bail!("clarify must not include brief");
                }
            }
            BriefDecision::Proceed => {
                if self
                    .clarifying_question
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    anyhow::bail!("proceed must not include clarifying_question");
                }
                self.clarifying_question = None;
                self.brief
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("proceed requires brief"))?
                    .normalize_and_validate()?;
            }
        }
        Ok(())
    }
}

impl ResearchBrief {
    fn normalize_and_validate(&mut self) -> anyhow::Result<()> {
        normalize_required(&mut self.objective, "objective", MAX_OBJECTIVE_CHARS)?;
        normalize_required(&mut self.deliverable, "deliverable", MAX_DELIVERABLE_CHARS)?;
        self.scope.normalize_and_validate()?;
        if self.subquestions.is_empty() || self.subquestions.len() > MAX_SUBQUESTIONS {
            anyhow::bail!("subquestions must contain 1-{MAX_SUBQUESTIONS} items");
        }
        for (index, subquestion) in self.subquestions.iter_mut().enumerate() {
            subquestion.normalize_and_validate(index + 1)?;
        }
        normalize_list(
            &mut self.hard_constraints,
            "hard_constraints",
            MAX_HARD_CONSTRAINTS,
        )?;
        normalize_list(&mut self.assumptions, "assumptions", MAX_ASSUMPTIONS)?;
        normalize_list(
            &mut self.initial_search_angles,
            "initial_search_angles",
            MAX_SEARCH_ANGLES,
        )?;
        normalize_list(
            &mut self.stop_conditions,
            "stop_conditions",
            MAX_STOP_CONDITIONS,
        )?;
        if self.stop_conditions.is_empty() {
            anyhow::bail!("stop_conditions must contain at least one item");
        }
        Ok(())
    }

    pub fn execution_prompt(&self) -> anyhow::Result<String> {
        let rendered = serde_json::to_string_pretty(self).map_err(anyhow::Error::from)?;
        Ok(format!(
            "## Brief-guided research execution\n\n\
             The validated research brief below is the task contract for this run. It is a plan, not evidence.\n\n\
             - Use the existing Xiaohongshu tool playbook to gather real evidence for the required subquestions.\n\
             - Preserve the user's hard constraints and scope. You may adapt search queries and tool order when real results require it, but do not silently change the requested deliverable.\n\
             - Distinguish official or first-party facts, user note bodies, top comments, author-profile facts, OCR, and audio transcripts.\n\
             - Before ending, check each required subquestion and the brief's stop conditions against tool results. If evidence is missing or the site is blocked, state the limitation instead of inventing an answer.\n\
             - Do not repeat the brief in the final answer. Produce the requested deliverable.\n\n\
             <research_brief protocol=\"{RESEARCH_BRIEF_PROTOCOL_VERSION}\">\n\
             {rendered}\n\
             </research_brief>"
        ))
    }

    pub fn coverage_execution_prompt(&self) -> anyhow::Result<String> {
        let rendered = serde_json::to_string_pretty(self).map_err(anyhow::Error::from)?;
        Ok(format!(
            "## Coverage-guided research execution\n\n\
             The validated research brief below is the task contract for this run. It is a plan, not evidence.\n\n\
             - Research the required subquestions with the existing Xiaohongshu tools. You may address them in any efficient order, and one tool result may support more than one subquestion.\n\
             - Keep track of which required subquestions are covered, partial, missing, or blocked. Do not call a subquestion covered merely because the brief mentions it or because the answer sounds plausible.\n\
             - A covered subquestion needs evidence actually obtained in this run and an answer that uses that evidence to satisfy the requested deliverable.\n\
             - Preserve the user's hard constraints and scope. Distinguish first-party facts, note-body claims, comments, author-profile facts, OCR, and transcripts.\n\
             - For recommendation, comparison, or planning tasks, use the evidence to select, rank, or reject candidates against the user's important constraints; do not merely collect independent facts.\n\
             - Do not finish with ordinary prose. When ready to finish, call submit_research_completion exactly once and do not combine it with another tool call.\n\
             - In submit_research_completion, report every brief subquestion exactly once, use only evidence locators from this run, disclose material unresolved limitations, and include the complete user-facing final answer.\n\n\
             <research_brief protocol=\"{RESEARCH_BRIEF_PROTOCOL_VERSION}\">\n\
             {rendered}\n\
             </research_brief>"
        ))
    }
}

impl ResearchScope {
    fn normalize_and_validate(&mut self) -> anyhow::Result<()> {
        normalize_required(&mut self.time_range, "scope.time_range", MAX_SCOPE_CHARS)?;
        normalize_required(&mut self.location, "scope.location", MAX_SCOPE_CHARS)?;
        normalize_required(&mut self.language, "scope.language", MAX_SCOPE_CHARS)?;
        normalize_list(&mut self.subjects, "scope.subjects", MAX_SUBJECTS)?;
        if self.subjects.is_empty() {
            anyhow::bail!("scope.subjects must contain at least one item");
        }
        Ok(())
    }
}

impl ResearchSubquestion {
    fn normalize_and_validate(&mut self, position: usize) -> anyhow::Result<()> {
        let expected_id = format!("Q{position}");
        normalize_required(&mut self.id, "subquestion.id", 8)?;
        self.id = normalize_subquestion_id(&self.id, position);
        if self.id != expected_id {
            anyhow::bail!("subquestion {position} id must be {expected_id}");
        }
        normalize_required(
            &mut self.question,
            "subquestion.question",
            MAX_SUBQUESTION_CHARS,
        )?;
        normalize_list(
            &mut self.evidence_requirements,
            "subquestion.evidence_requirements",
            MAX_REQUIREMENTS_PER_QUESTION,
        )?;
        if self.evidence_requirements.is_empty() {
            anyhow::bail!("subquestion {expected_id} requires evidence_requirements");
        }
        Ok(())
    }
}

pub fn planner_system_prompt() -> String {
    format!(
        "You are the planning stage of socai's Xiaohongshu research workflow.\n\n\
         Your only job is to convert the user's current request and relevant prior conversation into one decision-complete research brief. Do not research the topic, answer the user's question, browse Xiaohongshu, inspect local files, or invent facts.\n\n\
         You have exactly one tool: {SUBMIT_RESEARCH_BRIEF_TOOL}. Call it exactly once and do not produce a prose answer.\n\n\
         Planning rules:\n\
         1. Preserve the user's actual objective, requested deliverable, explicit constraints, time range, location, subjects, and language.\n\
         2. Create 1-4 distinct, non-overlapping subquestions. Merge similar questions. Include only questions necessary to produce the requested deliverable. Use consecutive subquestion ids Q1, Q2, Q3, Q4; do not use SQ1 or other prefixes.\n\
         3. For each subquestion, specify the main evidence types needed. Distinguish official or first-party facts from user experiences, note-body claims, top comments, author-profile facts, OCR, and audio transcripts.\n\
         4. Add hard constraints only when they come from the user or are required for evidence-grounded research. Do not invent product requirements.\n\
         5. State reversible assumptions explicitly. Never convert an assumption into a fact.\n\
         6. Provide at most four initial search angles. They are starting points, not a fixed execution script; the research agent may adapt after seeing real results.\n\
         7. Define concrete stop conditions based on subquestion coverage and evidence, not on a fixed number of searches alone.\n\
         8. Ask one concise clarification question only when missing user-owned information would materially change the research scope or deliverable. Do not ask for facts that socai can discover with its tools.\n\
         9. The brief is a plan, not evidence. Do not include unverified claims about the subject.\n\
         10. Use the same language as the user's current request for all brief fields and any clarification question.\n\n\
         Today's date is {}.",
        chrono::Local::now().format("%Y-%m-%d (%A)")
    )
}

pub fn planner_correction_prompt() -> &'static str {
    "Your previous planning response was not a valid submit_research_brief call and has been discarded. Do not answer the research question. Call submit_research_brief exactly once with a schema-valid clarification or complete brief. If you include subquestions, their ids must be consecutive Q1, Q2, Q3, Q4."
}

pub fn research_brief_tool_schema() -> ToolSchema {
    ToolSchema {
        name: SUBMIT_RESEARCH_BRIEF_TOOL.to_string(),
        description: "Submit the complete planning result for this task. Call exactly once. This tool records either one necessary clarification question or one complete research brief. It does not conduct research.".to_string(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "decision": { "type": "string", "enum": ["proceed", "clarify"] },
                "clarifying_question": { "type": "string" },
                "brief": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "objective": { "type": "string" },
                        "deliverable": { "type": "string" },
                        "scope": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "time_range": { "type": "string" },
                                "location": { "type": "string" },
                                "subjects": {
                                    "type": "array",
                                    "minItems": 1,
                                    "maxItems": MAX_SUBJECTS,
                                    "items": { "type": "string" }
                                },
                                "language": { "type": "string" }
                            },
                            "required": ["time_range", "location", "subjects", "language"]
                        },
                        "subquestions": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": MAX_SUBQUESTIONS,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "id": { "type": "string" },
                                    "question": { "type": "string" },
                                    "priority": {
                                        "type": "string",
                                        "enum": ["required", "optional"]
                                    },
                                    "evidence_requirements": {
                                        "type": "array",
                                        "minItems": 1,
                                        "maxItems": MAX_REQUIREMENTS_PER_QUESTION,
                                        "items": { "type": "string" }
                                    }
                                },
                                "required": ["id", "question", "priority", "evidence_requirements"]
                            }
                        },
                        "hard_constraints": {
                            "type": "array",
                            "maxItems": MAX_HARD_CONSTRAINTS,
                            "items": { "type": "string" }
                        },
                        "assumptions": {
                            "type": "array",
                            "maxItems": MAX_ASSUMPTIONS,
                            "items": { "type": "string" }
                        },
                        "initial_search_angles": {
                            "type": "array",
                            "maxItems": MAX_SEARCH_ANGLES,
                            "items": { "type": "string" }
                        },
                        "stop_conditions": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": MAX_STOP_CONDITIONS,
                            "items": { "type": "string" }
                        }
                    },
                    "required": [
                        "objective", "deliverable", "scope", "subquestions",
                        "hard_constraints", "assumptions", "initial_search_angles",
                        "stop_conditions"
                    ]
                }
            },
            "required": ["decision"]
        }),
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
        if value.chars().count() > MAX_LIST_ITEM_CHARS {
            anyhow::bail!("{name} item exceeds {MAX_LIST_ITEM_CHARS} characters");
        }
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    *values = normalized;
    Ok(())
}

fn normalize_subquestion_id(raw_id: &str, position: usize) -> String {
    let id = raw_id.trim().to_ascii_uppercase();
    let expected = format!("Q{position}");
    let accepted_alias = format!("SQ{position}");
    if id == expected || id == accepted_alias {
        expected
    } else {
        id
    }
}

fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let rendered = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, rendered)?;
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    std::fs::rename(temp, path)
}
