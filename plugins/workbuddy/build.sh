#!/usr/bin/env bash
# 打包 plugins/workbuddy 下的技能与专家为上架用 zip。
#
#   ./build.sh              # 打包全部
#   ./build.sh skill        # 只打技能
#   ./build.sh expert       # 只打专家
#
# 专家包依赖技能包：专家 zip 会包含一份技能副本（XHS_SKILL）。
# 技能源码的唯一位置是 plugins/workbuddy/xiaohongshu-socai，
# 专家目录下的 skills/ 是构建产物，不要手工编辑。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$ROOT/dist"
SKILL_SRC="$ROOT/xiaohongshu-socai"
EXPERT_SRC="$ROOT/xiaohongshu-research-expert"

SKILL_NAME="xiaohongshu-socai"
EXPERT_NAME="xiaohongshu-research-expert"

TARGET="${1:-all}"

die() { echo "error: $*" >&2; exit 1; }

require_dir() { [ -d "$1" ] || die "缺少目录：$1"; }

zip_dir() {
  local src="$1" out="$2"
  rm -f "$out"
  ( cd "$(dirname "$src")" && zip -rq "$out" "$(basename "$src")" \
      -x "*.DS_Store" "*__pycache__*" "*.pyc" ".git/*" )
  printf '  %-38s %s\n' "$(basename "$out")" "$(du -h "$out" | cut -f1)"
}

check_frontmatter() {
  local file="$1"
  python3 - "$file" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path, encoding="utf-8").read()
m = re.match(r"^---\n(.*?)\n---\n", text, re.S)
if not m:
    sys.exit(f"{path}: 缺少 YAML frontmatter")
fm = m.group(1)
required = ["description", "description_zh", "description_en", "version", "author"]
missing = [k for k in required if not re.search(rf"^{k}:", fm, re.M)]
if missing:
    sys.exit(f"{path}: frontmatter 缺字段 {', '.join(missing)}")
print(f"  frontmatter ok  {path.rsplit('/', 1)[-1]}")
PY
}

check_plugin_json() {
  python3 - "$1" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path, encoding="utf-8"))
required = [
    "name", "expertType", "version", "description", "author", "agents",
    "agentName", "displayName", "profession", "displayDescription",
    "avatar", "categoryId", "defaultInitPrompt", "plugin", "tags", "quickPrompts",
]
missing = [k for k in required if k not in data]
if missing:
    sys.exit(f"{path}: 缺必填字段 {', '.join(missing)}")
for field in ("displayName", "profession", "displayDescription", "defaultInitPrompt"):
    val = data[field]
    if not isinstance(val, dict) or not {"zh", "en"} <= set(val):
        sys.exit(f"{path}: {field} 必须同时含 zh 与 en")
zh_len = len(data["displayDescription"]["zh"])
if not 40 <= zh_len <= 50:
    sys.exit(f"{path}: displayDescription.zh 为 {zh_len} 字，规范要求 40-50 字")
if len(data["tags"]) != 3:
    sys.exit(f"{path}: tags 必须正好 3 个")
if len(data["quickPrompts"]) != 3:
    sys.exit(f"{path}: quickPrompts 必须正好 3 个")
if data["defaultInitPrompt"] != data["quickPrompts"][0]:
    sys.exit(f"{path}: defaultInitPrompt 必须与 quickPrompts 第一条一致")
if data["plugin"] != data["name"]:
    sys.exit(f"{path}: plugin 必须等于 name")
avatar = path.rsplit("/", 2)[0] + "/" + data["avatar"]
import os
if not os.path.exists(avatar):
    sys.exit(f"{path}: avatar 不存在 {data['avatar']}")
size = os.path.getsize(avatar)
if size > 512_000:
    sys.exit(f"{path}: 头像 {size} 字节，超过 500KB")
print(f"  plugin.json ok  category={data['categoryId']}  avatar={size // 1024}KB")
PY
}

build_skill() {
  require_dir "$SKILL_SRC"
  echo "技能 $SKILL_NAME"
  check_frontmatter "$SKILL_SRC/SKILL.md"
  zip_dir "$SKILL_SRC" "$DIST/$SKILL_NAME.zip"
}

build_expert() {
  require_dir "$EXPERT_SRC"
  require_dir "$SKILL_SRC"
  echo "专家 $EXPERT_NAME"

  check_plugin_json "$EXPERT_SRC/.codebuddy-plugin/plugin.json"

  # 组装暂存目录：专家源码 + 内联技能副本
  local stage="$DIST/.stage/$EXPERT_NAME"
  rm -rf "$DIST/.stage"
  mkdir -p "$stage/skills"
  ( cd "$EXPERT_SRC" && tar cf - --exclude=".DS_Store" --exclude="skills" . ) \
    | ( cd "$stage" && tar xf - )
  cp -R "$SKILL_SRC" "$stage/skills/$SKILL_NAME"

  zip_dir "$stage" "$DIST/$EXPERT_NAME.zip"
  rm -rf "$DIST/.stage"
}

mkdir -p "$DIST"

case "$TARGET" in
  skill)  build_skill ;;
  expert) build_expert ;;
  all)    build_skill; build_expert ;;
  *)      die "未知参数：$TARGET（可用：skill / expert / all）" ;;
esac

echo "完成 → $DIST"
