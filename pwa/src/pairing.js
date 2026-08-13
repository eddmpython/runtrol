import { base64UrlDecode, text } from "./bytes.js";

const PAIRING_FIELDS = [
  "credential",
  "expires_at_ms",
  "pairing_secret",
  "pc_public_key",
  "relay_origin",
  "route",
  "version",
];

export function consumePairingFragment(locationLike, historyLike, now = Date.now()) {
  const match = /^#pair=([A-Za-z0-9_-]+)$/u.exec(locationLike.hash);
  if (!match) return null;
  try {
    return parsePairingValue(match[1], now);
  } finally {
    historyLike.replaceState(null, "", `${locationLike.pathname}${locationLike.search}`);
  }
}

export function parsePairingValue(encoded, now = Date.now()) {
  let candidate;
  try {
    candidate = JSON.parse(text(base64UrlDecode(encoded)));
  } catch (error) {
    throw new Error("pairing QR is malformed", { cause: error });
  }
  if (candidate === null || typeof candidate !== "object" || Array.isArray(candidate)) {
    throw new Error("pairing QR payload must be an object");
  }
  const fields = Object.keys(candidate).sort();
  if (JSON.stringify(fields) !== JSON.stringify(PAIRING_FIELDS)) {
    throw new Error("pairing QR payload has an unexpected field set");
  }
  if (candidate.version !== 1) throw new Error("pairing QR version is not supported");
  if (!Number.isSafeInteger(candidate.expires_at_ms) || candidate.expires_at_ms <= now) {
    throw new Error("pairing QR has expired");
  }
  const relayUrl = new URL(candidate.relay_origin);
  if (
    relayUrl.protocol !== "https:"
    || relayUrl.username !== ""
    || relayUrl.password !== ""
    || relayUrl.pathname !== "/"
    || relayUrl.search !== ""
    || relayUrl.hash !== ""
    || relayUrl.origin !== candidate.relay_origin
  ) {
    throw new Error("pairing relay origin is not canonical HTTPS");
  }
  base64UrlDecode(candidate.route, 32);
  base64UrlDecode(candidate.credential, 32);
  base64UrlDecode(candidate.pc_public_key, 32);
  base64UrlDecode(candidate.pairing_secret, 16);
  return Object.freeze({
    relayOrigin: candidate.relay_origin,
    route: candidate.route,
    routeCredential: candidate.credential,
    pcPublicKey: candidate.pc_public_key,
    pairingSecret: candidate.pairing_secret,
    expiresAtMs: candidate.expires_at_ms,
  });
}
