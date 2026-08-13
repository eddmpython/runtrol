import { asBytes, base64UrlDecode, concat, equalBytes, utf8 } from "./bytes.js";

const HASH_LENGTH = 32;
const TAG_LENGTH = 16;
const EMPTY = new Uint8Array();
const SESSION_PATTERN = "Noise_IK_25519_AESGCM_SHA256";
const PAIRING_PATTERN = "Noise_IKpsk1_25519_AESGCM_SHA256";
const PAIRING_PROLOGUE = utf8("runtrol/pair/1");
const PAIRING_SALT = utf8("runtrol/pairing-psk/1");
const PAIRING_INFO = utf8(PAIRING_PATTERN);

export async function pairingInitiator(identity, remotePublic, qrSecret, ephemeral) {
  const psk = await hkdf(base64UrlDecode(qrSecret, 16), PAIRING_SALT, PAIRING_INFO, 32);
  return NoiseInitiator.create(
    PAIRING_PATTERN,
    identity,
    base64UrlDecode(remotePublic, 32),
    PAIRING_PROLOGUE,
    psk,
    ephemeral,
  );
}

export async function sessionInitiator(identity, remotePublic, relayOrigin, peerId, ephemeral) {
  const prologue = concat(utf8("runtrol/1"), Uint8Array.of(4), utf8(relayOrigin), asBytes(peerId));
  return NoiseInitiator.create(
    SESSION_PATTERN,
    identity,
    base64UrlDecode(remotePublic, 32),
    prologue,
    null,
    ephemeral,
  );
}

export class NoiseInitiator {
  static async create(pattern, identity, remoteStatic, prologue, psk = null, ephemeral = undefined) {
    const localStatic = await exportPublic(identity.publicKey);
    const remoteKey = await importPublic(remoteStatic);
    const symmetric = await SymmetricState.create(pattern, prologue, remoteStatic);
    return new NoiseInitiator(
      pattern,
      identity.privateKey,
      localStatic,
      remoteStatic,
      remoteKey,
      symmetric,
      psk,
      ephemeral,
    );
  }

  constructor(pattern, localPrivate, localStatic, remoteStatic, remoteKey, symmetric, psk, ephemeral) {
    this.pattern = pattern;
    this.localPrivate = localPrivate;
    this.localStatic = localStatic;
    this.remoteStatic = remoteStatic;
    this.remoteKey = remoteKey;
    this.symmetric = symmetric;
    this.psk = psk;
    this.ephemeral = ephemeral;
    this.sent = false;
  }

  async writeFirst(payload) {
    if (this.sent) throw new Error("Noise message one was already sent");
    this.sent = true;
    const ephemeral = this.ephemeral ?? await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"]);
    this.ephemeral = ephemeral;
    const ephemeralPublic = await exportPublic(ephemeral.publicKey);
    await this.symmetric.mixHash(ephemeralPublic);
    if (this.pattern === PAIRING_PATTERN) await this.symmetric.mixKey(ephemeralPublic);
    await this.symmetric.mixKey(await dh(ephemeral.privateKey, this.remoteKey));
    const encryptedStatic = await this.symmetric.encryptAndHash(this.localStatic);
    await this.symmetric.mixKey(await dh(this.localPrivate, this.remoteKey));
    if (this.pattern === PAIRING_PATTERN) await this.symmetric.mixKeyAndHash(this.psk);
    const encryptedPayload = await this.symmetric.encryptAndHash(asBytes(payload));
    return concat(ephemeralPublic, encryptedStatic, encryptedPayload);
  }

  async finish(message) {
    if (!this.sent || !this.ephemeral) throw new Error("Noise message one has not been sent");
    const bytes = asBytes(message);
    if (bytes.byteLength < 32 + TAG_LENGTH) throw new Error("Noise message two is too short");
    const remoteEphemeral = bytes.slice(0, 32);
    const remoteEphemeralKey = await importPublic(remoteEphemeral);
    await this.symmetric.mixHash(remoteEphemeral);
    if (this.pattern === PAIRING_PATTERN) await this.symmetric.mixKey(remoteEphemeral);
    await this.symmetric.mixKey(await dh(this.ephemeral.privateKey, remoteEphemeralKey));
    await this.symmetric.mixKey(await dh(this.localPrivate, remoteEphemeralKey));
    const payload = await this.symmetric.decryptAndHash(bytes.slice(32));
    const keys = await this.symmetric.split();
    return { payload, cipher: new TransportCipher(keys[0], keys[1]) };
  }
}

class SymmetricState {
  static async create(pattern, prologue, remoteStatic) {
    const protocol = utf8(pattern);
    const initial = protocol.byteLength <= HASH_LENGTH
      ? concat(protocol, new Uint8Array(HASH_LENGTH - protocol.byteLength))
      : await sha256(protocol);
    const state = new SymmetricState(initial, initial);
    await state.mixHash(prologue);
    await state.mixHash(remoteStatic);
    return state;
  }

  constructor(chainingKey, handshakeHash) {
    this.chainingKey = chainingKey;
    this.handshakeHash = handshakeHash;
    this.key = null;
    this.nonce = 0n;
  }

  async mixHash(data) {
    this.handshakeHash = await sha256(concat(this.handshakeHash, asBytes(data)));
  }

  async mixKey(input) {
    const output = await hkdf(input, this.chainingKey, EMPTY, 64);
    this.chainingKey = output.slice(0, 32);
    this.key = output.slice(32);
    this.nonce = 0n;
  }

  async mixKeyAndHash(input) {
    const output = await hkdf(input, this.chainingKey, EMPTY, 96);
    this.chainingKey = output.slice(0, 32);
    await this.mixHash(output.slice(32, 64));
    this.key = output.slice(64);
    this.nonce = 0n;
  }

  async encryptAndHash(plaintext) {
    const ciphertext = this.key === null
      ? asBytes(plaintext)
      : await aeadEncrypt(this.key, this.nonce, this.handshakeHash, plaintext);
    if (this.key !== null) this.nonce += 1n;
    await this.mixHash(ciphertext);
    return ciphertext;
  }

  async decryptAndHash(ciphertext) {
    const plaintext = this.key === null
      ? asBytes(ciphertext)
      : await aeadDecrypt(this.key, this.nonce, this.handshakeHash, ciphertext);
    if (this.key !== null) this.nonce += 1n;
    await this.mixHash(ciphertext);
    return plaintext;
  }

  async split() {
    const output = await hkdf(EMPTY, this.chainingKey, EMPTY, 64);
    return [output.slice(0, 32), output.slice(32)];
  }
}

export class TransportCipher {
  constructor(sendingKey, receivingKey) {
    this.sendingKey = sendingKey;
    this.receivingKey = receivingKey;
    this.sendingNonce = 0n;
    this.receivingNonce = 0n;
  }

  async encrypt(plaintext) {
    const ciphertext = await aeadEncrypt(this.sendingKey, this.sendingNonce, EMPTY, plaintext);
    this.sendingNonce += 1n;
    return ciphertext;
  }

  async decrypt(ciphertext) {
    const plaintext = await aeadDecrypt(this.receivingKey, this.receivingNonce, EMPTY, ciphertext);
    this.receivingNonce += 1n;
    return plaintext;
  }

  async rekeySending() {
    this.sendingKey = await rekey(this.sendingKey);
    this.sendingNonce = 0n;
  }

  async rekeyReceiving() {
    this.receivingKey = await rekey(this.receivingKey);
    this.receivingNonce = 0n;
  }
}

async function exportPublic(key) {
  const bytes = new Uint8Array(await crypto.subtle.exportKey("raw", key));
  if (bytes.byteLength !== 32) throw new Error("X25519 public key is not 32 bytes");
  return bytes;
}

async function importPublic(bytes) {
  return crypto.subtle.importKey("raw", asBytes(bytes), { name: "X25519" }, false, []);
}

async function dh(privateKey, publicKey) {
  const result = new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: publicKey }, privateKey, 256));
  if (equalBytes(result, new Uint8Array(32))) throw new Error("X25519 produced an invalid all-zero secret");
  return result;
}

async function sha256(value) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", asBytes(value)));
}

async function hkdf(input, salt, info, length) {
  const key = await crypto.subtle.importKey("raw", asBytes(input), "HKDF", false, ["deriveBits"]);
  return new Uint8Array(await crypto.subtle.deriveBits(
    { name: "HKDF", hash: "SHA-256", salt: asBytes(salt), info: asBytes(info) },
    key,
    length * 8,
  ));
}

async function aeadEncrypt(keyBytes, nonce, additionalData, plaintext) {
  const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["encrypt"]);
  return new Uint8Array(await crypto.subtle.encrypt(
    { name: "AES-GCM", iv: noiseNonce(nonce), additionalData: asBytes(additionalData), tagLength: 128 },
    key,
    asBytes(plaintext),
  ));
}

async function aeadDecrypt(keyBytes, nonce, additionalData, ciphertext) {
  const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["decrypt"]);
  try {
    return new Uint8Array(await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: noiseNonce(nonce), additionalData: asBytes(additionalData), tagLength: 128 },
      key,
      asBytes(ciphertext),
    ));
  } catch (error) {
    throw new Error("Noise authentication failed", { cause: error });
  }
}

function noiseNonce(value) {
  if (value < 0n || value > 0xffff_ffff_ffff_ffffn) throw new Error("Noise nonce was exhausted");
  const nonce = new Uint8Array(12);
  new DataView(nonce.buffer).setBigUint64(4, value, true);
  return nonce;
}

async function rekey(key) {
  const ciphertext = await aeadEncrypt(key, 0xffff_ffff_ffff_ffffn, EMPTY, new Uint8Array(32));
  return ciphertext.slice(0, 32);
}
