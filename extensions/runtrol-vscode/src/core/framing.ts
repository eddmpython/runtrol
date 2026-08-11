import net from "node:net";

export const MAX_FRAME_BYTES = 16 * 1024 * 1024 + 64 * 1024;
export const MAX_QUEUED_FRAMES = 64;
export const MAX_QUEUED_BYTES = 2 * 1024 * 1024;
const HEADER_BYTES = 4;
const DECODE_BATCH = 64;

export function encodeFrame(payload: Buffer): Buffer {
  if (payload.length > MAX_FRAME_BYTES) {
    throw new Error(`frame ${payload.length} exceeds ${MAX_FRAME_BYTES}`);
  }
  const frame = Buffer.allocUnsafe(HEADER_BYTES + payload.length);
  frame.writeUInt32BE(payload.length, 0);
  payload.copy(frame, HEADER_BYTES);
  return frame;
}

export class FrameDecoder {
  private readonly chunks: Buffer[] = [];
  private bytes = 0;

  push(chunk: Buffer): void {
    if (chunk.length > 0) {
      this.chunks.push(chunk);
      this.bytes += chunk.length;
    }
    this.validateHeader();
  }

  get bufferedBytes(): number {
    return this.bytes;
  }

  take(maxFrames = DECODE_BATCH): Buffer[] {
    const frames: Buffer[] = [];
    while (frames.length < maxFrames) {
      const length = this.header();
      if (length === null || this.bytes < HEADER_BYTES + length) {
        break;
      }
      this.takeBytes(HEADER_BYTES);
      frames.push(this.takeBytes(length));
      this.validateHeader();
    }
    return frames;
  }

  hasCompleteFrame(): boolean {
    const length = this.header();
    return length !== null && this.bytes >= HEADER_BYTES + length;
  }

  private validateHeader(): void {
    const length = this.header();
    if (length !== null && length > MAX_FRAME_BYTES) {
      throw new Error(`frame ${length} exceeds ${MAX_FRAME_BYTES}`);
    }
  }

  private header(): number | null {
    if (this.bytes < HEADER_BYTES) {
      return null;
    }
    let value = 0;
    let remaining = HEADER_BYTES;
    for (const chunk of this.chunks) {
      const take = Math.min(remaining, chunk.length);
      for (let index = 0; index < take; index += 1) {
        value = (value << 8) | chunk[index];
      }
      remaining -= take;
      if (remaining === 0) {
        return value >>> 0;
      }
    }
    return null;
  }

  private takeBytes(length: number): Buffer {
    if (length === 0) {
      return Buffer.alloc(0);
    }
    const first = this.chunks[0];
    if (first && first.length >= length) {
      const result = first.subarray(0, length);
      if (first.length === length) {
        this.chunks.shift();
      } else {
        this.chunks[0] = first.subarray(length);
      }
      this.bytes -= length;
      return result;
    }

    const result = Buffer.allocUnsafe(length);
    let written = 0;
    while (written < length) {
      const chunk = this.chunks[0];
      if (!chunk) {
        throw new Error("frame decoder lost buffered bytes");
      }
      const take = Math.min(length - written, chunk.length);
      chunk.copy(result, written, 0, take);
      written += take;
      if (take === chunk.length) {
        this.chunks.shift();
      } else {
        this.chunks[0] = chunk.subarray(take);
      }
    }
    this.bytes -= length;
    return result;
  }
}

type Waiting = {
  resolve: (payload: Buffer) => void;
  reject: (error: Error) => void;
};

export class FrameTransport {
  private readonly decoder = new FrameDecoder();
  private readonly queued: Buffer[] = [];
  private queuedBytes = 0;
  private waiting: Waiting | null = null;
  private failure: Error | null = null;
  private draining = false;

  private constructor(private readonly socket: net.Socket) {
    socket.on("data", (chunk: Buffer) => this.onData(chunk));
    socket.once("error", (error) => this.fail(error));
    socket.once("close", () => this.fail(new Error("the daemon connection closed")));
  }

  static connect(endpoint: string, timeoutMs = 5_000): Promise<FrameTransport> {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection(endpoint);
      const timeout = setTimeout(() => {
        socket.destroy();
        reject(new Error(`the daemon did not accept a connection within ${timeoutMs} milliseconds`));
      }, timeoutMs);
      socket.once("connect", () => {
        clearTimeout(timeout);
        resolve(new FrameTransport(socket));
      });
      socket.once("error", (error) => {
        clearTimeout(timeout);
        reject(error);
      });
    });
  }

  send(value: unknown): Promise<void> {
    if (this.failure) {
      return Promise.reject(this.failure);
    }
    const payload = Buffer.from(JSON.stringify(value));
    const frame = encodeFrame(payload);
    return new Promise((resolve, reject) => {
      this.socket.write(frame, (error) => (error ? reject(error) : resolve()));
    });
  }

  receive(): Promise<Buffer> {
    const first = this.queued.shift();
    if (first) {
      this.queuedBytes -= first.length;
      return Promise.resolve(first);
    }
    if (this.failure) {
      return Promise.reject(this.failure);
    }
    if (this.waiting) {
      return Promise.reject(new Error("only one frame receiver may wait on a connection"));
    }
    return new Promise((resolve, reject) => {
      this.waiting = { resolve, reject };
    });
  }

  close(): void {
    this.socket.destroy();
  }

  private onData(chunk: Buffer): void {
    try {
      this.decoder.push(chunk);
      this.drain();
    } catch (error) {
      this.fail(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private drain(): void {
    if (this.draining || this.failure) {
      return;
    }
    this.draining = true;
    try {
      for (const frame of this.decoder.take()) {
        this.deliver(frame);
      }
    } catch (error) {
      this.fail(error instanceof Error ? error : new Error(String(error)));
      return;
    } finally {
      this.draining = false;
    }
    if (this.decoder.hasCompleteFrame()) {
      setImmediate(() => this.drain());
    }
  }

  private deliver(frame: Buffer): void {
    const waiting = this.waiting;
    if (waiting) {
      this.waiting = null;
      waiting.resolve(frame);
      return;
    }
    const overFrames = this.queued.length >= MAX_QUEUED_FRAMES;
    const overBytes = this.queued.length > 0 && this.queuedBytes + frame.length > MAX_QUEUED_BYTES;
    if (overFrames || overBytes) {
      this.fail(new Error("the extension fell behind the bounded daemon stream"));
      return;
    }
    this.queued.push(frame);
    this.queuedBytes += frame.length;
  }

  private fail(error: Error): void {
    if (this.failure) {
      return;
    }
    this.failure = error;
    const waiting = this.waiting;
    this.waiting = null;
    waiting?.reject(error);
    this.socket.destroy();
  }
}

