import assert from "node:assert/strict";
import test from "node:test";
import type { IntegrationGrant } from "@runtrol/runtime-client";
import type { CoreClient } from "./core/client";
import type { IntegrationLine, Response } from "./protocol";
import { requestStudioInputPriority } from "./integrationGrant";
import { STUDIO_INPUT_PRIORITY, STUDIO_SCOPES, reviewStudioInputPriority,
  readStudioInputPriorityReview, persistStudioInputPriorityReview } from "./studioInputPriority";

function grant(): IntegrationGrant {
  return { integrationId: "studio", keyGeneration: 2, grantGeneration: 4, roots: ["C:\\owned"],
    scopes: STUDIO_SCOPES.filter((scope) => scope !== STUDIO_INPUT_PRIORITY) };
}

test("an old Runtime defers review and a supported upgrade records it before one owner request", async () => {
  const stored: { grant: IntegrationGrant; inputPriorityReviewed?: true } = { grant: grant() };
  const events: string[] = [];
  const save = async () => { stored.inputPriorityReviewed = true; events.push("saved"); };
  const change = async () => { assert.equal(stored.inputPriorityReviewed, true); events.push("changed"); return true; };
  assert.equal(await reviewStudioInputPriority(false, stored, save, change), false);
  assert.deepEqual(events, []);
  assert.equal(await reviewStudioInputPriority(true, stored, save, change), true);
  // A later owner removal cannot reopen this identity's completed capability review.
  assert.equal(await reviewStudioInputPriority(true, stored, save, change), false);
  assert.deepEqual(events, ["saved", "changed"]);
});

test("owner-narrowed and already-elevated grants are reviewed without widening", async () => {
  for (const scopes of [["session.list"] as const, STUDIO_SCOPES]) {
    let saved = 0;
    const changed = await reviewStudioInputPriority(true, { grant: { ...grant(), scopes } },
      async () => { saved += 1; }, async () => { assert.fail("no owner change is allowed"); });
    assert.equal(changed, false);
    assert.equal(saved, 1);
  }
});

test("an older window replacing credentials cannot erase the review after owner removal", async () => {
  const entries = new Map<string, string>();
  const storage = {
    get: async (key: string) => entries.get(key),
    store: async (key: string, value: string) => { entries.set(key, value); },
  };
  const original = grant();
  await storage.store("runtrol.runtime.integration.v1", JSON.stringify({ grant: original }));
  await persistStudioInputPriorityReview(storage, original);
  // An older window knows only the shared credential key and writes a snapshot without the review field.
  const narrowed = { ...original, grantGeneration: original.grantGeneration + 2 };
  await storage.store("runtrol.runtime.integration.v1", JSON.stringify({ grant: narrowed }));
  const reviewed = await readStudioInputPriorityReview(storage, narrowed);
  assert.equal(reviewed, true);
  assert.equal(await reviewStudioInputPriority(true, { grant: narrowed, inputPriorityReviewed: true },
    async () => { assert.fail("the review remains durable across windows"); },
    async () => { assert.fail("owner removal must not cause another elevation"); }), false);
  assert.equal(await readStudioInputPriorityReview(storage, { ...narrowed, integrationId: "another" }), false);
});

test("a failed review checkpoint prevents elevation and a lost decision is never retried", async () => {
  const stored: { grant: IntegrationGrant; inputPriorityReviewed?: true } = { grant: grant() };
  await assert.rejects(reviewStudioInputPriority(true, stored, async () => { throw new Error("storage unavailable"); },
    async () => { assert.fail("authority cannot change before its review is durable"); }), /storage unavailable/);
  let attempts = 0;
  const save = async () => { stored.inputPriorityReviewed = true; };
  const change = async () => { attempts += 1; throw new Error("decision reply lost"); };
  await assert.rejects(reviewStudioInputPriority(true, stored, save, change), /reply lost/);
  assert.equal(await reviewStudioInputPriority(true, stored, save, change), false);
  assert.equal(attempts, 1);
});

test("owner administration preserves identity and roots and never retries a competing grant change", async () => {
  for (const race of [false, true]) {
    const admitted = grant();
    const row: IntegrationLine = { integration_id: admitted.integrationId, label: "Studio", client_instance_id: "fixture",
      scopes: [...admitted.scopes], roots: [...admitted.roots], available_scopes: [...STUDIO_SCOPES],
      key_generation: admitted.keyGeneration, grant_generation: admitted.grantGeneration, revoked: false };
    let changes = 0;
    const client = { once: async (request: { ask: string; with?: unknown }): Promise<{ response: Response }> => {
      if (request.ask === "integrations") return { response: { say: "integrations", with: [structuredClone(row)] } };
      assert.equal(request.ask, "integrationGrantChange");
      changes += 1;
      assert.deepEqual(request.with, { integration_id: "studio", expected_grant_generation: 4,
        scopes: [...admitted.scopes, STUDIO_INPUT_PRIORITY], roots: admitted.roots });
      row.grant_generation += 1;
      if (race) {
        row.scopes = ["session.list"];
        return { response: { say: "failed", with: { message: "concurrent owner change" } } as Response };
      }
      row.scopes.push(STUDIO_INPUT_PRIORITY);
      return { response: { say: "done" } };
    } } as unknown as CoreClient;
    assert.equal(await requestStudioInputPriority(client, admitted), !race);
    assert.equal(changes, 1);
    assert.equal(row.key_generation, admitted.keyGeneration);
    assert.deepEqual(row.roots, admitted.roots);
    if (race) assert.deepEqual(row.scopes, ["session.list"]);
  }
});
