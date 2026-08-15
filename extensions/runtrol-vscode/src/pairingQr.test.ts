import assert from "node:assert/strict";
import test from "node:test";

import { pairingQrDataUrl } from "./pairingQr";

test("phone pairing produces a bounded SVG data URL without the PNG runtime", async () => {
  const value = await pairingQrDataUrl("https://example.invalid/pair/one-use-value");
  const prefix = "data:image/svg+xml;base64,";

  assert.ok(value.startsWith(prefix));
  const svg = Buffer.from(value.slice(prefix.length), "base64").toString("utf8");
  assert.match(svg, /^<svg /u);
  assert.match(svg, /width="320"/u);
  assert.ok(value.length < 32 * 1024);
});
