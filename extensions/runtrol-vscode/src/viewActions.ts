// The one vocabulary of actions the conversation Webview may ask the Extension Host to perform.
//
// This module is deliberately free of any `vscode` import so the validator can be unit-tested in
// plain Node. Every message that crosses the Webview boundary is hostile until it passes
// `isViewAction`: the page runs remote-rendered conversation content, so an action name that the
// dispatcher does not explicitly handle must be dropped, never defaulted into another action.

export type ViewAction =
  | { type: "prompt"; text: string }
  | { type: "startChat" }
  | { type: "answerApproval"; approval: string; option: number; subjectDigest: number[] }
  | { type: "switchModel"; available: string[] }
  | { type: "switchMode"; available: string[] }
  | { type: "interrupt" };

export function isViewAction(value: unknown): value is ViewAction {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const type = (value as { type?: unknown }).type;
  if (type === "prompt") {
    return typeof (value as { text?: unknown }).text === "string";
  }
  if (type === "answerApproval") {
    const candidate = value as {
      approval?: unknown;
      option?: unknown;
      subjectDigest?: unknown;
    };
    return typeof candidate.approval === "string"
      && typeof candidate.option === "number"
      && Array.isArray(candidate.subjectDigest)
      && candidate.subjectDigest.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255);
  }
  if (type === "switchModel" || type === "switchMode") {
    const available = (value as { available?: unknown }).available;
    // Bounded like everything else that crosses the webview boundary: the set is display data from the
    // provider's own announcement, and a hostile page must not be able to smuggle bulk through it.
    return Array.isArray(available)
      && available.length <= 64
      && available.every((id) => typeof id === "string" && id.length > 0 && id.length <= 200);
  }
  // Exactly the actions the dispatcher handles. "openWorkspace" and "close" once passed here without a
  // dispatcher branch, so they fell into the interrupt fallback: a message the page never sends today,
  // but one hostile byte away from stopping a running agent.
  return type === "startChat" || type === "interrupt";
}
