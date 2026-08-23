import assert from "node:assert/strict";
import test from "node:test";

import type { CatalogueCoverage, NativeChatCatalogue, ProviderLine, SessionLine } from "./runtimeTypes";
import { discoveryNotice, incompleteDiscovery, providerRowsEqual, sessionRowsEqual } from "./stateRows";

/// One service's catalogue answer, carrying only what the sentence builder reads.
function catalogue(providerId: string, coverage: CatalogueCoverage): NativeChatCatalogue {
  return { providerId, coverage, chats: [], loadedAtMs: 0, warning: null };
}

function provider(providerId: string, displayName: string): ProviderLine {
  return { providerId, displayName, installation: { state: "usable", version: "1.0.0" } };
}

const SESSION: SessionLine = {
  sessionId: "session-1",
  providerId: "provider-1",
  nativeSessionId: "native-1",
  label: "Release repair",
  workspace: "C:\\work",
  hot: true,
  lifecycle: "hotRunning",
  looksStuck: false,
  sessionGeneration: 1,
};

const PROVIDER: ProviderLine = {
  providerId: "provider-1",
  displayName: "Provider One",
  installation: { state: "usable", version: "1.0.0" },
};

test("equal snapshots do not require a state publication", () => {
  assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION }]), true);
  assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER }]), true);
});

test("every visible session field invalidates the snapshot", () => {
  for (const changed of [
    { sessionId: "session-2" },
    { providerId: "provider-2" },
    { nativeSessionId: null },
    { label: null },
    { workspace: "C:\\other" },
    { hot: false },
    { lifecycle: "hotIdle" as const },
    { looksStuck: true },
    { waitingOn: "person" as const },
    { sessionGeneration: 2 },
  ]) {
    assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION, ...changed }]), false);
  }
  assert.equal(sessionRowsEqual([SESSION], []), false);
});

test("every visible provider field invalidates the snapshot", () => {
  for (const changed of [
    { providerId: "provider-2" },
    { displayName: "Provider Two" },
    { installation: { state: "missing" as const, why: "missing" } },
  ]) {
    assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER, ...changed }]), false);
  }
  assert.equal(providerRowsEqual([PROVIDER], []), false);
});

test("a complete catalogue says nothing, because there is nothing to qualify", () => {
  const reasons = incompleteDiscovery(
    [catalogue("claude", { kind: "complete", source: "officialCli" })],
    [provider("claude", "Claude Code")],
  );
  assert.equal(reasons, null);
});

test("a partial catalogue is quoted in the service's own words, under its own name", () => {
  const reasons = incompleteDiscovery(
    [catalogue("claude", {
      kind: "partial",
      source: "officialCli",
      why: "this CLI lists the sessions it is running, not the conversations it has stored",
    })],
    [provider("claude", "Claude Code")],
  );
  assert.equal(
    reasons,
    "Claude Code: this CLI lists the sessions it is running, not the conversations it has stored",
  );
});

test("every incomplete service is named, so nobody has to guess which chats are missing", () => {
  const reasons = incompleteDiscovery(
    [
      catalogue("claude", { kind: "partial", source: "officialCli", why: "running sessions only" }),
      catalogue("codex", { kind: "complete", source: "officialCli" }),
      catalogue("grok", { kind: "unsupported", why: "this service lists nothing" }),
    ],
    [provider("claude", "Claude Code"), provider("codex", "Codex"), provider("grok", "Grok")],
  );
  assert.equal(reasons, "Claude Code: running sessions only · Grok: this service lists nothing");
});

test("a service the provider list has never heard of is named by its identifier, not dropped", () => {
  const reasons = incompleteDiscovery(
    [catalogue("mystery", { kind: "unsupported", why: "no enumerable surface" })],
    [],
  );
  assert.equal(reasons, "mystery: no enumerable surface");
});

test("the visible coverage notice names every affected service without hiding the fact behind a click", () => {
  const notice = discoveryNotice(
    [
      catalogue("claude", { kind: "partial", source: "officialCli", why: "running sessions only" }),
      catalogue("codex", { kind: "complete", source: "officialCli" }),
      catalogue("cline", { kind: "partial", source: "providerStore", why: "recent history only" }),
      catalogue("grok", { kind: "unsupported", why: "no enumerable surface" }),
    ],
    [
      provider("claude", "Claude Code"),
      provider("codex", "Codex"),
      provider("cline", "Cline"),
      provider("grok", "Grok"),
    ],
  );
  assert.equal(notice, "History: partial for Claude Code, Cline; unavailable for Grok.");
});

test("complete history needs no coverage notice", () => {
  assert.equal(
    discoveryNotice(
      [catalogue("codex", { kind: "complete", source: "officialCli" })],
      [provider("codex", "Codex")],
    ),
    null,
  );
});
