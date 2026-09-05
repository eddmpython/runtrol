import assert from "node:assert/strict";
import test from "node:test";

import { IsolatedWorkspaces } from "./isolatedWorkspace";
import type { Request, Response } from "./protocol";

const LINE = {
  workspace_id: "01234567-89ab-cdef-0123-456789abcdef",
  project: "C:\\work\\alpha",
  workspace: "C:\\work\\.runtrol-worktrees\\chat-01234567-89ab-cdef-0123-456789abcdef",
  base_commit: "0123456789abcdef0123456789abcdef01234567",
  state: "ready" as const,
  session_id: null,
};

const INTEGRATION = {
  integration_id: "019c5a44-0c4e-7682-9ee4-33ddad767824",
  label: "Runtrol Studio",
  client_instance_id: "studio-test",
  scopes: ["session.start"],
  available_scopes: ["session.start"],
  roots: [LINE.project],
  key_generation: 1,
  grant_generation: 4,
  revoked: false,
};

function core(responses: Response[]): {
  requests: Request[];
  read(request: Request): Promise<{ response: Response }>;
} {
  const requests: Request[] = [];
  return {
    requests,
    async read(request) {
      requests.push(request);
      const response = responses.shift();
      if (!response) throw new Error("fixture has no response");
      return { response };
    },
  };
}

test("prepare, bind, and release carry the exact Core-owned identity", async () => {
  const exchange = core([
    { say: "isolatedWorkspace", with: LINE },
    { say: "integrations", with: [INTEGRATION] },
    { say: "done" },
    { say: "done" },
    {
      say: "integrations",
      with: [{ ...INTEGRATION, roots: [LINE.project, LINE.workspace], grant_generation: 5 }],
    },
    { say: "done" },
    {
      say: "isolatedWorkspaceReleased",
      with: { workspace_id: LINE.workspace_id, workspace: LINE.workspace, outcome: "removed" },
    },
  ]);
  let authorizationRefreshes = 0;
  const workspaces = new IsolatedWorkspaces(exchange, () => INTEGRATION.integration_id, async () => {
    authorizationRefreshes += 1;
  });
  const prepared = await workspaces.prepare(LINE.project, LINE.workspace_id);
  await workspaces.bind(prepared, "019c5a42-f60a-7f21-a4d4-acde5e1cc9d0");
  const released = await workspaces.release(LINE.workspace, LINE.workspace_id, null);

  assert.equal(released?.outcome, "removed");
  assert.equal(authorizationRefreshes, 2);
  assert.deepEqual(exchange.requests, [
    {
      ask: "workspaceIsolatePrepare",
      with: { request_id: LINE.workspace_id, project: LINE.project },
    },
    { ask: "integrations" },
    {
      ask: "integrationGrantChange",
      with: {
        integration_id: INTEGRATION.integration_id,
        expected_grant_generation: 4,
        scopes: INTEGRATION.scopes,
        roots: [LINE.project, LINE.workspace],
      },
    },
    {
      ask: "workspaceIsolateBind",
      with: {
        workspace_id: LINE.workspace_id,
        session_id: "019c5a42-f60a-7f21-a4d4-acde5e1cc9d0",
        workspace: LINE.workspace,
      },
    },
    { ask: "integrations" },
    {
      ask: "integrationGrantChange",
      with: {
        integration_id: INTEGRATION.integration_id,
        expected_grant_generation: 5,
        scopes: INTEGRATION.scopes,
        roots: [LINE.project],
      },
    },
    {
      ask: "workspaceIsolateRelease",
      with: { workspace_id: LINE.workspace_id, session_id: null, workspace: LINE.workspace },
    },
  ]);
});

test("Core refusals remain visible product failures", async () => {
  const workspaces = new IsolatedWorkspaces(core([
    { say: "failed", with: { message: "the project has no resolvable HEAD commit", retryable: false, needs_the_operator: false } },
  ]), () => INTEGRATION.integration_id, async () => {});
  await assert.rejects(
    workspaces.prepare(LINE.project, LINE.workspace_id),
    /no resolvable HEAD commit/,
  );
});

test("an ordinary workspace release cannot remove an approved root", async () => {
  const exchange = core([
    { say: "isolatedWorkspaces", with: [] },
  ]);
  const workspaces = new IsolatedWorkspaces(exchange, () => INTEGRATION.integration_id, async () => {});

  assert.equal(await workspaces.release(LINE.project, null, "019c5a42-f60a-7f21-a4d4-acde5e1cc9d0"), null);
  assert.deepEqual(exchange.requests, [{ ask: "workspaceIsolateList" }]);
});
