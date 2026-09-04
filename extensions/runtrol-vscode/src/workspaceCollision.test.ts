import assert from "node:assert/strict";
import test from "node:test";

import type { SessionLine } from "./runtimeTypes";
import { workingCollisions, workspaceCollisions } from "./workspaceCollision";

function session(workspace: string, hot = true, id = workspace): SessionLine {
  return {
    sessionId: id,
    providerId: "fixture",
    nativeSessionId: null,
    label: null,
    workspace,
    hot,
    lifecycle: hot ? "hotRunning" : "cold",
    sessionGeneration: 1,
  };
}

test("same, parent, and child workspaces collide on segment boundaries", () => {
  const sessions = [
    session("/work/repo", true, "same"),
    session("/work/repo/packages/one", true, "child"),
    session("/work", true, "parent"),
    session("/work/repo-copy", true, "sibling"),
  ];
  const collisions = workspaceCollisions("/work/repo/", sessions, "linux");
  assert.deepEqual(
    collisions.map(({ session: active, relation }) => [active.sessionId, relation]),
    [
      ["same", "same"],
      ["child", "candidateContainsSession"],
      ["parent", "sessionContainsCandidate"],
    ],
  );
});

test("Windows casing and separators cannot create a second workspace identity", () => {
  const collisions = workspaceCollisions(
    "c:\\WORK\\Repo\\src",
    [
      session("C:\\work\\repo", true, "windows"),
      session("C:\\work\\repository", true, "sibling"),
    ],
    "win32",
  );
  assert.deepEqual(collisions.map(({ session: active }) => active.sessionId), ["windows"]);
});

test("detached sessions are choices, not active writer collisions", () => {
  assert.deepEqual(workspaceCollisions("/work/repo", [session("/work/repo", false)], "linux"), []);
});

test("a conversation switch distinguishes idle processes from active turns", () => {
  const idle = { ...session("/work/repo", true, "idle"), lifecycle: "hotIdle" as const };
  const working = session("/work/repo", true, "working");
  const collisions = workspaceCollisions("/work/repo", [idle, working], "linux");
  assert.deepEqual(
    workingCollisions(collisions).map(({ session: value }) => value.sessionId),
    ["working"],
  );
});
