import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  createReadStream,
  createWriteStream,
  existsSync,
  mkdirSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs";
import { pipeline } from "node:stream/promises";
import { Readable } from "node:stream";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const VERSION = "1.0.76";
const RELEASE_BASE = `https://github.com/larksuite/cli/releases/download/v${VERSION}`;
const APP_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CACHE_DIR = path.join(APP_DIR, ".cache", "lark-cli", `v${VERSION}`);
const BIN_DIR = path.join(APP_DIR, "src-tauri", "binaries");

// Pinned from the checksums.txt attached to the official v1.0.76 release.
const ARCHIVES = {
  "darwin-amd64": {
    name: `lark-cli-${VERSION}-darwin-amd64.tar.gz`,
    sha256: "f13c35b4a2a83d0c32b4ab3c223357cacffa341f621a112ca51e01f80826782a",
  },
  "darwin-arm64": {
    name: `lark-cli-${VERSION}-darwin-arm64.tar.gz`,
    sha256: "6d9776cbde1b7d6a23c7279364578df2d5ea54cdbb041951d97b68567bce8cc8",
  },
  "linux-amd64": {
    name: `lark-cli-${VERSION}-linux-amd64.tar.gz`,
    sha256: "759a676dde001bdc015384cfd741bcaca873329bbcaad8c4ea4a06acb49b3f42",
  },
  "linux-arm64": {
    name: `lark-cli-${VERSION}-linux-arm64.tar.gz`,
    sha256: "7cd7ffc4350d689d46fc0fc763069e2dd1889ef28d5a6e8282193e71612caff6",
  },
  "windows-amd64": {
    name: `lark-cli-${VERSION}-windows-amd64.zip`,
    sha256: "cf59dcf3224a0753b1b11cae14f0513242ef7eab02f9c7d35c26427647ed6145",
  },
};

mkdirSync(CACHE_DIR, { recursive: true });
mkdirSync(BIN_DIR, { recursive: true });

async function sha256(file) {
  const hash = createHash("sha256");
  await pipeline(createReadStream(file), hash);
  return hash.digest("hex");
}

async function download(archive) {
  const destination = path.join(CACHE_DIR, archive.name);
  if (existsSync(destination) && (await sha256(destination)) === archive.sha256) {
    return destination;
  }
  rmSync(destination, { force: true });
  const partial = `${destination}.partial`;
  rmSync(partial, { force: true });

  console.log(`[lark-cli] downloading ${archive.name}`);
  const response = await fetch(`${RELEASE_BASE}/${archive.name}`, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`download failed: ${response.status} ${response.statusText}`);
  }
  await pipeline(Readable.fromWeb(response.body), createWriteStream(partial));
  const actual = await sha256(partial);
  if (actual !== archive.sha256) {
    rmSync(partial, { force: true });
    throw new Error(
      `checksum mismatch for ${archive.name}: expected ${archive.sha256}, got ${actual}`,
    );
  }
  renameSync(partial, destination);
  return destination;
}

function findBinary(root, windows) {
  const wanted = windows ? "lark-cli.exe" : "lark-cli";
  const pending = [root];
  while (pending.length > 0) {
    const current = pending.pop();
    for (const entry of readdirSync(current)) {
      const candidate = path.join(current, entry);
      if (statSync(candidate).isDirectory()) pending.push(candidate);
      else if (entry === wanted) return candidate;
    }
  }
  throw new Error(`${wanted} was not found in the official archive`);
}

function quotePowerShell(value) {
  return `'${value.replaceAll("'", "''")}'`;
}

async function extract(key) {
  const archive = ARCHIVES[key];
  const source = await download(archive);
  const destination = path.join(CACHE_DIR, `extract-${key}`);
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });

  if (archive.name.endsWith(".zip")) {
    const command = [
      "Expand-Archive",
      "-LiteralPath",
      quotePowerShell(source),
      "-DestinationPath",
      quotePowerShell(destination),
      "-Force",
    ].join(" ");
    execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", command], {
      stdio: "inherit",
    });
  } else {
    execFileSync("tar", ["-xzf", source, "-C", destination], { stdio: "inherit" });
  }
  return findBinary(destination, archive.name.endsWith(".zip"));
}

function install(source, targetTriple, windows = false) {
  const extension = windows ? ".exe" : "";
  const destination = path.join(BIN_DIR, `lark-cli-${targetTriple}${extension}`);
  copyFileSync(source, destination);
  if (!windows) chmodSync(destination, 0o755);
  console.log(`[lark-cli] ready ${path.relative(APP_DIR, destination)}`);
  return destination;
}

async function main() {
  if (process.platform === "darwin") {
    const arm = install(await extract("darwin-arm64"), "aarch64-apple-darwin");
    const intel = install(await extract("darwin-amd64"), "x86_64-apple-darwin");
    const universal = path.join(BIN_DIR, "lark-cli-universal-apple-darwin");
    execFileSync("lipo", ["-create", arm, intel, "-output", universal], { stdio: "inherit" });
    chmodSync(universal, 0o755);
    console.log(`[lark-cli] ready ${path.relative(APP_DIR, universal)}`);
    return;
  }

  if (process.platform === "win32" && process.arch === "x64") {
    install(await extract("windows-amd64"), "x86_64-pc-windows-msvc", true);
    return;
  }

  if (process.platform === "linux" && process.arch === "x64") {
    install(await extract("linux-amd64"), "x86_64-unknown-linux-gnu");
    return;
  }

  if (process.platform === "linux" && process.arch === "arm64") {
    install(await extract("linux-arm64"), "aarch64-unknown-linux-gnu");
    return;
  }

  throw new Error(`unsupported lark-cli sidecar platform: ${process.platform}/${process.arch}`);
}

await main();
