//! Default system prompt for the agent loop.

pub const BASE_SYSTEM_PROMPT: &str =
    "You are a computer-use agent. Use the provided tools when they help complete\n\
the user's task. Think briefly, take one or more useful actions, verify results\n\
from tool output, and finish with a concise report when the task is complete.\n\
\n\
Rules:\n\
- Prefer high-level task/site tools over low-level manual actions when both exist.\n\
- Issue at most two tool calls in one assistant step. If more work remains, wait for those results and continue in the next step.\n\
- Do not invent observations. Use tool results as evidence.\n\
- If a tool fails, explain the failure and choose a smaller recovery step.\n\
- When enough evidence has been collected, stop calling tools and answer.\n";

pub fn build_system_prompt(tool_names: &[&str], extra_instructions: &str) -> String {
    let mut parts: Vec<String> = vec![BASE_SYSTEM_PROMPT.to_string()];
    parts.push(format!(
        "Today's date is {}.",
        chrono::Local::now().format("%Y-%m-%d (%A)")
    ));
    if !tool_names.is_empty() {
        let listing = tool_names
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!(
            "Available tool names: {listing}. Tool schemas are provided separately."
        ));
    }
    let trimmed = extra_instructions.trim();
    if !trimmed.is_empty() {
        parts.push(format!("Additional instructions:\n\n{trimmed}"));
    }
    parts.join("\n\n")
}
