import { DurableObject } from "cloudflare:workers";

const ROUTE_PATTERN = /^[A-Za-z0-9_-]{43}$/;
const SECRET_PATTERN = /^[A-Za-z0-9_-]{43}$/;
const RELAY_PROTOCOL = "runtrol.relay.v1";
const TICKET_PREFIX = "runtrol.ticket.";
const TICKET_LIFETIME_MS = 30_000;
const MAX_LIVE_TICKETS = 16;
const MAX_PHONE_CONNECTIONS = 8;
const MAX_ENCRYPTED_RECORD_WIRE = 65_538;
const PEER_ID_BYTES = 16;

type Role = "pc" | "phone";

type TicketResult =
  | { ok: true; ticket: string; expiresAt: number }
  | { ok: false; status: number; reason: string };

type RegisterResult =
  { ok: true } | { ok: false; status: number; reason: string };

type SocketAttachment = {
  role: Role;
  peer?: string;
};

export default {
  async fetch(request, env): Promise<Response> {
    const url = new URL(request.url);
    const originRefusal = admitOrigin(request, env.PWA_ORIGIN);
    if (originRefusal !== undefined) {
      return originRefusal;
    }

    if (request.method === "OPTIONS") {
      return preflight(request, env.PWA_ORIGIN);
    }

    const route = parseRoute(url.pathname);
    if (route === undefined) {
      return answer(404, "not found", request, env.PWA_ORIGIN);
    }

    const stub = env.RELAY_ROUTES.getByName(route);
    if (request.method === "PUT" && url.pathname === `/v1/routes/${route}`) {
      const secret = bearer(request);
      if (secret === undefined) {
        return answer(
          401,
          "route credential required",
          request,
          env.PWA_ORIGIN,
        );
      }
      const result = await stub.register(secret);
      return result.ok
        ? answer(204, null, request, env.PWA_ORIGIN)
        : answer(result.status, result.reason, request, env.PWA_ORIGIN);
    }

    if (
      request.method === "POST" &&
      url.pathname === `/v1/routes/${route}/tickets`
    ) {
      const secret = bearer(request);
      if (secret === undefined) {
        return answer(
          401,
          "route credential required",
          request,
          env.PWA_ORIGIN,
        );
      }
      const role = await readRole(request);
      if (role === undefined) {
        return answer(400, "role must be pc or phone", request, env.PWA_ORIGIN);
      }
      const result = await stub.createTicket(secret, role);
      if (!result.ok) {
        return answer(result.status, result.reason, request, env.PWA_ORIGIN);
      }
      return json(
        201,
        { ticket: result.ticket, expiresAt: result.expiresAt },
        request,
        env.PWA_ORIGIN,
      );
    }

    if (
      request.method === "GET" &&
      url.pathname === `/v1/routes/${route}/connect` &&
      url.search === ""
    ) {
      return stub.fetch(request);
    }

    return answer(404, "not found", request, env.PWA_ORIGIN);
  },
} satisfies ExportedHandler<Env>;

export class RelayRoute extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS route_credentials (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        secret_digest BLOB NOT NULL,
        created_at INTEGER NOT NULL
      );
      CREATE TABLE IF NOT EXISTS connection_tickets (
        ticket_digest BLOB PRIMARY KEY,
        role TEXT NOT NULL CHECK (role IN ('pc', 'phone')),
        expires_at INTEGER NOT NULL
      );
      CREATE INDEX IF NOT EXISTS connection_tickets_expiry
        ON connection_tickets (expires_at);
    `);
  }

  async register(secret: string): Promise<RegisterResult> {
    if (!SECRET_PATTERN.test(secret)) {
      return { ok: false, status: 400, reason: "invalid route credential" };
    }
    const digest = await sha256(secret);
    const rows = this.ctx.storage.sql
      .exec<{ secret_digest: ArrayBuffer }>(
        "SELECT secret_digest FROM route_credentials WHERE singleton = 1",
      )
      .toArray();
    const existing = rows[0];
    if (existing !== undefined) {
      return timingSafeEqual(existing.secret_digest, digest)
        ? { ok: true }
        : { ok: false, status: 403, reason: "route credential refused" };
    }
    this.ctx.storage.sql.exec(
      "INSERT INTO route_credentials (singleton, secret_digest, created_at) VALUES (1, ?, ?)",
      digest,
      Date.now(),
    );
    return { ok: true };
  }

  async createTicket(secret: string, role: Role): Promise<TicketResult> {
    if (!SECRET_PATTERN.test(secret)) {
      return { ok: false, status: 403, reason: "route credential refused" };
    }
    const secretDigest = await sha256(secret);
    const rows = this.ctx.storage.sql
      .exec<{ secret_digest: ArrayBuffer }>(
        "SELECT secret_digest FROM route_credentials WHERE singleton = 1",
      )
      .toArray();
    const existing = rows[0];
    if (
      existing === undefined ||
      !timingSafeEqual(existing.secret_digest, secretDigest)
    ) {
      return { ok: false, status: 403, reason: "route credential refused" };
    }

    const now = Date.now();
    this.ctx.storage.sql.exec(
      "DELETE FROM connection_tickets WHERE expires_at <= ?",
      now,
    );
    const count = this.ctx.storage.sql
      .exec<{ count: number }>(
        "SELECT COUNT(*) AS count FROM connection_tickets",
      )
      .one().count;
    if (count >= MAX_LIVE_TICKETS) {
      return { ok: false, status: 429, reason: "too many live tickets" };
    }

    const ticket = randomToken();
    const ticketDigest = await sha256(ticket);
    const expiresAt = now + TICKET_LIFETIME_MS;
    this.ctx.storage.sql.exec(
      "INSERT INTO connection_tickets (ticket_digest, role, expires_at) VALUES (?, ?, ?)",
      ticketDigest,
      role,
      expiresAt,
    );
    return { ok: true, ticket, expiresAt };
  }

  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return new Response("websocket upgrade required", { status: 426 });
    }
    const ticket = parseTicketProtocol(
      request.headers.get("Sec-WebSocket-Protocol"),
    );
    if (ticket === undefined) {
      return new Response("valid relay ticket required", { status: 401 });
    }
    const role = await this.consumeTicket(ticket);
    if (role === undefined) {
      return new Response("relay ticket refused", { status: 401 });
    }
    if (role === "pc" && this.ctx.getWebSockets("pc").length !== 0) {
      return new Response("pc already connected", { status: 409 });
    }
    if (
      role === "phone" &&
      this.ctx.getWebSockets("phone").length >= MAX_PHONE_CONNECTIONS
    ) {
      return new Response("too many phones", { status: 429 });
    }

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    const peer = role === "phone" ? randomPeer() : undefined;
    const attachment: SocketAttachment =
      peer === undefined ? { role } : { role, peer };
    server.serializeAttachment(attachment);
    this.ctx.acceptWebSocket(server, [role]);
    if (peer !== undefined) {
      server.send(decodePeer(peer));
    }
    return new Response(null, {
      status: 101,
      webSocket: client,
      headers: { "Sec-WebSocket-Protocol": RELAY_PROTOCOL },
    });
  }

  webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): void {
    const attachment =
      socket.deserializeAttachment() as SocketAttachment | null;
    if (attachment === null || !isRole(attachment.role)) {
      socket.close(1008, "missing relay role");
      return;
    }
    if (typeof message === "string") {
      socket.close(1003, "binary ciphertext required");
      return;
    }
    if (attachment.role === "phone") {
      this.forwardFromPhone(socket, attachment, message);
    } else {
      this.forwardFromPc(socket, message);
    }
  }

  webSocketError(socket: WebSocket): void {
    this.reportPhoneClose(socket);
    socket.close(1011, "relay socket failed");
  }

  webSocketClose(socket: WebSocket): void {
    this.reportPhoneClose(socket);
    socket.close(1000, "relay peer closed");
  }

  private async consumeTicket(ticket: string): Promise<Role | undefined> {
    const digest = await sha256(ticket);
    const now = Date.now();
    this.ctx.storage.sql.exec(
      "DELETE FROM connection_tickets WHERE expires_at <= ?",
      now,
    );
    const rows = this.ctx.storage.sql
      .exec<{ role: string }>(
        "SELECT role FROM connection_tickets WHERE ticket_digest = ? AND expires_at > ?",
        digest,
        now,
      )
      .toArray();
    this.ctx.storage.sql.exec(
      "DELETE FROM connection_tickets WHERE ticket_digest = ?",
      digest,
    );
    const role = rows[0]?.role;
    return isRole(role) ? role : undefined;
  }

  private forwardFromPhone(
    socket: WebSocket,
    attachment: SocketAttachment,
    message: ArrayBuffer,
  ): void {
    if (
      message.byteLength > MAX_ENCRYPTED_RECORD_WIRE ||
      attachment.peer === undefined
    ) {
      socket.close(1009, "ciphertext record too large");
      return;
    }
    const targets = this.ctx.getWebSockets("pc");
    if (targets.length === 0) {
      socket.close(1013, "pc offline");
      return;
    }
    const envelope = new Uint8Array(PEER_ID_BYTES + message.byteLength);
    envelope.set(decodePeer(attachment.peer));
    envelope.set(new Uint8Array(message), PEER_ID_BYTES);
    sendAll(targets, envelope.buffer);
  }

  private forwardFromPc(socket: WebSocket, message: ArrayBuffer): void {
    if (
      message.byteLength <= PEER_ID_BYTES ||
      message.byteLength > PEER_ID_BYTES + MAX_ENCRYPTED_RECORD_WIRE
    ) {
      socket.close(1009, "routed ciphertext record has invalid size");
      return;
    }
    const envelope = new Uint8Array(message);
    const peer = encodePeer(envelope.slice(0, PEER_ID_BYTES));
    const target = this.ctx
      .getWebSockets("phone")
      .find((candidate) => candidate.deserializeAttachment()?.peer === peer);
    if (target === undefined) {
      socket.close(1008, "relay peer is not connected");
      return;
    }
    try {
      target.send(envelope.slice(PEER_ID_BYTES).buffer);
    } catch {
      target.close(1011, "relay send failed");
    }
  }

  private reportPhoneClose(socket: WebSocket): void {
    const attachment =
      socket.deserializeAttachment() as SocketAttachment | null;
    if (attachment?.role !== "phone" || attachment.peer === undefined) {
      return;
    }
    sendAll(this.ctx.getWebSockets("pc"), decodePeer(attachment.peer).buffer);
  }
}

function parseRoute(pathname: string): string | undefined {
  const segments = pathname.split("/");
  const route = segments[3];
  if (
    segments[0] !== "" ||
    segments[1] !== "v1" ||
    segments[2] !== "routes" ||
    route === undefined ||
    !ROUTE_PATTERN.test(route)
  ) {
    return undefined;
  }
  return route;
}

function bearer(request: Request): string | undefined {
  const value = request.headers.get("Authorization");
  if (value === null || !value.startsWith("Bearer ")) {
    return undefined;
  }
  const secret = value.slice("Bearer ".length);
  return SECRET_PATTERN.test(secret) ? secret : undefined;
}

async function readRole(request: Request): Promise<Role | undefined> {
  if (request.headers.get("Content-Type") !== "application/json") {
    return undefined;
  }
  try {
    const encoded = await readBoundedBody(request, 64);
    if (encoded === undefined) {
      return undefined;
    }
    const value = JSON.parse(new TextDecoder().decode(encoded)) as {
      role?: unknown;
    };
    return isRole(value.role) ? value.role : undefined;
  } catch {
    return undefined;
  }
}

async function readBoundedBody(
  request: Request,
  limit: number,
): Promise<Uint8Array | undefined> {
  const declared = request.headers.get("Content-Length");
  if (
    declared !== null &&
    (!/^\d+$/.test(declared) || Number(declared) > limit)
  ) {
    return undefined;
  }
  if (request.body === null) {
    return undefined;
  }
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const read = await reader.read();
    if (read.done) {
      break;
    }
    length += read.value.byteLength;
    if (length > limit) {
      await reader.cancel("request body is too large");
      return undefined;
    }
    chunks.push(read.value);
  }
  const body = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

function isRole(value: unknown): value is Role {
  return value === "pc" || value === "phone";
}

function parseTicketProtocol(header: string | null): string | undefined {
  if (header === null) {
    return undefined;
  }
  const protocols = header.split(",").map((protocol) => protocol.trim());
  if (protocols.length !== 2 || protocols[0] !== RELAY_PROTOCOL) {
    return undefined;
  }
  const ticketProtocol = protocols[1];
  if (!ticketProtocol?.startsWith(TICKET_PREFIX)) {
    return undefined;
  }
  const ticket = ticketProtocol.slice(TICKET_PREFIX.length);
  return SECRET_PATTERN.test(ticket) ? ticket : undefined;
}

function admitOrigin(request: Request, allowed: string): Response | undefined {
  const origin = request.headers.get("Origin");
  if (origin !== null && origin !== allowed) {
    return new Response("origin refused", { status: 403 });
  }
  return undefined;
}

function preflight(request: Request, allowed: string): Response {
  if (
    request.headers.get("Origin") !== allowed ||
    request.headers.get("Access-Control-Request-Method") !== "POST"
  ) {
    return new Response("preflight refused", { status: 403 });
  }
  return new Response(null, {
    status: 204,
    headers: corsHeaders(allowed),
  });
}

function answer(
  status: number,
  body: string | null,
  request: Request,
  allowed: string,
): Response {
  const headers = responseHeaders(request, allowed);
  return new Response(body, { status, headers });
}

function json(
  status: number,
  body: unknown,
  request: Request,
  allowed: string,
): Response {
  const headers = responseHeaders(request, allowed);
  headers.set("Content-Type", "application/json");
  return new Response(JSON.stringify(body), { status, headers });
}

function corsHeaders(origin: string): HeadersInit {
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Authorization, Content-Type",
    "Access-Control-Max-Age": "600",
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    Vary: "Origin",
  };
}

function responseHeaders(request: Request, allowed: string): Headers {
  const headers = new Headers({
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  if (request.headers.get("Origin") === allowed) {
    for (const [name, value] of new Headers(corsHeaders(allowed))) {
      headers.set(name, value);
    }
  }
  return headers;
}

async function sha256(value: string): Promise<ArrayBuffer> {
  return crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
}

function timingSafeEqual(left: ArrayBuffer, right: ArrayBuffer): boolean {
  const subtle = crypto.subtle as SubtleCrypto & {
    timingSafeEqual(a: ArrayBuffer, b: ArrayBuffer): boolean;
  };
  return subtle.timingSafeEqual(left, right);
}

function randomToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return encodeBase64Url(bytes);
}

function randomPeer(): string {
  return encodePeer(crypto.getRandomValues(new Uint8Array(PEER_ID_BYTES)));
}

function encodePeer(peer: Uint8Array): string {
  if (peer.byteLength !== PEER_ID_BYTES) {
    throw new Error("relay peer id has invalid size");
  }
  return encodeBase64Url(peer);
}

function decodePeer(peer: string): Uint8Array<ArrayBuffer> {
  const binary = atob(peer.replaceAll("-", "+").replaceAll("_", "/"));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  if (bytes.byteLength !== PEER_ID_BYTES) {
    throw new Error("stored relay peer id has invalid size");
  }
  return bytes;
}

function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
}

function sendAll(targets: WebSocket[], message: ArrayBuffer): void {
  for (const target of targets) {
    try {
      target.send(message);
    } catch {
      target.close(1011, "relay send failed");
    }
  }
}
