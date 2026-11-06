//! A run-bound human handoff. This tool never solves a challenge or claims
//! authentication; the original read tool must verify the page after resume.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use socai_core::agent::{Tool, ToolContext, ToolResult};
use socai_core::sites::login_wait::login_resume_receiver;
use socai_core::util::execution_clock::ExecutionClock;

pub(crate) struct WaitForBrowserAction;

struct HumanWaitClock(ExecutionClock);

impl Drop for HumanWaitClock {
    fn drop(&mut self) {
        self.0.resume();
    }
}

#[async_trait]
impl Tool for WaitForBrowserAction {
    fn name(&self) -> &str {
        "wait_for_browser_action"
    }

    fn description(&self) -> &str {
        "Pause browser work and ask the user to handle login, a captcha/security check, SMS/email two-factor verification, authorization confirmation, or a stuck page in this task's browser. Never navigate or solve challenges here. After the user clicks Continue, inspect the live page and recheck the blocked read operation. User confirmation is NOT proof of login or permission to perform an external write."
    }

    fn input_schema(&self) -> Value {
        json!({"type":"object", "properties":{
            "reason":{"type":"string", "enum":["login","captcha","two_factor","authorization","page_stuck","other"]}
        }, "required":["reason"], "additionalProperties":false})
    }

    fn always_available(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolResult> {
        let reason = input.get("reason").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            reason,
            "login" | "captcha" | "two_factor" | "authorization" | "page_stuck" | "other"
        ) {
            return Ok(ToolResult::failure(
                "Choose a supported human-action reason.",
            ));
        }
        // Human wait must not consume research time even before the user
        // opens the panel. Its own wall timeout is independent of that clock.
        let clock = ExecutionClock::current();
        clock.pause();
        let _human_clock = HumanWaitClock(clock);
        let timeout = tokio::time::sleep(Duration::from_secs(30 * 60));
        tokio::pin!(timeout);
        let mut ready = login_resume_receiver(&ctx.run_id);
        loop {
            if ready.was_signaled() {
                return Ok(ToolResult::text(json!({
                    "user_ready":true,
                    "verified":false,
                    "reason":reason,
                    "message":"The user finished interacting. Inspect the live page and retry the blocked read operation once. If the blocker remains, request human help again. Do not assume authentication, invent results, repeat external writes, or bypass verification."
                }).to_string()));
            }
            tokio::select! {
                _ = &mut timeout => {
                    return Ok(ToolResult::failure(
                        "Human interaction timed out. Preserve collected results and explain the blocker; do not repeat this wait automatically."
                    ));
                }
                changed = ready.changed() => {
                    if changed.is_err() {
                        return Ok(ToolResult::failure("Human handoff ended without confirmation."));
                    }
                }
            }
        }
    }
}
