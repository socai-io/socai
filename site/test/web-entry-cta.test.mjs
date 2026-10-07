import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("homepage promotes the canonical Agent Web experience before desktop downloads", async () => {
  const page = await read("src/pages/index.astro");

  assert.match(page, /const webDemoUrl = "https:\/\/agent\.socai\.io"/u);
  assert.match(page, /const bookDemoUrl = "https:\/\/agent\.socai\.io\/book-demo"/u);
  assert.match(page, /demo: "try now"/u);
  assert.match(page, /demo: "立即体验"/u);
  assert.ok(
    page.indexOf('class="hero__actions"') < page.indexOf('class="hero__resources"'),
    "Primary Web actions must appear before secondary download links",
  );
});
