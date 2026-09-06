import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, open, rename, rm, stat } from "node:fs/promises";
import { Readable } from "node:stream";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const REPO_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ARCHIVE_DIR = path.join(REPO_DIR, "target", "sherpa-onnx-archives");
const RELEASE_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.7";

const ARCHIVES = {
  "aarch64-apple-darwin": {
    name: "sherpa-onnx-v1.13.7-osx-arm64-static-lib.tar.bz2",
    bytes: 20_339_537,
    sha256: "126daa2e8c09a4c5d54dc985722c43bd22f598adc56445905b377454b1b27e38",
  },
  "x86_64-apple-darwin": {
    name: "sherpa-onnx-v1.13.7-osx-x64-static-lib.tar.bz2",
    bytes: 20_017_522,
    sha256: "8d8db6199af0119b16f6e2b01c5548b90c4392a462388dd59491e8c471283cca",
  },
  "x86_64-pc-windows-msvc": {
    name: "sherpa-onnx-v1.13.7-win-x64-static-MT-Release-lib.tar.bz2",
    bytes: 120_228_352,
    sha256: "04734146fb3a21a297604c586ea826346dbb167c19b9ccc79c1f85d39f490395",
  },
};

function rustHost() {
  const output = execFileSync("rustc", ["-vV"], { cwd: REPO_DIR, encoding: "utf8" });
  const line = output.split("\n").find((item) => item.startsWith("host: "));
  if (!line) throw new Error("rustc -vV did not report a host target");
  return line.slice("host: ".length).trim();
}

async function sha256(filePath) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(filePath)) hash.update(chunk);
  return hash.digest("hex");
}

async function archiveIsValid(filePath, expected) {
  try {
    const metadata = await stat(filePath);
    return metadata.size === expected.bytes && (await sha256(filePath)) === expected.sha256;
  } catch {
    return false;
  }
}

async function downloadArchive(target) {
  const expected = ARCHIVES[target];
  if (!expected) throw new Error(`unsupported sherpa-onnx target: ${target}`);
  const destination = path.join(ARCHIVE_DIR, expected.name);
  if (await archiveIsValid(destination, expected)) {
    console.log(`[sherpa-onnx] verified ${expected.name}`);
    return;
  }

  await rm(destination, { force: true });
  const partial = `${destination}.part-${process.pid}`;
  await rm(partial, { force: true });
  const response = await fetch(`${RELEASE_URL}/${expected.name}`, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`failed to download ${expected.name}: HTTP ${response.status}`);
  }
  const contentLength = Number(response.headers.get("content-length"));
  if (Number.isFinite(contentLength) && contentLength !== expected.bytes) {
    throw new Error(
      `size header mismatch for ${expected.name}: expected ${expected.bytes}, got ${contentLength}`,
    );
  }

  const file = await open(partial, "wx");
  const hash = createHash("sha256");
  let downloaded = 0;
  try {
    for await (const chunk of Readable.fromWeb(response.body)) {
      downloaded += chunk.length;
      if (downloaded > expected.bytes) {
        throw new Error(`download exceeded pinned size for ${expected.name}`);
      }
      hash.update(chunk);
      await file.write(chunk);
    }
    await file.sync();
  } catch (error) {
    await file.close();
    await rm(partial, { force: true });
    throw error;
  }
  await file.close();

  const digest = hash.digest("hex");
  if (downloaded !== expected.bytes || digest !== expected.sha256) {
    await rm(partial, { force: true });
    throw new Error(
      `verification failed for ${expected.name}: ${downloaded} bytes, sha256 ${digest}`,
    );
  }
  await rename(partial, destination);
  console.log(`[sherpa-onnx] downloaded and verified ${expected.name}`);
}

export async function prepareSherpaArchives(targets) {
  await mkdir(ARCHIVE_DIR, { recursive: true });
  for (const target of [...new Set(targets)]) await downloadArchive(target);
}

const invokedDirectly = process.argv[1]
  && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  const targets = process.argv.slice(2);
  await prepareSherpaArchives(targets.length > 0 ? targets : [rustHost()]);
}
