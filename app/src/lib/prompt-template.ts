import { esc } from "./html";
import { getLanguage } from "./i18n";
import { nextPromptField, promptFields, type PromptField } from "./prompt-template-model";

let dispose: (() => void) | undefined;
let boundInput: HTMLTextAreaElement | undefined;

export function focusPromptField(input: HTMLTextAreaElement, field = promptFields(input.value)[0]): void {
  input.focus();
  if (field) input.setSelectionRange(field.start, field.end);
}

/** Keep the actual textarea, IME, undo, voice and submission path intact. */
export function bindPromptTemplate(input: HTMLTextAreaElement): void {
  if (boundInput === input && input.isConnected) return;
  dispose?.();
  boundInput = input;
  const chinese = getLanguage() === "zh";
  const wrapper = document.createElement("div");
  wrapper.className = "prompt-template";
  input.before(wrapper);
  wrapper.append(input);
  const mirror = document.createElement("div");
  mirror.className = "prompt-template__mirror";
  mirror.setAttribute("aria-hidden", "true");
  const controls = document.createElement("div");
  controls.className = "prompt-template__fields";
  controls.id = "composer-template-fields";
  controls.setAttribute("role", "group");
  controls.setAttribute("aria-label", chinese ? "待填写的内容" : "Fields to complete");
  wrapper.append(mirror, controls);
  const priorDescription = input.getAttribute("aria-describedby");
  const sync = (): void => {
    const fields = promptFields(input.value);
    input.setCustomValidity("");
    controls.hidden = fields.length === 0;
    if (fields.length) input.setAttribute("aria-describedby", [priorDescription, controls.id].filter(Boolean).join(" "));
    else if (priorDescription) input.setAttribute("aria-describedby", priorDescription);
    else input.removeAttribute("aria-describedby");
    let cursor = 0;
    mirror.innerHTML = fields.map(field => {
      const before = esc(input.value.slice(cursor, field.start));
      cursor = field.end;
      return before + `<mark>${esc(input.value.slice(field.start, field.end))}</mark>`;
    }).join("") + esc(input.value.slice(cursor)) + "\n";
    mirror.hidden = !fields.length;
    const style = getComputedStyle(input);
    for (const property of ["font", "letter-spacing", "line-height", "padding", "text-indent", "text-align", "tab-size", "word-break", "overflow-wrap"])
      mirror.style.setProperty(property, style.getPropertyValue(property));
    mirror.style.width = `${input.clientWidth}px`;
    mirror.style.height = `${input.clientHeight}px`;
    mirror.scrollTop = input.scrollTop;
    controls.innerHTML = fields.length ? `<span>${chinese ? "填写" : "Fill in"}</span>` + fields.map((field, index) =>
      `<button type="button" tabindex="-1" data-prompt-field="${index}">${esc(field.label)}</button>`).join("")
      + `<span class="prompt-template__hint"><kbd>Tab</kbd> ${chinese ? "下一项" : "next field"}</span>` : "";
  };
  const choose = (field: PromptField): void => focusPromptField(input, field);
  controls.addEventListener("click", event => {
    const button = (event.target as Element).closest<HTMLButtonElement>("[data-prompt-field]");
    const field = button && promptFields(input.value)[Number(button.dataset.promptField)];
    if (field) choose(field);
  });
  const keydown = (event: KeyboardEvent): void => {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === "Tab" && !event.altKey && !event.ctrlKey && !event.metaKey) {
      const field = nextPromptField(input.value, input.selectionStart, input.selectionEnd, event.shiftKey);
      if (field) { event.preventDefault(); choose(field); }
    }
  };
  const click = (): void => {
    if (input.selectionStart !== input.selectionEnd) return;
    const field = promptFields(input.value).find(field => input.selectionStart >= field.start && input.selectionStart < field.end);
    if (field) choose(field);
  };
  const form = input.closest("form");
  const submit = (event: Event): void => {
    const first = promptFields(input.value)[0];
    if (!first) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    choose(first);
    input.setCustomValidity(chinese ? `请先填写：${first.label}` : `Please complete: ${first.label}`);
    input.reportValidity();
  };
  input.addEventListener("input", sync);
  input.addEventListener("scroll", sync);
  input.addEventListener("keydown", keydown);
  input.addEventListener("click", click);
  form?.addEventListener("submit", submit, true);
  const observer = new ResizeObserver(sync);
  observer.observe(input);
  sync();
  dispose = () => {
    observer.disconnect();
    input.removeEventListener("input", sync);
    input.removeEventListener("scroll", sync);
    input.removeEventListener("keydown", keydown);
    input.removeEventListener("click", click);
    form?.removeEventListener("submit", submit, true);
  };
}
