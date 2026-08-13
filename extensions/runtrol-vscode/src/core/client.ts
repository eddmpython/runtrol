import { FrameTransport } from "./framing";
import type { CoreLocator } from "./locator";
import {
  type Request,
  type Response,
  readResponse,
  requestHello,
} from "../protocol";

type Connected = {
  transport: FrameTransport;
};

export class CoreClient {
  private commandConnection: Promise<Connected> | null = null;
  private commandTail: Promise<void> = Promise.resolve();

  constructor(private readonly locator: CoreLocator) {}

  async ensureRuntime(): Promise<void> {
    await this.command();
  }

  once(request: Request): Promise<{ response: Response }> {
    return this.serial(async () => {
      const connected = await this.command();
      try {
        await connected.transport.send(request);
        const response = readResponse(JSON.parse((await connected.transport.receive()).toString("utf8")));
        return { response };
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
      return { transport };
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
