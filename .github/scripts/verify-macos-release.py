#!/usr/bin/env python3
"""Qualify the built macOS artifacts before publishing, on each runner's CPU."""

from __future__ import annotations

import base64
import functools
import http.server
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile
import threading
import time


def run(*args: str | Path, **kwargs) -> str:
    print("+", " ".join(map(str, args)), flush=True)
    return subprocess.check_output(
        list(map(str, args)), text=True, stderr=subprocess.STDOUT, **kwargs
    )


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def validate_ticket(target: Path) -> None:
    # stapler consults Apple even for a stapled artifact. Retry only transport
    # failures; an invalid/missing ticket must still fail qualification.
    for attempt in range(3):
        try:
            run("xcrun", "stapler", "validate", target)
            return
        except subprocess.CalledProcessError as error:
            transient = re.search(r"NSURLErrorDomain Code=-(?:1001|1004|1005|1009)\b", error.output or "")
            if not transient or attempt == 2:
                raise
            print(f"Apple ticket lookup transport failure; retry {attempt + 1}/2", flush=True)
            time.sleep(5 * (attempt + 1))


def check_binary(binary: Path, intel_floor: str) -> None:
    require(set(run("lipo", "-archs", binary).split()) == {"arm64", "x86_64"},
            f"missing universal architecture: {binary}")
    for arch, maximum in (("x86_64", intel_floor), ("arm64", "11.0")):
        load_commands = run("otool", "-arch", arch, "-l", binary)
        versions = re.findall(
            r"cmd LC_(?:BUILD_VERSION|VERSION_MIN_MACOSX)\b.*?"
            r"\n\s+(?:minos|version) (\d+(?:\.\d+)+)", load_commands, re.S
        )
        require(len(versions) == 1, f"cannot determine deployment target: {binary} {arch}")
        version = lambda value: tuple(int(part) for part in value.split(".")) + (0,) * (3 - len(value.split(".")))
        require(version(versions[0]) <= version(maximum),
                f"deployment target increased: {binary.name} {arch}: {versions[0]} > {maximum}")
        print(f"{binary.name} {arch}: minimum macOS {versions[0]}", flush=True)


def check_asr(binary: Path, env: dict[str, str]) -> None:
    # No models or network needed: reaching the expected argument error proves
    # the helper's native libraries loaded on this OS and CPU.
    result = subprocess.run([str(binary)], env=env, text=True, capture_output=True, timeout=30)
    require(result.returncode == 1 and "requires --serve" in result.stderr,
            f"ASR helper did not start: {result.returncode}: {result.stderr}")


def check_app(app: Path, version: str, env: dict[str, str], *, launch: bool) -> None:
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    require(info["CFBundleShortVersionString"] == version, "app version mismatch")
    require(info["LSMinimumSystemVersion"] == "10.13", "desktop minimum OS changed")
    run("codesign", "--verify", "--deep", "--strict", app)
    run("spctl", "-a", "-vvv", "-t", "execute", app)
    validate_ticket(app)
    binaries = app / "Contents/MacOS"
    for name, minimum in ((info["CFBundleExecutable"], "10.13"),
                          ("socai-asr", "10.12"), ("lark-cli", "11.0")):
        check_binary(binaries / name, minimum)
    check_asr(binaries / "socai-asr", env)
    print(run(binaries / "lark-cli", "--version", env=env, timeout=30))
    if launch:
        # Exercise dyld, Tauri initialization, and the WebView on a fresh profile.
        # This is a startup smoke check, not a logged-in feature/UI test.
        with tempfile.TemporaryFile(mode="w+") as log:
            process = subprocess.Popen([str(binaries / info["CFBundleExecutable"])],
                                       env=env, stdout=log, stderr=log)
            try:
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    print("desktop remained running for 15 seconds", flush=True)
                else:
                    log.seek(0)
                    raise RuntimeError(f"desktop exited during startup ({process.returncode}): {log.read()}")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


def verify(artifacts: Path, version: str, scratch: Path) -> None:
    env = dict(os.environ, HOME=str(scratch / "home"),
               SOCAI_HOME=str(scratch / "socai-home"),
               SOCAI_RUNS_DIR=str(scratch / "runs"), SHELL="/bin/zsh",
               SOCAI_INSTALL_DIR=str(scratch / "bin"))
    Path(env["HOME"]).mkdir()
    print(run("sw_vers"), run("uname", "-m"), flush=True)
    run("shasum", "-a", "256", "-c", "socai-cli-macos-universal.tar.gz.sha256", cwd=artifacts)
    # Exercise the real installer against these artifacts, not the previous
    # public release. Its version check below also detects fallback downloads.
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(artifacts))
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env["SOCAI_DOWNLOAD_BASE_URL"] = f"http://127.0.0.1:{server.server_port}"
        try:
            print(run("sh", artifacts / "install.sh", env=env, timeout=180))
        finally:
            server.shutdown()
            thread.join()
    cli = Path(env["SOCAI_INSTALL_DIR"])
    for name in ("socai", "socai-asr"):
        check_binary(cli / name, "10.12")
    require(run(cli / "socai", "--version", env=env).strip() == f"socai {version}",
            "installed CLI version mismatch")
    run(cli / "socai", "--help", env=env, timeout=30)
    check_asr(cli / "socai-asr", env)
    require(str(cli) in (Path(env["HOME"]) / ".zshrc").read_text(), "installer did not configure PATH")

    dmg = artifacts / "socai-macos-universal.dmg"
    run("hdiutil", "verify", dmg)
    validate_ticket(dmg)
    run("spctl", "-a", "-vvv", "-t", "open", "--context", "context:primary-signature", dmg)
    mount = scratch / "mounted"
    mount.mkdir()
    run("hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mount, dmg)
    installed_app = scratch / "Applications/socai.app"
    try:
        run("ditto", mount / "socai.app", installed_app)
    finally:
        run("hdiutil", "detach", mount)
    check_app(installed_app, version, env, launch=True)

    tarball = artifacts / "socai-macos-universal.app.tar.gz"
    signature = Path(str(tarball) + ".sig").read_text().strip()
    config = json.loads(Path("app/src-tauri/tauri.conf.json").read_text())
    public_key = scratch / "updater.pub"
    public_key.write_bytes(base64.b64decode(config["plugins"]["updater"]["pubkey"]))
    detached_signature = scratch / "updater.minisig"
    detached_signature.write_bytes(base64.b64decode(signature))
    run("minisign", "-Vm", tarball, "-p", public_key, "-x", detached_signature)
    manifest = json.loads((artifacts / "latest.json").read_text())
    require(manifest["version"] == version, "updater version mismatch")
    for arch in ("darwin-aarch64", "darwin-x86_64"):
        entry = manifest["platforms"][arch]
        require(entry["signature"] == signature, f"updater signature mismatch: {arch}")
        require(entry["url"].endswith(f"/v{version}/{tarball.name}"), f"updater URL mismatch: {arch}")
    update_dir = scratch / "update"
    update_dir.mkdir()
    run("tar", "-xzf", tarball, "-C", update_dir)
    check_app(update_dir / "socai.app", version, env, launch=True)
    print("macOS artifact qualification passed", flush=True)


if __name__ == "__main__":
    try:
        with tempfile.TemporaryDirectory(prefix="socai-release-check-") as directory:
            verify(Path(sys.argv[1]).resolve(), sys.argv[2], Path(directory))
    except subprocess.CalledProcessError as error:
        print(error.output, file=sys.stderr)
        raise
