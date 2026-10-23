#!/usr/bin/env python3
"""Update release version metadata for the app and Rust workspace."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")
VERSION_LINE_RE = re.compile(r'^(\s*version\s*=\s*")[^"]+(")')
LOCKED_RUST_PACKAGES = (
    "socai-asr",
    "socai-cli",
    "socai-core",
    "socai_app",
)


def update_json_version(path: Path, version: str) -> None:
    data = json.loads(path.read_text())
    data["version"] = version
    path.write_text(json.dumps(data, indent=2) + "\n")


def update_section_version(path: Path, section: str, version: str) -> None:
    lines = path.read_text().splitlines(keepends=True)
    in_section = False
    count = 0

    for index, line in enumerate(lines):
        if re.match(r"^\s*\[[^\]]+\]\s*$", line):
            in_section = line.strip() == f"[{section}]"

        if not in_section:
            continue

        body = line[:-1] if line.endswith("\n") else line
        newline = "\n" if line.endswith("\n") else ""
        match = VERSION_LINE_RE.match(body)
        if not match:
            continue

        lines[index] = f'{match.group(1)}{version}{match.group(2)}{newline}'
        count += 1

    if count != 1:
        raise SystemExit(f"expected exactly one {section} version in {path}")

    path.write_text("".join(lines))


def update_cargo_lock(path: Path, version: str) -> None:
    if not path.exists():
        return

    text = path.read_text()
    for package_name in LOCKED_RUST_PACKAGES:
        pattern = re.compile(
            rf'(?m)^(\[\[package\]\]\nname = "{re.escape(package_name)}"\nversion = ")[^"]+(")'
        )
        text, count = pattern.subn(
            lambda match: f"{match.group(1)}{version}{match.group(2)}",
            text,
            count=1,
        )
        if count != 1:
            raise SystemExit(f"expected exactly one {package_name} package entry in {path}")

    path.write_text(text)


def update_core_dependency(path: Path, version: str) -> None:
    text, count = re.subn(
        r'(?m)^(socai-core\s*=\s*\{[^\n]*\bversion\s*=\s*")=[^"]+("[^\n]*\})',
        lambda match: f"{match.group(1)}={version}{match.group(2)}",
        path.read_text(),
    )
    if count != 1:
        raise SystemExit(f"expected exactly one pinned socai-core dependency in {path}")
    path.write_text(text)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: set-app-version.py VERSION")

    version = sys.argv[1]
    if not SEMVER_RE.fullmatch(version):
        raise SystemExit(f"version must be strict MAJOR.MINOR.PATCH semver, got: {version}")

    update_json_version(Path("app/package.json"), version)
    update_json_version(Path("app/src-tauri/tauri.conf.json"), version)
    update_section_version(Path("app/src-tauri/Cargo.toml"), "package", version)
    update_section_version(Path("Cargo.toml"), "workspace.package", version)
    update_core_dependency(Path("Cargo.toml"), version)
    update_cargo_lock(Path("Cargo.lock"), version)


if __name__ == "__main__":
    main()
