# Connection troubleshooting

Inspect the affected machine, not a different healthy installation. Resolve the executable with `command -v socai` (`Get-Command socai` on Windows), then run `socai version --no-check --json` and `socai status --json`. Do not automatically update or reinstall.

Under the current `SOCAI_HOME` (default `~/.socai`), inspect `rust-daemon.pid` and recent `rust-daemon.log` records, including modification times. On macOS/Linux verify the PID and the process listening on `rust-daemon.sock`. On Windows inspect the port in `rust-daemon-endpoint.json` and its listener without exposing its token. A marker file is not proof of a live service; old logs are historical evidence only.

For local Chrome, check the selected user-data directory's `DevToolsActivePort` first-line port and its listener without exposing the WebSocket path. Some Chrome builds return 404 from `/json/version` while the browser WebSocket works. Identify an existing CDP owner before creating another connection; do not stop unknown services or bypass Chrome's user-approved debugging prompt.

A timeout does not prove the command stopped or had no effect. Inspect existing work and saved output before retrying. Preserve Chrome, tabs, profile and foreground state. If a verified idle task-owned daemon must be stopped, or the browser profile must change, explain the impact and obtain user confirmation first. After a verified correction, retry only one read-only query.

Report the observed failure stage and missing evidence. Do not expose credentials, original user text or token-bearing URLs, and do not claim restored access until a real query returns successfully.
