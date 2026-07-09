//! Composer send shortcut: enter (or cmd/ctrl+enter) sends, shift+enter is a
//! newline. One definition so the keydown check and the hover hint can't drift.
//!
//! Two guards keep the IME's confirm-enter from sending — important since the
//! UI defaults to zh. `isComposing` covers Chromium, where the confirming enter
//! still reports it. But Tauri on macOS runs WKWebView, which fires
//! `compositionend` *before* the keydown, so `isComposing` is already false by
//! then; there `keyCode === 229` is the sentinel that the key is still being
//! consumed by the IME. Checking both catches the confirm-enter on either
//! engine. (`keyCode` is deprecated but still populated and is the standard
//! IME guard used by CodeMirror/ProseMirror for exactly this reason.)

export const sendShortcutLabel = "enter";

export function isSendShortcut(e: KeyboardEvent): boolean {
  return e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229;
}
