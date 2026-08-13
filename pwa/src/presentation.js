const ANSI_PATTERN = /[\u001b\u009b](?:\][^\u0007\u001b]*(?:\u0007|\u001b\\)|\[[0-?]*[ -/]*[@-~]|[@-_])/gu;
const BIDI_CONTROLS = /[\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/gu;
const MAX_VISIBLE_TEXT = 8 * 1024;

export function safeVisibleText(value) {
  return String(value)
    .replace(ANSI_PATTERN, "")
    .replace(BIDI_CONTROLS, (character) => `<U+${character.codePointAt(0).toString(16).toUpperCase().padStart(4, "0")}>`)
    .slice(0, MAX_VISIBLE_TEXT);
}

export function exactSubject(value) {
  try {
    return safeVisibleText(typeof value === "string" ? value : JSON.stringify(value, null, 2));
  } catch {
    return "The provider supplied a subject that cannot be displayed.";
  }
}

export function eventBody(payload) {
  const envelope = record(payload);
  const body = record(envelope?.body);
  return typeof body?.event === "string" ? body : null;
}

export function contentText(body) {
  const content = body?.content;
  if (typeof content === "string") return safeVisibleText(content);
  const source = record(content);
  if (typeof source?.text === "string") return safeVisibleText(source.text);
  if (typeof source?.delta === "string") return safeVisibleText(source.delta);
  if (Array.isArray(source?.content)) {
    return safeVisibleText(source.content.map((part) => record(part)?.text ?? "").join(""));
  }
  return "";
}

export function approvalOptions(body, scopes) {
  const highAuthority = scopes.includes("approval.respond.high");
  const lowAuthority = highAuthority || scopes.includes("approval.respond.low");
  const highRequest = body.risk === "high";
  return (Array.isArray(body.options) ? body.options : []).flatMap((candidate) => {
    const option = record(candidate);
    if (!Number.isInteger(option?.id) || typeof option?.kind !== "string") return [];
    const rejection = option.kind === "rejectOnce" || option.kind === "rejectAlways";
    const standing = option.kind === "allowAlways" || option.kind === "rejectAlways";
    let unavailable = null;
    if (body.subject_incomplete === true && !rejection) {
      unavailable = "The complete action is unavailable, so only refusal is allowed.";
    } else if ((highRequest || standing) && !highAuthority) {
      unavailable = "This phone does not hold high-risk approval authority.";
    } else if (!highRequest && !standing && !lowAuthority) {
      unavailable = "This phone does not hold approval authority.";
    }
    return [{
      id: option.id,
      kind: option.kind,
      label: safeVisibleText(typeof option.label === "string" ? option.label : option.kind),
      unavailable,
    }];
  });
}

function record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : null;
}
