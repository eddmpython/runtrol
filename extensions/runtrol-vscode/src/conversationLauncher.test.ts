import assert from "node:assert/strict";
import test from "node:test";

import { ConversationLauncher, type FreshConversationRequest } from "./conversationLauncher";
import type { IsolatedWorkspaceLine } from "./protocol";

const REQUEST: FreshConversationRequest = {
  providerId: "service",
  model: "model-a",
  reasoningEffort: "high",
  permission: "plan",
};

test("an ordinary start does not create or bind a worktree", async () => {
  const calls: string[] = [];
  const runtime = {
    async start(providerId: string, workspace: string, access: string) {
      calls.push(`start:${providerId}:${workspace}:${access}`);
      return session(workspace);
    },
  };
  const workspaces = {
    async prepare() { throw new Error("prepare should not run"); },
    async bind() { throw new Error("bind should not run"); },
    async release() { throw new Error("release should not run"); },
  };
  const launcher = new ConversationLauncher(runtime as never, workspaces as never, async () => {
    throw new Error("refresh should not run");
  });

  assert.equal(await launcher.openFresh(REQUEST, "C:\\project", "exclusive"), "session-1");
  assert.deepEqual(calls, ["start:service:C:\\project:exclusive"]);
});

test("an explicitly isolated start binds the exact prepared worktree", async () => {
  const calls: string[] = [];
  const isolated = workspace();
  const runtime = {
    async start(_providerId: string, folder: string, access: string) {
      calls.push(`start:${folder}:${access}`);
      return session(folder);
    },
  };
  const workspaces = {
    async prepare(project: string) { calls.push(`prepare:${project}`); return isolated; },
    async bind(candidate: IsolatedWorkspaceLine, sessionId: string) {
      calls.push(`bind:${candidate.workspace_id}:${sessionId}`);
    },
    async release() { throw new Error("release should not run"); },
  };
  const launcher = new ConversationLauncher(runtime as never, workspaces as never, async () => {
    calls.push("refresh");
  });

  assert.equal(await launcher.openFresh(REQUEST, isolated.project, "isolated"), "session-1");
  assert.deepEqual(calls, [
    `prepare:${isolated.project}`,
    `start:${isolated.workspace}:exclusive`,
    "bind:workspace-1:session-1",
    "refresh",
  ]);
});

test("a failed isolated start releases only its prepared worktree", async () => {
  const calls: string[] = [];
  const isolated = workspace();
  const runtime = { async start() { throw new Error("provider refused"); } };
  const workspaces = {
    async prepare() { calls.push("prepare"); return isolated; },
    async bind() { throw new Error("bind should not run"); },
    async release(folder: string, id: string | null, sessionId: string | null) {
      calls.push(`release:${folder}:${id}:${sessionId}`);
    },
  };
  const launcher = new ConversationLauncher(runtime as never, workspaces as never, async () => {
    calls.push("refresh");
  });

  await assert.rejects(launcher.openFresh(REQUEST, isolated.project, "isolated"), /provider refused/);
  assert.deepEqual(calls, [
    "prepare",
    `release:${isolated.workspace}:workspace-1:null`,
    "refresh",
  ]);
});

test("a bind failure closes the live session before releasing its worktree", async () => {
  const calls: string[] = [];
  const isolated = workspace();
  const runtime = {
    async start() { return session(isolated.workspace); },
    async close(candidate: { sessionId: string }) { calls.push(`close:${candidate.sessionId}`); },
  };
  const workspaces = {
    async prepare() { return isolated; },
    async bind() { calls.push("bind"); throw new Error("bind refused"); },
    async release(folder: string, id: string | null, sessionId: string | null) {
      calls.push(`release:${folder}:${id}:${sessionId}`);
    },
  };
  const launcher = new ConversationLauncher(runtime as never, workspaces as never, async () => {
    calls.push("refresh");
  });

  await assert.rejects(launcher.openFresh(REQUEST, isolated.project, "isolated"), /bind refused/);
  assert.deepEqual(calls, [
    "bind",
    "close:session-1",
    `release:${isolated.workspace}:workspace-1:null`,
    "refresh",
  ]);
});

function workspace(): IsolatedWorkspaceLine {
  return {
    workspace_id: "workspace-1",
    project: "C:\\project",
    workspace: "C:\\isolated\\workspace-1",
    base_commit: "0123456789abcdef0123456789abcdef01234567",
    state: "ready",
    session_id: null,
  };
}

function session(workspace: string) {
  return {
    sessionId: "session-1",
    providerId: "service",
    workspace,
    lifecycle: "hotIdle" as const,
    sessionGeneration: 1,
    hot: true,
    looksStuck: false,
  };
}
