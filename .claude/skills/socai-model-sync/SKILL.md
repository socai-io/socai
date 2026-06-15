---
name: socai-model-sync
description: Refresh, inspect, and validate socai's generated desktop/CLI LLM model catalog. Use when adding or updating model versions such as Qwen 3.6/3.7, aligning with pi's model list, checking official provider model APIs, or troubleshooting the desktop model picker.
---

# socai model sync

Use this skill when the task involves the selectable LLM model list for the
socai desktop app or TUI. The shipped app must not depend on runtime model
network calls; model discovery is a maintainer/developer-time operation that
updates a generated catalog committed to the repo.

## Source of truth and priorities

Generated file:

```text
core/src/agent/model_catalog.generated.json
```

Runtime readers:

- `core/src/agent/provider.rs` — provider config, model catalog loading, persisted defaults.
- `app/src-tauri/src/commands.rs` — `agent_list_models` returns model-version rows to the desktop UI.
- `app/src/panels/tasks.ts` / `app/src/panels/task_new.ts` — desktop model picker and task summary.
- `cli/src/tui.rs` — `/model` picker.

Prefer model sources in this order:

1. **Official provider APIs** when an API key is available and the endpoint returns
   relevant chat/agent models.
2. **pi's generated catalog** via `@earendil-works/pi-ai`, when available.
3. **socai fallback entries** in `scripts/sync-model-catalog.mjs`, for defaults
   and providers whose APIs/catalogs do not map cleanly.

Do not blindly expose every pi model. socai currently implements only:

- `anthropic`
- `openai`
- `kimi` / Moonshot
- `qwen` / DashScope
- `deepseek`

Qwen/DashScope deserves extra judgment: pi often lists Qwen through OpenRouter,
Together, Fireworks, or opencode providers. Normalize only model IDs that are
valid for DashScope-compatible calls, such as `qwen3.7-plus`, `qwen3.7-max`, and
`qwen3.6-plus`. If uncertain, keep the known fallback and note the uncertainty.

## Refresh commands

Offline/pi-aligned refresh, deterministic when no official keys are desired:

```bash
node scripts/sync-model-catalog.mjs --no-official --write
```

Official API refresh. Set any available keys; missing providers fall back to pi
or local fallback entries:

```bash
ANTHROPIC_API_KEY=... \
OPENAI_API_KEY=... \
KIMI_API_KEY=... \
QWEN_API_KEY=... \
DEEPSEEK_API_KEY=... \
node scripts/sync-model-catalog.mjs --write
```

From the app package, the shortcut is:

```bash
pnpm --dir app sync-models
```

For a read-only freshness check:

```bash
node scripts/sync-model-catalog.mjs --check
```

Use `SOCAI_MODEL_SYNC_OFFLINE=1` or `--no-official` to avoid provider network
calls. Use `--no-pi` to test the built-in fallback path.

## Official endpoint notes

The sync script knows these endpoints:

```text
Anthropic: GET https://api.anthropic.com/v1/models
OpenAI:    GET https://api.openai.com/v1/models
Kimi:      GET https://api.moonshot.cn/v1/models
Qwen:      GET https://dashscope.aliyuncs.com/compatible-mode/v1/models
DeepSeek:  GET https://api.deepseek.com/v1/models
```

Provider model APIs frequently include non-agent models. Filter out embeddings,
image/audio/realtime/moderation models, and anything that socai's current LLM
backends cannot call. If an official API returns a suspicious list, prefer a
smaller safe catalog over showing broken options.

## Validation after a refresh

Run these from the repo root unless noted:

```bash
cargo test
pnpm --dir app build
```

For quick UI-level sanity, inspect the generated catalog and ensure every
provider still has its compiled default model visible:

```bash
python3 - <<'PY'
import json
from pathlib import Path
catalog = json.loads(Path('core/src/agent/model_catalog.generated.json').read_text())
for provider, models in catalog['providers'].items():
    print(provider, len(models), models[0]['id'] if models else 'EMPTY')
PY
```

Expected UI behavior:

- The desktop model menu groups rows by provider.
- Each row shows a human name plus the concrete API model id.
- API-key entry remains provider-level, not model-level.
- Selecting a model persists `defaults.provider` and `defaults.{provider}_model`
  in `~/.socai/auth.json`.
- Starting an agent task sends the concrete model id to the Rust backend.

## When editing manually

Manual edits are allowed for urgent fallbacks, but keep them conservative:

- Keep the JSON deterministic and sorted enough for review.
- Preserve the provider's compiled default model in its list.
- Mark exactly one practical default per provider with `"recommended": true`.
- Use real provider API model IDs, not display labels.
- Do not add unsupported providers here without implementing their backend and
  credential handling in `socai-core`.

## Reporting back

Include:

- Which sources were used: official API, pi catalog, fallback.
- Providers/model counts changed.
- Notable additions/removals, especially Qwen/ChatGPT/Anthropic versions.
- Validation commands run and their results.
- Any uncertainty about official availability or provider compatibility.
