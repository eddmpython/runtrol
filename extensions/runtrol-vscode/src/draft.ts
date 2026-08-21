import { workspaceName } from "./sessionDisplay";

/// A conversation that has not started yet: the tab is open, the chips are set, nothing has been said.
///
/// This is how every chat app people already use begins a conversation (the operator named the Codex app's
/// screen as the reference: a greeting, a composer, and chips for the project, the service, the model, the
/// effort and the access mode). Nothing here costs a process: the coding CLI is started by the first message,
/// with exactly these choices, and the tab then becomes that session's tab. Until then the draft is page
/// memory plus this record, and the record holds only choices, never text.
export type DraftState = {
  /// Stable while the tab lives, legal as a map key and as webview state.
  readonly id: string;
  /// The folder the conversation will run in, or null for no project (the scratch folder).
  readonly workspace: string | null;
  /// The coding service, or null while none is usable or chosen.
  readonly providerId: string | null;
  /// Further services asked the same first message, each in its own tab beside this one (one prompt, N
  /// agents). Empty for the ordinary one-service draft.
  readonly alsoProviderIds: readonly string[];
  /// Explicit choices, or null for the installed CLI's own setting.
  readonly model: string | null;
  readonly effort: string | null;
  readonly permission: string | null;
};

/// What the page draws for a draft: labels only, already resolved to words a person reads.
export type DraftChips = {
  readonly project: string;
  readonly projectPath: string | null;
  readonly branch: string | null;
  readonly service: string;
  readonly model: string;
  readonly effort: string;
  readonly mode: string;
};

export const NO_PROJECT_LABEL = "No project";
export const DEFAULT_MODEL_LABEL = "Default model";
export const DEFAULT_EFFORT_LABEL = "Default effort";
export const DEFAULT_MODE_LABEL = "Default mode";
export const NO_SERVICE_LABEL = "Choose a service";

let draftCounter = 0;

/// A fresh draft identity. Monotonic within one Extension Host; restored drafts keep their stored id.
export function newDraftId(): string {
  draftCounter += 1;
  return `draft:${Date.now().toString(36)}:${draftCounter}`;
}

/// The chips for a draft, given the service's display name and the folder's branch when known.
export function draftChips(
  draft: DraftState,
  serviceName: string | null,
  branch: string | null,
): DraftChips {
  const more = draft.alsoProviderIds.length;
  return {
    project: draft.workspace === null ? NO_PROJECT_LABEL : workspaceName(draft.workspace) || draft.workspace,
    projectPath: draft.workspace,
    branch: draft.workspace === null ? null : branch,
    // "+N": the same message goes to N more services, each in its own tab.
    service: more > 0 ? `${serviceName ?? NO_SERVICE_LABEL} +${more}` : serviceName ?? NO_SERVICE_LABEL,
    model: draft.model ?? DEFAULT_MODEL_LABEL,
    effort: draft.effort ?? DEFAULT_EFFORT_LABEL,
    mode: draft.permission ?? DEFAULT_MODE_LABEL,
  };
}

/// The greeting above an empty draft, in the words the chat apps use.
export function draftGreeting(chips: Pick<DraftChips, "project" | "projectPath">): string {
  return chips.projectPath === null
    ? "What can I help with?"
    : `What should we build in ${chips.project}?`;
}

/// Read a draft back out of webview state, keeping only a record that still makes sense.
///
/// Defensive by field: a restored tab with a malformed record closes rather than opening on a guess.
export function readDraftState(value: unknown): DraftState | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>;
  if (typeof raw.id !== "string" || !raw.id.startsWith("draft:") || raw.id.length > 64) return null;
  const text = (field: unknown): string | null =>
    typeof field === "string" && field.length > 0 && field.length <= 4_096 ? field : null;
  const also = Array.isArray(raw.alsoProviderIds)
    ? raw.alsoProviderIds.filter((id): id is string => typeof id === "string" && id.length > 0 && id.length <= 64).slice(0, 8)
    : [];
  return {
    id: raw.id,
    workspace: text(raw.workspace),
    providerId: text(raw.providerId),
    alsoProviderIds: also,
    model: text(raw.model),
    effort: text(raw.effort),
    permission: text(raw.permission),
  };
}
