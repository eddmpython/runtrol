const MAX_SIGNALS = 64;
const SIGNAL_KINDS = new Set(["person", "stopped", "landing"]);
const CURSOR = /^[0-9a-f]{32}$/u;
const DIGEST = /^[0-9a-f]{64}$/u;

export function readMissionFlightSignals(value) {
  if (
    value === null
    || typeof value !== "object"
    || Array.isArray(value)
    || !exactFields(value, ["gap", "next_cursor", "signals"])
    || !Array.isArray(value.signals)
    || value.signals.length > MAX_SIGNALS
    || typeof value.gap !== "boolean"
    || (value.next_cursor !== null && !CURSOR.test(value.next_cursor))
  ) {
    throw new Error("Core returned an invalid Mission Flight Signal page");
  }
  const signals = Object.freeze(value.signals.map(readSignal));
  for (let index = 1; index < signals.length; index += 1) {
    if (signals[index - 1].signal_id >= signals[index].signal_id) {
      throw new Error("Core returned Mission Flight Signals out of order");
    }
  }
  if (signals.length > 0 && (value.next_cursor === null || signals.at(-1).signal_id > value.next_cursor)) {
    throw new Error("Core returned an invalid Mission Flight Signal cursor");
  }
  return Object.freeze({
    signals,
    next_cursor: value.next_cursor,
    gap: value.gap,
  });
}

export function missionFlightDestination(signals, sessions) {
  if (!Array.isArray(signals) || !Array.isArray(sessions)) return null;
  for (let index = signals.length - 1; index >= 0; index -= 1) {
    const signal = signals[index];
    if (signal.kind === "person") {
      const session = sessions.find((row) => (
        row?.session === signal.session_id && row.waiting_on === "person"
      ));
      if (session) {
        return Object.freeze({
          surface: "session",
          missionId: signal.mission_id,
          session,
          kind: signal.kind,
        });
      }
      continue;
    }
    return Object.freeze({
      surface: "mission",
      missionId: signal.mission_id,
      session: null,
      kind: signal.kind,
    });
  }
  return null;
}

export function missionFlightLabel(kind) {
  if (kind === "person") return "Mission needs you";
  if (kind === "stopped") return "Auto Flight stopped safely";
  if (kind === "landing") return "Receipt Landing ready";
  throw new Error("unknown Mission Flight Signal kind");
}

export function missionFlightBadge(kind) {
  if (kind === "person") return "NEEDS YOU";
  if (kind === "stopped") return "STOPPED";
  if (kind === "landing") return "LANDED";
  throw new Error("unknown Mission Flight Signal kind");
}

function readSignal(value) {
  if (
    value === null
    || typeof value !== "object"
    || Array.isArray(value)
    || !exactFields(value, ["kind", "mission_id", "mission_sha256", "session_id", "signal_id"])
    || !CURSOR.test(value.signal_id)
    || !shortText(value.mission_id, 128)
    || !DIGEST.test(value.mission_sha256)
    || !SIGNAL_KINDS.has(value.kind)
  ) {
    throw new Error("Core returned an invalid Mission Flight Signal");
  }
  if (
    (value.kind === "person" && !shortText(value.session_id, 128))
    || (value.kind !== "person" && value.session_id !== null)
  ) {
    throw new Error("Core returned an invalid Mission Flight Signal destination");
  }
  return Object.freeze({
    signal_id: value.signal_id,
    mission_id: value.mission_id,
    mission_sha256: value.mission_sha256,
    kind: value.kind,
    session_id: value.session_id,
  });
}

function exactFields(value, expected) {
  return JSON.stringify(Object.keys(value).sort()) === JSON.stringify(expected);
}

function shortText(value, max) {
  return typeof value === "string" && value.length > 0 && value.length <= max;
}
