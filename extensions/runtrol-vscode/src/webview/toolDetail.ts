import { record, type UnknownRecord } from "./presentation";

/// Bounded because a tool result can be a whole file. The reader opens the file to see the rest, and a panel that
/// grows without limit makes the transcript unscrollable.
const MAX_TOOL_DETAIL = 4000;

/// The names already on the summary line. Repeating them inside the panel is noise.
const ON_THE_SUMMARY = new Set(["title", "name"]);

/// What a tool call carries, laid out the way a person reads it.
///
/// Every key the service sent is shown, in the order it sent them, under the name it gave them. Nothing is
/// renamed, reordered, dropped or summarised, because that would be rewriting a conversation this product only
/// transports. Each service shapes this differently (Claude Code sends `input` and a result, the Agent Client
/// Protocol sends content blocks and locations, Codex sends its own patch shape) and the differences stay visible.
///
/// What changes is only how a value is spelled. This used to be `JSON.stringify`, which writes a newline as the
/// two characters backslash and n: a command's output came out as one line running off the right edge of a
/// panel two hundred pixels wide, which is the opposite of showing what the tool printed. Text is shown as text.
export function toolDetail(body: UnknownRecord): string {
  const payload = record(body.payload);
  if (!payload) return "";
  const shown = Object.entries(payload).filter(([key]) => !ON_THE_SUMMARY.has(key));
  if (shown.length === 0) return "";
  const text = fields(shown, "");
  return text.length > MAX_TOOL_DETAIL ? `${text.slice(0, MAX_TOOL_DETAIL)}\n...` : text;
}

/// A run of named values, one per line, each indented under the name that holds it.
function fields(entries: readonly (readonly [string, unknown])[], indent: string): string {
  return entries.map(([key, value]) => `${indent}${key}:${spelled(value, indent)}`).join("\n");
}

/// One value, after its name.
///
/// A value that needs more than a line opens a block under the name instead of running past the edge, which is
/// the same shape a nested object gets. That keeps a long string and a nested object reading alike.
function spelled(value: unknown, indent: string): string {
  const deeper = `${indent}  `;
  if (typeof value === "string") {
    return value.includes("\n")
      ? `\n${deeper}${value.split("\n").join(`\n${deeper}`)}`
      : ` ${value}`;
  }
  if (Array.isArray(value)) {
    if (value.length === 0) return " (none)";
    return `\n${fields(value.map((item, index) => [String(index), item] as const), deeper)}`;
  }
  const nested = record(value);
  if (nested) {
    const entries = Object.entries(nested);
    if (entries.length === 0) return " (none)";
    return `\n${fields(entries, deeper)}`;
  }
  return ` ${String(value)}`;
}
