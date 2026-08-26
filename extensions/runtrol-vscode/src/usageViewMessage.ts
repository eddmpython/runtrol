import type { SetupRow, UsageRow } from "./usageDisplay";

/// The complete bounded snapshot drawn by the fixed usage surface.
export type UsageViewSnapshot = {
  readonly type: "snapshot";
  readonly rows: readonly UsageRow[];
  /// Every service this build serves and what each still needs, drawn only while the set-up list is open.
  readonly setup: readonly SetupRow[];
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
