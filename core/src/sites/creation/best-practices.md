# Site development

Extractor and result rules. The procedure is in [SKILL.md](./SKILL.md).

Use the real snapshot DOM and follow the path a person would: find the visible control, click it, read the overlay, then close it. Open a URL directly only when the page has no such control.

A shell or the first result row does not mean comments, counts, or media URLs are ready. Wait until that region is stable, or the page shows an empty state, before reading or closing. Counts used to decide the wait stay in the host loop.

A success result is the record itself. Drop `ok`, wrappers, always-false gates, duplicate ids, and parent URLs repeated on every child. Nested items keep only what the next task needs — a comment is text, likes, and reply count. Keep long fields that are still usable, such as a downloadable video URL. Return the reason only on failure.
