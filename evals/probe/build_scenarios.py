#!/usr/bin/env python3
"""Build benchmark scenarios from checked-in cases + shared environment.

A checked-in case (cases/<name>.json) is intentionally minimal — just what is
specific to that situation:

  {
    "recorded_date": "2026-07-30",        # "today" for the frozen moment
    "task": "…user search prompt…",
    "search_reasoning": "…",              # the agent's one-line thought before searching
    "search_call": {"query": "…", …},     # the search tool args it issued
    "cassette": { …recorded search tool response… }
  }

Everything identical across cases lives once in shared/ (captured from a real
production request at import time, sanitized):

  shared/system_scaffold.txt   system prompt with __DATE__ / __TOOL_NAMES__ /
                               __KNOWLEDGE__ placeholders
  shared/tools.json            the current agent tool schemas
  shared/request_params.json   model + sampling params

At build time the scaffold's __KNOWLEDGE__ is filled from the repo's CURRENT
core/src/sites/xhs/knowledge.md — checked-in files never bake a prompt, or the
benchmark could not catch prompt regressions. Cassette dates are re-normalized
the way PR #243 emits them, so old recordings match today's data shape.

Contributing a case:
  1. Reproduce the situation in the app/CLI so a run dir exists under
     ~/.socai/runs/<run>/turn-*/llm/002.request.json.
  2. python3 build_scenarios.py --import <turn-dir> --name <case> [--date YYYY-MM-DD]
     (writes the sanitized cases/<case>.json and refreshes shared/ from the
     recording — home paths and xsec tokens are scrubbed)
  3. Add a SCENARIOS entry below with meta + an `expect` block.

The derived `bloc1_trap` scenario removes the fresh official 换线 notice from
the bloc1 cassette: the realistic failure world where search ranking sampled
only the stale official notice, so only the author_scan hop can find the truth.
"""

import copy
import json
import re
import sys
from datetime import date, timedelta
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
CASES = HERE / "cases"
SHARED = HERE / "shared"

KNOWLEDGE_START = "# Xiaohongshu Macro-Agent Knowledge"
CITING_START = "## Citing notes in the final answer"

# Patch rule for recordings captured before PR #243 (author_scan gained the
# 认证 clause). Applied at import only; shared/tools.json is stored current.
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


def patch_entity_dates(cassette: dict, anchor: date) -> None:
    for note in cassette.get("notes", []):
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


# ---------------------------------------------------------------- import ----

def sanitize_text(text: str) -> str:
    """Scrub machine-local and session-bound bytes before anything is checked
    in: the recording user's home directory and xsec page tokens. Neither
    influences the probed decision."""
    text = text.replace(str(Path.home()), "/Users/user")
    return re.sub(r"xsec_token=[A-Za-z0-9%_.=\-]+", "xsec_token=REDACTED", text)


def scaffold_from_system(system_text: str) -> str:
    """Turn a recorded system prompt into the shared scaffold: knowledge slice,
    date, and tool-name list become placeholders; the conversation dir becomes
    case-neutral."""
    start = system_text.index(KNOWLEDGE_START)
    tail_idx = system_text.find(CITING_START, start)
    tail = system_text[tail_idx:] if tail_idx != -1 else ""
    scaffold = system_text[:start] + "__KNOWLEDGE__" + ("\n\n" + tail if tail else "\n")
    scaffold = re.sub(
        r"Today's date is \d{4}-\d{2}-\d{2} \([A-Za-z]+\)\.",
        "Today's date is __DATE__.",
        scaffold,
    )
    scaffold = re.sub(
        r"Available tool names: [^\n]+",
        "Available tool names: __TOOL_NAMES__. Tool schemas are provided separately.",
        scaffold,
    )
    return re.sub(
        r"conversation dir: [^)]+", "conversation dir: /Users/user/.socai/runs/benchmark", scaffold
    )


def redact_token_fields(node):
    """Recursively redact xsec_token VALUES wherever they appear as fields —
    the URL-form regex in sanitize_text misses tokens serialized as tool-call
    arguments (e.g. get_notes args in multi-turn history)."""
    if isinstance(node, dict):
        return {
            k: ("REDACTED" if k == "xsec_token" and isinstance(v, str) else redact_token_fields(v))
            for k, v in node.items()
        }
    if isinstance(node, list):
        return [redact_token_fields(v) for v in node]
    return node


def import_recording(turn_dir: Path, name: str, recorded_date: str) -> None:
    raw = (turn_dir / "llm" / "002.request.json").read_text()
    request = json.loads(sanitize_text(raw))
    messages = request["messages"]
    roles = [m.get("role") for m in messages]
    if roles != ["system", "user", "assistant", "tool"]:
        sys.exit(
            f"importer expects a fresh single-turn run ([system, user, assistant, tool]), got {roles}. "
            "Record the case as a NEW conversation (first turn), not a follow-up — earlier-turn "
            "history would drag unrelated content into the checked-in cassette."
        )
    assistant = messages[2]
    search_tc = next(tc for tc in assistant["tool_calls"] if tc["function"]["name"] == "search")

    CASES.mkdir(parents=True, exist_ok=True)
    case = {
        "recorded_date": recorded_date,
        "task": messages[1]["content"],
        "search_reasoning": assistant.get("reasoning_content") or "",
        "search_call": json.loads(search_tc["function"]["arguments"]),
        "cassette": json.loads(messages[3]["content"]),
    }
    case_path = CASES / f"{name}.json"
    # Field-form tokens live inside the parsed cassette/args (the URL-form
    # regex can't see them), so redact on the final parsed structure.
    case_path.write_text(json.dumps(redact_token_fields(case), ensure_ascii=False, indent=1))

    # Refresh the shared environment from this recording (latest import wins —
    # keep shared/ in sync with the current agent by importing from a run made
    # with the current binary after tool/prompt-scaffold changes).
    SHARED.mkdir(parents=True, exist_ok=True)
    tools = request["tools"]
    for tool in tools:
        if tool["function"]["name"] == "author_scan":
            desc = tool["function"]["description"]
            if AUTHOR_SCAN_NEW in desc:
                continue  # recorded post-#243 — already current
            if AUTHOR_SCAN_OLD not in desc:
                sys.exit("author_scan description drifted; update AUTHOR_SCAN_OLD/NEW")
            tool["function"]["description"] = desc.replace(AUTHOR_SCAN_OLD, AUTHOR_SCAN_NEW)
    names = [t["function"]["name"] for t in tools]
    if "wait_for_rate_limit" not in names:
        tools.insert(names.index("wait_for_login") + 1, WAIT_FOR_RATE_LIMIT_TOOL)
    (SHARED / "tools.json").write_text(json.dumps(tools, ensure_ascii=False, indent=1))
    (SHARED / "system_scaffold.txt").write_text(scaffold_from_system(messages[0]["content"]))
    params = {k: v for k, v in request.items() if k not in ("messages", "tools")}
    (SHARED / "request_params.json").write_text(json.dumps(params, ensure_ascii=False, indent=1))
    print(f"imported case -> {case_path.relative_to(REPO)} (recorded {recorded_date}); shared/ refreshed")


# ----------------------------------------------------------------- build ----

def assemble_request(case: dict, cassette: dict) -> dict:
    scaffold = (SHARED / "system_scaffold.txt").read_text()
    tools = json.loads((SHARED / "tools.json").read_text())
    params = json.loads((SHARED / "request_params.json").read_text())
    anchor = date.fromisoformat(case["recorded_date"])
    knowledge = (REPO / "core/src/sites/xhs/knowledge.md").read_text().strip()
    system = (
        scaffold
        .replace("__DATE__", anchor.strftime("%Y-%m-%d (%A)"))
        .replace("__TOOL_NAMES__", ", ".join(f"`{t['function']['name']}`" for t in tools))
        .replace("__KNOWLEDGE__", knowledge)
    )
    call_id = "call_benchmark_search"
    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": case["task"]},
        {
            "role": "assistant",
            "content": None,
            "reasoning_content": case["search_reasoning"],
            "tool_calls": [{
                "id": call_id,
                "type": "function",
                "function": {
                    "name": "search",
                    "arguments": json.dumps(case["search_call"], ensure_ascii=False),
                },
            }],
        },
        {
            "role": "tool",
            "tool_call_id": call_id,
            "content": json.dumps(cassette, ensure_ascii=False),
        },
    ]
    return {**params, "messages": messages, "tools": tools}


def load_case(name: str) -> tuple[dict, dict]:
    case = json.loads((CASES / f"{name}.json").read_text())
    cassette = copy.deepcopy(case["cassette"])
    patch_entity_dates(cassette, date.fromisoformat(case["recorded_date"]))
    return case, cassette


def drop_note(cassette: dict, note_id_prefix: str) -> None:
    notes = cassette["notes"]
    kept = [n for n in notes if not n["entity"]["note_id"].startswith(note_id_prefix)]
    if len(kept) != len(notes) - 1:
        sys.exit(f"expected to drop exactly one note with prefix {note_id_prefix}")
    cassette["notes"] = kept


def main() -> None:
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--import", dest="import_dir", metavar="TURN_DIR",
                    help="Import a run's llm/002.request.json as a checked-in case")
    ap.add_argument("--name", help="Case name for --import")
    ap.add_argument("--date", dest="recorded_date",
                    help="Recording date YYYY-MM-DD (default: parsed from the run dir name)")
    args = ap.parse_args()
    if args.import_dir:
        if not args.name:
            sys.exit("--import requires --name")
        turn_dir = Path(args.import_dir).expanduser()
        recorded = args.recorded_date
        if not recorded:
            m = re.search(r"(20\d{6})_", turn_dir.parent.name + "_")
            if not m:
                sys.exit("cannot parse date from run dir name; pass --date YYYY-MM-DD")
            recorded = f"{m.group(1)[:4]}-{m.group(1)[4:6]}-{m.group(1)[6:8]}"
        import_recording(turn_dir, args.name, recorded)
        return

    out = HERE / "scenarios"
    out.mkdir(parents=True, exist_ok=True)

    bloc1_case, bloc1_cassette = load_case("bloc1")
    trap_cassette = copy.deepcopy(bloc1_cassette)
    drop_note(trap_cassette, "6a65d768")
    col_case, col_cassette = load_case("col")

    bloc1_meta = {
        "task": bloc1_case["task"],
        "official_author_id": "681c1223000000000e0126f9",
        "official_author_name": "Bloc1 Climbing",
        "query_terms": ["bloc1", "bloc 1"],
        "anchor_date": bloc1_case["recorded_date"],
        # 换线 window 7-27..7-30; the fresh official notice is dated 07-26.
        "fresh_patterns": [r"7\s*月\s*2[6-9]", r"07-2[6-9]", r"7\s*月\s*3[01]", r"07-3[01]"],
        "stale_patterns": [r"1\s*月\s*28", r"01-28", r"一月"],
    }
    scenarios = {
        "bloc1_replay": {
            "request": assemble_request(bloc1_case, bloc1_cassette),
            "meta": {
                **bloc1_meta,
                "note": "Cassette as recorded: stale official notice ranked #1, fresh one ranked #5.",
                # Fresh official notice is in the cassette: answering from it
                # directly and verifying via the profile are both correct.
                "expect": {"pass": ["author_scan_official", "direct_fresh"], "min_pass_rate": 0.75},
            },
        },
        "bloc1_trap": {
            "request": assemble_request(bloc1_case, trap_cassette),
            "meta": {
                **bloc1_meta,
                "note": "Fresh official notice (6a65d768…, 07-26) removed: only the profile check can find the truth.",
                "expect": {"pass": ["author_scan_official"], "min_pass_rate": 0.75},
            },
        },
        "col_replay": {
            "request": assemble_request(col_case, col_cassette),
            "meta": {
                "task": col_case["task"],
                "official_author_id": "60ce095a00000000010077b5",
                "official_author_name": "Climb On Gym攀岩",
                "query_terms": ["col", "climb on"],
                "anchor_date": col_case["recorded_date"],
                "fresh_patterns": [r"7\s*月\s*2[7-9]", r"07-2[7-9]", r"7\s*月\s*3[01]", r"07-3[01]"],
                "stale_patterns": [r"2025", r"6\s*月\s*2[23]", r"06-2[23]"],
                "note": "Cassette as recorded: fresh official notice ranked #1 (fresh-official exception applies).",
                # Direct answer expected; a correct profile check is not wrong,
                # just unnecessary latency.
                "expect": {"pass": ["direct_fresh", "author_scan_official"], "min_pass_rate": 0.75},
            },
        },
    }

    for name, payload in scenarios.items():
        path = out / f"{name}.json"
        path.write_text(json.dumps(payload, ensure_ascii=False, indent=1))
        cassette = json.loads(payload["request"]["messages"][3]["content"])
        print(
            f"{name}: {len(cassette['notes'])} notes, "
            f"system {len(payload['request']['messages'][0]['content'])} chars, "
            f"{len(payload['request']['tools'])} tools -> {path.relative_to(REPO)}"
        )


if __name__ == "__main__":
    main()
