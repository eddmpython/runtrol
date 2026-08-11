import assert from "node:assert/strict";
import test from "node:test";

import type { SessionLine } from "./protocol";
import { workspaceCollisions } from "./workspaceCollision";

function session(workspace: string, hot = true, id = workspace): SessionLine {
  return {
    session: id,
    provider: "fixture",
    native: null,
    workspace,
    hot,
    doing: hot ? "running" : "detached",
    looks_stuck: false,
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
    collisions.map(({ session: active, relation }) => [active.session, relation]),
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
  assert.deepEqual(collisions.map(({ session: active }) => active.session), ["windows"]);
});

test("detached sessions are choices, not active writer collisions", () => {
  assert.deepEqual(workspaceCollisions("/work/repo", [session("/work/repo", false)], "linux"), []);
});
