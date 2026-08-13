import { asBytes, text, utf8 } from "./bytes.js";
import { pairingInitiator, sessionInitiator } from "./noise.js";
import { decodeRecord, encodeRecord, RecordChannel } from "./records.js";

const RELAY_PROTOCOL = "runtrol.relay.v1";
const TICKET_PREFIX = "runtrol.ticket.";

export async function pairThroughRelay(material, identity, labels, dependencies = {}) {
  const socket = await openRelay(material, dependencies);
  try {
    const initiator = await pairingInitiator(
      identity,
      material.pcPublicKey,
      material.pairingSecret,
    );
    const proposal = utf8(JSON.stringify(validateLabels(labels)));
    socket.send(encodeRecord(await initiator.writeFirst(proposal)));
    const reply = decodeRecord(await socket.receive());
    const completed = await initiator.finish(reply);
    const result = parsePairingReply(completed.payload);
    return {
      connection: Object.freeze({
        relayOrigin: material.relayOrigin,
        route: material.route,
        routeCredential: material.routeCredential,
        pcPublicKey: material.pcPublicKey,
        deviceCredential: result.credential,
        scopes: Object.freeze(result.scopes),
        roots: Object.freeze([]),
        providers: Object.freeze([]),
      }),
      pcFingerprint: await keyFingerprint(material.pcPublicKey),
      channel: new RelayChannel(socket, new RecordChannel(completed.cipher)),
    };
  } catch (error) {
    socket.close();
    throw error;
  }
}

export async function connectThroughRelay(connection, identity, dependencies = {}) {
  const socket = await openRelay(connection, dependencies);
  try {
    const initiator = await sessionInitiator(
      identity,
      connection.pcPublicKey,
      connection.relayOrigin,
      socket.peerId,
    );
    socket.send(encodeRecord(await initiator.writeFirst(new Uint8Array())));
    const reply = decodeRecord(await socket.receive());
    const completed = await initiator.finish(reply);
    if (completed.payload.byteLength !== 0) throw new Error("session handshake returned an unexpected payload");
    return new RelayChannel(socket, new RecordChannel(completed.cipher));
  } catch (error) {
    socket.close();
    throw error;
  }
}

export class RelayChannel {
  constructor(socket, records) {
    this.socket = socket;
    this.records = records;
  }

  async send(frame) {
    for (const record of await this.records.sealFrame(frame)) this.socket.send(record);
  }

  async receive() {
    for (;;) {
      const frame = await this.records.openRecord(await this.socket.receive());
      if (frame !== null) return frame;
    }
  }

  close() {
    this.socket.close();
  }
}

export async function requestTicket(material, fetchImpl = fetch) {
  const response = await fetchImpl(`${material.relayOrigin}/v1/routes/${material.route}/tickets`, {
    method: "POST",
    mode: "cors",
    cache: "no-store",
    credentials: "omit",
    referrerPolicy: "no-referrer",
    headers: {
      Authorization: `Bearer ${material.routeCredential}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ role: "phone" }),
  });
  if (!response.ok) throw new Error(`relay ticket request failed with HTTP ${response.status}`);
  const payload = await response.json();
  if (
    payload === null
    || typeof payload !== "object"
    || typeof payload.ticket !== "string"
    || !/^[A-Za-z0-9_-]{43}$/u.test(payload.ticket)
    || !Number.isSafeInteger(payload.expiresAt)
    || payload.expiresAt <= Date.now()
  ) {
    throw new Error("relay ticket response is malformed");
  }
  return payload.ticket;
}

async function openRelay(material, dependencies) {
  const ticket = await requestTicket(material, dependencies.fetchImpl);
  const WebSocketImpl = dependencies.WebSocketImpl ?? WebSocket;
  const url = new URL(material.relayOrigin);
  url.protocol = "wss:";
  url.pathname = `/v1/routes/${material.route}/connect`;
  const socket = new WebSocketImpl(url.href, [RELAY_PROTOCOL, `${TICKET_PREFIX}${ticket}`]);
  socket.binaryType = "arraybuffer";
  const queue = new BinarySocket(socket);
  await queue.opened;
  if (socket.protocol !== RELAY_PROTOCOL) {
    queue.close();
    throw new Error("relay selected an unexpected WebSocket protocol");
  }
  const peerId = asBytes(await queue.receive());
  if (peerId.byteLength !== 32) {
    queue.close();
    throw new Error("relay peer identity is malformed");
  }
  queue.peerId = peerId;
  return queue;
}

class BinarySocket {
  constructor(socket) {
    this.socket = socket;
    this.messages = [];
    this.waiters = [];
    this.failure = null;
    this.opened = new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", () => reject(new Error("relay WebSocket could not open")), { once: true });
    });
    socket.addEventListener("message", (event) => this.accept(event.data));
    socket.addEventListener("close", () => this.fail(new Error("relay WebSocket closed")));
    socket.addEventListener("error", () => this.fail(new Error("relay WebSocket failed")));
  }

  send(value) {
    this.socket.send(asBytes(value));
  }

  receive() {
    if (this.messages.length > 0) return Promise.resolve(this.messages.shift());
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  close() {
    this.socket.close(1000, "client closed");
  }

  async accept(value) {
    try {
      const bytes = value instanceof Blob ? new Uint8Array(await value.arrayBuffer()) : asBytes(value);
      const waiter = this.waiters.shift();
      if (waiter) waiter.resolve(bytes);
      else this.messages.push(bytes);
    } catch (error) {
      this.fail(new Error("relay sent a non-binary message", { cause: error }));
    }
  }

  fail(error) {
    if (this.failure) return;
    this.failure = error;
    for (const waiter of this.waiters.splice(0)) waiter.reject(error);
  }
}

function validateLabels(labels) {
  if (labels === null || typeof labels !== "object" || Array.isArray(labels)) {
    throw new Error("phone labels are invalid");
  }
  for (const field of ["name", "platform"]) {
    const value = labels[field];
    const maxLength = field === "name" ? 64 : 32;
    if (typeof value !== "string" || value.trim() !== value || value.length < 1 || value.length > maxLength) {
      throw new Error(`phone ${field} is invalid`);
    }
  }
  return { name: labels.name, platform: labels.platform };
}

function parsePairingReply(payload) {
  let value;
  try {
    value = JSON.parse(text(payload));
  } catch (error) {
    throw new Error("pairing response is malformed", { cause: error });
  }
  if (
    value === null
    || typeof value !== "object"
    || JSON.stringify(Object.keys(value).sort()) !== JSON.stringify(["credential", "scopes"])
    || typeof value.credential !== "string"
    || !/^[0-9a-f]{64}$/u.test(value.credential)
    || !Array.isArray(value.scopes)
    || value.scopes.some((scope) => typeof scope !== "string")
  ) {
    throw new Error("pairing response has an invalid contract");
  }
  return value;
}

export async function keyFingerprint(encodedPublicKey) {
  const key = Uint8Array.from(atob(encodedPublicKey.replaceAll("-", "+").replaceAll("_", "/") + "=".repeat((4 - encodedPublicKey.length % 4) % 4)), (character) => character.charCodeAt(0));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", key));
  return btoa(String.fromCharCode(...digest.slice(0, 8))).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}
