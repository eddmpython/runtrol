import type { StartDecision } from "./chatPlacement";
import type { IsolatedWorkspaces } from "./isolatedWorkspace";
import type { IsolatedWorkspaceLine } from "./protocol";
import type { RuntimeSessionAction, StudioRuntimeClient } from "./runtimeClient";

export type FreshConversationRequest = {
  readonly providerId: string;
  readonly model: string | null;
  readonly reasoningEffort: string | null;
  readonly permission: string | null;
};

type LaunchedSession = Awaited<ReturnType<StudioRuntimeClient["start"]>>;
type LaunchRuntime = Pick<StudioRuntimeClient, "start" | "close">;
type WorkspaceOwner = Pick<IsolatedWorkspaces, "prepare" | "bind" | "release">;

/// Fresh conversation placement and lifecycle ownership, separate from VS Code presentation.
///
/// A worktree is prepared only after the person explicitly chooses isolated placement. A successful provider is
/// bound to that exact Core-owned path before first input, while any failed or unbound start is cleaned precisely.
export class ConversationLauncher {
  constructor(
    private readonly runtime: LaunchRuntime,
    private readonly workspaces: WorkspaceOwner,
    private readonly refreshWorkspaces: () => Promise<void>,
  ) {}

  async openFresh(
    request: FreshConversationRequest,
    project: string,
    decision: StartDecision,
  ): Promise<string> {
    if (decision !== "isolated") {
      return (await this.runtime.start(
        request.providerId,
        project,
        decision,
        request.model,
        request.reasoningEffort,
        request.permission,
      )).sessionId;
    }
    const isolated = await this.workspaces.prepare(project);
    let opened: LaunchedSession;
    try {
      opened = await this.runtime.start(
        request.providerId,
        isolated.workspace,
        "exclusive",
        request.model,
        request.reasoningEffort,
        request.permission,
      );
    } catch (error) {
      const cleanup = await this.releaseUnused(isolated, null);
      await this.refreshWorkspaces().catch(() => undefined);
      if (cleanup) throw combined(error, "Core also could not release the unused isolated workspace", cleanup);
      throw error;
    }
    try {
      await this.workspaces.bind(isolated, opened.sessionId);
    } catch (error) {
      const cleanup = await this.closeUnbound(opened, isolated);
      await this.refreshWorkspaces().catch(() => undefined);
      if (cleanup) {
        throw combined(
          error,
          `The chat started in ${isolated.workspace}, but its cleanup ownership could not be recovered`,
          cleanup,
        );
      }
      throw new Error(
        `The chat started in ${isolated.workspace}, but Core could not bind its cleanup ownership: ${message(error)}`,
      );
    }
    await this.refreshWorkspaces();
    return opened.sessionId;
  }

  private async releaseUnused(workspace: IsolatedWorkspaceLine, sessionId: string | null): Promise<unknown | null> {
    try {
      await this.workspaces.release(workspace.workspace, workspace.workspace_id, sessionId);
      return null;
    } catch (error) {
      return error;
    }
  }

  private async closeUnbound(session: LaunchedSession, workspace: IsolatedWorkspaceLine): Promise<unknown | null> {
    try {
      await this.runtime.close(runtimeAction(session), session.lifecycle === "hotRunning");
    } catch (error) {
      // A live session still owns this path even though durable binding failed. Never remove under it.
      return error;
    }
    return this.releaseUnused(workspace, null);
  }
}

function runtimeAction(session: LaunchedSession): RuntimeSessionAction {
  return {
    sessionId: session.sessionId,
    lifecycle: session.lifecycle,
    generation: session.sessionGeneration,
    workspace: session.workspace,
  };
}

function combined(cause: unknown, context: string, cleanup: unknown): Error {
  return new Error(`${message(cause)}; ${context}: ${message(cleanup)}`);
}

function message(value: unknown): string {
  return value instanceof Error ? value.message : String(value);
}
