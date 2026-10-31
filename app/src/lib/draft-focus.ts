// Keep user editing state across asynchronous full-shell updates. Never copy
// values into a different task or override focus chosen by a dialog binding.
type DraftField = HTMLInputElement | HTMLTextAreaElement;
let listenersInstalled = false;
let composing: Element | null = null;
let pendingRender: (() => void) | undefined;

function draftField(element: Element | null): element is DraftField {
  return (element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement)
    && (element.id === "composer-input"
      || (!!element.closest("[data-research-starter-form]")
        && ["target", "context", "count"].includes(element.name)));
}

export function deferDraftRender(render: () => void): boolean {
  if (!listenersInstalled) {
    listenersInstalled = true;
    document.addEventListener("compositionstart", event => {
      if (event.target instanceof Element && draftField(event.target)) composing = event.target;
    }, true);
    document.addEventListener("compositionend", event => {
      if (event.target !== composing) return;
      composing = null;
      const queued = pendingRender;
      pendingRender = undefined;
      // Let the final input event persist the committed IME text first.
      if (queued) window.setTimeout(queued, 0);
    }, true);
  }
  if (composing?.isConnected) {
    pendingRender = render;
    return true;
  }
  composing = null;
  pendingRender = undefined;
  return false;
}

export function preserveDraftFocus(root: HTMLElement): () => void {
  const previous = document.activeElement;
  if (!draftField(previous) || !root.contains(previous)) return () => {};
  const form = previous.closest<HTMLFormElement>("form");
  if (!form) return () => {};
  const composer = previous.id === "composer-input";
  const context = composer
    ? `${form.dataset.composerMode}:${form.dataset.composerTaskId || ""}`
    : form.dataset.researchStarterContext;
  if (!context) return () => {};
  const name = previous.name;
  const value = previous.value;
  const start = previous.selectionStart, end = previous.selectionEnd;
  const direction = previous.selectionDirection;
  const top = previous.scrollTop, left = previous.scrollLeft;
  return () => {
    // Bindings may intentionally focus a modal or a new control.
    if (document.activeElement && document.activeElement !== document.body
      && document.activeElement !== root) return;
    const nextForm = root.querySelector<HTMLFormElement>(
      composer ? "#composer-form" : "[data-research-starter-form]");
    if (!nextForm) return;
    const nextContext = composer
      ? `${nextForm.dataset.composerMode}:${nextForm.dataset.composerTaskId || ""}`
      : nextForm.dataset.researchStarterContext;
    if (context !== nextContext) return;
    const next = composer
      ? nextForm.querySelector<HTMLTextAreaElement>("#composer-input")
      : nextForm.elements.namedItem(name);
    if (!draftField(next instanceof Element ? next : null)) return;
    const field = next as DraftField;
    if (field.disabled || field.value !== value) return;
    field.focus({preventScroll: true});
    if (typeof start === "number" && typeof end === "number") {
      field.setSelectionRange(start, end, direction || undefined);
    }
    field.scrollTop = top; field.scrollLeft = left;
  };
}
