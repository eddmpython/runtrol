const CANCELLABLE_STATES = new Set(["ready", "running", "paused", "blocked", "integrating"]);
const MISSION_STATES = new Set([
  "draft", "validated", "ready", "running", "paused", "blocked", "integrating",
  "completed", "failed", "cancelled", "archived", "rejected",
]);
const TASK_STATES = new Set([
  "pending", "eligible", "reserved", "awaitingInput", "running", "awaitingApproval",
  "verifying", "retryable", "blocked", "passed", "skipped", "failed", "cancelled",
]);
const MAX_MISSIONS = 100;
const MAX_TASKS = 1_000;

export function readMissionCatalogue(value) {
  if (!Array.isArray(value) || value.length > MAX_MISSIONS) {
    throw new Error("Core returned an invalid Mission catalogue");
  }
  return Object.freeze(value.map(readMissionLine));
}

export function readMissionSnapshot(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value) || !Array.isArray(value.tasks)) {
    throw new Error("Core returned an invalid Mission snapshot");
  }
  if (value.tasks.length > MAX_TASKS) throw new Error("Core returned too many Mission Tasks");
  const mission = readMissionLine(value.mission);
  const missionRef = shortText(value.mission_ref, "Mission source", 4_096);
  const policySha256 = digest(value.policy_sha256, "Mission policy digest");
  const tasks = Object.freeze(value.tasks.map(readTaskLine));
  return Object.freeze({ mission, mission_ref: missionRef, policy_sha256: policySha256, tasks });
}

export function missionActions(mission, scopes) {
  const state = typeof mission?.state === "string" ? mission.state : "";
  const held = new Set(Array.isArray(scopes) ? scopes : []);
  return Object.freeze({
    pause: state === "running" && held.has("mission.pause"),
    resume: ["paused", "blocked"].includes(state) && held.has("mission.resumeSafe"),
    cancel: CANCELLABLE_STATES.has(state) && held.has("mission.cancel"),
  });
}

function readMissionLine(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Core returned an invalid Mission row");
  }
  const state = shortText(value.state, "Mission state", 32);
  if (!MISSION_STATES.has(state)) throw new Error("Core returned an unknown Mission state");
  return Object.freeze({
    mission_id: shortText(value.mission_id, "Mission identity", 128),
    name: shortText(value.name, "Mission name", 256),
    project: shortText(value.project, "Mission project", 4_096),
    state,
    passed_tasks: count(value.passed_tasks, "passed Task count"),
    total_tasks: count(value.total_tasks, "total Task count"),
    awaiting_input: count(value.awaiting_input, "awaiting-input Task count"),
  });
}

function readTaskLine(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Core returned an invalid Mission Task row");
  }
  const state = shortText(value.state, "Task state", 32);
  if (!TASK_STATES.has(state)) throw new Error("Core returned an unknown Mission Task state");
  return Object.freeze({
    task_id: shortText(value.task_id, "Task identity", 128),
    key: shortText(value.key, "Task key", 64),
    state,
    instruction_ref: shortText(value.instruction_ref, "Task instruction", 4_096),
    workspace_mode: shortText(value.workspace_mode, "Task workspace mode", 32),
    provider_selector: shortText(value.provider_selector, "Task provider selector", 256),
    receipt_id: value.receipt_id === null ? null : shortText(value.receipt_id, "Task Receipt", 128),
    passed_gates: count(value.passed_gates, "passed Gate count"),
    failed_gates: count(value.failed_gates, "failed Gate count"),
  });
}

function shortText(value, field, max) {
  if (typeof value !== "string" || value.length === 0 || value.length > max) {
    throw new Error(`Core returned invalid ${field}`);
  }
  return value;
}

function count(value, field) {
  if (!Number.isInteger(value) || value < 0 || value > 65_535) {
    throw new Error(`Core returned invalid ${field}`);
  }
  return value;
}

function digest(value, field) {
  if (typeof value !== "string" || !/^[0-9a-f]{64}$/u.test(value)) {
    throw new Error(`Core returned invalid ${field}`);
  }
  return value;
}
