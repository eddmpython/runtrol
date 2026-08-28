import assert from "node:assert/strict";
import test from "node:test";

import { awaitsVerification, isBroken, isUsable, unaskedUsable } from "./providerHealth";
import type { ProviderLine } from "./runtimeTypes";

function provider(providerId: string, state: ProviderLine["installation"]["state"], why?: string): ProviderLine {
  return {
    providerId,
    displayName: providerId,
    icon: providerId,
    installation: why === undefined ? { state } : { state, why },
  } as ProviderLine;
}

const AWAITING_PROBE = "the installed executable has not completed a verified probe";

test("a service still being probed is not broken, and a probed one that cannot run is", () => {
  assert.equal(awaitsVerification(provider("claude", "unavailable", AWAITING_PROBE)), true);
  assert.equal(isBroken(provider("claude", "unavailable", AWAITING_PROBE)), false);
  assert.equal(isBroken(provider("claude", "unavailable", "the executable exited 127")), true);
  assert.equal(isUsable(provider("claude", "usable")), true);
});

test("a service that becomes usable late is asked for its conversations, and asked once", () => {
  // The regression this holds: the window asks for stored conversations at startup and on refresh, and a CLI
  // replacing itself is unusable at every one of those moments. Measured on the operator machine 2026-08-28,
  // the Claude probe landed five and a half minutes after the window opened; nothing asked again, and every
  // project sat empty with no notice until Refresh Conversations was run by hand.
  const starting = [provider("claude", "unavailable", AWAITING_PROBE), provider("codex", "usable")];
  const asked = new Set<string>();

  const first = unaskedUsable(starting, asked);
  assert.deepEqual(first, ["codex"], "only what can answer is asked");
  for (const id of first) asked.add(id);

  // The watch reports the same listing again. Nothing new to ask.
  assert.deepEqual(unaskedUsable(starting, asked), []);

  // The probe lands. This is the moment that used to be missed.
  const probed = [provider("claude", "usable"), provider("codex", "usable")];
  assert.deepEqual(unaskedUsable(probed, asked), ["claude"]);
  for (const id of unaskedUsable(probed, asked)) asked.add(id);
  assert.deepEqual(unaskedUsable(probed, asked), [], "and not on every listing after it");

  // A service that breaks later is not asked again on the strength of having once been usable.
  assert.deepEqual(unaskedUsable([provider("grok", "unavailable", "signed out")], asked), []);
});
