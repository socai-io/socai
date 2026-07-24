#!/usr/bin/env python3
"""Publish signed release artifacts to Alibaba Cloud OSS.

The release is staged under an immutable version prefix before GitHub is
published. The mutable latest prefix is promoted only after GitHub succeeds.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import mimetypes
import os
import sys
from pathlib import Path
from urllib.parse import quote, urlparse
from urllib.request import Request, urlopen

import oss2


IMMUTABLE_CACHE_CONTROL = "public, max-age=31536000, immutable"
LATEST_CACHE_CONTROL = "no-cache, no-store, must-revalidate"
PLATFORMS = ("darwin-aarch64", "darwin-x86_64", "windows-x86_64")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("stage", "promote"))
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--bucket", required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--prefix", default="releases")
    parser.add_argument("--public-base-url", required=True)
    return parser.parse_args()


def content_type(name: str) -> str:
    explicit = {
        ".dmg": "application/x-apple-diskimage",
        ".exe": "application/vnd.microsoft.portable-executable",
        ".json": "application/json",
        ".ps1": "text/plain; charset=utf-8",
        ".sh": "text/plain; charset=utf-8",
        ".sig": "text/plain; charset=utf-8",
    }
    for suffix, value in explicit.items():
        if name.endswith(suffix):
            return value
    guessed, _ = mimetypes.guess_type(name)
    return guessed or "application/octet-stream"


def headers_for(name: str, cache_control: str) -> dict[str, str]:
    headers = {
        "Cache-Control": cache_control,
        "Content-Type": content_type(name),
    }
    if name.endswith((".dmg", ".exe", ".tar.gz", ".zip")):
        headers["Content-Disposition"] = f'attachment; filename="{name}"'
    return headers


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def public_url(base_url: str, *parts: str) -> str:
    encoded = "/".join(quote(part, safe="") for part in parts)
    return f"{base_url.rstrip('/')}/{encoded}"


def build_oss_manifest(source_dir: Path, tag: str, base_url: str) -> bytes:
    source = json.loads((source_dir / "latest.json").read_text())
    platforms = source.get("platforms", {})
    for platform in PLATFORMS:
        entry = platforms.get(platform)
        if not isinstance(entry, dict):
            raise SystemExit(f"latest.json missing platform {platform}")
        asset_name = Path(urlparse(entry.get("url", "")).path).name
        if not asset_name or not (source_dir / asset_name).is_file():
            raise SystemExit(
                f"latest.json {platform} references missing asset {asset_name!r}"
            )
        entry["url"] = public_url(base_url, tag, asset_name)
    return (json.dumps(source, indent=2) + "\n").encode()


def source_payloads(source_dir: Path, oss_manifest: bytes) -> dict[str, bytes | Path]:
    payloads: dict[str, bytes | Path] = {}
    for path in sorted(source_dir.iterdir()):
        if not path.is_file():
            continue
        payloads[path.name] = oss_manifest if path.name == "latest.json" else path
    if "latest.json" not in payloads:
        raise SystemExit(f"missing {source_dir / 'latest.json'}")
    return payloads


def payload_size(payload: bytes | Path) -> int:
    return len(payload) if isinstance(payload, bytes) else payload.stat().st_size


def payload_sha256(payload: bytes | Path) -> str:
    if isinstance(payload, bytes):
        return sha256_bytes(payload)
    digest = hashlib.sha256()
    with payload.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def upload(
    bucket: oss2.Bucket,
    key: str,
    name: str,
    payload: bytes | Path,
    cache_control: str,
) -> None:
    headers = headers_for(name, cache_control)
    if isinstance(payload, bytes):
        bucket.put_object(key, payload, headers=headers)
    else:
        oss2.resumable_upload(bucket, key, str(payload), headers=headers)
    result = bucket.head_object(key)
    expected_size = payload_size(payload)
    if result.content_length != expected_size:
        raise SystemExit(
            f"OSS size mismatch for {key}: {result.content_length} != {expected_size}"
        )
    print(
        f"uploaded {key} size={expected_size} sha256={payload_sha256(payload)}",
        flush=True,
    )


def verify_public_object(url: str, expected_size: int | None = None) -> None:
    request = Request(url, method="HEAD", headers={"Cache-Control": "no-cache"})
    with urlopen(request, timeout=60) as response:
        if response.status != 200:
            raise SystemExit(f"public verification failed for {url}: {response.status}")
        length = response.headers.get("Content-Length")
        if expected_size is not None and length and int(length) != expected_size:
            raise SystemExit(
                f"public size mismatch for {url}: {length} != {expected_size}"
            )
    print(f"verified public {url}", flush=True)


def verify_public_manifest(url: str, tag: str, base_url: str) -> None:
    request = Request(url, headers={"Cache-Control": "no-cache"})
    with urlopen(request, timeout=60) as response:
        manifest = json.load(response)
    expected_version = tag.removeprefix("v")
    if manifest.get("version") != expected_version:
        raise SystemExit(
            f"public manifest version {manifest.get('version')} != {expected_version}"
        )
    for platform in PLATFORMS:
        entry = manifest.get("platforms", {}).get(platform, {})
        asset_url = entry.get("url", "")
        signature = entry.get("signature", "")
        if not signature:
            raise SystemExit(f"public manifest {platform} signature is empty")
        if not asset_url.startswith(f"{base_url.rstrip('/')}/{tag}/"):
            raise SystemExit(f"public manifest {platform} has unexpected URL {asset_url}")
        verify_public_object(asset_url)
    print(f"verified public manifest {url}", flush=True)


def main() -> None:
    args = parse_args()
    source_dir = args.source_dir.resolve()
    if not source_dir.is_dir():
        raise SystemExit(f"source directory does not exist: {source_dir}")

    access_key_id = os.environ.get("ALIYUN_ACCESS_KEY_ID", "")
    access_key_secret = os.environ.get("ALIYUN_ACCESS_KEY_SECRET", "")
    security_token = os.environ.get("ALIYUN_SECURITY_TOKEN", "")
    if not access_key_id or not access_key_secret:
        raise SystemExit("missing ALIYUN_ACCESS_KEY_ID or ALIYUN_ACCESS_KEY_SECRET")

    auth: oss2.Auth | oss2.StsAuth
    if security_token:
        auth = oss2.StsAuth(access_key_id, access_key_secret, security_token)
    else:
        auth = oss2.Auth(access_key_id, access_key_secret)

    bucket = oss2.Bucket(auth, args.endpoint, args.bucket)
    prefix = args.prefix.strip("/")
    base_url = args.public_base_url.rstrip("/")
    oss_manifest = build_oss_manifest(source_dir, args.tag, base_url)
    payloads = source_payloads(source_dir, oss_manifest)

    if args.phase == "stage":
        for name, payload in payloads.items():
            key = f"{prefix}/{args.tag}/{name}"
            upload(bucket, key, name, payload, IMMUTABLE_CACHE_CONTROL)
        for name, payload in payloads.items():
            verify_public_object(
                public_url(base_url, args.tag, name),
                payload_size(payload),
            )
        verify_public_manifest(
            public_url(base_url, args.tag, "latest.json"),
            args.tag,
            base_url,
        )
        return

    # Update mutable assets first and latest.json last. Existing updater
    # manifests always reference immutable versioned assets, so this promotion
    # cannot invalidate an older updater response.
    for name, payload in payloads.items():
        if name == "latest.json":
            continue
        versioned_key = f"{prefix}/{args.tag}/{name}"
        versioned = bucket.head_object(versioned_key)
        if versioned.content_length != payload_size(payload):
            raise SystemExit(f"staged OSS object is missing or incomplete: {versioned_key}")
        upload(
            bucket,
            f"{prefix}/latest/{name}",
            name,
            payload,
            LATEST_CACHE_CONTROL,
        )

    upload(
        bucket,
        f"{prefix}/latest/latest.json",
        "latest.json",
        oss_manifest,
        LATEST_CACHE_CONTROL,
    )
    verify_public_manifest(
        public_url(base_url, "latest", "latest.json"),
        args.tag,
        base_url,
    )
    for name, payload in payloads.items():
        verify_public_object(
            public_url(base_url, "latest", name),
            payload_size(payload),
        )


if __name__ == "__main__":
    try:
        main()
    except oss2.exceptions.OssError as exc:
        print(
            f"OSS request failed: status={exc.status} code={exc.code} "
            f"request_id={exc.request_id}",
            file=sys.stderr,
        )
        raise SystemExit(1) from exc
