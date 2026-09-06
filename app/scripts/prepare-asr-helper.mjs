import { chmodSync, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const APP_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO_DIR = path.resolve(APP_DIR, "..");
const BIN_DIR = path.join(APP_DIR, "src-tauri", "binaries");
const explicitTarget = process.env.TAURI_ENV_TARGET_TRIPLE;
const release = process.env.TAURI_ENV_DEBUG === "false" || explicitTarget === "universal-apple-darwin";
const profile = release ? "release" : "debug";

mkdirSync(BIN_DIR, { recursive: true });

function rustHost() {
  const output = execFileSync("rustc", ["-vV"], { cwd: REPO_DIR, encoding: "utf8" });
  const line = output.split("\n").find((item) => item.startsWith("host: "));
  if (!line) throw new Error("rustc -vV did not report a host target");
  return line.slice("host: ".length).trim();
}

function build(target) {
  const args = ["build", "-p", "socai-asr", "--target", target];
  if (release) args.push("--release");
  execFileSync("cargo", args, {
    cwd: REPO_DIR,
    stdio: "inherit",
    env: {
      ...process.env,
      SHERPA_ONNX_ARCHIVE_DIR: path.join(REPO_DIR, "target", "sherpa-onnx-archives"),
    },
  });
  const extension = target.includes("windows") ? ".exe" : "";
  const source = path.join(REPO_DIR, "target", target, profile, `socai-asr${extension}`);
  if (!existsSync(source)) throw new Error(`ASR helper build missing: ${source}`);
  const destination = path.join(BIN_DIR, `socai-asr-${target}${extension}`);
  copyFileSync(source, destination);
  if (!extension) chmodSync(destination, 0o755);
  console.log(`[socai-asr] ready ${path.relative(APP_DIR, destination)}`);
  return destination;
}

const target = explicitTarget || rustHost();
if (target === "universal-apple-darwin") {
  execFileSync(
    process.execPath,
    [path.join(REPO_DIR, "scripts", "prepare-sherpa-onnx-libs.mjs"), "aarch64-apple-darwin", "x86_64-apple-darwin"],
    { cwd: REPO_DIR, stdio: "inherit" },
  );
  const arm = build("aarch64-apple-darwin");
  const intel = build("x86_64-apple-darwin");
  const universal = path.join(BIN_DIR, "socai-asr-universal-apple-darwin");
  execFileSync("lipo", ["-create", arm, intel, "-output", universal], { stdio: "inherit" });
  chmodSync(universal, 0o755);
  console.log(`[socai-asr] ready ${path.relative(APP_DIR, universal)}`);
} else {
  execFileSync(
    process.execPath,
    [path.join(REPO_DIR, "scripts", "prepare-sherpa-onnx-libs.mjs"), target],
    { cwd: REPO_DIR, stdio: "inherit" },
  );
  build(target);
}
