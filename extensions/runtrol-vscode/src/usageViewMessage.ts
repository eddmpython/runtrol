import type { SetupRow, UsageRow } from "./usageDisplay";

/// The complete bounded snapshot drawn by the fixed usage surface.
export type UsageViewSnapshot = {
  readonly type: "snapshot";
  readonly rows: readonly UsageRow[];
  /// Every service this build serves and what each still needs, drawn only while the set-up list is open.
  readonly setup: readonly SetupRow[];
  /// One sentence under the strip while an older Core generation is still serving this window.
  readonly notice: string | null;
  readonly error: string | null;
};

/// Actions accepted from the untrusted Webview document.
export type UsageViewAction =
  | { readonly type: "ready" }
  | { readonly type: "fix"; readonly providerId: string }
  | { readonly type: "signIn"; readonly providerId: string }
  | { readonly type: "setUp"; readonly providerId: string };

/// Validate the small action vocabulary before any Webview value reaches extension commands.
export function usageViewAction(value: unknown): UsageViewAction | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (record.type === "ready") return { type: "ready" };
  if (
    (record.type === "fix" || record.type === "signIn" || record.type === "setUp")
    && typeof record.providerId === "string"
    && record.providerId.length > 0
    && record.providerId.length <= 256
  ) {
    return { type: record.type, providerId: record.providerId };
  }
  return null;
}

/// Validate a snapshot from the host before the document draws it.
///
/// The document trusts nothing it is handed, including its own host: a message that does not match this shape is
/// dropped. That makes the check part of the message contract rather than a detail of the drawing code, so a
/// field renamed on one side and not the other fails a test here instead of silently emptying the panel, which
/// is what happened when `installableCount` became `setup` (measured 2026-08-26: every snapshot was discarded
/// and the strip drew nothing at all, not even its own empty sentence).
export function usageSnapshot(value: unknown): UsageViewSnapshot | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (
    record.type !== "snapshot"
    || !Array.isArray(record.rows)
    || !Array.isArray(record.setup)
    || !(typeof record.notice === "string" || record.notice === null)
    || !(typeof record.error === "string" || record.error === null)
  ) return null;
  return value as UsageViewSnapshot;
}
