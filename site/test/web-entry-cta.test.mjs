import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  LANGUAGE_OPTIONS,
  choosePreferredLanguage,
  resolvePageLanguage,
  withLanguage,
} from "../src/lib/languages.ts";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("homepage promotes the canonical Agent Web experience before desktop downloads", async () => {
  const [page, header, runtime, styles, showcaseStyles] = await Promise.all([
    read("src/pages/index.astro"),
    read("src/components/SiteHeader.astro"),
    read("src/scripts/site.ts"),
    read("src/styles/global.css"),
    read("src/styles/creator-showcase.css"),
  ]);

  assert.match(page, /const webDemoUrl = "https:\/\/agent\.socai\.io"/u);
  assert.match(page, /const bookDemoUrl = "https:\/\/agent\.socai\.io\/book-demo"/u);
  assert.match(page, /demo: "try now"/u);
  assert.match(page, /demo: "立即体验"/u);
  assert.ok(
    page.indexOf('class="hero__actions"') < page.indexOf('class="hero__resources"'),
    "Primary Web actions must appear before secondary download links",
  );
  assert.doesNotMatch(page, /data-i18n="hero\.github"/u);
  assert.match(page, /data-platform-icon="macos"/u);
  assert.match(page, /data-platform-icon="windows"/u);
  assert.doesNotMatch(page, /<span>macOS<\/span>|<span>Windows<\/span>/u);
  assert.match(styles, /\.hero__actions \.button[\s\S]*?min-width:\s*180px/u);

  for (const language of ["zh", "en", "ja", "ko", "es", "fr", "de", "pt"]) {
    assert.match(page, new RegExp(`^    ${language}: \\{`, "mu"));
    assert.ok(LANGUAGE_OPTIONS.some(({ value }) => value === language));
  }
  assert.match(header, /data-language-select/u);
  assert.match(runtime, /choosePreferredLanguage/u);
  assert.match(runtime, /navigator\.languages/u);
  assert.match(runtime, /data-language-link/u);
  assert.match(showcaseStyles, /:not\(\[data-language="zh"\]\)/u);
});

test("language preference survives fallback pages and keeps outbound URLs aligned", () => {
  const languages = LANGUAGE_OPTIONS.map(({ value }) => value);
  const preferred = choosePreferredLanguage({
    search: "?campaign=launch&lang=ja",
    storedLanguage: "de",
    browserLanguages: ["fr-FR"],
  });

  assert.equal(preferred, "ja", "URL language must win over stored and browser preferences");
  assert.equal(resolvePageLanguage(preferred, languages), "ja");
  assert.equal(
    resolvePageLanguage(preferred, ["zh", "en"]),
    "en",
    "A page fallback must not change the preferred language",
  );

  const switched = withLanguage("https://socai.io/?campaign=launch#hero", "de");
  assert.equal(switched.searchParams.get("campaign"), "launch");
  assert.equal(switched.searchParams.get("lang"), "de");
  assert.equal(switched.hash, "#hero");
  assert.equal(withLanguage("https://agent.socai.io", "de").toString(), "https://agent.socai.io/?lang=de");
  assert.equal(withLanguage("https://agent.socai.io/book-demo", "de").toString(), "https://agent.socai.io/book-demo?lang=de");
});
