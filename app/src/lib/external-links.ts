import { invoke } from "@tauri-apps/api/core";

/** Hand web links to the system browser before other UI click handlers run. */
export function bindExternalLinks(): void {
  document.addEventListener("click", (event) => {
    const anchor = event.composedPath()
      .find((item): item is HTMLAnchorElement => item instanceof HTMLAnchorElement);
    if (!anchor) return;
    const href = anchor.getAttribute("href") ?? "";
    if (!/^https?:\/\//i.test(href)) return;
    event.preventDefault();
    invoke("open_external", { url: href }).catch((error) => {
      console.error("open_external failed:", error);
    });
  }, true);
}
