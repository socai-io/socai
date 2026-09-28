#!/usr/bin/env python3
"""Export a socai xhs result to CSV for spreadsheet work.

Companion to summarize.py: when a study cites more than a handful of notes,
hand the user a CSV so they can sort, filter and paste into their own sheets.
Markdown carries the analysis; the CSV carries the source list.

Requires summarize.py in the same directory (reuses its parsing and the
Chinese-count conversion so numbers never drift between the two tools).

Usage:
    python3 export_csv.py <result.json> -o out.csv [--sort likes|comments|date] [--limit N]
"""

import argparse
import csv
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize import BLOCKERS, collect, to_number, unwrap  # noqa: E402

COLUMNS = [
    "序号",
    "标题",
    "作者",
    "作者ID",
    "日期",
    "是否编辑过",
    "点赞(原文)",
    "点赞数",
    "收藏数",
    "评论数",
    "类型",
    "链接",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("path", help="socai xhs result JSON file")
    parser.add_argument("-o", "--output", required=True, help="output CSV path")
    parser.add_argument("--sort", choices=["none", "likes", "comments", "date"], default="none")
    parser.add_argument("--limit", type=int, default=0, help="max rows (0 = all)")
    args = parser.parse_args()

    try:
        with open(args.path, "r", encoding="utf-8") as handle:
            data = unwrap(__import__("json").load(handle))
    except FileNotFoundError:
        print(f"file not found: {args.path}", file=sys.stderr)
        return 1
    except ValueError as error:
        print(f"not valid JSON: {error}", file=sys.stderr)
        return 1

    if isinstance(data, dict):
        for key, hint in BLOCKERS.items():
            if data.get("reason") == key or data.get(key) is True:
                print(f"BLOCKER [{key}] {hint}", file=sys.stderr)
                return 2

    _kind, rows, _meta = collect(data)

    if args.sort != "none":
        key = {
            "likes": lambda r: to_number(r.get("likes")) or 0,
            "comments": lambda r: to_number(r.get("comments_count")) or 0,
            "date": lambda r: r.get("date", ""),
        }[args.sort]
        rows = sorted(rows, key=key, reverse=True)

    if args.limit:
        rows = rows[: args.limit]

    # utf-8-sig writes the BOM Excel needs to render Chinese without mojibake.
    with open(args.output, "w", encoding="utf-8-sig", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(COLUMNS)
        for index, row in enumerate(rows):
            date = row.get("date", "")
            writer.writerow([
                index,
                (row.get("title") or "").replace("\n", " ").strip(),
                row.get("author", ""),
                row.get("author_id", ""),
                date,
                "是" if row.get("date_edited") else "",
                row.get("likes", ""),
                to_number(row.get("likes")) if to_number(row.get("likes")) is not None else "",
                to_number(row.get("favorites")) if to_number(row.get("favorites")) is not None else "",
                to_number(row.get("comments_count")) if to_number(row.get("comments_count")) is not None else "",
                row.get("type", ""),
                row.get("url", ""),
            ])

    print(f"已写入 {args.output}（{len(rows)} 条）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
