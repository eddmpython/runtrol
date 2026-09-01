import { execFile } from "node:child_process";
import path from "node:path";

export type LegacyCleanupOutput = {
  stdout: string;
  stderr: string;
};

export type LegacyCleanupRunner = (
  executable: string,
  words: readonly string[],
  workspace: string,
) => Promise<LegacyCleanupOutput>;

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

/// The one VS Code seam for removing what earlier Runtrol builds registered: provider MCP entries, Runtime grants,
/// and local credentials for the retired Agent Tools and cross-consult surfaces.
///
/// It runs the exact Core already selected by the extension and lets Core own every judgement and every official
/// provider command. Foreign entries stay, and the Core says so line by line. No provider configuration or
/// integration secret enters the Extension Host.
export class LegacyCleanup {
  private tail: Promise<void> = Promise.resolve();

  constructor(
    private readonly locateCore: () => Promise<string>,
    private readonly runner: LegacyCleanupRunner = run,
  ) {}

  run(workspace: string): Promise<readonly string[]> {
    const result = this.tail.then(async () => {
      if (!path.isAbsolute(workspace)) {
        throw new Error(`legacy cleanup needs an absolute project path, got ${JSON.stringify(workspace)}`);
      }
      const executable = await this.locateCore();
      const lines = linesOf(await this.runner(executable, ["legacy", "cleanup"], workspace));
      for (const line of lines) {
        if (!line.startsWith("legacy-mcp  ") && !line.startsWith("legacy-local  ")) {
          throw new Error(`the Core returned an invalid legacy cleanup line: ${JSON.stringify(line)}`);
        }
      }
      return lines;
    });
    this.tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function run(
  executable: string,
  words: readonly string[],
  workspace: string,
): Promise<LegacyCleanupOutput> {
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
          reject(new Error(`legacy cleanup failed: ${detail}`));
        } else {
          resolve({ stdout, stderr });
        }
      },
    );
  });
}

function linesOf(output: LegacyCleanupOutput): string[] {
  return `${output.stdout}\n${output.stderr}`
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}
