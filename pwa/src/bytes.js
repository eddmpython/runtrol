const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export function utf8(value) {
  return encoder.encode(value);
}

export function text(value) {
  return decoder.decode(value);
}

export function concat(...parts) {
  const length = parts.reduce((total, part) => total + part.byteLength, 0);
  const joined = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    joined.set(asBytes(part), offset);
    offset += part.byteLength;
  }
  return joined;
}

export function asBytes(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  }
  throw new TypeError("value is not binary data");
}

export function base64UrlEncode(value) {
  const bytes = asBytes(value);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}

export function base64UrlDecode(value, expectedLength) {
  if (typeof value !== "string" || !/^[A-Za-z0-9_-]+$/u.test(value)) {
    throw new Error("value is not canonical unpadded base64url");
  }
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  const binary = atob(value.replaceAll("-", "+").replaceAll("_", "/") + padding);
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  if (expectedLength !== undefined && bytes.byteLength !== expectedLength) {
    throw new Error(`decoded value must contain ${expectedLength} bytes`);
  }
  if (base64UrlEncode(bytes) !== value) {
    throw new Error("value is not canonical unpadded base64url");
  }
  return bytes;
}

export function littleEndianU32(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > 0xffff_ffff) {
    throw new RangeError("value does not fit in u32");
  }
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value, true);
  return bytes;
}

export function readLittleEndianU32(bytes, offset) {
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(offset, true);
}

export function equalBytes(left, right) {
  const a = asBytes(left);
  const b = asBytes(right);
  if (a.byteLength !== b.byteLength) return false;
  let difference = 0;
  for (let index = 0; index < a.byteLength; index += 1) difference |= a[index] ^ b[index];
  return difference === 0;
}
