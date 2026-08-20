/// The composer's @-mention: a way to type a file's path without typing the path.
///
/// Nothing here interprets anything. The picker lives in the Extension Host (it is the one who may
/// list workspace files), and what comes back is inserted as plain text where the @ was typed. What
/// an @path means is the coding service's own business, exactly like a slash command's argument.

/// Whether the character just typed at `caret` is an @ starting a word.
///
/// Word-start only, because an @ inside an email address or a code span is content, not a request.
export function mentionTriggered(value: string, caret: number): boolean {
  if (caret < 1 || value[caret - 1] !== "@") return false;
  return caret === 1 || /\s/u.test(value[caret - 2] ?? "");
}

/// Replace the mention token under `caret` (the word-starting @ and anything typed after it) with `text`.
///
/// When no such token exists (the composer changed while the picker was open), the text is inserted
/// at the caret instead of guessing at a replacement.
export function insertMention(
  value: string,
  caret: number,
  text: string,
): { value: string; caret: number } {
  const bounded = Math.max(0, Math.min(caret, value.length));
  const head = value.slice(0, bounded);
  const start = head.lastIndexOf("@");
  const wordStart = start >= 0
    && (start === 0 || /\s/u.test(head[start - 1] ?? ""))
    && !/\s/u.test(head.slice(start + 1));
  const from = wordStart ? start : bounded;
  const next = value.slice(0, from) + text + value.slice(bounded);
  return { value: next, caret: from + text.length };
}
