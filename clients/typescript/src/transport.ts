import { createConnection, type Socket } from "node:net";

import { PUBLIC_LIMITS } from "./generated/protocol.js";
import { RuntimeProtocolError, RuntimeTransportError } from "./errors.js";

export interface RuntimeTransport {
  send(payload: Uint8Array): Promise<void>;
  receive(): Promise<Uint8Array>;
  close(): void;
  abort?(): void;
}

export type RuntimeTransportFactory = (
  endpoint: string,
  signal?: AbortSignal,
) => Promise<RuntimeTransport>;

const LOCAL_CONNECT_TIMEOUT_MS = 250;

export async function connectLocalTransport(
  endpoint: string,
  signal?: AbortSignal,
): Promise<RuntimeTransport> {
  signal?.throwIfAborted();
  const socket = createConnection(endpoint);
  try {
    await waitForConnection(socket, signal);
  } catch (error) {
    socket.destroy();
    if (signal?.aborted) {
      throw signal.reason ?? new RuntimeTransportError("local Runtime connection was aborted");
    }
    if (error instanceof RuntimeTransportError) throw error;
    throw new RuntimeTransportError("could not connect to the local Runtime endpoint", {
      cause: error,
    });
  }
  return new FramedSocket(socket);
}

function waitForConnection(socket: Socket, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      finish(new RuntimeTransportError(
        `local Runtime connection exceeded ${LOCAL_CONNECT_TIMEOUT_MS} milliseconds`,
      ));
    }, LOCAL_CONNECT_TIMEOUT_MS);
    const connected = (): void => finish();
    const failed = (error: Error): void => finish(error);
    const aborted = (): void => finish(signal?.reason ?? new RuntimeTransportError(
      "local Runtime connection was aborted",
    ));
    const finish = (error?: unknown): void => {
      clearTimeout(timeout);
      socket.removeListener("connect", connected);
      socket.removeListener("error", failed);
      signal?.removeEventListener("abort", aborted);
      if (error === undefined) resolve();
      else reject(error);
    };
    socket.once("connect", connected);
    socket.once("error", failed);
    signal?.addEventListener("abort", aborted, { once: true });
    if (signal?.aborted) aborted();
  });
}

class FramedSocket implements RuntimeTransport {
  readonly #socket: Socket;
  readonly #incoming: AsyncIterator<Buffer>;
  #buffer = Buffer.alloc(0);

  public constructor(socket: Socket) {
    this.#socket = socket;
    this.#incoming = socket[Symbol.asyncIterator]();
  }

  public async send(payload: Uint8Array): Promise<void> {
    if (payload.byteLength > PUBLIC_LIMITS.maxFrameBytes) {
      throw new RuntimeProtocolError(
        `frame has ${payload.byteLength} bytes, above ${PUBLIC_LIMITS.maxFrameBytes}`,
      );
    }
    const frame = Buffer.allocUnsafe(4 + payload.byteLength);
    frame.writeUInt32BE(payload.byteLength);
    frame.set(payload, 4);
    await this.#write(frame);
  }

  public async receive(): Promise<Uint8Array> {
    const header = await this.#readExact(4);
    const length = header.readUInt32BE(0);
    if (length > PUBLIC_LIMITS.maxFrameBytes) {
      throw new RuntimeProtocolError(
        `Runtime announced ${length} frame bytes, above ${PUBLIC_LIMITS.maxFrameBytes}`,
      );
    }
    return this.#readExact(length);
  }

  public close(): void {
    this.#socket.end();
  }

  public abort(): void {
    // A stream cancellation has to wake a pending read immediately, but a raw reset leaves the Runtime's
    // named-pipe accept pool cleaning up behind a rapid sequence of tab switches. `destroySoon` drains the
    // already-written protocol bytes, sends the peer an orderly end, and then tears down this unread side
    // without waiting for the peer to answer.
    this.#socket.destroySoon();
  }

  async #write(payload: Uint8Array): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.#socket.write(payload, (error) => {
        if (error) reject(new RuntimeTransportError("Runtime frame write failed", { cause: error }));
        else resolve();
      });
    });
  }

  async #readExact(length: number): Promise<Buffer> {
    while (this.#buffer.byteLength < length) {
      let next: IteratorResult<Buffer>;
      try {
        next = await this.#incoming.next();
      } catch (error) {
        throw new RuntimeTransportError("Runtime frame read failed", { cause: error });
      }
      if (next.done) throw new RuntimeTransportError("Runtime closed during a frame");
      this.#buffer = Buffer.concat([this.#buffer, next.value]);
    }
    const answer = this.#buffer.subarray(0, length);
    this.#buffer = this.#buffer.subarray(length);
    return answer;
  }
}
