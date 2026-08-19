import * as path from "node:path";

import { ask, expectDone } from "./core/ask";
import type { CoreClient } from "./core/client";
import type { IntegrationLine } from "./protocol";
import { identityCovers, workspaceIdentity } from "./workspaceCollision";

/// The window's open folders, followed into the integration's approved roots.
///
/// # Why this exists
///
/// Enrollment reads the window's folders exactly once, and nothing ever read them again: the extension had no
/// `onDidChangeWorkspaceFolders` handler at all. Every folder opened after first activation was invisible to
/// conversation discovery, silently, forever. That is why the conversation panel stayed empty on a machine full
/// of conversations.
///
/// # The trust model, stated plainly
///
/// First enrollment already self-approves whatever folders are open in the window, on the grounds that opening a
/// folder in VS Code is a physical act at this PC. Following folders opened later is the same judgement applied
/// at the moment it becomes true, through the same local-only administration surface. Nothing here is reachable
/// from a remote caller, and nothing here approves a folder the operator did not open themselves.
///
/// The corollary is also policy: a folder open in a Studio window is an approved root. An operator who wants a
/// folder kept out of the grant keeps it out of the window. Removing the Studio's own root by hand while that
/// folder is open in a Studio window is a contradiction under this model, and the follower will restore it.
///
/// # Why one folder per change
///
/// The daemon refuses a root that overlaps a credential directory, and refuses the whole change with it. Asked
/// as a union, one home-directory folder would block every other folder in the window. Asked one at a time, the
/// refusable folder is refused alone, the operator is told the daemon's own sentence for it once, and the rest
/// still arrive.
export class WorkspaceRootFollowing {
  /// Serialises follow passes so a burst of folder events cannot interleave two of them.
  private queue: Promise<unknown> = Promise.resolve();
  /// Folders the daemon refused, by identity, with the sentence it refused them with. Warned once per window.
  private readonly declined = new Map<string, string>();

  constructor(
    private readonly ports: {
      client: CoreClient;
      /// The Studio's own integration, or null before enrollment has ever succeeded.
      integrationId: () => string | null;
      /// Refresh what the wider grant can now see. The daemon re-reads the grant per request, so the live
      /// connection already has the new root in force; this asks again with the wider eyes rather than
      /// reconnecting, which is how a widened root reaches conversation discovery without this module
      /// holding any grant state or paying a teardown.
      refreshRoots: () => Promise<void>;
      openFolders: () => readonly string[];
      warn: (message: string) => void;
    },
  ) {}

  /// Bring the grant's roots up to date with the window's folders.
  ///
  /// Safe to call at any time after enrollment: covered folders cost one listing call and change nothing.
  follow(): Promise<void> {
    const next = this.queue.catch(() => undefined).then(() => this.followNow());
    this.queue = next;
    return next;
  }

  private async followNow(): Promise<void> {
    const integrationId = this.ports.integrationId();
    if (!integrationId) return;
    const folders = this.ports.openFolders();
    if (folders.length === 0) return;
    let row = await this.ownRow(integrationId);
    if (!row || row.revoked) return;
    const outside = foldersOutsideRoots(folders, row.roots).filter(
      (folder) => !this.declined.has(workspaceIdentity(folder)),
    );
    if (outside.length === 0) return;

    let widened = false;
    for (const folder of outside) {
      // Two attempts: losing the compare-and-set to a concurrent change is not the same thing as the daemon
      // refusing the folder, and only the refusal is worth remembering and saying.
      for (let attempt = 0; attempt < 2; attempt += 1) {
        if (!row || row.revoked) return;
        const before = row;
        try {
          expectDone(
            await ask(this.ports.client, {
              ask: "integrationGrantChange",
              with: {
                integration_id: integrationId,
                expected_grant_generation: before.grant_generation,
                scopes: before.scopes,
                roots: [...before.roots, folder],
              },
            }),
            "workspace root following",
          );
          widened = true;
          row = await this.ownRow(integrationId);
          break;
        } catch (error) {
          row = await this.ownRow(integrationId);
          const decision = followDecision(folder, before, row);
          if (decision === "followed") {
            // Another window of this same integration won the race. The root is there; only the refresh remains.
            widened = true;
            break;
          }
          if (decision === "retry") continue;
          if (decision === "declined") {
            const why = error instanceof Error ? error.message : String(error);
            this.declined.set(workspaceIdentity(folder), why);
            this.ports.warn(`Runtrol: ${folderName(folder)} cannot become an approved project root: ${why}`);
          }
          break;
        }
      }
      if (!row || row.revoked) return;
    }
    if (widened) await this.ports.refreshRoots();
  }

  private async ownRow(integrationId: string): Promise<IntegrationLine | null> {
    const rows = await ask(this.ports.client, { ask: "integrations" });
    if (rows.say !== "integrations") {
      throw new Error(`the daemon answered the integration listing with ${rows.say}`);
    }
    return rows.with.find((row) => row.integration_id === integrationId) ?? null;
  }
}

/// The open folders the grant does not cover, first spelling kept, duplicates collapsed.
///
/// Coverage means the folder is one of the roots or sits inside one, compared through the same identity function
/// collision detection and project grouping use. A third comparator here would be a third answer to "is this the
/// same folder", and the drive-letter case is exactly where the answers would start disagreeing.
export function foldersOutsideRoots(
  folders: readonly string[],
  roots: readonly string[],
  paths: typeof path.posix | typeof path.win32 = path,
  platform: NodeJS.Platform = process.platform,
): string[] {
  const bases = roots.map((root) => trimmedIdentity(root, paths, platform));
  const outside: string[] = [];
  const seen = new Set<string>();
  for (const folder of folders) {
    const identity = trimmedIdentity(folder, paths, platform);
    if (seen.has(identity)) continue;
    seen.add(identity);
    const covered = bases.some((base) => identityCovers(base, identity, paths.sep));
    if (!covered) outside.push(folder);
  }
  return outside;
}

/// What a failed change attempt turned out to mean, read from the row before and after it.
///
/// The daemon's failure is a sentence, and matching sentences is how a wording change becomes a behaviour
/// change. The rows say the same thing structurally: the folder being covered now means another writer added it,
/// a moved generation means the compare-and-set lost to some other change, and an unmoved generation means the
/// daemon looked at this folder and said no.
export function followDecision(
  folder: string,
  before: Pick<IntegrationLine, "grant_generation">,
  after: Pick<IntegrationLine, "roots" | "grant_generation" | "revoked"> | null,
  paths: typeof path.posix | typeof path.win32 = path,
  platform: NodeJS.Platform = process.platform,
): "followed" | "retry" | "declined" | "gone" {
  if (!after || after.revoked) return "gone";
  if (foldersOutsideRoots([folder], after.roots, paths, platform).length === 0) return "followed";
  if (after.grant_generation !== before.grant_generation) return "retry";
  return "declined";
}

/// An identity with no trailing separator, so "inside" can be asked with plain string prefixes.
///
/// A drive root resolves with its separator kept, and concatenating another separator onto it would make every
/// folder on that drive read as outside it.
function trimmedIdentity(
  value: string,
  paths: typeof path.posix | typeof path.win32,
  platform: NodeJS.Platform,
): string {
  const identity = workspaceIdentity(value, paths, platform);
  return identity.endsWith(paths.sep) ? identity.slice(0, -paths.sep.length) : identity;
}

/// The folder as an operator says it, for the one warning a refused folder earns.
function folderName(folder: string): string {
  const parts = folder.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.at(-1) ?? folder;
}
