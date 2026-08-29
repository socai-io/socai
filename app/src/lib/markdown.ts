import { marked } from "marked";
import DOMPurify from "dompurify";

// Markdown → sanitized HTML for agent final answers. `marked` handles GFM
// (tables, strikethrough, task lists, autolinks, fenced code, …) and
// `DOMPurify` strips anything unsafe from the result before it reaches the DOM.

marked.setOptions({ gfm: true, breaks: true });

// Preserve `note:<id>` citation links (a custom scheme the answer upgrades into
// embedded note pills). DOMPurify's default URI allowlist would strip them.
DOMPurify.addHook("uponSanitizeAttribute", (_node, data) => {
  if (data.attrName === "href" && data.attrValue.startsWith("note:")) {
    data.forceKeepAttr = true;
  }
});

// Open links in a new tab/window and harden against reverse-tabnabbing. Runs
// after sanitization so the attributes we add are not themselves stripped.
// `note:` links are left as-is — the answer renderer upgrades them.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A" && !(node.getAttribute("href") || "").startsWith("note:")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

export function renderMarkdown(src: string): string {
  const html = marked.parse(src, { async: false });
  return DOMPurify.sanitize(html);
}

/** Artifact previews never auto-load media from generated Markdown. External
 * links remain visible and still require an explicit user click. */
export function renderArtifactMarkdown(src: string): string {
  const html = marked.parse(src, { async: false });
  return DOMPurify.sanitize(html, {
    // Keep this HTML-only. DOMPurify's broad defaults include SVG/MathML and
    // style-bearing elements whose href/url() values can fetch remote content
    // even when ordinary image/media tags are forbidden.
    ALLOWED_TAGS: [
      "a", "blockquote", "br", "code", "del", "em", "h1", "h2", "h3", "h4", "h5", "h6",
      "hr", "input", "li", "ol", "p", "pre", "strong", "table", "tbody", "td", "th", "thead",
      "tr", "ul",
    ],
    ALLOWED_ATTR: [
      "checked", "class", "colspan", "disabled", "href", "rowspan", "scope", "title", "type",
    ],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
  });
}
