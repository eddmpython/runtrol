import { record, string, type UnknownRecord } from "./presentation";

/// One line describing what an agent is doing to the project.
export type ToolActivity = {
  /// What the provider says the tool does, in a word a person reads.
  readonly verb: string;
  /// What it is doing it to, when the provider names a target.
  readonly target: string;
  /// Where it is up to.
  readonly state: "pending" | "running" | "done" | "failed" | "cancelled" | "unknown";
};

/// The provider's own classification, spelled for a reader.
///
/// Runtrol never infers a kind from a tool name, because a name-to-kind table is stale the first time a vendor
/// renames a tool. A provider that does not classify its tools gets the neutral word, and that asymmetry stays
/// visible instead of being papered over with a guess.
const VERBS: Record<string, string> = {
  read: "Read",
  edit: "Edit",
  delete: "Delete",
  move: "Move",
  search: "Search",
  execute: "Run",
  think: "Think",
  fetch: "Fetch",
  switchMode: "Switch mode",
  other: "Tool",
};

const STATES: Record<string, ToolActivity["state"]> = {
  pending: "pending",
  inProgress: "running",
  completed: "done",
  failed: "failed",
  cancelled: "cancelled",
};

/// What a tool call is doing, from the fields Runtime lifted plus the labels providers agree on.
///
/// Two names are read out of the payload and nothing else. `title` is the human label the Agent Client Protocol
/// standardises. `name` is the tool's own name, which is all Claude Code reports, and it is read second so a
/// service that gives both keeps its label.
///
/// Neither is provider-specific parsing: both are names a service chose to put in its own frame. What stays unread
/// is everything else in the payload. Raw input, raw output, diffs and terminal bytes are the conversation, and
/// reaching into them to compose a nicer label would be interpreting what no service offered for display. That is
/// the same move that made a wrapper unmaintainable the first time a vendor changed an argument shape.
export function toolActivityOf(body: UnknownRecord): ToolActivity {
  const kind = string(body.kind);
  const status = string(body.status);
  const payload = record(body.payload);
  return {
    verb: VERBS[kind] ?? "Tool",
    target: (string(payload?.title).trim() || string(payload?.name).trim()),
    state: STATES[status] ?? "unknown",
  };
}

/// The one line shown in the thread.
///
/// A service that classified its tool gets "verb target". A service that only named the tool gets that name alone,
/// because `Tool Read` puts a filler word in front of the only real information on the line. The neutral verb exists
/// for the service that gave nothing at all.
export function toolActivityLine(activity: ToolActivity): string {
  const head = activity.target
    ? activity.verb === VERBS.other
      ? activity.target
      : `${activity.verb} ${activity.target}`
    : activity.verb;
  switch (activity.state) {
    case "failed":
      return `${head} · failed`;
    case "cancelled":
      return `${head} · cancelled`;
    case "running":
      return `${head}...`;
    case "pending":
      return `${head} · queued`;
    case "done":
    case "unknown":
      return head;
  }
}
