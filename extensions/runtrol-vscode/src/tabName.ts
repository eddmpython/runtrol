/// What a conversation tab is called.
///
/// A service that has not named a conversation uses its first prompt, and a first prompt is a paragraph. The
/// whole of it went into the tab, so one conversation took the width of the tab bar and every other tab was
/// pushed out of reach (operator, 2026-08-28, with a picture of one tab holding a paragraph). The sidebar row
/// still carries the full name, which is where a person reads it.
///
/// Its own module because the tab is built with the editor's API and this is not: a name is a string, and a
/// string can be held to its rule without a window.
const TAB_NAME_LIMIT = 24;

export function tabName(title: string): string {
  const name = title.trim().replace(/\s+/gu, " ");
  if (name.length <= TAB_NAME_LIMIT) return name;
  return `${name.slice(0, TAB_NAME_LIMIT - 1).trimEnd()}…`;
}
