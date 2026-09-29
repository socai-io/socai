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

test("pins the current Jev Social runtime across official entrypoints", async () => {
  const [readme, index, english, chinese] = await Promise.all([
    readFile(new URL("../../README.md", import.meta.url), "utf8"),
    read("src/pages/index.astro"),
    read("src/pages/blog/jev-social-media-automation.md"),
    read("src/pages/blog/zh/jev-social-media-automation.md"),
  ]);

  for (const content of [readme, index, english, chinese]) {
    assert.doesNotMatch(content, /(?:v0\.1\.10|Version 0\.1\.10)/u);
  }
  assert.match(readme, /Version 0\.1\.13/u);
  assert.match(readme, /Node 22 and 24/u);
  assert.match(readme, /tree\/v0\.1\.13\/skills\/jev-social/u);
  assert.equal(index.match(/jev-social#v0\.1\.13/gu)?.length, 2);
  for (const article of [english, chinese]) {
    assert.doesNotMatch(article, /Node 20\+/u);
    assert.match(article, /Node 22\+/u);
    assert.equal(article.match(/jev-social#v0\.1\.13/gu)?.length, 2);
  }
});
