import { FrameTransport } from "./framing";
import { CoreLocator } from "./locator";
import {
  type ProviderLine,
  type Request,
  type Response,
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
  event: (payload: unknown, nextExpected: WatchCursor) => void;
  gap: (nextExpected: WatchCursor, message: string) => void;
};

export class CoreClient {
  constructor(private readonly locator: CoreLocator) {}

  async once(request: Request): Promise<{ response: Response; providers: ProviderLine[] }> {
    const connected = await this.connect();
    try {
      await connected.transport.send(request);
      const response = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
      return { response, providers: connected.providers };
    } finally {
      connected.transport.close();
    }
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
}

