import {
  createLibrary,
  createParser,
  defineComponent,
  type ElementNode,
} from "@openuidev/lang-core";
import { z } from "zod/v4";

import { esc } from "./html";

const MAX_SOURCE_CHARACTERS = 256 * 1024;
const MAX_RENDER_NODES = 500;
const MAX_RENDER_DEPTH = 12;
const MAX_TABLE_ROWS = 200;
const MAX_TABLE_COLUMNS = 24;
const MAX_CHART_ITEMS = 40;

const childList = z.array(z.any()).max(MAX_RENDER_NODES);
const tone = z.enum(["neutral", "positive", "warning", "critical"]);

const Stack = defineComponent({
  name: "Stack",
  description: "Vertical layout for report sections.",
  props: z.object({
    children: childList,
    gap: z.enum(["compact", "normal", "relaxed"]).optional(),
  }),
  component: null,
});

const Columns = defineComponent({
  name: "Columns",
  description: "Responsive columns for related report content.",
  props: z.object({ children: childList }),
  component: null,
});

const Section = defineComponent({
  name: "Section",
  description: "A bordered report section with an optional subtitle.",
  props: z.object({
    title: z.string().max(200),
    subtitle: z.string().max(500).optional(),
    children: childList,
  }),
  component: null,
});

const Heading = defineComponent({
  name: "Heading",
  description: "A report heading.",
  props: z.object({
    text: z.string().max(500),
    level: z.number().int().min(1).max(3).optional(),
  }),
  component: null,
});

const Text = defineComponent({
  name: "Text",
  description: "A paragraph of report text.",
  props: z.object({
    text: z.string().max(10_000),
    tone: z.enum(["default", "muted"]).optional(),
  }),
  component: null,
});

const Metric = defineComponent({
  name: "Metric",
  description: "One prominent metric with supporting context.",
  props: z.object({
    label: z.string().max(120),
    value: z.string().max(120),
    detail: z.string().max(300).optional(),
    tone: tone.optional(),
  }),
  component: null,
});

const MetricGrid = defineComponent({
  name: "MetricGrid",
  description: "A responsive group of Metric components.",
  props: z.object({ children: childList }),
  component: null,
});

const DataTable = defineComponent({
  name: "DataTable",
  description: "A compact table for comparable findings.",
  props: z.object({
    columns: z.array(z.string().max(200)).max(MAX_TABLE_COLUMNS),
    rows: z.array(z.array(z.string().max(5_000)).max(MAX_TABLE_COLUMNS)).max(MAX_TABLE_ROWS),
    caption: z.string().max(300).optional(),
  }),
  component: null,
});

const BarChart = defineComponent({
  name: "BarChart",
  description: "A horizontal bar chart for non-negative values.",
  props: z.object({
    title: z.string().max(200),
    items: z.array(z.object({
      label: z.string().max(200),
      value: z.number().finite().nonnegative(),
      display: z.string().max(120).optional(),
    })).max(MAX_CHART_ITEMS),
  }),
  component: null,
});

const Callout = defineComponent({
  name: "Callout",
  description: "A highlighted conclusion, caveat, or recommendation.",
  props: z.object({
    title: z.string().max(200),
    body: z.string().max(5_000),
    tone: tone.optional(),
  }),
  component: null,
});

const SourceLink = defineComponent({
  name: "SourceLink",
  description: "A cited HTTP(S) source.",
  props: z.object({
    label: z.string().max(300),
    url: z.string().max(2_048),
    description: z.string().max(1_000).optional(),
  }),
  component: null,
});

const generativeUiLibrary = createLibrary({
  id: "socai-desktop-report@1",
  root: "Stack",
  components: [
    Stack,
    Columns,
    Section,
    Heading,
    Text,
    Metric,
    MetricGrid,
    DataTable,
    BarChart,
    Callout,
    SourceLink,
  ],
});

const parser = createParser(generativeUiLibrary.toJSONSchema(), generativeUiLibrary.root);

export function renderGenerativeUi(source: string): string {
  if (source.length > MAX_SOURCE_CHARACTERS) {
    throw new Error("Generative UI source exceeds the preview limit");
  }
  const result = parser.parse(source);
  const errors = result.meta.errors.filter((error) => error.severity !== "warning");
  if (!result.root || errors.length > 0 || result.meta.unresolved.length > 0) {
    const detail = errors[0]?.message
      ?? (result.meta.unresolved.length > 0 ? `Unresolved component: ${result.meta.unresolved[0]}` : "Missing root component");
    throw new Error(detail);
  }
  const budget = { nodes: 0 };
  return `<article class="generative-ui" data-openui-library="socai-desktop-report@1">${renderNode(result.root, 0, budget)}</article>`;
}

function renderNode(node: ElementNode, depth: number, budget: { nodes: number }): string {
  budget.nodes += 1;
  if (budget.nodes > MAX_RENDER_NODES || depth > MAX_RENDER_DEPTH) {
    throw new Error("Generative UI exceeds the render complexity limit");
  }
  const props = node.props;
  switch (node.typeName) {
    case "Stack":
      return `<div class="generative-ui__stack generative-ui__stack--${enumValue(props.gap, ["compact", "normal", "relaxed"], "normal")}">${renderChildren(props.children, depth, budget)}</div>`;
    case "Columns":
      return `<div class="generative-ui__columns">${renderChildren(props.children, depth, budget)}</div>`;
    case "Section":
      return `<section class="generative-ui__section"><header><h2>${esc(textValue(props.title))}</h2>${props.subtitle ? `<p>${esc(textValue(props.subtitle))}</p>` : ""}</header>${renderChildren(props.children, depth, budget)}</section>`;
    case "Heading": {
      const level = numberValue(props.level, 2, 1, 3);
      return `<h${level} class="generative-ui__heading generative-ui__heading--${level}">${esc(textValue(props.text))}</h${level}>`;
    }
    case "Text":
      return `<p class="generative-ui__text${props.tone === "muted" ? " is-muted" : ""}">${esc(textValue(props.text))}</p>`;
    case "Metric":
      return `<div class="generative-ui__metric" data-tone="${toneValue(props.tone)}"><span>${esc(textValue(props.label))}</span><strong>${esc(textValue(props.value))}</strong>${props.detail ? `<small>${esc(textValue(props.detail))}</small>` : ""}</div>`;
    case "MetricGrid":
      return `<div class="generative-ui__metrics">${renderChildren(props.children, depth, budget)}</div>`;
    case "DataTable":
      return renderTable(props);
    case "BarChart":
      return renderBarChart(props);
    case "Callout":
      return `<aside class="generative-ui__callout" data-tone="${toneValue(props.tone)}"><strong>${esc(textValue(props.title))}</strong><p>${esc(textValue(props.body))}</p></aside>`;
    case "SourceLink":
      return renderSourceLink(props);
    default:
      throw new Error(`Unsupported Generative UI component: ${node.typeName}`);
  }
}

function renderChildren(value: unknown, depth: number, budget: { nodes: number }): string {
  if (!Array.isArray(value)) return "";
  return value.map((child) => {
    if (!isElementNode(child)) throw new Error("Invalid Generative UI child");
    return renderNode(child, depth + 1, budget);
  }).join("");
}

function renderTable(props: Record<string, unknown>): string {
  const columns = stringList(props.columns, MAX_TABLE_COLUMNS);
  const rows = Array.isArray(props.rows) ? props.rows.slice(0, MAX_TABLE_ROWS) : [];
  const header = columns.map((column) => `<th scope="col">${esc(column)}</th>`).join("");
  const body = rows.map((row) => {
    const cells = stringList(row, columns.length);
    return `<tr>${columns.map((_, index) => `<td>${esc(cells[index] ?? "")}</td>`).join("")}</tr>`;
  }).join("");
  return `<figure class="generative-ui__table">${props.caption ? `<figcaption>${esc(textValue(props.caption))}</figcaption>` : ""}<div><table><thead><tr>${header}</tr></thead><tbody>${body}</tbody></table></div></figure>`;
}

function renderBarChart(props: Record<string, unknown>): string {
  const rawItems = Array.isArray(props.items) ? props.items.slice(0, MAX_CHART_ITEMS) : [];
  const items = rawItems.flatMap((item) => {
    if (!item || typeof item !== "object") return [];
    const candidate = item as Record<string, unknown>;
    const value = typeof candidate.value === "number" && Number.isFinite(candidate.value)
      ? Math.max(candidate.value, 0)
      : 0;
    return [{
      label: textValue(candidate.label),
      value,
      display: candidate.display ? textValue(candidate.display) : String(value),
    }];
  });
  const maximum = Math.max(1, ...items.map((item) => item.value));
  const bars = items.map((item) => {
    const width = Math.max(item.value > 0 ? 2 : 0, Math.min(100, (item.value / maximum) * 100));
    return `<div class="generative-ui__bar-row"><span title="${esc(item.label)}">${esc(item.label)}</span><div class="generative-ui__bar-track"><i style="width:${width.toFixed(2)}%"></i></div><strong>${esc(item.display)}</strong></div>`;
  }).join("");
  return `<figure class="generative-ui__chart"><figcaption>${esc(textValue(props.title))}</figcaption>${bars}</figure>`;
}

function renderSourceLink(props: Record<string, unknown>): string {
  const label = textValue(props.label);
  const url = safeHttpUrl(props.url);
  if (!url) {
    return `<div class="generative-ui__source is-disabled"><strong>${esc(label)}</strong>${props.description ? `<span>${esc(textValue(props.description))}</span>` : ""}</div>`;
  }
  return `<a class="generative-ui__source" href="${esc(url)}"><strong>${esc(label)}</strong>${props.description ? `<span>${esc(textValue(props.description))}</span>` : ""}<i aria-hidden="true">&#8599;</i></a>`;
}

function isElementNode(value: unknown): value is ElementNode {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<ElementNode>;
  return candidate.type === "element"
    && typeof candidate.typeName === "string"
    && !!candidate.props
    && typeof candidate.props === "object";
}

function textValue(value: unknown): string {
  return typeof value === "string" ? value : String(value ?? "");
}

function stringList(value: unknown, maximum: number): string[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, maximum).map(textValue);
}

function numberValue(value: unknown, fallback: number, minimum: number, maximum: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(maximum, Math.max(minimum, Math.round(value)));
}

function enumValue<T extends string>(value: unknown, values: readonly T[], fallback: T): T {
  return typeof value === "string" && values.includes(value as T) ? value as T : fallback;
}

function toneValue(value: unknown): "neutral" | "positive" | "warning" | "critical" {
  return enumValue(value, ["neutral", "positive", "warning", "critical"], "neutral");
}

function safeHttpUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.href : null;
  } catch {
    return null;
  }
}
