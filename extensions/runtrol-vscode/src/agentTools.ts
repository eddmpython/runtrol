import { execFile } from "node:child_process";
import path from "node:path";

export type AgentToolsAction = "enable" | "disable";

export type AgentToolsCommandOutput = {
  stdout: string;
  stderr: string;
};

export type AgentToolsRunner = (
  executable: string,
  words: readonly string[],
  workspace: string,
) => Promise<AgentToolsCommandOutput>;

export type AgentToolsResult = {
  workspace: string;
  lines: readonly string[];
  alreadySettled: boolean;
};

/// The globalState key remembering which Core image last ran the legacy cleanup.
export const LEGACY_CLEANUP_KEY = "runtrol.legacyMcpCleanup";

/// What one Core image is remembered as once it has cleaned up after its predecessors.
///
/// An extension-managed Core is named by its digest, so every upgrade is a new stamp and runs cleanup once. A
/// Core the operator configured by path has no digest; it is one stamp for as long as it stays configured.
export function legacyCleanupStamp(managedDigest: string | null): string {
  return managedDigest ?? "unmanaged";
}

/// Whether this Core image still has to clean up after earlier Runtrol builds.
export function legacyCleanupDue(remembered: string | undefined, managedDigest: string | null): boolean {
  return remembered !== legacyCleanupStamp(managedDigest);
}

/// The one VS Code seam for local Agent Tools administration.
///
/// It runs the exact Core already selected by the extension, passes the project as one argv word, and lets Core own
/// enrollment, official provider registration, Runtime revocation, and protected credential deletion. No provider
/// configuration or integration secret enters the Extension Host.
export class AgentToolsController {
  private tail: Promise<void> = Promise.resolve();
  private roots = new Set<string>();
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly locateCore: () => Promise<string>,
    private readonly runner: AgentToolsRunner = run,
  ) {}

  enable(workspace: string): Promise<AgentToolsResult> {
    return this.change("enable", workspace);
  }

  disable(workspace: string): Promise<AgentToolsResult> {
    return this.change("disable", workspace);
  }

  enabled(workspace: string): boolean {
    return this.roots.has(identity(workspace));
  }

  /// Ask the selected Core to remove every provider registration, Runtime grant, and local credential an earlier
  /// Runtrol build left behind. Foreign entries stay; the Core says so line by line.
  cleanupLegacy(workspace: string): Promise<readonly string[]> {
    const result = this.tail.then(async () => {
      if (!path.isAbsolute(workspace)) {
        throw new Error(`Agent Tools needs an absolute project path, got ${JSON.stringify(workspace)}`);
      }
      const executable = await this.locateCore();
      const lines = linesOf(await this.runner(executable, ["tools", "cleanup"], workspace));
      for (const line of lines) {
        if (!line.startsWith("legacy-mcp  ") && !line.startsWith("legacy-local  ")) {
          throw new Error(`the Core returned an invalid legacy cleanup line: ${JSON.stringify(line)}`);
        }
      }
      if (this.roots.size > 0) {
        this.roots = new Set();
        this.publish();
      }
      return lines;
    });
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  dispose(): void {
    this.listeners.clear();
  }

  refresh(workspace: string): Promise<void> {
    const result = this.tail.then(async () => {
      if (!path.isAbsolute(workspace)) {
        throw new Error(`Agent Tools needs an absolute project path, got ${JSON.stringify(workspace)}`);
      }
      const executable = await this.locateCore();
      const lines = linesOf(await this.runner(executable, ["tools", "list"], workspace));
      const next = new Set<string>();
      if (!(lines.length === 1 && lines[0] === "no projects enabled")) {
        for (const line of lines) {
          const prefix = "enabled  ";
          if (!line.startsWith(prefix) || !path.isAbsolute(line.slice(prefix.length))) {
            throw new Error(`the Core returned an invalid Agent Tools project line: ${JSON.stringify(line)}`);
          }
          next.add(identity(line.slice(prefix.length)));
        }
      }
      if (!sameRoots(this.roots, next)) {
        this.roots = next;
        this.publish();
      }
    });
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private change(action: AgentToolsAction, workspace: string): Promise<AgentToolsResult> {
    const result = this.tail.then(async () => {
      if (!path.isAbsolute(workspace)) {
        throw new Error(`Agent Tools needs an absolute project path, got ${JSON.stringify(workspace)}`);
      }
      const executable = await this.locateCore();
      const output = await this.runner(executable, ["tools", action, workspace], workspace);
      const lines = linesOf(output);
      const expected = action === "enable"
        ? lines.some((line) => line.startsWith("Agent Tools enabled for "))
        : lines.some((line) => line.startsWith("Agent Tools disabled and Runtime authority revoked for "))
          || lines.some((line) => line.startsWith("Agent Tools is already disabled for "));
      if (!expected) {
        throw new Error(`the Core did not confirm Agent Tools ${action}: ${lines.join(" ") || "no output"}`);
      }
      const before = new Set(this.roots);
      const settled = settledWorkspace(action, lines) ?? workspace;
      if (!path.isAbsolute(settled)) {
        throw new Error(`the Core returned an invalid Agent Tools project path: ${JSON.stringify(settled)}`);
      }
      if (action === "enable") {
        this.roots.add(identity(settled));
      } else {
        this.roots.delete(identity(workspace));
        this.roots.delete(identity(settled));
      }
      if (!sameRoots(before, this.roots)) this.publish();
      return {
        workspace,
        lines,
        alreadySettled: lines.some((line) => line.includes("already disabled")),
      };
    });
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private publish(): void {
    for (const listener of this.listeners) listener();
  }
}

function run(
  executable: string,
  words: readonly string[],
  workspace: string,
): Promise<AgentToolsCommandOutput> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      [...words],
      {
        cwd: workspace,
        encoding: "utf8",
        timeout: 120_000,
        maxBuffer: 64 * 1024,
        windowsHide: true,
      },
      (error, stdout, stderr) => {
        if (error) {
          const detail = linesOf({ stdout, stderr }).join(" ") || error.message;
          reject(new Error(`Agent Tools command failed: ${detail}`));
        } else {
          resolve({ stdout, stderr });
        }
      },
    );
  });
}

function linesOf(output: AgentToolsCommandOutput): string[] {
  return `${output.stdout}\n${output.stderr}`
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function identity(workspace: string): string {
  const resolved = path.resolve(workspace);
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function sameRoots(left: ReadonlySet<string>, right: ReadonlySet<string>): boolean {
  return left.size === right.size && [...left].every((root) => right.has(root));
}

function settledWorkspace(action: AgentToolsAction, lines: readonly string[]): string | null {
  const prefixes = action === "enable"
    ? ["Agent Tools enabled for "]
    : [
      "Agent Tools disabled and Runtime authority revoked for ",
      "Agent Tools is already disabled for ",
    ];
  for (const prefix of prefixes) {
    const line = lines.find((candidate) => candidate.startsWith(prefix));
    if (line) return line.slice(prefix.length);
  }
  return null;
}
