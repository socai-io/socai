let converter: Promise<(text: string) => string> | undefined;

/** Loaded while recording, never from a third-party CDN or an LLM rewrite. */
export function prepareVoiceTranscript(language: string): void {
  if (!isSimplified(language)) return;
  void getConverter().catch(() => { /* A later transcription can retry loading. */ });
}

export async function normalizeVoiceTranscript(text: string, language: string): Promise<string> {
  const trimmed = text.trim();
  if (!isSimplified(language)) return trimmed;
  const convert = await getConverter();
  // Do not alter identifiers, URLs, email addresses or quoted code.
  return trimmed.split(/(https?:\/\/[^\s]+|[\w.+-]+@[\w.-]+\.[a-z]{2,}|`[^`]*`)/gi)
    .map((part, index) => index % 2 ? part : convert(part)).join("");
}

function isSimplified(language: string): boolean {
  return /^(zh|zh-cn|zh-hans(?:-.+)?)$/i.test(language);
}

function getConverter(): Promise<(text: string) => string> {
  converter ??= import("opencc-js/t2cn")
    .then(({ Converter }) => Converter({ from: "t", to: "cn" }))
    .catch((error: unknown) => { converter = undefined; throw error; });
  return converter;
}
