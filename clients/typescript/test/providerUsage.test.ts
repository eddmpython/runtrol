import assert from "node:assert/strict";
import { test } from "node:test";

import { ProviderSubscription, RuntimeProtocolError } from "../src/index.js";
import { ScriptedRuntimeTransport } from "../src/testing.js";

const started = {
  subscriptionId: "sub_fixture",
  snapshot: { providers: [] },
};

test("a pushed usage snapshot arrives on the provider subscription as its own notification", async () => {
  const transport = new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "providers/usageChanged",
      params: {
        subscriptionId: "sub_fixture",
        snapshot: {
          providers: [{
            providerId: "codex",
            reached: false,
            windows: [{ id: "codex.primary", usedPercent: 65, windowMinutes: 10_080 }],
            tokensToday: 1234,
            atMs: 1,
          }],
        },
      },
    },
  ]);
  const subscription = new ProviderSubscription(transport, started);
  const notification = await subscription.next();
  assert.equal(notification.kind, "usageChanged");
  if (notification.kind === "usageChanged") {
    assert.equal(notification.usageChanged.snapshot.providers[0]?.tokensToday, 1234);
    assert.equal(
      notification.usageChanged.snapshot.providers[0]?.windows?.[0]?.usedPercent,
      65,
    );
  }
});

test("a usage notification for another subscription is refused", async () => {
  const transport = new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "providers/usageChanged",
      params: { subscriptionId: "sub_other", snapshot: { providers: [] } },
    },
  ]);
  const subscription = new ProviderSubscription(transport, started);
  await assert.rejects(() => subscription.next(), RuntimeProtocolError);
});

test("native structural snapshots preserve unknown proof and require explicit opt-in", async () => {
  const frame = {
    jsonrpc: "2.0",
    method: "providers/nativeActivityChanged",
    params: {
      subscriptionId: "sub_fixture",
      snapshot: [
        {
          state: "observed",
          activity: { providerId: "fixture", live: ["native_fixture"], active: [], attachable: [], focusable: [] },
          catalogueRevision: "opaque:2",
          unknownActivity: ["native_fixture"],
        },
        { state: "unavailable", providerId: "other" },
      ],
    },
  };
  const opted = new ProviderSubscription(new ScriptedRuntimeTransport([frame]), started, true);
  const notification = await opted.next();
  assert.equal(notification.kind, "nativeActivityChanged");
  if (notification.kind === "nativeActivityChanged") {
    assert.deepEqual(notification.nativeActivityChanged.snapshot, frame.params.snapshot);
  }
  const legacy = new ProviderSubscription(new ScriptedRuntimeTransport([frame]), started);
  await assert.rejects(() => legacy.next(), /native activity was not requested/);
  const foreign = new ProviderSubscription(new ScriptedRuntimeTransport([
    { ...frame, params: { ...frame.params, subscriptionId: "sub_other" } },
  ]), started, true);
  await assert.rejects(() => foreign.next(), RuntimeProtocolError);
});
