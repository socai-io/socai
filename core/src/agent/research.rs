//! Research brief planning for the default agent workflow.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;

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
    pub fn from_tool_input(mut input: Value) -> anyhow::Result<Self> {
        normalize_planner_tool_input(&mut input);
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
            "## Coverage-guided research execution\n\n\
             The validated research brief below is the task contract for this run. It is a plan, not evidence.\n\n\
             - Before the first research action, call read_skill with name `insight-research` and follow that workflow for evidence selection, links, considerations, and report delivery.\n\
             - Research the required subquestions with the existing Xiaohongshu tools. You may address them in any efficient order, and one tool result may support more than one subquestion.\n\
             - Keep track of which required subquestions are covered, partial, missing, or blocked. Do not call a subquestion covered merely because the brief mentions it or because the answer sounds plausible.\n\
             - A covered subquestion needs evidence actually obtained in this run and an answer that uses that evidence to satisfy the requested deliverable.\n\
             - Preserve the user's hard constraints and scope. Distinguish first-party facts, note-body claims, comments, author-profile facts, OCR, and transcripts.\n\
             - For recommendation, comparison, or planning tasks, use the evidence to select, rank, or reject candidates against the user's important constraints; do not merely collect independent facts.\n\
             - Do not finish with ordinary prose. When ready to finish, call submit_research_completion exactly once and do not combine it with another tool call.\n\
             - Each successful tool result includes a short system-generated evidence_id such as E1 or E2. In submit_research_completion, report every brief subquestion exactly once and cite those short IDs; do not invent or reconstruct locator paths. IDs from older compacted results remain listed in Earlier compacted context.\n\
             - For each covered subquestion, cite current-run E-IDs and set gap=null. For each non-covered subquestion, include one nested gap: material means an ordinary available-tool action can still change the core answer; search_scarcity means one precise last-mile search may find a higher-match example; capability_blocked means current tools cannot enter the required source; source_unavailable means the fact is not public; limitation means the gap does not change the core answer. A failed keyword search, captcha, login wall, rate limit, or browser failure is never a blocked coverage gap.\n\
             - Material and search_scarcity gaps need a precise next_action. Blocked gaps need blocks_hard_constraint, required_source_type, and unblock_requirement. Use unmet_requirements only for global hard-constraint or delivery issues not owned by one subquestion; it must be non-empty when hard_constraints_satisfied or stop_conditions_satisfied is false, otherwise empty.\n\
             - Boundary examples: (a) broad searches found adjacent people but an untried missing-attribute query could find a higher-match first-person case -> search_scarcity; (b) several targeted queries yielded no new high-match evidence and the core deliverable is already stable -> limitation; (c) the task requires an official webpage but the available tools cannot browse that source -> capability_blocked, with the affected subquestion non-covered and any source-dependent hard constraint false.\n\
             - Include the complete user-facing final answer. For blocked claims, lower certainty, explain the impact, and state what the user or a future tool must provide to unblock them.\n\n\
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

pub fn planner_correction_prompt(validation_error: &str) -> String {
    let error: String = validation_error.chars().take(500).collect();
    format!(
        "Your previous planning response was not a valid submit_research_brief call and has been discarded. Validation error: {error}. Do not answer the research question. Call submit_research_brief exactly once with a schema-valid clarification or complete brief. If you include subquestions, merge them into at most four non-empty items; their ids must be consecutive Q1, Q2, Q3, Q4."
    )
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
    if id == expected || looks_like_question_id(&id) {
        expected
    } else {
        id
    }
}

fn looks_like_question_id(id: &str) -> bool {
    let suffix = id.strip_prefix("SQ").or_else(|| id.strip_prefix('Q'));
    let Some(suffix) = suffix else {
        return false;
    };
    let mut chars = suffix.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Normalize only planner-output variations observed in persisted runs. Unknown
/// business fields and structurally incomplete briefs remain validation errors.
fn normalize_planner_tool_input(input: &mut Value) {
    let Some(root) = input.as_object_mut() else {
        return;
    };

    if !root.contains_key("decision") {
        let has_brief = root.get("brief").is_some_and(Value::is_object);
        let has_clarification = root
            .get("clarifying_question")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        match (has_brief, has_clarification) {
            (true, false) => {
                root.insert("decision".to_string(), json!("proceed"));
            }
            (false, true) => {
                root.insert("decision".to_string(), json!("clarify"));
            }
            _ => {}
        }
    }

    let Some(brief) = root.get_mut("brief").and_then(Value::as_object_mut) else {
        return;
    };
    normalize_observed_brief_key_alias(brief, "stop_conditions");

    let Some(subquestions) = brief.get_mut("subquestions").and_then(Value::as_array_mut) else {
        return;
    };
    subquestions.retain(|value| !is_empty_optional_placeholder(value));
    for subquestion in subquestions {
        let Some(object) = subquestion.as_object_mut() else {
            continue;
        };
        for annotation in [
            "id_note",
            "optional",
            "priority_note",
            "initial_search_angles_note",
        ] {
            object.remove(annotation);
        }
    }
}

fn normalize_observed_brief_key_alias(brief: &mut Map<String, Value>, canonical: &str) {
    let aliases: Vec<String> = brief
        .keys()
        .filter(|key| {
            key.as_str() != canonical && key.trim().trim_end_matches(':').trim() == canonical
        })
        .cloned()
        .collect();
    for alias in aliases {
        if let Some(value) = brief.remove(&alias) {
            brief.entry(canonical.to_string()).or_insert(value);
        }
    }
}

fn is_empty_optional_placeholder(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    const PLACEHOLDER_FIELDS: [&str; 8] = [
        "id",
        "question",
        "priority",
        "evidence_requirements",
        "id_note",
        "optional",
        "priority_note",
        "initial_search_angles_note",
    ];
    let only_known_fields = object
        .keys()
        .all(|key| PLACEHOLDER_FIELDS.contains(&key.as_str()));
    let optional = object
        .get("priority")
        .and_then(Value::as_str)
        .is_some_and(|priority| priority.eq_ignore_ascii_case("optional"));
    let empty_question = object
        .get("question")
        .and_then(Value::as_str)
        .is_some_and(|question| question.trim().is_empty());
    let empty_evidence = object
        .get("evidence_requirements")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty);
    only_known_fields && optional && empty_question && empty_evidence
}

fn write_json_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let rendered = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, rendered)?;
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    std::fs::rename(temp, path)
}
