import { FrameTransport } from "./framing";
import type { CoreLocator } from "./locator";
import {
  type ProviderLine,
  type Request,
  type Response,
  type SessionListing,
  type WatchCursor,
  failureMessage,
  readResponse,
  requestHello,
} from "../protocol";

type Connected = {
  transport: FrameTransport;
  providers: ProviderLine[];
};

export type WatchHandlers = {
  started?: () => void;
  event: (payload: unknown, nextExpected: WatchCursor) => void;
  gap: (nextExpected: WatchCursor, message: string) => void;
};

export type SessionIndexHandlers = {
  snapshot: (listing: SessionListing, providers: readonly ProviderLine[]) => void;
};

export class CoreClient {
  private commandConnection: Promise<Connected> | null = null;
  private commandTail: Promise<void> = Promise.resolve();

  constructor(private readonly locator: CoreLocator) {}

  async ensureRuntime(): Promise<void> {
    await this.command();
  }

  once(request: Request): Promise<{ response: Response; providers: ProviderLine[] }> {
    return this.serial(async () => {
      const connected = await this.command();
      try {
        await connected.transport.send(request);
        const response = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
        return { response, providers: connected.providers };
      } catch (error) {
        this.dropCommandConnection();
        throw error;
      }
    });
  }

  reset(): Promise<void> {
    return this.serial(async () => this.dropCommandConnection());
  }

  dispose(): void {
    this.dropCommandConnection();
  }

  async watch(
    session: string,
    after: WatchCursor | null,
    handlers: WatchHandlers,
    signal: AbortSignal,
  ): Promise<void> {
    const connected = await this.connect();
    const abort = () => connected.transport.close();
    signal.addEventListener("abort", abort, { once: true });
    try {
      await connected.transport.send({ ask: "watch", with: { session, after } });
      const started = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
      if (started.say === "failed") {
        throw new Error(started.with.message);
      }
      if (started.say !== "watching") {
        throw new Error(`the daemon answered watch with ${started.say}`);
      }
      handlers.started?.();
      if (started.with.gap) {
        handlers.gap(started.with.starts_at, "The bounded replay window has a gap.");
      }

      while (!signal.aborted) {
        const response = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
        if (response.say === "event") {
          handlers.event(response.with.payload, response.with.next_expected);
          continue;
        }
        if (response.say === "lagged") {
          handlers.gap(response.with.next_expected, "The active view fell behind the bounded stream.");
          return;
        }
        const failed = failureMessage(response);
        if (failed) {
          throw new Error(failed);
        }
      }
    } catch (error) {
      if (!signal.aborted) {
        throw error;
      }
    } finally {
      signal.removeEventListener("abort", abort);
      connected.transport.close();
    }
  }

  async watchSessions(handlers: SessionIndexHandlers, signal: AbortSignal): Promise<void> {
    const connected = await this.connect();
    const abort = () => connected.transport.close();
    signal.addEventListener("abort", abort, { once: true });
    try {
      await connected.transport.send({ ask: "watchSessions" });
      const started = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
      if (started.say === "failed") {
        throw new Error(started.with.message);
      }
      if (started.say !== "watchingSessions") {
        throw new Error(`the daemon answered session watch with ${started.say}`);
      }

      while (!signal.aborted) {
        const response = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
        if (response.say === "sessions") {
          handlers.snapshot(response.with, connected.providers);
          continue;
        }
        const failed = failureMessage(response);
        throw new Error(failed ?? `the session watch received ${response.say}`);
      }
    } catch (error) {
      if (!signal.aborted) {
        throw error;
      }
    } finally {
      signal.removeEventListener("abort", abort);
      connected.transport.close();
    }
  }

  private async connect(): Promise<Connected> {
    const located = await this.locator.locate();
    const transport = await FrameTransport.connect(located.endpoint);
    try {
      await transport.send(requestHello());
      const welcome = readResponse(JSON.parse((await transport.receive()).toString("utf8")));
      if (welcome.say === "failed") {
        throw new Error(welcome.with.message);
      }
      if (welcome.say !== "welcome") {
        throw new Error(`the daemon greeted with ${welcome.say}`);
      }
      return { transport, providers: welcome.with.providers };
    } catch (error) {
      transport.close();
      throw error;
    }
  }

  private command(): Promise<Connected> {
    this.commandConnection ??= this.connect().catch((error: unknown) => {
      this.commandConnection = null;
      throw error;
    });
    return this.commandConnection;
  }

  private serial<T>(action: () => Promise<T>): Promise<T> {
    const result = this.commandTail.then(action);
    this.commandTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private dropCommandConnection(): void {
    const connected = this.commandConnection;
    this.commandConnection = null;
    void connected?.then(
      (value) => value.transport.close(),
      () => undefined,
    );
  }
}
