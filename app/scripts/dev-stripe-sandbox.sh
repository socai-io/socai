#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/../.." && pwd)"
server_dir="${SOCAI_SERVER_DIR:-$(dirname "$repo_dir")/socai-server}"
sandbox_dir="$server_dir/tmp/stripe-desktop"
mkdir -p "$sandbox_dir"

# Optional private configuration, kept outside both repositories.
stripe_env_file="${SOCAI_STRIPE_ENV_FILE:-$HOME/.config/socai/stripe/sandbox.env}"
if [ -f "$stripe_env_file" ]; then
  set -a
  source "$stripe_env_file"
  set +a
fi

# Force isolated local state, regardless of the backend's normal .env defaults.
export APP_ENV=development
export DATABASE_URL="sqlite+aiosqlite:///$server_dir/tmp/stripe_sandbox.db"
export PUBLIC_BASE_URL=http://127.0.0.1:8010
export STRIPE_ENABLED=true STRIPE_SANDBOX=true
export WECHAT_PAY_ENABLED=false ALIPAY_ENABLED=false MOCK_RECHARGE_ENABLED=false
export TELEMETRY_ENDPOINT=""
export SOCAI_CLOUD_AUTH_FILE="$sandbox_dir/auth.json"
export SOCAI_HOME="$repo_dir/.socai/stripe-sandbox"
export SOCAI_RUNS_DIR="$SOCAI_HOME/runs"

# The backend also supports credentials in its ignored .env file.
for port in 8010 1421; do
  if lsof -tiTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "Port $port is already in use. Stop that development server before starting this sandbox."
    exit 1
  fi
done
cd "$server_dir"
backend_pid=""
frontend_pid=""
listener_pid=""
cleanup() {
  [ -z "$backend_pid" ] || kill "$backend_pid" 2>/dev/null || true
  [ -z "$frontend_pid" ] || kill "$frontend_pid" 2>/dev/null || true
  [ -z "$listener_pid" ] || kill "$listener_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
stripe_cli="${STRIPE_CLI_PATH:-$(command -v stripe || true)}"
if [ -z "$stripe_cli" ]; then
  stripe_cli="$HOME/.local/share/socai-stripe-tools/node_modules/.bin/stripe"
fi
if [ ! -x "$stripe_cli" ]; then
  echo "Install Stripe CLI or set STRIPE_CLI_PATH before starting the sandbox."
  exit 1
fi
webhook_env="$sandbox_dir/webhook.env"
rm -f "$webhook_env"
uv run python -m app.scripts.listen_stripe --cli "$stripe_cli" --secret-file "$webhook_env" &
listener_pid=$!
for attempt in $(seq 1 150); do
  [ ! -s "$webhook_env" ] || break
  if ! kill -0 "$listener_pid" 2>/dev/null; then
    echo "Stripe webhook listener failed to start."
    exit 1
  fi
  sleep 0.2
done
if [ ! -s "$webhook_env" ]; then
  echo "Stripe webhook listener did not become ready."
  exit 1
fi
source "$webhook_env"
uv run python -m app.scripts.setup_stripe
uv run python -m app.scripts.prepare_stripe_sandbox --auth-file "$SOCAI_CLOUD_AUTH_FILE" \
  --profile "${SOCAI_STRIPE_PROFILE:-default}"
uv run uvicorn app.main:app --host 127.0.0.1 --port 8010 &
backend_pid=$!
cd "$repo_dir/app"
env -u STRIPE_SECRET_KEY -u STRIPE_WEBHOOK_SECRET -u STRIPE_AGENT_KEY \
  node node_modules/vite/bin/vite.js --host 127.0.0.1 --port 1421 --strictPort &
frontend_pid=$!
# Build without the runtime backend override, so it is not embedded into builds.
cd "$repo_dir"
(env -u SOCAI_PRO_BASE_URL -u SOCAI_CLOUD_BASE_URL \
  -u STRIPE_SECRET_KEY -u STRIPE_WEBHOOK_SECRET -u STRIPE_AGENT_KEY cargo build -p socai_app)
desktop_executable="$repo_dir/target/debug/socai_app"
if [ "$(uname)" = Darwin ]; then
  # Give the isolated dev app its own macOS identity and a discoverable window.
  sandbox_bundle="$repo_dir/.socai/stripe-sandbox/socai sandbox.app"
  python3 - "$desktop_executable" "$sandbox_bundle" <<'PY'
import plistlib
import shutil
import sys
from pathlib import Path
source, bundle = map(Path, sys.argv[1:])
macos = bundle / "Contents/MacOS"
macos.mkdir(parents=True, exist_ok=True)
shutil.copy2(source, macos / "socai_app")
with (bundle / "Contents/Info.plist").open("wb") as f:
    plistlib.dump({"CFBundleExecutable": "socai_app", "CFBundleIdentifier": "com.socai.stripe-sandbox",
                  "CFBundleName": "socai sandbox", "CFBundlePackageType": "APPL",
                  "CFBundleVersion": "1", "NSHighResolutionCapable": True}, f)
PY
  desktop_executable="$sandbox_bundle/Contents/MacOS/socai_app"
fi
for attempt in $(seq 1 50); do
  if curl -fsS http://127.0.0.1:8010/healthz >/dev/null && curl -fsS http://127.0.0.1:1421/ >/dev/null; then
    env -u STRIPE_SECRET_KEY -u STRIPE_WEBHOOK_SECRET -u STRIPE_AGENT_KEY \
      SOCAI_PRO_BASE_URL=http://127.0.0.1:8010 "$desktop_executable"
    exit $?
  fi
  sleep 0.2
done
echo "Local sandbox services did not become ready."
exit 1
