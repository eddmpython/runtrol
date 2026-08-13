import { asBytes, concat, littleEndianU32, readLittleEndianU32 } from "./bytes.js";

export const MAX_NOISE_PLAINTEXT = 65_519;
export const MAX_TRANSPORT_FRAME = 16 * 1024 * 1024 + 64 * 1024;
const CHUNK_HEADER_LENGTH = 10;
const RECORD_FRAME = 0x01;
const RECORD_REKEY = 0x08;

export function encodeRecord(ciphertext) {
  const bytes = asBytes(ciphertext);
  if (bytes.byteLength < 16 || bytes.byteLength > 65_535) throw new Error("invalid Noise ciphertext length");
  let remaining = bytes.byteLength;
  const prefix = [];
  do {
    const low = remaining & 0x7f;
    remaining >>>= 7;
    prefix.push(remaining === 0 ? low : low | 0x80);
  } while (remaining !== 0);
  return concat(Uint8Array.from(prefix), bytes);
}

export function decodeRecord(wire) {
  const bytes = asBytes(wire);
  let length = 0;
  let prefixLength = 0;
  for (;;) {
    if (prefixLength >= 3 || prefixLength >= bytes.byteLength) throw new Error("invalid Noise record prefix");
    const byte = bytes[prefixLength];
    length |= (byte & 0x7f) << (prefixLength * 7);
    prefixLength += 1;
    if ((byte & 0x80) === 0) {
      if (prefixLength > 1 && byte === 0) throw new Error("non-canonical Noise record prefix");
      break;
    }
  }
  if (length < 16 || length > 65_535 || prefixLength + length !== bytes.byteLength) {
    throw new Error("Noise record envelope does not contain exactly one ciphertext");
  }
  return bytes.slice(prefixLength);
}

export class RecordChannel {
  constructor(cipher) {
    this.cipher = cipher;
    this.partial = new Uint8Array();
    this.expectedTotal = null;
  }

  async sealFrame(frame) {
    const bytes = asBytes(frame);
    if (bytes.byteLength > MAX_TRANSPORT_FRAME) throw new Error("transport frame is too large");
    const records = [];
    const capacity = MAX_NOISE_PLAINTEXT - CHUNK_HEADER_LENGTH;
    let offset = 0;
    do {
      const end = Math.min(offset + capacity, bytes.byteLength);
      const final = end === bytes.byteLength;
      const plaintext = concat(
        Uint8Array.of(RECORD_FRAME),
        littleEndianU32(bytes.byteLength),
        littleEndianU32(offset),
        Uint8Array.of(final ? 1 : 0),
        bytes.slice(offset, end),
      );
      records.push(encodeRecord(await this.cipher.encrypt(plaintext)));
      offset = end;
    } while (offset < bytes.byteLength);
    return records;
  }

  async requestRekey() {
    const record = encodeRecord(await this.cipher.encrypt(Uint8Array.of(RECORD_REKEY)));
    await this.cipher.rekeySending();
    return record;
  }

  async openRecord(wire) {
    const plaintext = await this.cipher.decrypt(decodeRecord(wire));
    const kind = plaintext[0];
    if (kind === RECORD_REKEY) {
      if (plaintext.byteLength !== 1) throw new Error("Noise rekey record has a body");
      await this.cipher.rekeyReceiving();
      return null;
    }
    if (kind !== RECORD_FRAME || plaintext.byteLength < CHUNK_HEADER_LENGTH) {
      throw new Error("Noise transport record is malformed");
    }
    const total = readLittleEndianU32(plaintext, 1);
    const offset = readLittleEndianU32(plaintext, 5);
    const final = plaintext[9];
    const chunk = plaintext.slice(CHUNK_HEADER_LENGTH);
    if (total > MAX_TRANSPORT_FRAME || final > 1 || offset !== this.partial.byteLength) {
      throw new Error("Noise transport chunk metadata is invalid");
    }
    if (this.expectedTotal === null && offset === 0) this.expectedTotal = total;
    if (this.expectedTotal !== total) throw new Error("Noise transport total changed mid-frame");
    const nextLength = this.partial.byteLength + chunk.byteLength;
    if (nextLength > total || (final === 1) !== (nextLength === total)) {
      throw new Error("Noise transport final chunk is inconsistent");
    }
    this.partial = concat(this.partial, chunk);
    if (final === 0) return null;
    const frame = this.partial;
    this.partial = new Uint8Array();
    this.expectedTotal = null;
    return frame;
  }
}
