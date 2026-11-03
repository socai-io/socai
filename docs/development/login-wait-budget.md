# Login wait budget

The Xiaohongshu login-recovery tool has one blocking wait per agent execution,
shared across cloned `ToolContext`s. Changing `timeout_seconds` does not reserve
another wait. The default and maximum are 180 seconds. This policy is in the
shared core, so native Desktop and hosted workers use the same limit.

The tool checks the actual browser before reserving a wait. An already signed-in
browser can proceed immediately, including after a previous wait timed out.
Manual resume signals accelerate live verification; they do not authorize
claiming login without checking the page. A new user-confirmed follow-up creates
a fresh context and budget. Tool arguments cannot reset the budget.

On timeout, the tool returns `ok:false`, `reason:login_required`,
`requires_human_action:true`, and `retry_allowed:false`. A repeated call in the
same execution additionally returns `login_wait_already_attempted:true` without
another blocking wait. The model-facing guidance must not tell the agent to
repeat this wait, retry acquisition, or increase the timeout.

This bounds idle waiting. It does not by itself implement a durable paused
execution, retain an expired cloud browser, complete a QR challenge, or prove
successful research. Hosts must distinguish execution termination from useful
delivery and provide their normal login/recovery flow. Never copy another
browser's credentials or bypass platform authentication.

Validation for this change: shared core/CLI/worker compile checks and formatting
of changed Rust files. Native and hosted real-user login/timeout/resume workflows
must still be checked before claiming end-to-end completion. Repository-wide
formatting differences outside this change must not be rewritten incidentally.
