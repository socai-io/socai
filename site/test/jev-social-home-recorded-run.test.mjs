import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("links the recorded Jev Social run from the localized homepage", async () => {
  const index = await read("src/pages/index.astro");
  const replayUrl = "https://socai-io.github.io/jev-social/recorded-run/";

  assert.equal(index.split(replayUrl).length - 1, 1);
  assert.match(index, /replay: "watch the recorded run"/u);
  assert.match(index, /replay: "观看录制回放"/u);
  assert.match(
    index,
    /href="https:\/\/socai-io\.github\.io\/jev-social\/recorded-run\/"[\s\S]*?data-i18n="project\.replay"[\s\S]*?>\{message\("project\.replay"\)\}<\/a/u,
  );
});
