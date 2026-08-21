import { createHash, randomBytes } from "node:crypto";
import { createConnection } from "node:net";
import { argv, exit, stdin, stdout } from "node:process";
import { createInterface } from "node:readline";

import { base64UrlEncode } from "../src/bytes.js";
import { CoreClient, readDeviceAuthority, WIRE_VERSION } from "../src/core.js";
import { directSessionInitiator } from "../src/noise.js";
import { RecordChannel, decodeRecord, encodeRecord } from "../src/records.js";
import { RelayChannel } from "../src/relay.js";

const mode = argv[2];
if (!["drive", "approval", "resilience"].includes(mode)) fail("live phone mode is invalid");

// A watch is a correctness boundary, not the latency ratchet. Cold provider startup shares the machine with
// preceding Extension Host and packaging gates in preflight, so one scheduler pause must not outrank the explicit
// 60-second journey deadline. The separate vscodeHostPerformance gate owns latency measurements.
const WATCH_EVENT_TIMEOUT_MS = 15_000;

async function journey(identity, config, journeyMode) {
  if (journeyMode === "resilience") return resilienceJourney(identity, config);
  const controller = await connectCore(identity, config);
  let watcher = null;
  let session = null;
  const evidence = {
    started: false,
    prompted: false,
    output_seen: false,
    provider_ended: false,
    approval_seen: false,
    subject_complete: false,
    reject_once: false,
    answered: false,
    close_confirmed: false,
  };
  try {
    const started = await controller.start(config.provider, config.workspace);
    if (started.say !== "started" || typeof started.with?.session !== "string") {
      throw new Error("Core did not start a session for the phone");
    }
    evidence.started = true;
    session = started.with.session;
    watcher = await connectCore(identity, config);
    await watcher.beginWatch(session);
    const prompted = await controller.prompt(session, "Return the deterministic fixture response.");
    if (prompted.say !== "done") throw new Error("Core did not accept the phone prompt");
    evidence.prompted = true;

    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline && !complete(evidence, journeyMode)) {
      const response = await Promise.race([
        watcher.nextWatch(),
        timeout(WATCH_EVENT_TIMEOUT_MS, "the phone watch produced no event"),
      ]);
      if (response.say === "lagged") throw new Error("the phone event watch lagged");
      inspectEvent(response.with?.payload, evidence);
      if (journeyMode === "approval" && evidence.approval_seen && !evidence.answered) {
        const body = eventBody(response.with?.payload);
        const rejection = body.options.find((option) => option.kind === "rejectOnce");
        const answered = await controller.answerApproval(session, body.id, rejection.id, body.subject_digest);
        if (answered.say !== "done") throw new Error("Core did not accept the phone approval answer");
        evidence.answered = true;
      }
    }
    if (!complete(evidence, journeyMode)) throw new Error("the phone journey did not reach its terminal evidence");
    const closed = await controller.closeSession(session, true);
    if (closed.say !== "done") throw new Error("Core did not close the phone-owned session");
    evidence.close_confirmed = true;
    return { facts: Object.keys(evidence).filter((fact) => evidence[fact]) };
  } finally {
    watcher?.close();
    controller.close();
  }
}

async function resilienceJourney(identity, config) {
  let controller = await connectCore(identity, config);
  let watcher = null;
  let session = null;
  let resumed = null;
  const facts = new Set();
  try {
    const started = await controller.start(config.provider, config.workspace);
    if (started.say !== "started" || typeof started.with?.session !== "string") {
      throw new Error("Core did not start the resilience session");
    }
    session = started.with.session;
    facts.add("started");

    watcher = await connectCore(identity, config);
    const firstStart = await watcher.beginWatch(session);
    if (firstStart.gap !== null) throw new Error("the initial phone watch reported a gap");
    const firstPrompt = await controller.prompt(session, "Return resilience turn one.");
    if (firstPrompt.say !== "done") throw new Error("Core refused resilience turn one");
    facts.add("prompted");
    const first = await collectTurn(watcher, firstStart.starts_at);
    facts.add("first_turn");
    facts.add("output_seen");
    facts.add("provider_ended");

    const before = sessionRow(await controller.list(), session);
    if (typeof before.native !== "string" || before.native.length === 0) {
      throw new Error("the provider did not disclose its native session identity");
    }

    watcher.channel.socket.abort();
    watcher = null;
    facts.add("network_cut");
    const secondPrompt = await controller.prompt(session, "Return resilience turn two.");
    if (secondPrompt.say !== "done") throw new Error("Core refused resilience turn two");
    await wait(500);

    watcher = await connectCore(identity, config);
    const replayStart = await watcher.beginWatch(session, first.cursor);
    if (replayStart.gap !== null || !sameCursor(replayStart.starts_at, first.cursor)) {
      throw new Error("bounded remote replay did not begin at the exact disconnected cursor");
    }
    const second = await collectTurn(watcher, first.cursor);
    if (second.cursors.some((cursor) => sameCursor(cursor, first.cursor))) {
      throw new Error("bounded remote replay duplicated the last delivered event");
    }
    facts.add("exact_replay");
    facts.add("no_duplicate");

    watcher.close();
    watcher = null;
    controller.close();
    await wait(100);
    writeLine({ control: "restart" });

    controller = await reconnectCore(identity, config, 30_000);
    facts.add("reconnected");
    const restored = sessionRow(await controller.list(), session);
    if (restored.native !== before.native) {
      throw new Error("provider native identity changed across the Core restart");
    }
    facts.add("native_preserved");

    const resumedAnswer = await controller.resume(restored);
    if (resumedAnswer.say !== "started" || typeof resumedAnswer.with?.session !== "string") {
      throw new Error("Core did not resume the provider-owned session after restart");
    }
    resumed = resumedAnswer.with.session;
    if (resumed === session) throw new Error("Core reused the pre-restart session identity");
    watcher = await connectCore(identity, config);
    const restartStart = await watcher.beginWatch(resumed, second.cursor);
    if (
      restartStart.gap === null
      || !sameCursor(restartStart.gap.requested, second.cursor)
      || restartStart.starts_at.stream === second.cursor.stream
    ) {
      throw new Error("the Core restart did not expose an explicit cross-stream gap");
    }
    facts.add("explicit_gap");

    const thirdPrompt = await controller.prompt(resumed, "Return resilience turn three.");
    if (thirdPrompt.say !== "done") throw new Error("Core refused the resumed turn");
    const third = await collectTurn(watcher, restartStart.starts_at);
    if (third.cursor.stream === second.cursor.stream) {
      throw new Error("the resumed provider reused the old event stream");
    }
    const after = sessionRow(await controller.list(), resumed);
    if (after.native !== before.native) {
      throw new Error("the resumed row lost the provider native identity");
    }
    facts.add("resumed_turn");

    const closed = await controller.closeSession(resumed, true);
    if (closed.say !== "done") throw new Error("Core did not close the resumed session");
    facts.add("close_confirmed");
    return { facts: [...facts] };
  } finally {
    watcher?.close();
    controller?.close();
  }
}

async function collectTurn(watcher, after) {
  const cursors = [];
  let cursor = readCursor(after);
  let outputSeen = false;
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const response = await Promise.race([
      watcher.nextWatch(),
      timeout(WATCH_EVENT_TIMEOUT_MS, "the resilience watch produced no event"),
    ]);
    if (response.say === "lagged") throw new Error("the resilience watch lagged");
    const next = readCursor(response.with?.next_expected);
    if (next.stream !== cursor.stream || next.epoch !== cursor.epoch || next.seq !== cursor.seq + 1) {
      throw new Error("the resilience cursor sequence was not dense");
    }
    if (cursors.some((seen) => sameCursor(seen, next))) {
      throw new Error("the resilience watch delivered a duplicate cursor");
    }
    cursors.push(next);
    cursor = next;
    const body = eventBody(response.with?.payload);
    if (!body) throw new Error("the resilience watch returned a malformed event");
    if (body.event === "agentMessageChunk" && deepText(body).includes("denial consumed")) {
      outputSeen = true;
    }
    if (
      outputSeen
      && body.event === "turn"
      && body.step === "ended"
      && body.stop === "endTurn"
      && body.declared_by?.by === "provider"
    ) {
      return { cursor, cursors };
    }
  }
  throw new Error("the resilience turn did not reach provider-declared completion");
}

function sessionRow(response, session) {
  if (response.say !== "sessions" || !Array.isArray(response.with?.sessions)) {
    throw new Error("Core returned no session catalogue during resilience testing");
  }
  const row = response.with.sessions.find((candidate) => candidate?.session === session);
  if (!row) throw new Error("the resilience session disappeared from the Core catalogue");
  return row;
}

async function reconnectCore(identity, config, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let delay = 100;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      return await connectCore(identity, config);
    } catch (error) {
      lastError = error;
      await wait(delay);
      delay = Math.min(delay * 2, 2_000);
    }
  }
  throw new Error("the phone did not reconnect after the Core restart", { cause: lastError });
}

function readCursor(value) {
  if (
    value === null
    || typeof value !== "object"
    || typeof value.stream !== "string"
    || !Number.isSafeInteger(value.epoch)
    || !Number.isSafeInteger(value.seq)
  ) {
    throw new Error("Core returned a malformed watch cursor");
  }
  return { stream: value.stream, epoch: value.epoch, seq: value.seq };
}

function sameCursor(left, right) {
  const a = readCursor(left);
  const b = readCursor(right);
  return a.stream === b.stream && a.epoch === b.epoch && a.seq === b.seq;
}

function inspectEvent(payload, evidence) {
  const body = eventBody(payload);
  if (!body) throw new Error("Core returned a malformed provider event");
  if (body.event === "agentMessageChunk" && deepText(body).includes("denial consumed")) {
    evidence.output_seen = true;
  }
  if (
    body.event === "turn"
    && body.step === "ended"
    && body.stop === "endTurn"
    && body.declared_by?.by === "provider"
  ) {
    evidence.provider_ended = true;
  }
  if (body.event === "approvalRequested") {
    evidence.approval_seen = true;
    evidence.subject_complete = body.subject_incomplete === false && body.subject !== null;
    evidence.reject_once = Array.isArray(body.options)
      && body.options.filter((option) => option?.kind === "rejectOnce").length === 1
      && Array.isArray(body.subject_digest)
      && body.subject_digest.length === 32;
  }
}

function eventBody(payload) {
  if (payload === null || typeof payload !== "object" || Array.isArray(payload)) return null;
  const body = payload.body;
  return body !== null && typeof body === "object" && !Array.isArray(body) ? body : null;
}

function deepText(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(deepText).join(" ");
  if (value !== null && typeof value === "object") return Object.values(value).map(deepText).join(" ");
  return "";
}

function complete(evidence, journeyMode) {
  const terminal = evidence.output_seen && evidence.provider_ended;
  return journeyMode === "drive"
    ? terminal
    : terminal && evidence.approval_seen && evidence.subject_complete && evidence.reject_once && evidence.answered;
}

async function connectCore(identity, config) {
  const socket = await RawWebSocket.connect(config.address);
  try {
    const initiator = await directSessionInitiator(identity, config.pc_public, 1, decodePublic(config.pc_public));
    socket.send(encodeRecord(await initiator.writeFirst(new Uint8Array())));
    const completed = await initiator.finish(decodeRecord(await socket.receive()));
    if (completed.payload.byteLength !== 0) throw new Error("direct Noise handshake returned a payload");
    const client = new CoreClient(new RelayChannel(socket, new RecordChannel(completed.cipher)));
    const welcome = await client.exchange({ ask: "hello", with: { wire: WIRE_VERSION } });
    if (welcome.say !== "welcome" || welcome.with?.wire !== WIRE_VERSION) {
      throw new Error("Core wire version did not match the phone");
    }
    readDeviceAuthority(welcome.with.device);
    return client;
  } catch (error) {
    socket.close();
    throw error;
  }
}

function decodePublic(value) {
  if (typeof value !== "string" || !/^[A-Za-z0-9_-]{43}$/u.test(value)) {
    throw new Error("PC public key is malformed");
  }
  return Uint8Array.from(Buffer.from(value, "base64url"));
}

class RawWebSocket {
  static async connect(address) {
    if (typeof address !== "string" || !/^127\.0\.0\.1:[1-9][0-9]{0,4}$/u.test(address)) {
      throw new Error("phone listener address is malformed");
    }
    const [host, portText] = address.split(":");
    const socket = createConnection({ host, port: Number(portText) });
    await new Promise((resolve, reject) => {
      socket.once("connect", resolve);
      socket.once("error", reject);
    });
    const key = randomBytes(16).toString("base64");
    socket.write([
      "GET /v1/link HTTP/1.1",
      `Host: ${address}`,
      "Origin: https://phone.runtrol.test",
      "Sec-Fetch-Site: same-origin",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Key: ${key}`,
      "Sec-WebSocket-Version: 13",
      "Sec-WebSocket-Protocol: runtrol.noise.v1",
      "",
      "",
    ].join("\r\n"));
    const accepted = new RawWebSocket(socket);
    await accepted.readUpgrade(key);
    return accepted;
  }

  constructor(socket) {
    this.socket = socket;
    this.buffer = Buffer.alloc(0);
    this.messages = [];
    this.waiters = [];
    this.failure = null;
    socket.on("data", (chunk) => this.accept(chunk));
    socket.on("error", (error) => this.fail(error));
    socket.on("close", () => this.fail(new Error("phone WebSocket closed")));
  }

  async readUpgrade(key) {
    const head = await this.untilHeader();
    const lines = head.toString("ascii").split("\r\n");
    if (lines.shift() !== "HTTP/1.1 101 Switching Protocols") throw new Error("phone WebSocket upgrade was refused");
    const headers = new Map(lines.filter(Boolean).map((line) => {
      const at = line.indexOf(":");
      return [line.slice(0, at).trim().toLowerCase(), line.slice(at + 1).trim()];
    }));
    const expected = createHash("sha1").update(`${key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
    if (headers.get("sec-websocket-accept") !== expected) throw new Error("phone WebSocket accept value is invalid");
    if (headers.get("sec-websocket-protocol") !== "runtrol.noise.v1") throw new Error("phone WebSocket protocol is invalid");
    this.parseFrames();
  }

  untilHeader() {
    const existing = this.buffer.indexOf("\r\n\r\n");
    if (existing >= 0) return Promise.resolve(this.takeHeader(existing));
    return new Promise((resolve, reject) => {
      this.headerWaiter = { resolve, reject };
    });
  }

  takeHeader(at) {
    const head = this.buffer.subarray(0, at);
    this.buffer = this.buffer.subarray(at + 4);
    return head;
  }

  send(value) {
    const body = Buffer.from(value);
    const mask = randomBytes(4);
    const length = body.length;
    let header;
    if (length < 126) {
      header = Buffer.from([0x82, 0x80 | length]);
    } else if (length <= 0xffff) {
      header = Buffer.alloc(4);
      header[0] = 0x82;
      header[1] = 0xfe;
      header.writeUInt16BE(length, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = 0x82;
      header[1] = 0xff;
      header.writeBigUInt64BE(BigInt(length), 2);
    }
    const masked = Buffer.from(body);
    for (let index = 0; index < masked.length; index += 1) masked[index] ^= mask[index % 4];
    this.socket.write(Buffer.concat([header, mask, masked]));
  }

  receive() {
    if (this.messages.length > 0) return Promise.resolve(this.messages.shift());
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  close() {
    if (!this.socket.destroyed) this.writeControl(0x08, Buffer.from([0x03, 0xe8]));
    this.socket.end();
    this.socket.unref();
  }

  abort() {
    this.socket.destroy();
    this.socket.unref();
  }

  accept(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    if (this.headerWaiter) {
      const at = this.buffer.indexOf("\r\n\r\n");
      if (at < 0) return;
      const waiter = this.headerWaiter;
      this.headerWaiter = null;
      waiter.resolve(this.takeHeader(at));
      return;
    }
    this.parseFrames();
  }

  parseFrames() {
    for (;;) {
      if (this.buffer.length < 2) return;
      const first = this.buffer[0];
      const second = this.buffer[1];
      if ((first & 0x70) !== 0 || (first & 0x80) === 0 || (second & 0x80) !== 0) {
        return this.fail(new Error("server returned an invalid WebSocket frame"));
      }
      let length = second & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (this.buffer.length < 4) return;
        length = this.buffer.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (this.buffer.length < 10) return;
        const wide = this.buffer.readBigUInt64BE(2);
        if (wide > BigInt(Number.MAX_SAFE_INTEGER)) return this.fail(new Error("WebSocket frame is too large"));
        length = Number(wide);
        offset = 10;
      }
      if (this.buffer.length < offset + length) return;
      const opcode = first & 0x0f;
      const payload = this.buffer.subarray(offset, offset + length);
      this.buffer = this.buffer.subarray(offset + length);
      if (opcode === 0x02) this.deliver(new Uint8Array(payload));
      else if (opcode === 0x09) this.writeControl(0x0a, payload);
      else if (opcode === 0x0a) continue;
      else if (opcode === 0x08) return this.fail(new Error("phone WebSocket closed before completion"));
      else return this.fail(new Error("server returned a non-binary WebSocket frame"));
    }
  }

  writeControl(opcode, payload) {
    const mask = randomBytes(4);
    const body = Buffer.from(payload);
    const masked = Buffer.from(body);
    for (let index = 0; index < masked.length; index += 1) masked[index] ^= mask[index % 4];
    this.socket.write(Buffer.concat([Buffer.from([0x80 | opcode, 0x80 | body.length]), mask, masked]));
  }

  deliver(message) {
    const waiter = this.waiters.shift();
    if (waiter) waiter.resolve(message);
    else this.messages.push(message);
  }

  fail(error) {
    if (this.failure) return;
    this.failure = error;
    this.headerWaiter?.reject(error);
    this.headerWaiter = null;
    for (const waiter of this.waiters.splice(0)) waiter.reject(error);
  }
}

async function readConfig() {
  const lines = createInterface({ input: stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    const value = JSON.parse(line);
    if (
      value === null
      || typeof value !== "object"
      || typeof value.address !== "string"
      || typeof value.pc_public !== "string"
      || typeof value.workspace !== "string"
      || typeof value.provider !== "string"
    ) {
      throw new Error("live phone config is malformed");
    }
    return value;
  }
  throw new Error("live phone config was not provided");
}

function timeout(milliseconds, message) {
  return new Promise((_, reject) => setTimeout(() => reject(new Error(message)), milliseconds));
}

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function writeLine(value) {
  stdout.write(`${JSON.stringify(value)}\n`);
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  exit(2);
}

try {
  const identity = await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"]);
  const phonePublic = new Uint8Array(await crypto.subtle.exportKey("raw", identity.publicKey));
  writeLine({ phone_public: base64UrlEncode(phonePublic) });
  const config = await readConfig();
  const evidence = await journey(identity, config, mode);
  await writeFinalLine(evidence);
  exit(0);
} catch (error) {
  fail(error instanceof Error ? error.stack ?? error.message : String(error));
}

function writeFinalLine(value) {
  return new Promise((resolve, reject) => {
    stdout.write(`${JSON.stringify(value)}\n`, (error) => error ? reject(error) : resolve());
  });
}
