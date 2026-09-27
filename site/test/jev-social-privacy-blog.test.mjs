import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test, { before } from "node:test";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");
const siteRoot = fileURLToPath(new URL("..", import.meta.url));

before(() => {
  execFileSync("pnpm", ["build"], {
    cwd: siteRoot,
    encoding: "utf8",
    stdio: "pipe",
  });
});

test("publishes the Jev Social privacy guide in both languages", async () => {
  const [english, chinese, index, llms] = await Promise.all([
    read("src/pages/blog/jev-social-privacy-boundary.md"),
    read("src/pages/blog/zh/jev-social-privacy-boundary.md"),
    read("src/pages/blog/index.astro"),
    read("public/llms.txt"),
  ]);

  assert.match(english, /href: \/blog\/zh\/jev-social-privacy-boundary/u);
  assert.match(chinese, /href: \/blog\/jev-social-privacy-boundary/u);

  for (const content of [english, chinese]) {
    assert.match(content, /https:\/\/github\.com\/socai-io\/jev-social/u);
    assert.match(content, /https:\/\/socai-io\.github\.io\/jev-social\/privacy\//u);
    assert.match(content, /SOCAI_TELEMETRY=0/u);
    assert.match(content, /OPENROUTER_REPORT_MODEL=off/u);
    assert.match(content, /https:\/\/openrouter\.ai\/api\/v1\/auth\/key/u);
    assert.match(content, /OPENROUTER_API_KEY/u);
    assert.match(content, /TYPESAFE_API_KEY/u);
    assert.match(content, /SOCAI_API_KEY/u);
    assert.match(content, /JEV_SOCIAL_HOME/u);
    assert.match(content, /config\.json/u);
    assert.match(content, /downloadMedia: true/u);
    assert.doesNotMatch(content, /fully offline|完全离线/iu);
  }

  assert.match(english, /not a filesystem sandbox/u);
  assert.match(english, /there is no second confirmation/u);
  assert.match(english, /Neither store has automatic cleanup/u);
  assert.match(chinese, /不是文件系统沙箱/u);
  assert.match(chinese, /下载、保存、留存、归档或抓取视频、媒体或文件/u);
  assert.doesNotMatch(chinese, /录制媒体|保留本地副本/u);
  assert.match(chinese, /不会再出现第二次确认/u);
  assert.match(chinese, /没有自动清理/u);

  assert.match(index, /href: "\/blog\/jev-social-privacy-boundary"/u);
  assert.match(index, /href: "\/blog\/zh\/jev-social-privacy-boundary"/u);
  assert.match(llms, /https:\/\/socai\.io\/blog\/jev-social-privacy-boundary\//u);
  assert.match(llms, /https:\/\/socai\.io\/blog\/zh\/jev-social-privacy-boundary\//u);
});

test("renders discoverable canonical and localized pages", async () => {
  const [english, chinese, index, llms, sitemap] = await Promise.all([
    read("dist/blog/jev-social-privacy-boundary/index.html"),
    read("dist/blog/zh/jev-social-privacy-boundary/index.html"),
    read("dist/blog/index.html"),
    read("dist/llms.txt"),
    read("dist/sitemap-0.xml"),
  ]);

  assert.match(english, /<html lang="en"/u);
  assert.match(english, /rel="canonical" href="https:\/\/socai\.io\/blog\/jev-social-privacy-boundary"/u);
  assert.match(english, /hreflang="zh" href="https:\/\/socai\.io\/blog\/zh\/jev-social-privacy-boundary"/u);
  assert.match(english, /"@type":"FAQPage"/u);

  assert.match(chinese, /<html lang="zh-CN"/u);
  assert.match(chinese, /rel="canonical" href="https:\/\/socai\.io\/blog\/zh\/jev-social-privacy-boundary"/u);
  assert.match(chinese, /hreflang="en" href="https:\/\/socai\.io\/blog\/jev-social-privacy-boundary"/u);
  assert.match(chinese, /"@type":"FAQPage"/u);

  assert.match(index, /href="\/blog\/jev-social-privacy-boundary"/u);
  assert.match(index, /href="\/blog\/zh\/jev-social-privacy-boundary"/u);

  for (const rendered of [llms, sitemap]) {
    assert.match(rendered, /https:\/\/socai\.io\/blog\/jev-social-privacy-boundary\//u);
    assert.match(rendered, /https:\/\/socai\.io\/blog\/zh\/jev-social-privacy-boundary\//u);
  }
});
