/** Grow a textarea without collapsing its live layout on every keystroke. */
export function bindTextareaAutosize(input: HTMLTextAreaElement): () => void {
  // Measuring the live input at height:auto also resizes the conversation's
  // flex viewport. Use an isolated, non-interactive copy for that measurement.
  const measure = input.cloneNode(false) as HTMLTextAreaElement;
  measure.removeAttribute("id");
  measure.removeAttribute("name");
  measure.removeAttribute("placeholder");
  measure.tabIndex = -1;
  measure.setAttribute("aria-hidden", "true");
  measure.inert = true;
  measure.style.cssText = `
    position: fixed; top: 0; left: 0; visibility: hidden;
    pointer-events: none; contain: strict; height: 0;
    min-height: 0; max-height: none; overflow: hidden;
  `;
  document.body.append(measure);

  const style = getComputedStyle(input);
  const padding = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
  const border = parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
  const borderBox = style.boxSizing === "border-box";
  const minHeight = parseFloat(style.minHeight) || 0;
  const maxHeight = parseFloat(style.maxHeight) || Infinity;
  let width = input.getBoundingClientRect().width;
  let frame = 0;
  let disposed = false;

  const resize = (): void => {
    frame = 0;
    measure.style.width = `${width}px`;
    measure.style.boxSizing = "border-box";
    measure.value = input.value;
    const height = Math.min(maxHeight, Math.max(
      minHeight,
      measure.scrollHeight + (borderBox ? border : -padding),
    ));
    const next = `${height}px`;
    if (input.style.height !== next) input.style.height = next;
  };
  const schedule = (): void => {
    if (!disposed && !frame) frame = requestAnimationFrame(resize);
  };
  const observer = new ResizeObserver(() => {
    const next = input.getBoundingClientRect().width;
    if (next === width) return;
    width = next;
    schedule();
  });
  observer.observe(input);
  input.addEventListener("input", schedule);
  document.fonts.addEventListener("loadingdone", schedule);
  resize();

  return () => {
    disposed = true;
    cancelAnimationFrame(frame);
    observer.disconnect();
    input.removeEventListener("input", schedule);
    document.fonts.removeEventListener("loadingdone", schedule);
    measure.remove();
  };
}
