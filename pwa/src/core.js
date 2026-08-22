import { text, utf8 } from "./bytes.js";
import { connectThroughRelay } from "./relay.js";

export const WIRE_VERSION = 27;

export class CoreClient {
  static async connect(connection, identity, dependencies) {
    const channel = await connectThroughRelay(connection, identity, dependencies);
    const client = new CoreClient(channel);
    const welcome = await client.exchange({ ask: "hello", with: { wire: WIRE_VERSION } });
    if (welcome.say !== "welcome" || welcome.with?.wire !== WIRE_VERSION) {
      client.close();
      throw new Error("Core wire version does not match this phone app");
    }
    readDeviceAuthority(welcome.with.device);
    client.welcome = welcome.with;
    return client;
  }

  constructor(channel) {
    this.channel = channel;
    this.busy = false;
  }

  async exchange(request) {
    if (this.busy) throw new Error("one Core channel cannot carry overlapping requests");
    this.busy = true;
    try {
      await this.channel.send(utf8(JSON.stringify(request)));
      const response = parseResponse(await this.channel.receive());
      if (response.say === "failed") throw new CoreFailure(response.with);
      return response;
    } finally {
      this.busy = false;
    }
  }

  list() {
    return this.exchange({ ask: "list" });
  }

  start(provider, workspace) {
    return this.exchange({
      ask: "start",
      with: {
        provider,
        workspace,
        workspace_access: "exclusive",
        model: null,
        permission: null,
      },
    });
  }

  resume(session) {
    return this.exchange({
      ask: "resume",
      with: {
        provider: session.provider,
        native: session.native,
        workspace: session.workspace,
        workspace_access: "exclusive",
      },
    });
  }

  prompt(session, value) {
    return this.exchange({ ask: "prompt", with: { session, text: value } });
  }

  interrupt(session) {
    return this.exchange({ ask: "interrupt", with: { session } });
  }

  closeSession(session, now) {
    return this.exchange({ ask: "close", with: { session, now } });
  }

  stopEverything() {
    return this.exchange({ ask: "stopEverything" });
  }

  setPushSubscription(endpoint) {
    return this.exchange({ ask: "pushSubscription", with: { endpoint } });
  }

  listMissions() {
    return this.exchange({ ask: "missionList" });
  }

  getMission(mission) {
    return this.exchange({ ask: "missionGet", with: { mission_id: mission } });
  }

  listMissionFlightSignals(after = null) {
    return this.exchange({ ask: "missionFlightSignals", with: { after } });
  }

  pauseMission(mission) {
    return this.exchange({ ask: "missionPause", with: { mission_id: mission } });
  }

  resumeMission(mission) {
    return this.exchange({ ask: "missionResumeSafe", with: { mission_id: mission } });
  }

  cancelMission(mission) {
    return this.exchange({ ask: "missionCancel", with: { mission_id: mission } });
  }

  answerApproval(session, approval, option, subjectDigest) {
    return this.exchange({
      ask: "answerApproval",
      with: { session, approval, option, subject_digest: subjectDigest },
    });
  }

  async beginWatch(session, after = null) {
    if (this.busy) throw new Error("one Core channel cannot carry overlapping requests");
    this.busy = true;
    await this.channel.send(utf8(JSON.stringify({
      ask: "watch",
      with: { session, ...(after === null ? {} : { after }) },
    })));
    const response = parseResponse(await this.channel.receive());
    if (response.say === "failed") {
      this.busy = false;
      throw new CoreFailure(response.with);
    }
    if (response.say !== "watching") {
      this.busy = false;
      throw new Error("Core did not acknowledge the event watch");
    }
    return response.with;
  }

  async nextWatch() {
    if (!this.busy) throw new Error("Core event watch is not active");
    const response = parseResponse(await this.channel.receive());
    if (!["event", "lagged", "failed"].includes(response.say)) {
      throw new Error("Core event watch returned an unexpected response");
    }
    if (response.say === "failed") throw new CoreFailure(response.with);
    return response;
  }

  close() {
    this.channel.close();
  }
}

export function readDeviceAuthority(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Core did not disclose this phone's current authority");
  }
  const fields = ["providers", "roots", "scopes"];
  if (JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(fields)) {
    throw new Error("Core returned an unexpected device authority contract");
  }
  const authority = {};
  for (const field of fields) {
    if (
      !Array.isArray(value[field])
      || value[field].some((entry) => typeof entry !== "string")
      || new Set(value[field]).size !== value[field].length
    ) {
      throw new Error(`Core returned invalid device authority ${field}`);
    }
    authority[field] = Object.freeze([...value[field]]);
  }
  return Object.freeze(authority);
}

export class CoreFailure extends Error {
  constructor(failure) {
    super(typeof failure?.message === "string" ? failure.message : "Core request failed");
    this.name = "CoreFailure";
    this.retryable = failure?.retryable === true;
    this.needsOperator = failure?.needs_the_operator === true;
  }
}

export function parseResponse(payload) {
  let response;
  try {
    response = JSON.parse(text(payload));
  } catch (error) {
    throw new Error("Core response is not UTF-8 JSON", { cause: error });
  }
  if (response === null || typeof response !== "object" || Array.isArray(response) || typeof response.say !== "string") {
    throw new Error("Core response has no discriminator");
  }
  return response;
}

export async function withCore(connection, identity, operation, dependencies) {
  const client = await CoreClient.connect(connection, identity, dependencies);
  try {
    return await operation(client);
  } finally {
    client.close();
  }
}
