import {
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  sign,
  type KeyObject,
} from "node:crypto";

import type { IntegrationGrant } from "./generated/protocol.js";
import { RuntimeProtocolError } from "./errors.js";

const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

export class IntegrationIdentity {
  readonly #privateKey: KeyObject;

  private constructor(privateKey: KeyObject) {
    if (privateKey.asymmetricKeyType !== "ed25519") {
      throw new RuntimeProtocolError("integration identity is not an Ed25519 private key");
    }
    this.#privateKey = privateKey;
  }

  public static generate(): IntegrationIdentity {
    const { privateKey } = generateKeyPairSync("ed25519");
    return new IntegrationIdentity(privateKey);
  }

  public static fromPkcs8(bytes: Uint8Array): IntegrationIdentity {
    try {
      return new IntegrationIdentity(createPrivateKey({
        key: Buffer.from(bytes),
        format: "der",
        type: "pkcs8",
      }));
    } catch (error) {
      throw new RuntimeProtocolError(`integration private key is malformed: ${String(error)}`);
    }
  }

  public exportPkcs8(): Uint8Array {
    return this.#privateKey.export({ format: "der", type: "pkcs8" });
  }

  public publicKeyBase64(): string {
    const encoded = createPublicKey(this.#privateKey).export({ format: "der", type: "spki" });
    if (encoded.byteLength !== ED25519_SPKI_PREFIX.byteLength + 32
      || !encoded.subarray(0, ED25519_SPKI_PREFIX.byteLength).equals(ED25519_SPKI_PREFIX)) {
      throw new RuntimeProtocolError("integration public key has an unexpected Ed25519 encoding");
    }
    return encoded.subarray(ED25519_SPKI_PREFIX.byteLength).toString("base64url");
  }

  public signBase64(payload: Uint8Array): string {
    return sign(null, payload, this.#privateKey).toString("base64url");
  }
}

export class IntegrationCredentials {
  public constructor(
    public readonly identity: IntegrationIdentity,
    public readonly grant: IntegrationGrant,
  ) {}
}
