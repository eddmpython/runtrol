const ATTENTION_PARAMETER = "attention";
const PERSON_WAIT = "person";

export function needsAttention(session) {
  return session?.waiting_on === PERSON_WAIT;
}

export function attentionCount(sessions) {
  return sessions.filter(needsAttention).length;
}

export function nextAttentionSession(sessions, currentSession = null) {
  const waiting = sessions.filter(needsAttention);
  if (waiting.length === 0) return null;
  const current = waiting.findIndex((session) => session.session === currentSession);
  return waiting[(current + 1) % waiting.length];
}

export function preferredSession(
  sessions,
  requestedSession,
  currentSession,
  attentionRequested,
  narrowViewport,
) {
  const requested = sessions.find((session) => session.session === requestedSession) ?? null;
  if (requested) return requested;
  if (attentionRequested) return nextAttentionSession(sessions);
  const current = sessions.find((session) => session.session === currentSession) ?? null;
  if (current) return current;
  return narrowViewport ? null : (sessions[0] ?? null);
}

export function consumeAttentionRequest(location, history) {
  const url = new URL(location.href);
  if (url.searchParams.get(ATTENTION_PARAMETER) !== "1") return false;
  url.searchParams.delete(ATTENTION_PARAMETER);
  history.replaceState(null, "", `${url.pathname}${url.search}${url.hash}`);
  return true;
}

export function isAttentionMessage(value) {
  return value !== null
    && typeof value === "object"
    && !Array.isArray(value)
    && Object.keys(value).length === 1
    && value.kind === "runtrolAttention";
}
