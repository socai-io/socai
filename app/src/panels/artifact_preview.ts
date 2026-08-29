import Papa from "papaparse";

import type { AgentArtifact } from "../main";
import { esc } from "../lib/html";
import { t } from "../lib/i18n";
import { renderArtifactMarkdown } from "../lib/markdown";
import type { ArtifactDownloadState } from "./conversation";

export interface ArtifactPreviewPaneState {
  taskId: string;
  path: string;
  name: string;
  kind: string;
  sizeBytes: number;
  version: string;
  previewKind: NonNullable<AgentArtifact["preview_kind"]>;
  status: "loading" | "ready" | "error";
  text?: string;
  blobUrl?: string;
  error?: string;
  sheetIndex?: number;
}

interface SpreadsheetPreviewPayload {
  sheets: Array<{
    name: string;
    rows: string[][];
    truncated: boolean;
  }>;
  sheet_count: number;
  truncated: boolean;
}

export function renderArtifactPreview(
  preview: ArtifactPreviewPaneState | null,
  downloadState: ArtifactDownloadState | undefined,
  width: number,
): string {
  if (!preview) return "";
  const openingState = downloadState?.status === "downloaded"
    || downloadState?.status === "opening"
    || downloadState?.status === "open_failed";
  const downloadLabel = downloadState?.status === "downloading"
    ? t("artifact.downloadingAria", { name: preview.name })
    : downloadState?.status === "download_failed"
      ? t("artifact.downloadFailedAria", { name: preview.name })
      : downloadState?.status === "opening"
        ? t("artifact.openingAria", { name: preview.name })
        : downloadState?.status === "open_failed"
          ? t("artifact.openFailedAria", { name: preview.name })
          : openingState
            ? t("artifact.openAria", { name: preview.name })
            : t("artifact.downloadAria", { name: preview.name });
  return `
    <aside class="artifact-preview" style="--artifact-preview-width: ${width}px" aria-label="${esc(t("artifact.previewPanelAria", { name: preview.name }))}">
      <div
        class="artifact-preview__resize"
        data-artifact-preview-resize
        role="separator"
        aria-orientation="vertical"
        aria-label="${esc(t("artifact.previewResize"))}"
        aria-valuemin="320"
        aria-valuemax="1200"
        aria-valuenow="${width}"
        tabindex="0"
      ></div>
      <header class="artifact-preview__head">
        <div class="artifact-preview__identity">
          <span class="artifact-preview__name" title="${esc(preview.name)}">${esc(preview.name)}</span>
          <span class="artifact-preview__meta">${esc(preview.kind)} · ${esc(formatArtifactSize(preview.sizeBytes))}</span>
        </div>
        <div class="artifact-preview__actions">
          <button
            type="button"
            class="artifact-preview__action${downloadState?.status === "open_failed" || downloadState?.status === "download_failed" ? " is-error" : ""}"
            data-artifact-action="${esc(preview.taskId)}"
            data-artifact-path="${esc(preview.path)}"
            title="${esc(downloadLabel)}"
            aria-label="${esc(downloadLabel)}"
            ${downloadState?.status === "downloading" || downloadState?.status === "opening" ? 'aria-disabled="true" aria-busy="true"' : ""}
          >${downloadIcon()}</button>
          <button type="button" class="artifact-preview__action" data-artifact-preview-close aria-label="${esc(t("artifact.previewClose"))}" title="${esc(t("artifact.previewClose"))}">${closeIcon()}</button>
        </div>
      </header>
      <div class="artifact-preview__body">
        ${renderPreviewBody(preview)}
      </div>
    </aside>
  `;
}

function renderPreviewBody(preview: ArtifactPreviewPaneState): string {
  if (preview.status === "loading") {
    return `<div class="artifact-preview__state"><span class="spinner" aria-hidden="true"></span>${esc(t("artifact.previewLoading"))}</div>`;
  }
  if (preview.status === "error") {
    return `<div class="artifact-preview__state artifact-preview__state--error"><span>${esc(t("artifact.previewFailed"))}</span><small>${esc(preview.error ?? "")}</small></div>`;
  }
  if (preview.previewKind === "markdown") {
    return `<article class="artifact-preview__markdown result-md">${renderArtifactMarkdown(preview.text ?? "")}</article>`;
  }
  if (preview.previewKind === "csv") {
    return renderCsv(preview.text ?? "", preview.kind === "TSV" ? "\t" : ",");
  }
  if (preview.previewKind === "text") {
    return `<pre class="artifact-preview__text">${esc(formatTextPreview(preview.name, preview.text ?? ""))}</pre>`;
  }
  if (preview.previewKind === "spreadsheet") {
    return renderSpreadsheet(preview.text ?? "", preview.sheetIndex ?? 0);
  }
  if (preview.previewKind === "pdf" && preview.blobUrl) {
    return `<object class="artifact-preview__pdf" data="${esc(preview.blobUrl)}" type="application/pdf"><p>${esc(t("artifact.previewPdfUnavailable"))}</p></object>`;
  }
  if (preview.previewKind === "image" && preview.blobUrl) {
    return `<div class="artifact-preview__image-wrap"><img class="artifact-preview__image" src="${esc(preview.blobUrl)}" alt="${esc(preview.name)}" /></div>`;
  }
  return `<div class="artifact-preview__state artifact-preview__state--error">${esc(t("artifact.previewFailed"))}</div>`;
}

export function artifactPreviewMime(name: string, kind: ArtifactPreviewPaneState["previewKind"]): string {
  if (kind === "pdf") return "application/pdf";
  if (kind !== "image") return "text/plain;charset=utf-8";
  const extension = name.split(".").pop()?.toLowerCase();
  if (extension === "png") return "image/png";
  if (extension === "jpg" || extension === "jpeg") return "image/jpeg";
  if (extension === "gif") return "image/gif";
  if (extension === "webp") return "image/webp";
  if (extension === "bmp") return "image/bmp";
  return "application/octet-stream";
}

function renderCsv(text: string, delimiter: string): string {
  const maxRows = 500;
  const maxColumns = 80;
  const maxParseCharacters = 128 * 1024;
  const maxCellCharacters = 8 * 1024;
  const parseText = text.slice(0, maxParseCharacters);
  const parsed = Papa.parse<string[]>(parseText, {
    delimiter,
    skipEmptyLines: false,
    // Stop before newline-heavy files allocate an array for every row. The
    // character cap also bounds work for one delimiter-heavy row.
    preview: maxRows + 1,
  });
  let truncated = text.length > maxParseCharacters || parsed.data.length > maxRows;
  const rows = parsed.data.slice(0, maxRows).map((row) => {
    if (row.length > maxColumns) truncated = true;
    return row.slice(0, maxColumns).map((cell) => {
      if (cell.length <= maxCellCharacters) return cell;
      truncated = true;
      return `${cell.slice(0, maxCellCharacters)}…`;
    });
  });
  const columnCount = Math.min(
    maxColumns,
    rows.reduce((maximum, row) => Math.max(maximum, row.length), 0),
  );
  return renderArtifactTable(
    rows,
    columnCount,
    truncated ? t("artifact.previewTableLimit", { rows: maxRows, columns: maxColumns }) : "",
  );
}

function renderSpreadsheet(text: string, selectedSheetIndex: number): string {
  const workbook = parseSpreadsheetPreview(text);
  if (!workbook) {
    return `<div class="artifact-preview__state artifact-preview__state--error">${esc(t("artifact.previewFailed"))}</div>`;
  }
  if (workbook.sheets.length === 0) {
    return `<div class="artifact-preview__state">${esc(t("artifact.previewWorkbookEmpty"))}</div>`;
  }
  const activeIndex = Math.min(Math.max(selectedSheetIndex, 0), workbook.sheets.length - 1);
  const activeSheet = workbook.sheets[activeIndex];
  const columnCount = Math.min(
    80,
    activeSheet.rows.reduce((maximum, row) => Math.max(maximum, row.length), 0),
  );
  const tabs = workbook.sheets.map((sheet, index) => `<button
    type="button"
    id="artifact-preview-sheet-tab-${index}"
    class="artifact-preview__workbook-tab${index === activeIndex ? " is-active" : ""}"
    data-artifact-preview-sheet="${index}"
    role="tab"
    aria-selected="${index === activeIndex ? "true" : "false"}"
    aria-controls="artifact-preview-sheet-panel"
    tabindex="${index === activeIndex ? "0" : "-1"}"
  >${esc(sheet.name)}</button>`).join("");
  const shown = workbook.sheets.length;
  const workbookLimit = workbook.truncated
    ? `<p class="artifact-preview__workbook-limit">${esc(t("artifact.previewWorkbookLimit", { shown, total: workbook.sheet_count }))}</p>`
    : "";
  const worksheetLimit = activeSheet.truncated
    ? t("artifact.previewWorksheetLimit")
    : "";
  return `<div class="artifact-preview__workbook">
    <div class="artifact-preview__workbook-tabs" role="tablist" aria-label="${esc(t("artifact.previewWorkbookSheets"))}">${tabs}</div>
    <section
      id="artifact-preview-sheet-panel"
      class="artifact-preview__workbook-sheet"
      role="tabpanel"
      aria-labelledby="artifact-preview-sheet-tab-${activeIndex}"
    >${renderArtifactTable(activeSheet.rows, columnCount, worksheetLimit)}</section>
    ${workbookLimit}
  </div>`;
}

function parseSpreadsheetPreview(text: string): SpreadsheetPreviewPayload | null {
  try {
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== "object") return null;
    const candidate = value as Partial<SpreadsheetPreviewPayload>;
    if (!Array.isArray(candidate.sheets)
      || !Number.isSafeInteger(candidate.sheet_count)
      || (candidate.sheet_count ?? -1) < 0
      || typeof candidate.truncated !== "boolean") return null;
    if (!candidate.sheets.every((sheet) => sheet
      && typeof sheet.name === "string"
      && typeof sheet.truncated === "boolean"
      && Array.isArray(sheet.rows)
      && sheet.rows.every((row) => Array.isArray(row) && row.every((cell) => typeof cell === "string")))) {
      return null;
    }
    return candidate as SpreadsheetPreviewPayload;
  } catch {
    return null;
  }
}

function renderArtifactTable(rows: string[][], columnCount: number, limitMessage: string): string {
  const body = rows.map((row, rowIndex) => `
    <tr>
      <th class="artifact-preview__row-number" scope="row">${rowIndex + 1}</th>
      ${Array.from({ length: columnCount }, (_, columnIndex) => {
        const tag = rowIndex === 0 ? "th" : "td";
        const scope = rowIndex === 0 ? " scope=\"col\"" : "";
        return `<${tag}${scope}>${esc(row[columnIndex] ?? "")}</${tag}>`;
      }).join("")}
    </tr>
  `).join("");
  return `
    <div class="artifact-preview__sheet">
      <table><tbody>${body}</tbody></table>
      ${limitMessage ? `<p class="artifact-preview__limit">${esc(limitMessage)}</p>` : ""}
    </div>
  `;
}

function formatTextPreview(name: string, text: string): string {
  if (!/\.json$/i.test(name)) return text;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

export function formatArtifactSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

const artifactIconLabels: Record<string, string> = {
  json: "JSON", jsonl: "JSON",
  csv: "CSV", tsv: "CSV",
  xls: "XLS", xlsx: "XLS", xlsm: "XLS", ods: "XLS",
  doc: "DOC", docx: "DOC", odt: "DOC", rtf: "DOC",
  pdf: "PDF",
  ppt: "PPT", pptx: "PPT", odp: "PPT",
  md: "MD", markdown: "MD",
  png: "IMG", jpg: "IMG", jpeg: "IMG", gif: "IMG", webp: "IMG", bmp: "IMG", svg: "IMG",
  zip: "ZIP", "7z": "ZIP", rar: "ZIP", tar: "ZIP", gz: "ZIP",
};

export function artifactFileIcon(name: string): string {
  const extensionIndex = name.lastIndexOf(".");
  const extension = extensionIndex > 0 ? name.slice(extensionIndex + 1).toLowerCase() : "";
  const label = artifactIconLabels[extension] ?? "FILE";
  return `<svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 2.5h9l5 5v14H5z"></path><path d="M14 2.5v5h5"></path><text x="12" y="17.5" fill="currentColor" stroke="none" text-anchor="middle" font-family="ui-monospace, monospace" font-size="5.2" font-weight="700">${label}</text></svg>`;
}

export function eyeIcon(): string {
  return `<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M2.5 12s3.4-5.5 9.5-5.5 9.5 5.5 9.5 5.5-3.4 5.5-9.5 5.5S2.5 12 2.5 12z"></path><circle cx="12" cy="12" r="2.4"></circle></svg>`;
}

export function downloadIcon(): string {
  return `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v11"></path><path d="m8 10 4 4 4-4"></path><path d="M5 17v3h14v-3"></path></svg>`;
}

function closeIcon(): string {
  return `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="m6 6 12 12M18 6 6 18"></path></svg>`;
}
