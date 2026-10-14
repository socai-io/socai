import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(
  new URL("../scripts/assert-vercel-project.mjs", import.meta.url),
);

function run(env = {}) {
  return spawnSync(process.execPath, [script], {
    encoding: "utf8",
    env: { PATH: process.env.PATH ?? "", ...env },
  });
}

test("Vercel project guard permits local and website project builds", () => {
  assert.equal(run().status, 0);
  assert.equal(
    run({
      VERCEL: "1",
      VERCEL_PROJECT_ID: "prj_bI9zARRkb95vSg2I1cR3IA9DaVpJ",
    }).status,
    0,
  );
});

test("Vercel project guard rejects a website build in the Agent project", () => {
  const result = run({
    VERCEL: "1",
    VERCEL_PROJECT_ID: "prj_rK1FFQ6n7Q1HNlzaIhjkaYellIWC",
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Refusing to build socai-site/u);
});

test("Vercel project guard rejects builds without a project identity", () => {
  const result = run({ VERCEL: "1" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /VERCEL_PROJECT_ID is unavailable/u);
});
