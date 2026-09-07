import { ask, expectDone } from "./core/ask";
import type { CoreClient } from "./core/client";
import type { IntegrationLine } from "./protocol";
import type { IntegrationGrant } from "@runtrol/runtime-client";
import { STUDIO_INPUT_PRIORITY } from "./studioInputPriority";

export async function readIntegrationGrant(client: CoreClient, integrationId: string): Promise<IntegrationLine | null> {
  const rows = await ask(client, { ask: "integrations" });
  if (rows.say !== "integrations") throw new Error(`the daemon answered integration listing with ${rows.say}`);
  return rows.with.find((row) => row.integration_id === integrationId) ?? null;
}

/// All local grant changes preserve compare-and-set ordering through the same private administration call.
export async function replaceIntegrationGrant(
  client: CoreClient,
  before: IntegrationLine,
  scopes: readonly string[],
  roots: readonly string[],
): Promise<void> {
  expectDone(await ask(client, {
    ask: "integrationGrantChange",
    with: {
      integration_id: before.integration_id,
      expected_grant_generation: before.grant_generation,
      scopes: [...scopes],
      roots: [...roots],
    },
  }), "integration authority replacement");
}

/// Upgrade only the exact grant authenticated by this Studio identity, without replaying a lost owner decision.
export async function requestStudioInputPriority(client: CoreClient, grant: IntegrationGrant): Promise<boolean> {
  const before = await readIntegrationGrant(client, grant.integrationId);
  if (!before || before.revoked || before.key_generation !== grant.keyGeneration
    || before.grant_generation !== grant.grantGeneration) return false;
  try {
    await replaceIntegrationGrant(client, before, [...before.scopes, STUDIO_INPUT_PRIORITY], before.roots);
    return true;
  } catch (error: unknown) {
    const after = await readIntegrationGrant(client, grant.integrationId);
    if (!after || after.revoked || after.key_generation !== before.key_generation
      || after.grant_generation !== before.grant_generation) return false;
    throw error;
  }
}
