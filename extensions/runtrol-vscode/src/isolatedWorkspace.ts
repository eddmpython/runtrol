import { randomUUID } from "node:crypto";
import path from "node:path";

import type { CoreClient } from "./core/client";
import type {
  IntegrationLine,
  IsolatedWorkspaceLine,
  IsolatedWorkspaceReleaseLine,
  Request,
  Response,
} from "./protocol";
import { identityCovers, workspaceIdentity } from "./workspaceCollision";

type CoreExchange = Pick<CoreClient, "read">;

/// Studio's narrow view of Core-owned ordinary-chat worktrees.
///
/// No filesystem operation lives here. The exact same request is retried after an ambiguous local connection,
/// while Core owns creation, durable identity checks, binding, and clean-only removal.
export class IsolatedWorkspaces {
  constructor(
    private readonly core: CoreExchange,
    private readonly integrationId: () => string | null,
    private readonly refreshAuthorization: () => Promise<void>,
  ) {}

  async list(): Promise<readonly IsolatedWorkspaceLine[]> {
    const response = await this.ask({ ask: "workspaceIsolateList" });
    if (response.say !== "isolatedWorkspaces") {
      throw new Error(`Core answered isolated workspace listing with ${response.say}`);
    }
    return response.with;
  }

  async authorizedRoots(): Promise<readonly string[]> {
    const integrationId = this.integrationId();
    if (!integrationId) return [];
    return (await this.integration(integrationId)).roots;
  }

  async prepare(project: string, requestId: string = randomUUID()): Promise<IsolatedWorkspaceLine> {
    const response = await this.ask({
      ask: "workspaceIsolatePrepare",
      with: { request_id: requestId, project },
    });
    if (response.say !== "isolatedWorkspace") {
      throw new Error(`Core answered isolated workspace preparation with ${response.say}`);
    }
    const workspace = response.with;
    let authorizationChanged = false;
    try {
      authorizationChanged = await this.changeWorkspaceRoot(workspace.workspace, "add");
      if (authorizationChanged) await this.refreshAuthorization();
    } catch (error) {
      try {
        if (authorizationChanged && await this.changeWorkspaceRoot(workspace.workspace, "remove")) {
          await this.refreshAuthorization();
        }
        await this.releaseOwned(workspace.workspace, workspace.workspace_id, null);
      } catch (cleanupError) {
        throw new Error(
          `${error instanceof Error ? error.message : String(error)}; Core also could not release the `
          + `unauthorized isolated workspace: ${cleanupError instanceof Error ? cleanupError.message : String(cleanupError)}`,
        );
      }
      throw error;
    }
    return workspace;
  }

  async bind(workspace: IsolatedWorkspaceLine, sessionId: string): Promise<void> {
    const response = await this.ask({
      ask: "workspaceIsolateBind",
      with: {
        workspace_id: workspace.workspace_id,
        session_id: sessionId,
        workspace: workspace.workspace,
      },
    });
    if (response.say !== "done") {
      throw new Error(`Core answered isolated workspace binding with ${response.say}`);
    }
  }

  async release(
    workspace: string,
    workspaceId: string | null,
    sessionId: string | null,
  ): Promise<IsolatedWorkspaceReleaseLine | null> {
    const owned = workspaceId !== null || (await this.list()).some((candidate) => (
      candidate.workspace === workspace && candidate.session_id === sessionId
    ));
    if (!owned) return null;
    // Revoke the public Runtime's exact generated root before Core removes or preserves the worktree. A broader
    // root the operator approved is never changed, and an ordinary non-isolated session never reaches this path.
    if (await this.changeWorkspaceRoot(workspace, "remove")) {
      await this.refreshAuthorization();
    }
    return this.releaseOwned(workspace, workspaceId, sessionId);
  }

  private async releaseOwned(
    workspace: string,
    workspaceId: string | null,
    sessionId: string | null,
  ): Promise<IsolatedWorkspaceReleaseLine | null> {
    const response = await this.ask({
      ask: "workspaceIsolateRelease",
      with: {
        workspace_id: workspaceId,
        session_id: sessionId,
        workspace,
      },
    });
    if (response.say === "done") return null;
    if (response.say !== "isolatedWorkspaceReleased") {
      throw new Error(`Core answered isolated workspace cleanup with ${response.say}`);
    }
    return response.with;
  }

  /// Grant the Studio's public Runtime client only the exact Core-created worktree, then remove that exact root
  /// when its ownership ends. The private local administration boundary still validates every root and the
  /// compare-and-set generation prevents this window from overwriting another local grant edit.
  private async changeWorkspaceRoot(workspace: string, action: "add" | "remove"): Promise<boolean> {
    const integrationId = this.integrationId();
    if (!integrationId) throw new Error("Runtrol Studio has no approved Runtime integration");
    let lastFailure: unknown = null;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const row = await this.integration(integrationId);
      if (row.revoked) throw new Error("Runtrol Studio Runtime access was revoked");
      const exact = row.roots.filter((root) => sameWorkspace(root, workspace));
      if (action === "add" && workspaceCovered(workspace, row.roots)) return false;
      if (action === "remove" && exact.length === 0) return false;
      const roots = action === "add"
        ? [...row.roots, workspace]
        : row.roots.filter((root) => !sameWorkspace(root, workspace));
      try {
        const response = await this.ask({
          ask: "integrationGrantChange",
          with: {
            integration_id: integrationId,
            expected_grant_generation: row.grant_generation,
            scopes: row.scopes,
            roots,
          },
        });
        if (response.say !== "done") {
          throw new Error(`Core answered isolated workspace authorization with ${response.say}`);
        }
        return true;
      } catch (error) {
        lastFailure = error;
      }
    }
    throw lastFailure instanceof Error
      ? lastFailure
      : new Error("Core could not change isolated workspace authorization");
  }

  private async integration(integrationId: string): Promise<IntegrationLine> {
    const response = await this.ask({ ask: "integrations" });
    if (response.say !== "integrations") {
      throw new Error(`Core answered Runtime integration listing with ${response.say}`);
    }
    const row = response.with.find((candidate) => candidate.integration_id === integrationId);
    if (!row) throw new Error("Runtrol Studio Runtime integration no longer exists");
    return row;
  }

  private async ask(request: Request): Promise<Response> {
    const { response } = await this.core.read(request);
    if (response.say === "failed") throw new Error(response.with.message);
    return response;
  }
}

function sameWorkspace(left: string, right: string): boolean {
  return workspaceIdentity(left) === workspaceIdentity(right);
}

function workspaceCovered(workspace: string, roots: readonly string[]): boolean {
  const candidate = workspaceIdentity(workspace);
  return roots.some((root) => {
    const base = workspaceIdentity(root);
    return identityCovers(base, candidate, path.sep);
  });
}
