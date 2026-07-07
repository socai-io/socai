//! Composer send shortcut: enter (or cmd/ctrl+enter) sends, shift+enter is a
//! newline. One definition so the keydown check and the hover hint can't drift.
//!
//! `isComposing` guards the IME: while composing a pinyin/kana candidate the
//! enter that confirms it must NOT send — important since the UI defaults to zh.

export const sendShortcutLabel = "enter";

export function isSendShortcut(e: KeyboardEvent): boolean {
  return e.key === "Enter" && !e.shiftKey && !e.isComposing;
}
