#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "..");
const outputPath = path.join(repoRoot, "core/src/agent/model_catalog.generated.json");

const args = new Set(process.argv.slice(2));
const shouldWrite = args.has("--write");
const shouldCheck = args.has("--check");
const noOfficial = args.has("--no-official") || process.env.SOCAI_MODEL_SYNC_OFFLINE === "1";
const noPi = args.has("--no-pi");

const FALLBACK = {
  anthropic: [
    entry("claude-sonnet-5", "Claude Sonnet 5", "fallback", true),
    entry("claude-sonnet-4-6", "Claude Sonnet 4.6"),
    entry("claude-opus-4-8", "Claude Opus 4.8"),
    entry("claude-opus-4-6", "Claude Opus 4.6"),
    entry("claude-haiku-4-5", "Claude Haiku 4.5"),
    entry("claude-3-7-sonnet-20250219", "Claude Sonnet 3.7"),
  ],
  openai: [
    entry("gpt-5.5", "GPT-5.5", "fallback", true),
    entry("gpt-5.4", "GPT-5.4"),
    entry("gpt-5", "GPT-5"),
    entry("gpt-4.1", "GPT-4.1"),
    entry("gpt-4o", "GPT-4o"),
  ],
  kimi: [
    entry("kimi-k2.6", "Kimi K2.6", "fallback", true),
    entry("kimi-k2.7-code", "Kimi K2.7 Code"),
    entry("kimi-k2.5", "Kimi K2.5"),
    entry("kimi-k2-thinking", "Kimi K2 Thinking"),
  ],
  qwen: [
    entry("qwen3.7-plus", "Qwen 3.7 Plus", "fallback", true),
    entry("qwen3.6-plus-2026-04-02", "Qwen 3.6 Plus (2026-04-02)"),
    entry("qwen3.7-max", "Qwen 3.7 Max"),
    entry("qwen3.6-plus", "Qwen 3.6 Plus"),
    entry("qwen3.6-max-preview", "Qwen 3.6 Max Preview"),
  ],
  doubao: [
    entry("doubao-seed-2-1-turbo-260628", "Doubao Seed 2.1 Turbo (260628)", "fallback", true),
    entry("doubao-seed-2-1-pro-260628", "Doubao Seed 2.1 Pro (260628)"),
    entry("doubao-seed-evolving", "Doubao Seed Evolving"),
  ],
  deepseek: [
    entry("deepseek-v4-pro", "DeepSeek V4 Pro", "fallback", true),
    entry("deepseek-v4-flash", "DeepSeek V4 Flash"),
  ],
};

const PROVIDERS = {
  anthropic: {
    env: ["ANTHROPIC_API_KEY"],
    url: "https://api.anthropic.com/v1/models",
    auth: (key) => ({ "x-api-key": key, "anthropic-version": "2023-06-01" }),
    data: (json) => json?.data,
    include: (id) => /^claude-/i.test(id),
    piProviders: ["anthropic"],
  },
  openai: {
    env: ["OPENAI_API_KEY"],
    url: "https://api.openai.com/v1/models",
    auth: bearer,
    data: (json) => json?.data,
    include: (id) => /^(gpt-|o[134]|chatgpt-)/i.test(id),
    exclude: (id) => /(embedding|whisper|tts|dall-e|image|audio|transcribe|moderation|realtime|search|instruct)/i.test(id),
    piProviders: ["openai"],
  },
  kimi: {
    env: ["KIMI_API_KEY", "MOONSHOT_API_KEY"],
    url: "https://api.moonshot.cn/v1/models",
    auth: bearer,
    data: (json) => json?.data,
    include: (id) => /^(kimi-|moonshot-)/i.test(id),
    piProviders: ["moonshotai-cn", "moonshotai"],
  },
  qwen: {
    env: ["QWEN_API_KEY", "DASHSCOPE_API_KEY"],
    url: "https://dashscope.aliyuncs.com/compatible-mode/v1/models",
    auth: bearer,
    data: (json) => json?.data,
    include: (id) => /^(qwen|qwq-|qvq-)/i.test(id),
    piProviders: [],
  },
  doubao: {
    env: ["DOUBAO_API_KEY"],
    url: "https://ark.cn-beijing.volces.com/api/v3/models",
    auth: bearer,
    data: (json) => json?.data,
    include: (id) => /^doubao-/i.test(id),
    piProviders: [],
  },
  deepseek: {
    env: ["DEEPSEEK_API_KEY"],
    url: "https://api.deepseek.com/v1/models",
    auth: bearer,
    data: (json) => json?.data,
    include: (id) => /^deepseek-/i.test(id),
    piProviders: ["deepseek"],
  },
};

function entry(id, displayName = titleFromId(id), source = "fallback", recommended = false) {
  return { id, display_name: displayName, source, ...(recommended ? { recommended: true } : {}) };
}

function bearer(key) {
  return { Authorization: `Bearer ${key}` };
}

function firstEnv(names) {
  for (const name of names) {
    const value = process.env[name]?.trim();
    if (value) return { name, value };
  }
  return undefined;
}

function titleFromId(id) {
  return id
    .replace(/^qwen\/qwen/i, "qwen")
    .replace(/^Qwen\/Qwen/, "Qwen")
    .replace(/^accounts\/fireworks\/models\//, "")
    .replace(/[-_/]+/g, " ")
    .replace(/\bqwen(\d)/gi, "Qwen $1")
    .replace(/\bdoubao\b/gi, "Doubao")
    .replace(/\bseed\b/gi, "Seed")
    .replace(/\bgpt\b/gi, "GPT")
    .replace(/\bkimi\b/gi, "Kimi")
    .replace(/\bdeepseek\b/gi, "DeepSeek")
    .replace(/\bclaude\b/gi, "Claude")
    .replace(/\bopus\b/gi, "Opus")
    .replace(/\bsonnet\b/gi, "Sonnet")
    .replace(/\bhaiku\b/gi, "Haiku")
    .replace(/\bplus\b/gi, "Plus")
    .replace(/\bmax\b/gi, "Max")
    .replace(/\bflash\b/gi, "Flash")
    .replace(/\bturbo\b/gi, "Turbo")
    .replace(/\bpro\b/gi, "Pro")
    .replace(/\bpreview\b/gi, "Preview")
    .replace(/\s+/g, " ")
    .trim();
}

async function fetchOfficial(provider, cfg) {
  if (noOfficial) return [];
  const env = firstEnv(cfg.env);
  if (!env) return [];
  const response = await fetch(cfg.url, { headers: cfg.auth(env.value) });
  if (!response.ok) {
    throw new Error(`${provider}: ${response.status} ${response.statusText} from ${cfg.url}`);
  }
  const payload = await response.json();
  const data = cfg.data(payload);
  if (!Array.isArray(data)) return [];
  return data
    .map((item) => typeof item === "string" ? { id: item } : item)
    .map((item) => ({ id: String(item.id || "").trim(), display_name: item.display_name || item.name }))
    .filter((item) => item.id)
    .filter((item) => !cfg.include || cfg.include(item.id))
    .filter((item) => !cfg.exclude || !cfg.exclude(item.id))
    .map((item) => entry(item.id, item.display_name || titleFromId(item.id), "official"));
}

async function loadPiAi() {
  if (noPi) return undefined;
  const tried = [];
  const tryImport = async (specifier) => {
    try {
      const mod = await import(specifier);
      // pi-ai < 0.80 exports getModels(provider) from the package root;
      // 0.80+ keeps that legacy read on the "compat" entry point.
      if (mod?.getModels) return mod;
      tried.push(`${specifier}: no getModels export`);
      return undefined;
    } catch (error) {
      tried.push(`${specifier}: ${error.message}`);
      return undefined;
    }
  };

  for (const specifier of ["@earendil-works/pi-ai", "@earendil-works/pi-ai/compat"]) {
    const direct = await tryImport(specifier);
    if (direct) return direct;
  }

  let globalRoot;
  try {
    globalRoot = execFileSync("npm", ["root", "-g"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    globalRoot = "";
  }

  const packageRoots = [
    path.join(repoRoot, "app/node_modules/@earendil-works/pi-ai"),
    globalRoot && path.join(globalRoot, "@earendil-works/pi-ai"),
    globalRoot && path.join(globalRoot, "@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai"),
    "/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/node_modules/@earendil-works/pi-ai",
  ].filter(Boolean);

  for (const root of packageRoots) {
    for (const file of ["dist/index.js", "dist/compat.js"]) {
      const candidate = path.join(root, file);
      if (!existsSync(candidate)) continue;
      const imported = await tryImport(pathToFileURL(candidate).href);
      if (imported) return imported;
    }
  }
  return undefined;
}

function piEntriesForProvider(pi, provider, cfg) {
  if (!pi?.getModels) return [];
  if (provider === "qwen") return piQwenEntries(pi);
  const out = [];
  for (const piProvider of cfg.piProviders ?? []) {
    let models = [];
    try {
      models = pi.getModels(piProvider) || [];
    } catch {
      continue;
    }
    for (const model of models) {
      const id = String(model.id || "").trim();
      if (!id) continue;
      if (cfg.include && !cfg.include(id)) continue;
      if (cfg.exclude && cfg.exclude(id)) continue;
      out.push(entry(id, model.name || titleFromId(id), "pi-ai"));
    }
    if (out.length > 0) break;
  }
  return out;
}

function piQwenEntries(pi) {
  const providers = ["opencode-go", "opencode", "together", "openrouter", "fireworks", "vercel-ai-gateway"];
  const out = [];
  for (const provider of providers) {
    let models = [];
    try {
      models = pi.getModels(provider) || [];
    } catch {
      continue;
    }
    for (const model of models) {
      const normalized = normalizeQwenDashScopeId(model.id, model.name);
      if (!normalized) continue;
      out.push(entry(normalized, titleFromId(normalized), "pi-ai"));
    }
  }
  return out;
}

function normalizeQwenDashScopeId(id, name = "") {
  const haystack = `${id} ${name}`.toLowerCase();
  const candidates = [
    ["qwen3.7-plus", [/qwen3[.\- ]?7[.\- ]?plus/, /qwen3p7[.\- ]?plus/]],
    ["qwen3.7-max", [/qwen3[.\- ]?7[.\- ]?max/]],
    ["qwen3.6-plus", [/qwen3[.\- ]?6[.\- ]?plus/]],
    ["qwen3.6-max-preview", [/qwen3[.\- ]?6[.\- ]?max[.\- ]?preview/]],
    ["qwen3.6-flash", [/qwen3[.\- ]?6[.\- ]?flash/]],
  ];
  for (const [normalized, patterns] of candidates) {
    if (patterns.some((pattern) => pattern.test(haystack))) return normalized;
  }
  return undefined;
}

function markRecommended(provider, models) {
  const fallbackDefault = FALLBACK[provider]?.find((item) => item.recommended)?.id;
  return models.map((item) => ({ ...item, ...(item.id === fallbackDefault ? { recommended: true } : {}) }));
}

function dedupe(entries) {
  const seen = new Set();
  const out = [];
  for (const item of entries) {
    const id = item.id.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    const clean = { id, display_name: item.display_name || titleFromId(id), source: item.source || "unknown" };
    if (item.recommended) clean.recommended = true;
    out.push(clean);
  }
  return out;
}

function sortEntries(provider, entries) {
  return [...entries].sort((a, b) => compareNewerFirst(provider, a, b));
}

function compareNewerFirst(provider, a, b) {
  const aKey = modelFreshnessKey(provider, a.id);
  const bKey = modelFreshnessKey(provider, b.id);
  const version = compareNumberArraysDesc(aKey.version, bKey.version);
  if (version !== 0) return version;
  if (aKey.date !== bKey.date) return bKey.date - aKey.date;
  if (aKey.tier !== bKey.tier) return bKey.tier - aKey.tier;
  if (!!a.recommended !== !!b.recommended) return a.recommended ? -1 : 1;
  return b.id.localeCompare(a.id, undefined, { numeric: true, sensitivity: "base" });
}

function modelFreshnessKey(provider, id) {
  const lower = id.toLowerCase();
  return {
    version: versionParts(provider, lower),
    date: dateScore(lower),
    tier: tierScore(lower),
  };
}

function versionParts(provider, lower) {
  if (provider === "openai") {
    const oSeries = lower.match(/^o(\d+)/);
    if (oSeries) return [4, 50 + (parseInt(oSeries[1], 10) || 0)];
    if (/^gpt-4o/.test(lower)) return [4, 40];
  }
  const matchers = {
    anthropic: /claude-(?:[a-z]+-)*(\d+)(?:[-.](\d+))?/,
    openai: /gpt-(\d+)(?:[.-](\d+|o))?/,
    kimi: /kimi-k(\d+)(?:[.-](\d+))?/,
    qwen: /qwen(\d+)(?:[.-](\d+))?/,
    doubao: /doubao-seed-(\d+)(?:[.-](\d+))?/,
    deepseek: /deepseek-v(\d+)(?:[.-](\d+))?/,
  };
  const match = lower.match(matchers[provider]) || lower.match(/(\d+)(?:[.-](\d+))?/);
  if (!match) return [0];
  const major = parseInt(match[1], 10) || 0;
  const rawMinor = match[2];
  const minor = rawMinor && !looksLikeDateToken(rawMinor)
    ? (rawMinor === "o" ? 9 : parseInt(rawMinor, 10) || 0)
    : 0;
  return [major, minor];
}

function looksLikeDateToken(token) {
  return /^\d{4,}$/.test(token);
}

function dateScore(lower) {
  const dashed = lower.match(/(20\d{2})-(\d{2})-(\d{2})/);
  if (dashed) return parseInt(`${dashed[1]}${dashed[2]}${dashed[3]}`, 10);
  const compact = lower.match(/\b(20\d{6})\b/);
  if (compact) return parseInt(compact[1], 10);
  const mmdd = lower.match(/-(\d{4})-(?:preview|latest|turbo|$)/);
  if (mmdd) return parseInt(`2000${mmdd[1]}`, 10);
  return 0;
}

function tierScore(lower) {
  if (/(max|opus|pro)/.test(lower)) return 50;
  if (/(sonnet|plus)/.test(lower)) return 40;
  if (/(turbo|thinking|codex)/.test(lower)) return 30;
  if (/(mini|haiku|flash)/.test(lower)) return 20;
  if (/(nano)/.test(lower)) return 10;
  return 0;
}

function compareNumberArraysDesc(a, b) {
  const len = Math.max(a.length, b.length);
  for (let i = 0; i < len; i += 1) {
    const diff = (b[i] || 0) - (a[i] || 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

async function buildCatalog() {
  const pi = await loadPiAi();
  const providers = {};
  for (const [provider, cfg] of Object.entries(PROVIDERS)) {
    const official = [];
    try {
      official.push(...await fetchOfficial(provider, cfg));
    } catch (error) {
      console.warn(`[model-sync] ${error.message}`);
    }
    const piEntries = piEntriesForProvider(pi, provider, cfg);
    const merged = dedupe([...official, ...piEntries, ...(FALLBACK[provider] || [])]);
    providers[provider] = sortEntries(provider, markRecommended(provider, merged));
    const sourceSummary = [...new Set(providers[provider].map((item) => item.source))].join(", ");
    console.error(`[model-sync] ${provider}: ${providers[provider].length} models (${sourceSummary})`);
  }
  return { version: 1, providers };
}

const catalog = await buildCatalog();
const rendered = `${JSON.stringify(catalog, null, 2)}\n`;

if (shouldCheck) {
  const existing = existsSync(outputPath) ? readFileSync(outputPath, "utf8") : "";
  if (existing !== rendered) {
    console.error(`[model-sync] ${path.relative(repoRoot, outputPath)} is out of date. Run: node scripts/sync-model-catalog.mjs --write`);
    process.exit(1);
  }
  console.error(`[model-sync] ${path.relative(repoRoot, outputPath)} is up to date`);
} else if (shouldWrite) {
  writeFileSync(outputPath, rendered);
  console.error(`[model-sync] wrote ${path.relative(repoRoot, outputPath)}`);
} else {
  process.stdout.write(rendered);
  console.error("[model-sync] preview only; pass --write to update the generated catalog");
}
