export type WebviewReadyKind = "startup" | "probe";

export function webviewReadyKind(value: unknown): WebviewReadyKind | null {
  if (
    value === null
    || typeof value !== "object"
    || Array.isArray(value)
    || (value as Record<string, unknown>).type !== "webviewReady"
  ) {
    return null;
  }
  return (value as Record<string, unknown>).probe === true ? "probe" : "startup";
}
