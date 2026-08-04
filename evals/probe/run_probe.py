#!/usr/bin/env python3
"""Decision benchmark / smoke test for the XHS agent policy.

Each case pairs a real user search prompt with a cassette — the recorded
search-tool response from a production run (see build_scenarios.py) — and
declares its expected next action(s) in an `expect` block. The runner replays
the exact step-2 request against the CURRENT system prompt N times per case,
classifies each sampled action, and gates every case on its expected-action
pass rate. Exit code 1 when any case fails — usable as a pre-release smoke.

Usage:
  python3 run_probe.py                    # full benchmark: all cases, n=8
  python3 run_probe.py -n 3               # quick smoke
  python3 run_probe.py --dry-run          # build requests, no API calls

To test a candidate prompt wording, edit core/src/sites/xhs/knowledge.md,
rebuild scenarios, and rerun — git is the variant switch.

The Qwen API key comes from $DASHSCOPE_API_KEY / $QWEN_API_KEY (CI) or
~/.socai/auth.json (local), and is never printed. Raw responses land in results/<stamp>/ for audit.
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

import http.client
import os
import urllib.request
import urllib.error

HERE = Path(__file__).resolve().parent
# Override for CI or the international endpoint (dashscope-intl) if the
# mainland endpoint is unreachable from the runner.
BASE_URL = os.environ.get(
    "DASHSCOPE_BASE_URL",
    "https://dashscope.aliyuncs.com/compatible-mode/v1",
).rstrip("/") + "/chat/completions"

def load_api_key() -> str:
    key = os.environ.get("DASHSCOPE_API_KEY") or os.environ.get("QWEN_API_KEY") or ""
    if not key:
        try:
            auth = json.loads((Path.home() / ".socai" / "auth.json").read_text())
        except OSError:
            auth = {}
        key = (auth.get("qwen") or {}).get("api_key", "")
    if not key:
        sys.exit("no qwen api key: set DASHSCOPE_API_KEY (CI) or configure ~/.socai/auth.json")
    return key


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
    if finish is None:
        # Stream ended without a final choice chunk: a transport failure, not a
        # model decision — surface it as retryable so it can't skew the totals.
        raise TimeoutError("stream ended without finish_reason")
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
    out = {"action": "direct_answer", "hop_correct": None, "answer_date": None}
    scans = [c for c in calls if c["name"] == "author_scan"]
    searches = [c for c in calls if c["name"] == "search"]

    def search_is_recency(call):
        filters = call["arguments"].get("filters") or {}
        # publish_time="不限" means no time restriction — only a real window
        # counts as forcing recency. And a recency search only verifies this
        # scenario if it's about the subject — an unrelated recency search must
        # not inflate pass rates. Older scenario files without query_terms keep
        # the permissive scoring.
        recency = filters.get("sort") == "最新" or filters.get("publish_time") not in (None, "", "不限")
        query = str(call["arguments"].get("query", "")).lower()
        terms = meta.get("query_terms") or []
        # Alphanumeric-boundary match: "col" must not match "collection"/"color".
        # CJK neighbors ("col攀岩馆") still match — the guard is ASCII-only.
        def on_topic(term):
            return re.search(rf"(?<![a-z0-9]){re.escape(term.lower())}(?![a-z0-9])", query)
        return recency and (not terms or any(on_topic(t) for t in terms))

    # The prompt allows two tool calls per step, so score the BEST verification
    # across all calls: correct scan > on-topic recency search > wrong scan >
    # plain search > other tools.
    correct_scan = next(
        (c for c in scans if c["arguments"].get("author_id") == meta["official_author_id"]), None
    )
    recency_search = next((c for c in searches if search_is_recency(c)), None)
    if correct_scan or (scans and not recency_search):
        out["action"] = "author_scan"
        out["hop_correct"] = correct_scan is not None
    elif recency_search:
        out["action"] = "recency_search"
    elif searches:
        out["action"] = "plain_search"
    elif calls:
        out["action"] = calls[0]["name"]
    else:
        # Strip as-of framing ("截至今天/截至2026-07-30, …") before matching:
        # the anchor date used as framing must not read as a fresh event claim.
        text = re.sub(r"截至[^，。；\n]{0,20}", "", result["content"])
        # Answers lead with their conclusion, so the EARLIEST match decides:
        # "最近一次换线是1月28日…（7月的通知是…）" is stale even though a fresh
        # date appears later, and a fresh conclusion that recaps January
        # history stays fresh.
        def first_pos(patterns):
            hits = [m.start() for m in (re.search(p, text) for p in patterns) if m]
            return min(hits) if hits else None
        fresh_pos = first_pos(meta["fresh_patterns"])
        stale_pos = first_pos(meta["stale_patterns"])
        out["answer_date"] = (
            "fresh" if fresh_pos is not None and (stale_pos is None or fresh_pos < stale_pos)
            else "stale" if stale_pos is not None
            else "unclear"
        )
    # Verified = consulted the official timeline. A recency-filtered re-search
    # is tracked as its own action but does NOT count: the policy keeps XHS's
    # default relevance ranking and handles stale no-official-source evidence
    # with an explicit date caveat instead (see knowledge.md).
    out["verification_hop"] = out["action"] == "author_scan" and bool(out["hop_correct"])
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenarios", default="bloc1_replay,bloc1_trap")
    ap.add_argument("-n", "--trials", type=int, default=8)
    ap.add_argument("--concurrency", type=int, default=3)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    cells = []
    for name in args.scenarios.split(","):
        payload = json.loads((HERE / "scenarios" / f"{name}.json").read_text())
        cells.append((name, payload["request"], payload["meta"]))

    for scenario, request, _ in cells:
        print(
            f"case {scenario}: system {len(request['messages'][0]['content'])} chars, "
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

    def run_trial(scenario, request, meta, trial):
        last_err = None
        for attempt in range(3):
            try:
                t0 = time.time()
                result = stream_completion(request, api_key)
                elapsed = time.time() - t0
                verdict = classify(result, meta)
                record = {
                    "scenario": scenario,
                    "trial": trial,
                    "elapsed_s": round(elapsed, 1),
                    **verdict,
                    "tool_calls": result["tool_calls"],
                    "usage": result["usage"],
                    "content_head": result["content"][:300],
                }
                raw_path = out_dir / f"{scenario}_{trial:02d}.json"
                raw_path.write_text(json.dumps(
                    {"record": record, "content": result["content"], "reasoning": result["reasoning"]},
                    ensure_ascii=False, indent=1))
                with lock:
                    rows.append(record)
                    print(
                        f"  {scenario} #{trial}: {verdict['action']}"
                        + (f" (author_id ok={verdict['hop_correct']})" if verdict["action"] == "author_scan" else "")
                        + (f" answer={verdict['answer_date']}" if verdict["action"] == "direct_answer" else "")
                        + f" [{elapsed:.0f}s]"
                    )
                return
            # OSError covers URLError/HTTPError/timeouts; HTTPException covers
            # read-phase failures like IncompleteRead — a mid-stream disconnect
            # must retry, not abort the whole pool.map.
            except (OSError, http.client.HTTPException, json.JSONDecodeError) as e:
                last_err = e
                detail = ""
                if isinstance(e, urllib.error.HTTPError):
                    try:
                        detail = e.read().decode("utf-8", "replace")[:200]
                    except Exception:
                        pass
                with lock:
                    print(f"  {scenario} #{trial}: retry after {type(e).__name__} {detail}")
                if attempt < 2:
                    time.sleep(10 * (attempt + 1))
        with lock:
            rows.append({"scenario": scenario, "trial": trial,
                         "action": "error", "error": str(last_err)})

    jobs = [
        (scenario, request, meta, trial)
        for scenario, request, meta in cells
        for trial in range(args.trials)
    ]
    with cf.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        list(pool.map(lambda j: run_trial(*j), jobs))

    summary = {}
    for row in rows:
        key = row['scenario']
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
    # Expectation gate: each case declares the outcomes that count as correct.
    def trial_outcome_tokens(r):
        toks = set()
        if r.get("action") == "author_scan" and r.get("hop_correct"):
            toks.add("author_scan_official")
        if r.get("action") == "direct_answer" and r.get("answer_date") == "fresh":
            toks.add("direct_fresh")
        if r.get("action") == "recency_search":
            toks.add("recency_search")
        return toks

    failures = []
    for scenario, _, meta in cells:
        expect = meta.get("expect")
        if not expect:
            continue
        sel = [r for r in rows if r["scenario"] == scenario]
        passed = sum(1 for r in sel if trial_outcome_tokens(r) & set(expect["pass"]))
        need = float(expect.get("min_pass_rate", 0.75))
        ok = bool(sel) and passed / len(sel) >= need
        if not ok:
            failures.append(scenario)
        print(
            f"{'PASS' if ok else 'FAIL'} {scenario}: {passed}/{len(sel)} trials took an "
            f"expected action ({' or '.join(expect['pass'])}; gate ≥{need:.0%})"
        )

    header = f"{'case':26} {'n':>2} {'scan✓':>6} {'scan✗':>6} {'recency':>8} {'plain':>6} {'direct':>7} {'stale!':>7} {'err':>4}"
    print(header)
    for key, cell in sorted(summary.items()):
        print(
            f"{key:26} {cell['n']:>2} {cell['hop_correct']:>6} "
            f"{cell['author_scan'] - cell['hop_correct']:>6} {cell['recency_search']:>8} "
            f"{cell['plain_search']:>6} {cell['direct_answer']:>7} {cell['answer_stale']:>7} {cell['error']:>4}"
        )
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
