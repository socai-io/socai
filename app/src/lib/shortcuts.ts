//! Composer send shortcut: cmd+enter on macOS, ctrl+enter elsewhere. Plain
//! enter stays a newline (textarea default). One definition so the keydown
//! check and the hover hint can't drift apart.

const isMac = navigator.userAgent.includes("Macintosh");

export const sendShortcutLabel = isMac ? "⌘ + enter" : "ctrl + enter";

export function isSendShortcut(e: KeyboardEvent): boolean {
  return e.key === "Enter" && (isMac ? e.metaKey : e.ctrlKey);
}
