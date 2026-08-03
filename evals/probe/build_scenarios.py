#!/usr/bin/env python3
"""Build decision-probe scenarios from recorded socai runs.

Reads the raw step-2 LLM request (`llm/002.request.json`) of a recorded app-agent
run — the exact request the model saw right after the `search` tool result — and
emits probe scenarios under scenarios/:

- The system prompt's knowledge slice is re-spliced from the repo's CURRENT
  core/src/sites/xhs/knowledge.md, and the recorded tools array is patched for
  the known post-recording changes (author_scan's 认证 clause from PR #243,
  wait_for_rate_limit from PR #241). Other tool descriptions are used exactly
  as recorded — if a later PR edits one, add a patch rule here (the
  AUTHOR_SCAN_OLD assert below is the pattern), or re-record the run.
- Note/comment dates in the tool result are re-normalized the way PR #243 now
  emits them (编辑于 prefix and territory tails stripped, relatives resolved
  against the run date, `date_edited` flag added), so fixtures match the data
  shape agents see today.
- `bloc1_trap` additionally removes the fresh official 换线 notice from the
  result, simulating the realistic failure world where search ranking sampled
  only the stale official notice. Correct behavior then requires the
  verification hop: author_scan of the official account.

Scenarios embed no secrets; they do embed public XHS note content and xsec URL
tokens from the local run dirs, so scenarios/ stays untracked (see ../.gitignore).
"""

import copy
import json
import re
import sys
from datetime import date, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
RUNS = Path.home() / ".socai" / "runs"

BLOC1_RUN = (
    RUNS
    / "20260730_104110_xhs_bloc1_攀岩馆上一次换线是什么时候"
    / "turn-01_bloc1_攀岩馆上一次换线是什么时候"
)
COL_RUN = (
    RUNS
    / "20260730_104536_xhs_COL_攀岩馆最近一次换线是什么时候"
    / "turn-01_COL_攀岩馆最近一次换线是什么时候"
)

KNOWLEDGE_START = "# Xiaohongshu Macro-Agent Knowledge"
CITING_START = "## Citing notes in the final answer"

# PR #243's addition to the author_scan tool description.
AUTHOR_SCAN_OLD = "read the author header (display name, xhs id, bio, IP location, follower/following/liked-&-collected counts)"
AUTHOR_SCAN_NEW = "read the author header (display name, xhs id, bio, IP location, official-verification/认证 status when present, follower/following/liked-&-collected counts)"

WAIT_FOR_RATE_LIMIT_TOOL = {
    "type": "function",
    "function": {
        "name": "wait_for_rate_limit",
        "description": (
            "Recover from Xiaohongshu rate limiting: after a tool returns "
            "`reason:rate_limited`, call this instead of immediately retrying. It "
            "visibly waits for a random 5-6 minute cooldown, then tells you to "
            "retry the original tool once. If that retry is still rate-limited, "
            "call this tool again."
        ),
        "parameters": {"type": "object", "properties": {}, "additionalProperties": False},
    },
}


def normalize_date(raw: str, anchor: date):
    """Python port of page_scripts.js normalizeXhsDate() as of PR #243.

    Returns (normalized, edited). `edited` mirrors isEditedDate().
    """
    t = " ".join(str(raw or "").split())
    if not t:
        return t, False
    edited = t.startswith("编辑于")
    t = re.sub(r"^编辑于\s*", "", t)
    toks = t.split(" ")
    if len(toks) > 1:
        tail = toks[-1]
        if re.fullmatch(r"\D{1,10}", tail) and not re.search(r"[前刚今昨于:]", tail):
            t = " ".join(toks[:-1])

    def fmt(d: date) -> str:
        if d.year == anchor.year:
            return f"{d.month:02d}-{d.day:02d}"
        return f"{d.year}-{d.month:02d}-{d.day:02d}"

    m = re.search(r"\d{4}-\d{1,2}-\d{1,2}|\d{1,2}-\d{1,2}", t)
    if m:
        return m.group(0), edited
    if re.search(r"刚刚|今天", t) or re.match(r"^\d+\s*(?:秒|分钟|小时)前", t):
        return fmt(anchor), edited
    if "昨天" in t:
        return fmt(anchor - timedelta(days=1)), edited
    if "前天" in t:
        return fmt(anchor - timedelta(days=2)), edited
    dm = re.match(r"^(\d+)\s*天前", t)
    if dm:
        return fmt(anchor - timedelta(days=int(dm.group(1)))), edited
    return t, edited


def patch_entity_dates(tool_result: dict, anchor: date) -> None:
    for note in tool_result.get("notes", []):
        entity = note.get("entity")
        if not isinstance(entity, dict):
            continue
        normalized, edited = normalize_date(entity.get("date", ""), anchor)
        rebuilt = {}
        for key, value in entity.items():
            if key == "date":
                rebuilt["date"] = normalized
                if edited:
                    rebuilt["date_edited"] = True
            else:
                rebuilt[key] = value
        entity.clear()
        entity.update(rebuilt)
        for comment in entity.get("top_comments") or []:
            if isinstance(comment, dict) and "time" in comment:
                comment["time"], _ = normalize_date(comment.get("time", ""), anchor)


def splice_current_prompt(system_text: str) -> str:
    """Replace the recorded knowledge slice with the repo's current knowledge.md
    and refresh the advertised tool-name list."""
    current = (REPO / "core/src/sites/xhs/knowledge.md").read_text().strip()
    start = system_text.index(KNOWLEDGE_START)
    tail_idx = system_text.find(CITING_START, start)
    tail = system_text[tail_idx:] if tail_idx != -1 else ""
    spliced = system_text[:start] + current + ("\n\n" + tail if tail else "\n")
    spliced = spliced.replace(
        "Available tool names: `get_notes`, `search`, `author_scan`, `wait_for_login`, `read_file`, `bash`.",
        "Available tool names: `get_notes`, `search`, `author_scan`, `wait_for_login`, `wait_for_rate_limit`, `read_file`, `bash`.",
    )
    return spliced


def current_prod_request(run_dir: Path, anchor: date) -> dict:
    request = json.loads((run_dir / "llm" / "002.request.json").read_text())
    request["messages"][0]["content"] = splice_current_prompt(request["messages"][0]["content"])

    tools = request["tools"]
    names = [t["function"]["name"] for t in tools]
    for tool in tools:
        if tool["function"]["name"] == "author_scan":
            desc = tool["function"]["description"]
            if AUTHOR_SCAN_OLD not in desc:
                sys.exit("author_scan description drifted; update AUTHOR_SCAN_OLD")
            tool["function"]["description"] = desc.replace(AUTHOR_SCAN_OLD, AUTHOR_SCAN_NEW)
    if "wait_for_rate_limit" not in names:
        # Keep tool order aligned with xhs_macro_tools_with_llm_provider: the
        # rate-limit tool sits right after wait_for_login.
        insert_at = names.index("wait_for_login") + 1
        tools.insert(insert_at, WAIT_FOR_RATE_LIMIT_TOOL)

    tool_result = json.loads(request["messages"][3]["content"])
    patch_entity_dates(tool_result, anchor)
    request["messages"][3]["content"] = json.dumps(tool_result, ensure_ascii=False)
    return request


def drop_note(request: dict, note_id_prefix: str) -> None:
    tool_result = json.loads(request["messages"][3]["content"])
    notes = tool_result["notes"]
    kept = [n for n in notes if not n["entity"]["note_id"].startswith(note_id_prefix)]
    if len(kept) != len(notes) - 1:
        sys.exit(f"expected to drop exactly one note with prefix {note_id_prefix}")
    tool_result["notes"] = kept
    request["messages"][3]["content"] = json.dumps(tool_result, ensure_ascii=False)


def main() -> None:
    anchor = date(2026, 7, 30)  # both runs were recorded on 2026-07-30
    out = HERE / "scenarios"
    out.mkdir(parents=True, exist_ok=True)

    bloc1 = current_prod_request(BLOC1_RUN, anchor)
    scenarios = {
        "bloc1_replay": {
            "request": bloc1,
            "meta": {
                "task": "bloc1 攀岩馆上一次换线是什么时候",
                "official_author_id": "681c1223000000000e0126f9",
                "official_author_name": "Bloc1 Climbing",
                "query_terms": ["bloc1", "bloc 1"],
                "anchor_date": anchor.isoformat(),
                # 换线 window 7-27..7-30; the fresh official notice is dated 07-26.
                "fresh_patterns": [r"7\s*月\s*2[6-9]", r"07-2[6-9]", r"7\s*月\s*3[01]", r"07-3[01]"],
                "stale_patterns": [r"1\s*月\s*28", r"01-28", r"一月"],
                "note": "Search result as recorded: stale official notice ranked #1, fresh one ranked #5.",
            },
        },
        "bloc1_trap": {
            "request": copy.deepcopy(bloc1),
            "meta": {
                "task": "bloc1 攀岩馆上一次换线是什么时候",
                "official_author_id": "681c1223000000000e0126f9",
                "official_author_name": "Bloc1 Climbing",
                "query_terms": ["bloc1", "bloc 1"],
                "anchor_date": anchor.isoformat(),
                "fresh_patterns": [r"7\s*月\s*2[6-9]", r"07-2[6-9]", r"7\s*月\s*3[01]", r"07-3[01]"],
                "stale_patterns": [r"1\s*月\s*28", r"01-28", r"一月"],
                "note": "Fresh official notice (6a65d768…, 07-26) removed: the world where search sampled only the stale official notice. A verification hop is required.",
            },
        },
        "col_replay": {
            "request": current_prod_request(COL_RUN, anchor),
            "meta": {
                "task": "COL 攀岩馆最近一次换线是什么时候？",
                "official_author_id": "60ce095a00000000010077b5",
                "official_author_name": "Climb On Gym攀岩",
                "query_terms": ["col", "climb on"],
                "anchor_date": anchor.isoformat(),
                "fresh_patterns": [r"7\s*月\s*2[7-9]", r"07-2[7-9]", r"7\s*月\s*3[01]", r"07-3[01]"],
                "stale_patterns": [r"2025", r"6\s*月\s*2[23]", r"06-2[23]"],
                "note": "Recorded lucky case: fresh official notice ranked #1.",
            },
        },
    }
    drop_note(scenarios["bloc1_trap"]["request"], "6a65d768")

    for name, payload in scenarios.items():
        path = out / f"{name}.json"
        path.write_text(json.dumps(payload, ensure_ascii=False, indent=1))
        request = payload["request"]
        tool_result = json.loads(request["messages"][3]["content"])
        print(
            f"{name}: {len(tool_result['notes'])} notes, "
            f"system {len(request['messages'][0]['content'])} chars, "
            f"{len(request['tools'])} tools -> {path.relative_to(REPO)}"
        )


if __name__ == "__main__":
    main()
