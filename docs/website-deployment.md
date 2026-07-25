# Website deployment

The socai marketing/download site lives in [`site/`](../site/) and is deployed to Vercel at [`https://socai.io`](https://socai.io).

The detailed deployment runbook is the shared project skill:

- [`.claude/skills/socai-site-deployment/SKILL.md`](../.claude/skills/socai-site-deployment/SKILL.md)

Claude Code can read that skill directly. Pi loads the same skill directory via [`.pi/settings.json`](../.pi/settings.json).

Use the skill for:

- Vercel project settings
- production deployment steps
- `www.socai.io` canonical redirect behavior
- `/download` and `/github` redirect verification
- Git preview setup
- troubleshooting deployment failures

Keeping the runbook in one place avoids drift between docs and agent instructions.

## Download analytics

The home-page macOS and Windows buttons send a Vercel Web Analytics custom event
named `download_click` before following the `/download` redirect. Its dimensions
are:

- `platform`: `macos` or `windows`
- `language`: `zh` or `en`

This event measures download intent. Actual binary delivery is measured
separately from Alibaba Cloud OSS real-time access logs:

- SLS endpoint: `cn-beijing.log.aliyuncs.com`
- Project: `oss-log-1006144703132379-cn-beijing`
- Logstore: `oss-log-store`
- Filters: topic `oss_access_log`, bucket `socai-download`

On the operator machine, use
`~/.config/socai/aliyun/sls-operator.csv` (RAM user `cym`) only for read-only
OSS/SLS analytics queries. The release publisher credential is
`~/.config/socai/aliyun/oss-release-publisher.csv` (RAM user `socai-oss`);
it can inspect and publish OSS release objects but cannot query SLS. Never
commit either credential or print its values.

OSS logging was enabled on 2026-07-25, so object-level results start from that
point. Exclude the `socai-analytics-probe/1.0` User-Agent used for ingestion
checks when reporting real traffic.
