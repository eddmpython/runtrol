import { record, string, type UnknownRecord } from "./presentation";

/// One command the coding service itself offers, as the service names it.
export type SlashCommand = {
  /// The name without its leading slash, exactly as the service spelled it.
  readonly name: string;
  /// The service's own one-line description, when it gave one.
  readonly description: string;
};

/// The most commands one service may offer.
///
/// A bound rather than a preference: this list arrives from a provider and is drawn on every keystroke, so an
/// unbounded one would be a provider deciding how much work the composer does.
const MAX_COMMANDS = 200;

/// The most candidates shown at once.
///
/// A menu longer than this is a list to read rather than a choice to make, and the reader can always type
/// another letter.
const MAX_VISIBLE = 8;

/// The commands a service just announced, read out of its own update.
///
/// Reads `name` and `description` and nothing else. A command's argument schema is the service's business:
/// interpreting it would mean Runtrol deciding what a command means, and the whole value of passing a slash
/// command through untouched is that the service decides.
///
/// Every coding CLI that has slash commands has a `/model` among them. That is the answer to changing a model
/// mid-conversation: the CLI's own command, in the CLI's own words, taking effect in the CLI's own state.
/// Runtrol adding a parallel model-switching call would create a second place that opinion lives, and the two
/// would disagree the moment somebody typed the command instead of using the button.
export function slashCommandsOf(body: UnknownRecord): SlashCommand[] {
  const payload = record(body.payload);
  const announced = payload?.availableCommands ?? payload?.commands ?? payload?.slash_commands;
  if (!Array.isArray(announced)) return [];
  const commands: SlashCommand[] = [];
  const seen = new Set<string>();
  for (const entry of announced.slice(0, MAX_COMMANDS)) {
    // A service may announce a bare string or a described object. Both are the same fact.
    const name = (typeof entry === "string" ? entry : string(record(entry)?.name)).trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    commands.push({
      name,
      description: typeof entry === "string" ? "" : string(record(entry)?.description).trim(),
    });
  }
  return commands;
}

/// Whether what is typed is asking for the command menu.
///
/// Only a slash that opens the message. A slash later in a sentence is part of a path or a fraction, and
/// popping a menu there interrupts somebody who is writing prose.
export function asksForCommands(text: string): boolean {
  return text.startsWith("/") && !text.slice(1).includes(" ");
}

/// The candidates for what is typed, best match first.
///
/// A prefix match ranks above a match anywhere else, because the reader is typing the beginning of a name and
/// expects what they typed to lead.
export function matchingCommands(
  commands: readonly SlashCommand[],
  text: string,
): SlashCommand[] {
  if (!asksForCommands(text)) return [];
  const typed = text.slice(1).toLowerCase();
  if (typed.length === 0) return commands.slice(0, MAX_VISIBLE);
  const starts: SlashCommand[] = [];
  const contains: SlashCommand[] = [];
  for (const command of commands) {
    const name = command.name.toLowerCase();
    if (name.startsWith(typed)) {
      starts.push(command);
    } else if (name.includes(typed)) {
      contains.push(command);
    }
  }
  return [...starts, ...contains].slice(0, MAX_VISIBLE);
}

/// What the composer should hold once a candidate is chosen.
///
/// A trailing space, because every one of these commands either takes an argument or ignores one, and the
/// person who wanted an argument would otherwise have to add the space themselves.
export function completed(command: SlashCommand): string {
  return `/${command.name} `;
}

/// Where the highlight moves for an arrow key, wrapping at both ends.
///
/// Wrapping because a menu of at most eight items is a ring, not a page: reaching the bottom and pressing down
/// again should return to the top rather than do nothing.
export function movedHighlight(current: number, count: number, delta: number): number {
  if (count === 0) return 0;
  return (((current + delta) % count) + count) % count;
}
