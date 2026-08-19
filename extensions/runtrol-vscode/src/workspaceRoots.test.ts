import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import type { CoreClient } from "./core/client";
import type { IntegrationLine, Response } from "./protocol";
import { WorkspaceRootFollowing, foldersOutsideRoots, followDecision } from "./workspaceRoots";

const win = path.win32;

test("a folder inside an approved root needs no approval of its own", () => {
  const outside = foldersOutsideRoots(
    ["C:\\work\\alpha\\crates", "C:\\work\\alpha"],
    ["C:\\work\\alpha"],
    win,
    "win32",
  );
  assert.deepEqual(outside, []);
});

test("the drive letter this window happened to print does not make a folder foreign", () => {
  // Measured on this machine: one folder appears as C:\ and c:\ in the same listing. Treating the spellings as
  // two folders would re-request approval for a root the operator already granted, forever.
  const outside = foldersOutsideRoots(["c:\\WORK\\alpha"], ["C:\\work\\alpha"], win, "win32");
  assert.deepEqual(outside, []);
});

test("a sibling whose name merely starts with a root is outside it", () => {
  const outside = foldersOutsideRoots(
    ["C:\\work\\alpha-other", "C:\\work\\alphabet"],
    ["C:\\work\\alpha"],
    win,
    "win32",
  );
  assert.deepEqual(outside, ["C:\\work\\alpha-other", "C:\\work\\alphabet"]);
});

test("one folder opened twice is asked about once, first spelling kept", () => {
  const outside = foldersOutsideRoots(
    ["C:\\work\\beta", "c:\\work\\beta"],
    [],
    win,
    "win32",
  );
  assert.deepEqual(outside, ["C:\\work\\beta"]);
});

test("a drive-root root covers the folders on that drive", () => {
  // A drive root resolves with its trailing separator kept. Concatenating another separator onto it unchecked
  // would make every folder on the drive read as outside it.
  const outside = foldersOutsideRoots(["C:\\work\\alpha"], ["C:\\"], win, "win32");
  assert.deepEqual(outside, []);
});

test("a failed change is read from the rows, never from the failure's wording", () => {
  const before = { grant_generation: 4 };
  assert.equal(followDecision("C:\\work\\beta", before, null, win, "win32"), "gone");
  assert.equal(
    followDecision(
      "C:\\work\\beta",
      before,
      { roots: [], grant_generation: 4, revoked: true },
      win,
      "win32",
    ),
    "gone",
  );
  assert.equal(
    followDecision(
      "C:\\work\\beta",
      before,
      { roots: ["c:\\work\\beta"], grant_generation: 5, revoked: false },
      win,
      "win32",
    ),
    "followed",
    "another window adding the folder is success, not refusal",
  );
  assert.equal(
    followDecision(
      "C:\\work\\beta",
      before,
      { roots: [], grant_generation: 5, revoked: false },
      win,
      "win32",
    ),
    "retry",
    "a moved generation means the compare-and-set lost, not that the folder was refused",
  );
  assert.equal(
    followDecision(
      "C:\\work\\beta",
      before,
      { roots: [], grant_generation: 4, revoked: false },
      win,
      "win32",
    ),
    "declined",
  );
});

// ---- the orchestration, against a fake daemon ----

type Exchange = { ask: string; with?: unknown };

function integration(overrides: Partial<IntegrationLine> = {}): IntegrationLine {
  return {
    integration_id: "studio",
    label: "Runtrol Studio",
    client_instance_id: "instance",
    scopes: ["session.list"],
    available_scopes: ["session.list"],
    roots: [path.resolve("approved")],
    grant_generation: 1,
    revoked: false,
    ...overrides,
  };
}

/// A daemon of one integration row: listings answer with the row, changes append the root and bump the
/// generation, and a change for a folder in `refuses` fails the way the admin surface fails.
function fakeDaemon(row: IntegrationLine, refuses: Map<string, string> = new Map()) {
  const asked: Exchange[] = [];
  const client = {
    once: async (request: Exchange): Promise<{ response: Response }> => {
      asked.push(request);
      if (request.ask === "integrations") {
        return { response: { say: "integrations", with: [structuredClone(row)] } as Response };
      }
      if (request.ask === "integrationGrantChange") {
        const change = request.with as { expected_grant_generation: number; roots: string[] };
        const added = change.roots.at(-1) ?? "";
        const refusal = refuses.get(added);
        if (refusal !== undefined) {
          return { response: { say: "failed", with: { message: refusal } } as Response };
        }
        if (change.expected_grant_generation !== row.grant_generation) {
          return {
            response: {
              say: "failed",
              with: { message: "the integration grant changed before it could be committed" },
            } as Response,
          };
        }
        row.roots = change.roots;
        row.grant_generation += 1;
        return { response: { say: "done" } as Response };
      }
      throw new Error(`unexpected ask ${request.ask}`);
    },
  } as unknown as CoreClient;
  return { client, asked, row: () => row };
}

function follower(
  daemon: ReturnType<typeof fakeDaemon>,
  folders: string[],
  warnings: string[] = [],
  reconnects: number[] = [],
) {
  return new WorkspaceRootFollowing({
    client: daemon.client,
    integrationId: () => "studio",
    reconnect: async () => {
      reconnects.push(1);
    },
    openFolders: () => folders,
    warn: (message) => warnings.push(message),
  });
}

test("opening a folder widens the grant and refreshes the connection once", async () => {
  const daemon = fakeDaemon(integration());
  const reconnects: number[] = [];
  const opened = path.resolve("opened");
  await follower(daemon, [path.resolve("approved"), opened], [], reconnects).follow();
  assert.ok(daemon.row().roots.includes(opened), "the opened folder became a root");
  assert.equal(daemon.row().grant_generation, 2);
  assert.equal(reconnects.length, 1, "one refresh, however many folders arrived");
});

test("folders already covered cost one listing and change nothing", async () => {
  const daemon = fakeDaemon(integration());
  const reconnects: number[] = [];
  await follower(daemon, [path.resolve("approved", "nested")], [], reconnects).follow();
  assert.deepEqual(
    daemon.asked.map((exchange) => exchange.ask),
    ["integrations"],
  );
  assert.equal(reconnects.length, 0, "nothing widened, so nothing reconnects");
});

test("a folder the daemon refuses is said once and does not block the others", async () => {
  const refused = path.resolve("home-like");
  const accepted = path.resolve("fine");
  const daemon = fakeDaemon(
    integration(),
    new Map([[refused, "the folder overlaps a credential directory"]]),
  );
  const warnings: string[] = [];
  const reconnects: number[] = [];
  const following = follower(daemon, [refused, accepted], warnings, reconnects);
  await following.follow();
  assert.ok(daemon.row().roots.includes(accepted), "the acceptable folder still arrived");
  assert.ok(!daemon.row().roots.includes(refused));
  assert.equal(warnings.length, 1);
  assert.ok(warnings[0]?.includes("credential"), "the daemon's own sentence reaches the operator");
  assert.equal(reconnects.length, 1);

  // The refusal is remembered: the next pass neither re-asks nor re-warns.
  await following.follow();
  assert.equal(warnings.length, 1);
  assert.equal(
    daemon.asked.filter((exchange) => exchange.ask === "integrationGrantChange").length,
    2,
    "one refused attempt and one accepted attempt, and never again",
  );
});

test("without an enrolled integration nothing is asked at all", async () => {
  const daemon = fakeDaemon(integration());
  const following = new WorkspaceRootFollowing({
    client: daemon.client,
    integrationId: () => null,
    reconnect: async () => {
      throw new Error("must not reconnect");
    },
    openFolders: () => [path.resolve("anything")],
    warn: () => {
      throw new Error("must not warn");
    },
  });
  await following.follow();
  assert.deepEqual(daemon.asked, []);
});
