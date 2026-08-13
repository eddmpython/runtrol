import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";

const ORIGIN = "https://eddmpython.github.io";
const ROUTE = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const SECRET = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const OTHER_SECRET = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const openSockets: WebSocket[] = [];

afterEach(() => {
  for (const socket of openSockets.splice(0)) {
    socket.close(1000, "test complete");
  }
});

describe("relay admission", () => {
  it("pins the PWA origin and exact route shape", async () => {
    const refused = await exports.default.fetch(
      `https://relay.test/v1/routes/${ROUTE}`,
      {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${SECRET}`,
          Origin: "https://attacker.test",
        },
      },
    );
    expect(refused.status).toBe(403);

    const malformed = await exports.default.fetch(
      "https://relay.test/v1/routes/short",
      {
        method: "PUT",
        headers: { Authorization: `Bearer ${SECRET}` },
      },
    );
    expect(malformed.status).toBe(404);
  });

  it("stores one credential digest and refuses a different credential", async () => {
    expect((await register()).status).toBe(204);
    expect((await register()).status).toBe(204);
    const refused = await register(OTHER_SECRET);
    expect(refused.status).toBe(403);

    const result = await storedState();
    expect(result.credentialRows).toBe(1);
    expect(result.ticketRows).toBe(0);
    expect(result.storedText).not.toContain(SECRET);
  });
});

describe("ciphertext relay", () => {
  it("consumes tickets once and forwards binary records without storing them", async () => {
    expect((await register()).status).toBe(204);
    const pcTicket = await ticket("pc");
    const phoneTicket = await ticket("phone");
    const pc = await connect(pcTicket);
    const phone = await connect(phoneTicket);
    const peer = new Uint8Array(await receive(phone));
    expect(peer.byteLength).toBe(32);

    const reused = await connectResponse(pcTicket);
    expect(reused.status).toBe(401);

    const pcPayload = new Uint8Array([0, 17, 99, 255]);
    const routed = new Uint8Array(peer.byteLength + pcPayload.byteLength);
    routed.set(peer);
    routed.set(pcPayload, peer.byteLength);
    const phoneReceived = receive(phone);
    pc.send(routed);
    expect(new Uint8Array(await phoneReceived)).toEqual(pcPayload);

    const phonePayload = new Uint8Array([6, 7, 8]);
    const pcReceived = receive(pc);
    phone.send(phonePayload);
    const envelope = new Uint8Array(await pcReceived);
    expect(envelope.slice(0, peer.byteLength)).toEqual(peer);
    expect(envelope.slice(peer.byteLength)).toEqual(phonePayload);

    const result = await storedState();
    expect(result.ticketRows).toBe(0);
    expect(result.storedText).not.toContain("0,17,99,255");
  });

  it("rejects text and reports an offline PC instead of buffering", async () => {
    expect((await register()).status).toBe(204);
    const phone = await connect(await ticket("phone"));
    await receive(phone);
    const closed = closeEvent(phone);
    phone.send(new Uint8Array([1]));
    expect((await closed).code).toBe(1013);

    const pc = await connect(await ticket("pc"));
    const pcClosed = closeEvent(pc);
    pc.send("plaintext");
    expect((await pcClosed).code).toBe(1003);
  });

  it("serializes every routing fact into hibernatable socket attachments", async () => {
    expect((await register()).status).toBe(204);
    await connect(await ticket("pc"));
    const phone = await connect(await ticket("phone"));
    const peer = new Uint8Array(await receive(phone));
    const stub = env.RELAY_ROUTES.getByName(ROUTE);

    const attachments = await runInDurableObject(stub, (_instance, state) => ({
      pc: state
        .getWebSockets("pc")
        .map((socket) => socket.deserializeAttachment()),
      phone: state
        .getWebSockets("phone")
        .map((socket) => socket.deserializeAttachment()),
    }));

    expect(attachments.pc).toEqual([{ role: "pc" }]);
    expect(attachments.phone).toHaveLength(1);
    expect(attachments.phone[0]).toMatchObject({ role: "phone" });
    expect(typeof attachments.phone[0]?.peer).toBe("string");
    expect(peer.byteLength).toBe(32);
  });
});

async function register(secret = SECRET): Promise<Response> {
  return exports.default.fetch(`https://relay.test/v1/routes/${ROUTE}`, {
    method: "PUT",
    headers: { Authorization: `Bearer ${secret}` },
  });
}

async function ticket(role: "pc" | "phone"): Promise<string> {
  const response = await exports.default.fetch(
    `https://relay.test/v1/routes/${ROUTE}/tickets`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${SECRET}`,
        "Content-Type": "application/json",
        Origin: ORIGIN,
      },
      body: JSON.stringify({ role }),
    },
  );
  expect(response.status).toBe(201);
  const body = (await response.json()) as { ticket: string };
  return body.ticket;
}

async function connect(ticketValue: string): Promise<WebSocket> {
  const response = await connectResponse(ticketValue);
  expect(response.status).toBe(101);
  const socket = response.webSocket;
  expect(socket).toBeDefined();
  if (socket !== undefined && socket !== null) {
    socket.binaryType = "arraybuffer";
  }
  socket?.accept();
  if (socket === undefined || socket === null) {
    throw new Error("upgrade response omitted its socket");
  }
  openSockets.push(socket);
  return socket;
}

async function connectResponse(ticketValue: string): Promise<Response> {
  return exports.default.fetch(
    `https://relay.test/v1/routes/${ROUTE}/connect`,
    {
      headers: {
        Upgrade: "websocket",
        Origin: ORIGIN,
        "Sec-WebSocket-Protocol": `runtrol.relay.v1, runtrol.ticket.${ticketValue}`,
      },
    },
  );
}

function receive(socket: WebSocket): Promise<ArrayBuffer> {
  return new Promise((resolve) => {
    socket.addEventListener(
      "message",
      (event) => resolve(event.data as ArrayBuffer),
      {
        once: true,
      },
    );
  });
}

function closeEvent(socket: WebSocket): Promise<CloseEvent> {
  return new Promise((resolve) => {
    socket.addEventListener("close", resolve, { once: true });
  });
}

async function storedState(): Promise<{
  credentialRows: number;
  ticketRows: number;
  storedText: string;
}> {
  const stub = env.RELAY_ROUTES.getByName(ROUTE);
  return runInDurableObject(stub, (_instance, state) => {
    const credentialRows = state.storage.sql
      .exec<{ count: number }>(
        "SELECT COUNT(*) AS count FROM route_credentials",
      )
      .one().count;
    const ticketRows = state.storage.sql
      .exec<{ count: number }>(
        "SELECT COUNT(*) AS count FROM connection_tickets",
      )
      .one().count;
    const storedText = state.storage.sql
      .exec<{ stored: string }>(
        "SELECT hex(secret_digest) AS stored FROM route_credentials",
      )
      .toArray()
      .map((row) => row.stored)
      .join("\n");
    return { credentialRows, ticketRows, storedText };
  });
}
