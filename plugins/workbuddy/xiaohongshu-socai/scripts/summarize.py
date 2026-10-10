#!/usr/bin/env python3
"""Summarize a socai xhs result JSON into a context-friendly digest.

socai stdout results are deliberately compact, but a 10-note run with comments
still runs to hundreds of KB. Reading that raw into an agent context is wasteful.
This prints one line per note (plus optional detail) so the agent can decide
what to actually open.

Usage:
    python3 summarize.py <result.json> [--limit N] [--sort likes|date|comments]
    python3 summarize.py <result.json> --note-id <id>        # full body of one note
    python3 summarize.py <result.json> --titles              # titles and urls only

Exit codes:
    0  ok
    1  file missing or unparseable
    2  result indicates a blocker (login / rate limit / challenge)
"""

import argparse
import json
import sys

BLOCKERS = {
    "login_required": "未登录小红书。让用户在浏览器里扫码登录后重跑，不要重试。",
    "rate_limited": "触发平台限流。换更窄的查询或等待后重试一次。",
    "challenge_required": "遇到安全验证/验证码。用已有证据作答并说明缺口。",
}


def load(path):
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def unwrap(data):
    """Accept either the daemon payload or the CLI's already-unwrapped data."""
    if isinstance(data, dict) and "data" in data and isinstance(data["data"], (dict, list)):
        return data["data"]
    return data


def to_number(raw):
    """'1.2万' -> 12000, '3.5w' -> 35000, '1.5k' -> 1500, '999+' -> 999.

    Returns None when the page hid the count entirely (a bare button label like
    '收藏' carries no digits) so callers never mistake 'hidden' for zero.
    """
    if raw is None:
        return None
    text = str(raw).strip()
    if not any(ch.isdigit() for ch in text):
        return None
    value = text.lower().replace(",", "").replace("+", "")
    number, unit = 0.0, 1.0
    for index, char in enumerate(value):
        if char.isdigit() or char == ".":
            continue
        suffix = value[index:]
        unit = 10000.0 if suffix[:1] in ("万", "w") else 1000.0 if suffix[:1] == "k" else 1.0
        value = value[:index]
        break
    try:
        number = float(value)
    except ValueError:
        return None
    return int(round(number * unit))


def collect(data):
    """Return (kind, rows, meta). rows are normalized dicts."""
    if not isinstance(data, dict):
        return "unknown", [], {}

    meta = {
        "artifact": (data.get("artifact") or {}).get("path", ""),
        "trimmed": (data.get("artifact") or {}).get("extra_note_properties", []),
        "media_manifest": data.get("media_manifest_path", ""),
        "query": data.get("query", ""),
        "author_id": data.get("author_id", ""),
    }

    rows = []
    for entry in data.get("notes") or []:
        entity = entry.get("entity", entry) if isinstance(entry, dict) else {}
        rows.append(entity)
    for card in data.get("cards") or []:
        rows.append(card)

    if rows:
        return "notes" if data.get("notes") else "cards", rows, meta

    profile = data.get("profile")
    if isinstance(profile, dict):
        meta["profile"] = profile
        for card in profile.get("note_cards") or []:
            rows.append(card)
        return "profile", rows, meta

    return "unknown", rows, meta


def line(row, index):
    title = (row.get("title") or "").replace("\n", " ").strip()
    title = title[:46] + "…" if len(title) > 46 else title
    published = row.get("published") or {}
    # Prefer the publication-time contract: an exact `+08:00` instant when the
    # extractor captured one, else its Beijing calendar date.
    date = published.get("at") or published.get("date") or row.get("date", "")
    if row.get("edited") or row.get("date_edited"):
        date += "(编辑)"
    comments = row.get("top_comments") or []
    return (
        f"[{index:02d}] {title}\n"
        f"     作者 {row.get('author', '')} ({row.get('author_id', '')})\n"
        f"     日期 {date}  赞 {row.get('likes', '')}  藏 {row.get('favorites', '')}  "
        f"评 {row.get('comments_count', '')}  已取评论 {len(comments)}\n"
        f"     {row.get('url', '')}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("path", help="socai xhs result JSON file")
    parser.add_argument("--limit", type=int, default=0, help="max rows to print (0 = all)")
    parser.add_argument("--sort", choices=["none", "likes", "date", "comments"], default="none")
    parser.add_argument("--note-id", help="print the full body of one note id")
    parser.add_argument("--titles", action="store_true", help="print only titles and urls")
    args = parser.parse_args()

    try:
        data = unwrap(load(args.path))
    except FileNotFoundError:
        print(f"file not found: {args.path}", file=sys.stderr)
        return 1
    except json.JSONDecodeError as error:
        print(f"not valid JSON: {error}", file=sys.stderr)
        return 1

    if isinstance(data, dict):
        for key, hint in BLOCKERS.items():
            if data.get("reason") == key or data.get(key) is True:
                print(f"BLOCKER [{key}] {hint}", file=sys.stderr)
                return 2
        if data.get("ok") is False and data.get("reason"):
            print(f"BLOCKER [{data['reason']}]", file=sys.stderr)

    kind, rows, meta = collect(data)

    if meta.get("profile"):
        profile = meta["profile"]
        verified = "已认证 " + profile.get("verification", "") if profile.get("verified") else "未认证"
        print(
            f"博主 {profile.get('display_name', '')} · {verified}\n"
            f"  小红书号 {profile.get('xhs_id', '')}  IP {profile.get('ip_location', '')}\n"
            f"  粉丝 {profile.get('followers', '')}  关注 {profile.get('following', '')}  "
            f"获赞与收藏 {profile.get('likes_and_collections', '')}  作品 {profile.get('note_count', 0)}\n"
            f"  简介 {profile.get('bio', '')}\n"
            f"  {profile.get('url', '')}\n"
        )

    if args.note_id:
        for row in rows:
            if str(row.get("note_id")) == args.note_id:
                print(json.dumps(row, ensure_ascii=False, indent=2))
                return 0
        print(f"note_id not found: {args.note_id}", file=sys.stderr)
        return 1

    if args.sort != "none":
        key = {
            "likes": lambda r: to_number(r.get("likes")) or 0,
            "comments": lambda r: to_number(r.get("comments_count")) or 0,
            "date": lambda r: r.get("date", ""),
        }[args.sort]
        rows = sorted(rows, key=key, reverse=True)

    if args.limit:
        rows = rows[: args.limit]

    print(f"共 {len(rows)} 条（{kind}）")
    if meta["query"]:
        print(f"查询 {meta['query']}")
    print()

    for index, row in enumerate(rows):
        if args.titles:
            print(f"[{index:02d}] {row.get('title', '')}\n     {row.get('url', '')}")
        else:
            print(line(row, index))

    if meta["artifact"]:
        print(f"\n完整字段（{', '.join(str(item) for item in meta['trimmed']) or '见文件'}）: {meta['artifact']}")
    if meta["media_manifest"]:
        print(f"媒体清单: {meta['media_manifest']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
