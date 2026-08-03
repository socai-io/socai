#!/usr/bin/env python3
"""Decision-probe runner: does the agent perform a profile check?

Replays a recorded step-2 request (see build_scenarios.py) against the real
model N times per (prompt-variant × scenario) cell and classifies the model's
next action. Measures P(verification hop) — the author_scan / recency-search
behavior the "official sources" knowledge.md policy is meant to induce.

Usage:
  python3 run_probe.py --scenarios bloc1_replay,bloc1_trap --variants v0 -n 8
  python3 run_probe.py --dry-run          # build requests, print cells, no API calls

`v1` applies only to fixtures generated before the official-sources policy
landed in knowledge.md; on current fixtures it exits via the drift guard
(see evals/README.md).

The Qwen API key is read from ~/.socai/auth.json (qwen.api_key) and never
printed. Raw responses land in results/<stamp>/ for audit.
"""

import argparse
import concurrent.futures as cf
import json
import re
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

import urllib.request
import urllib.error

HERE = Path(__file__).resolve().parent
BASE_URL = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"

# The paragraph V1 replaces — current knowledge.md's author_scan guidance
# ("skip this extra step for routine searches"). Matched exactly so drift
# fails loudly instead of silently testing the wrong baseline.
OLD_PARAGRAPH = """For deep, complex topic research only, consider `author_scan` when a search
finds a high-quality, suspicious, or representative note. A quick profile check
can distinguish firsthand expertise from soft ads/content farms, uncover related
notes, reveal the author's recurring style and so on. Skip this extra step for routine
searches."""


def load_api_key() -> str:
    auth = json.loads((Path.home() / ".socai" / "auth.json").read_text())
    key = (auth.get("qwen") or {}).get("api_key", "")
    if not key:
        sys.exit("no qwen api key in ~/.socai/auth.json")
    return key


def apply_variant(system_text: str, variant: str) -> str:
    if variant == "v0":
        return system_text
    if variant == "v1":
        if OLD_PARAGRAPH not in system_text:
            sys.exit("v1: baseline author_scan paragraph not found in system prompt (knowledge.md drifted?)")
        replacement = (HERE / "variants" / "v1_official_sources.md").read_text().strip()
        return system_text.replace(OLD_PARAGRAPH, replacement)
    sys.exit(f"unknown variant {variant}")


def stream_completion(request_body: dict, api_key: str, timeout: int = 300) -> dict:
    """POST with stream=true, accumulate deltas -> {content, reasoning, tool_calls, usage}."""
    body = dict(request_body)
    body["stream"] = True
    body["stream_options"] = {"include_usage": True}
    data = json.dumps(body, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(
        BASE_URL,
        data=data,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
    )
    content, reasoning = [], []
    tool_calls: dict[int, dict] = {}
    usage, finish = None, None
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        for raw_line in resp:
            line = raw_line.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            payload = line[len("data:"):].strip()
            if payload == "[DONE]":
                break
            chunk = json.loads(payload)
            if chunk.get("usage"):
                usage = chunk["usage"]
            for choice in chunk.get("choices") or []:
                finish = choice.get("finish_reason") or finish
                delta = choice.get("delta") or {}
                if delta.get("content"):
                    content.append(delta["content"])
                if delta.get("reasoning_content"):
                    reasoning.append(delta["reasoning_content"])
                for tc in delta.get("tool_calls") or []:
                    slot = tool_calls.setdefault(
                        tc.get("index", 0), {"name": "", "arguments": ""}
                    )
                    fn = tc.get("function") or {}
                    if fn.get("name"):
                        slot["name"] = fn["name"]
                    if fn.get("arguments"):
                        slot["arguments"] += fn["arguments"]
    calls = []
    for _, slot in sorted(tool_calls.items()):
        try:
            args = json.loads(slot["arguments"]) if slot["arguments"] else {}
        except json.JSONDecodeError:
            args = {"_unparsed": slot["arguments"]}
        calls.append({"name": slot["name"], "arguments": args})
    return {
        "content": "".join(content),
        "reasoning": "".join(reasoning),
        "tool_calls": calls,
        "usage": usage,
        "finish_reason": finish,
    }


def classify(result: dict, meta: dict) -> dict:
    calls = result["tool_calls"]
    names = [c["name"] for c in calls]
    out = {"action": "direct_answer", "hop_correct": None, "answer_date": None}
    scan = next((c for c in calls if c["name"] == "author_scan"), None)
    search = next((c for c in calls if c["name"] == "search"), None)
    if scan:
        out["action"] = "author_scan"
        out["hop_correct"] = scan["arguments"].get("author_id") == meta["official_author_id"]
    elif search:
        filters = search["arguments"].get("filters") or {}
        # publish_time="不限" means no time restriction — only a real window
        # counts as forcing recency.
        recency = filters.get("sort") == "最新" or filters.get("publish_time") not in (None, "", "不限")
        # A recency search only verifies this scenario if it's about the
        # subject — an unrelated recency search must not inflate pass rates.
        # Older scenario files without query_terms keep the permissive scoring.
        query = str(search["arguments"].get("query", ""))
        terms = meta.get("query_terms") or []
        on_topic = not terms or any(t.lower() in query.lower() for t in terms)
        out["action"] = "recency_search" if (recency and on_topic) else "plain_search"
    elif names:
        out["action"] = names[0]
    else:
        text = result["content"]
        fresh = any(re.search(p, text) for p in meta["fresh_patterns"])
        stale = any(re.search(p, text) for p in meta["stale_patterns"])
        out["answer_date"] = (
            "mixed" if fresh and stale else "fresh" if fresh else "stale" if stale else "unclear"
        )
    # Verified = the behavior the policy wants: consulted the official timeline
    # or forced recency ordering before answering.
    out["verification_hop"] = (
        out["action"] == "author_scan" and bool(out["hop_correct"])
    ) or out["action"] == "recency_search"
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenarios", default="bloc1_replay,bloc1_trap")
    ap.add_argument("--variants", default="v0")
    ap.add_argument("-n", "--trials", type=int, default=8)
    ap.add_argument("--concurrency", type=int, default=3)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    scenarios = {}
    for name in args.scenarios.split(","):
        payload = json.loads((HERE / "scenarios" / f"{name}.json").read_text())
        scenarios[name] = payload
    variants = args.variants.split(",")

    cells = []
    for variant in variants:
        for scenario, payload in scenarios.items():
            request = json.loads(json.dumps(payload["request"]))  # deep copy
            request["messages"][0]["content"] = apply_variant(
                request["messages"][0]["content"], variant
            )
            cells.append((variant, scenario, request, payload["meta"]))

    for variant, scenario, request, _ in cells:
        print(
            f"cell {variant}×{scenario}: system {len(request['messages'][0]['content'])} chars, "
            f"{len(request['tools'])} tools, model {request['model']}"
        )
    if args.dry_run:
        return

    api_key = load_api_key()
    stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    out_dir = HERE / "results" / stamp
    out_dir.mkdir(parents=True, exist_ok=True)

    lock = threading.Lock()
    rows = []

    def run_trial(variant, scenario, request, meta, trial):
        last_err = None
        for attempt in range(3):
            try:
                t0 = time.time()
                result = stream_completion(request, api_key)
                elapsed = time.time() - t0
                verdict = classify(result, meta)
                record = {
                    "variant": variant,
                    "scenario": scenario,
                    "trial": trial,
                    "elapsed_s": round(elapsed, 1),
                    **verdict,
                    "tool_calls": result["tool_calls"],
                    "usage": result["usage"],
                    "content_head": result["content"][:300],
                }
                raw_path = out_dir / f"{variant}_{scenario}_{trial:02d}.json"
                raw_path.write_text(json.dumps(
                    {"record": record, "content": result["content"], "reasoning": result["reasoning"]},
                    ensure_ascii=False, indent=1))
                with lock:
                    rows.append(record)
                    print(
                        f"  {variant}×{scenario} #{trial}: {verdict['action']}"
                        + (f" (author_id ok={verdict['hop_correct']})" if verdict["action"] == "author_scan" else "")
                        + (f" answer={verdict['answer_date']}" if verdict["action"] == "direct_answer" else "")
                        + f" [{elapsed:.0f}s]"
                    )
                return
            except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, json.JSONDecodeError) as e:
                last_err = e
                detail = ""
                if isinstance(e, urllib.error.HTTPError):
                    try:
                        detail = e.read().decode("utf-8", "replace")[:200]
                    except Exception:
                        pass
                with lock:
                    print(f"  {variant}×{scenario} #{trial}: retry after {type(e).__name__} {detail}")
                time.sleep(10 * (attempt + 1))
        with lock:
            rows.append({"variant": variant, "scenario": scenario, "trial": trial,
                         "action": "error", "error": str(last_err)})

    jobs = [
        (variant, scenario, request, meta, trial)
        for variant, scenario, request, meta in cells
        for trial in range(args.trials)
    ]
    with cf.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        list(pool.map(lambda j: run_trial(*j), jobs))

    summary = {}
    for row in rows:
        key = f"{row['variant']}×{row['scenario']}"
        cell = summary.setdefault(key, {"n": 0, "author_scan": 0, "hop_correct": 0,
                                        "recency_search": 0, "plain_search": 0,
                                        "direct_answer": 0, "answer_stale": 0,
                                        "answer_fresh": 0, "other": 0, "error": 0})
        cell["n"] += 1
        action = row.get("action")
        if action == "author_scan":
            cell["author_scan"] += 1
            if row.get("hop_correct"):
                cell["hop_correct"] += 1
        elif action in ("recency_search", "plain_search", "direct_answer", "error"):
            cell[action] += 1
            if action == "direct_answer":
                if row.get("answer_date") == "stale":
                    cell["answer_stale"] += 1
                elif row.get("answer_date") == "fresh":
                    cell["answer_fresh"] += 1
        else:
            cell["other"] += 1

    (out_dir / "summary.json").write_text(json.dumps(
        {"cells": summary, "rows": rows}, ensure_ascii=False, indent=1))
    total_cost = sum(
        (r.get("usage") or {}).get("prompt_tokens", 0) * 18.736 / 1e6
        + (r.get("usage") or {}).get("completion_tokens", 0) * 56.207 / 1e6
        for r in rows
    )
    print(f"\nresults -> {out_dir}")
    print(f"approx cost (uncached upper bound): ¥{total_cost:.2f}")
    header = f"{'cell':26} {'n':>2} {'scan✓':>6} {'scan✗':>6} {'recency':>8} {'plain':>6} {'direct':>7} {'stale!':>7} {'err':>4}"
    print(header)
    for key, cell in sorted(summary.items()):
        print(
            f"{key:26} {cell['n']:>2} {cell['hop_correct']:>6} "
            f"{cell['author_scan'] - cell['hop_correct']:>6} {cell['recency_search']:>8} "
            f"{cell['plain_search']:>6} {cell['direct_answer']:>7} {cell['answer_stale']:>7} {cell['error']:>4}"
        )


if __name__ == "__main__":
    main()
