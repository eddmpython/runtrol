import type { RuntimeTransport, RuntimeTransportFactory } from "./transport.js";
export {
  runtimeLocatorAtForTesting as runtimeLocatorAt,
  validatedLocatorForTesting as validatedLocator,
} from "./locator.js";

export class ScriptedRuntimeTransport implements RuntimeTransport {
  public readonly sent: Uint8Array[] = [];
  readonly #incoming: Uint8Array[];
  #closed = false;

  public constructor(incoming: ReadonlyArray<unknown>) {
    const encoder = new TextEncoder();
    this.#incoming = incoming.map((value) => encoder.encode(JSON.stringify(value)));
  }

  public async send(payload: Uint8Array): Promise<void> {
    if (this.#closed) throw new Error("scripted Runtime transport is closed");
    this.sent.push(Uint8Array.from(payload));
  }

  public async receive(): Promise<Uint8Array> {
    if (this.#closed) throw new Error("scripted Runtime transport is closed");
    const value = this.#incoming.shift();
    if (!value) throw new Error("scripted Runtime transport has no next frame");
    return value;
  }

  public close(): void {
    this.#closed = true;
  }
}

export function scriptedTransportFactory(
  transport: ScriptedRuntimeTransport,
): RuntimeTransportFactory {
  return async () => transport;
}
