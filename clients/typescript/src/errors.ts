import type { RuntimeError } from "./generated/protocol.js";

export type LocatorErrorCode = "environment" | "malformed" | "unsafe" | "io";

export class RuntimeLocatorError extends Error {
  public constructor(
    public readonly code: LocatorErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "RuntimeLocatorError";
  }
}

export class RuntimeProtocolError extends Error {
  public constructor(message: string) {
    super(message);
    this.name = "RuntimeProtocolError";
  }
}

export class RuntimeTransportError extends Error {
  public constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "RuntimeTransportError";
  }
}

export class RuntimeRequestError extends Error {
  public constructor(public readonly failure: RuntimeError) {
    super(`${failure.code}: ${failure.message}`);
    this.name = "RuntimeRequestError";
  }
}
