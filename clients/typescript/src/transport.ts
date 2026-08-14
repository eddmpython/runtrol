import { once } from "node:events";
import { createConnection, type Socket } from "node:net";

import { PUBLIC_LIMITS } from "./generated/protocol.js";
import { RuntimeProtocolError, RuntimeTransportError } from "./errors.js";

export interface RuntimeTransport {
  send(payload: Uint8Array): Promise<void>;
  receive(): Promise<Uint8Array>;
  close(): void;
}

export type RuntimeTransportFactory = (endpoint: string) => Promise<RuntimeTransport>;

export async function connectLocalTransport(endpoint: string): Promise<RuntimeTransport> {
  const socket = createConnection(endpoint);
  try {
    await Promise.race([
      once(socket, "connect"),
      once(socket, "error").then(([error]) => Promise.reject(error)),
    ]);
  } catch (error) {
    socket.destroy();
    throw new RuntimeTransportError("could not connect to the local Runtime endpoint", {
      cause: error,
    });
  }
  return new FramedSocket(socket);
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
    this.#socket.destroy();
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
