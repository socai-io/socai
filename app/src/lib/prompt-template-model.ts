export interface PromptField { label: string; start: number; end: number }

/** Explicit template syntax only; ordinary citations and quoted headings remain text. */
export function promptFields(value: string): PromptField[] {
  return [...value.matchAll(/【(你的[^【】\n]{1,46})】|\[\[(Your [^\[\]\n]{1,43})\]\]/gu)]
    .map(match => ({ label: match[1] ?? match[2], start: match.index!, end: match.index! + match[0].length }));
}

export function nextPromptField(value: string, start: number, end: number, backwards = false): PromptField | undefined {
  const fields = promptFields(value);
  const current = fields.findIndex(field => start >= field.start && end <= field.end);
  if (current >= 0) return fields[current + (backwards ? -1 : 1)];
  return backwards
    ? [...fields].reverse().find(field => field.end <= start)
    : fields.find(field => field.start >= end);
}
