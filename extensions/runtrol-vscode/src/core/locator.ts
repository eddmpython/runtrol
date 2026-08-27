import { execFile } from "node:child_process";
import { access } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { FrameTransport } from "./framing";
import { materializeManagedCore } from "./managedCore";

type LocatedCore = {
  executable: string;
  endpoint: string;
  // SHA-256 of the managed Core this extension installed, when that is the executable in use.
  // Null for an operator-configured corePath or a PATH fallback: currency is only checked against a
  // binary this extension owns, never against somebody else's build.
  managedDigest: string | null;
};

type Candidate = { executable: string; managedDigest: string | null };

export class CoreLocator {
  private located: Promise<LocatedCore> | null = null;
  private candidates: Promise<Candidate[]> | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    /// The control endpoint the public Runtime locator lists for the installed generation, or null when none
    /// is listed. Starting a Core process costs a few hundred milliseconds (measured 2026-08-27), so when a
    /// generation is already published the probe is not run at all: the listed endpoint is tried first and
    /// `endpoint` is spawned only when nothing answers there, which is also the only case that needs a daemon
    /// started.
    private readonly listedControlEndpoint: () => Promise<string | null> = async () => null,
  ) {}

  locate(): Promise<LocatedCore> {
    this.located ??= this.discover();
    return this.located;
  }

  /// The executable the probe will try first, known before the probe answers.
  ///
  /// The public Runtime locator verifies itself by running this file, and that verification can start while
  /// the private endpoint probe is still running instead of after it. The probe may still settle on a later
  /// candidate; `runtimeExecutable` remains the answer once it has.
  async firstCandidate(): Promise<Candidate> {
    const [first] = await this.candidateExecutables();
    return first!;
  }

  async runtimeExecutable(): Promise<string> {
    return (await this.locate()).executable;
  }

  /// The digest of the extension-installed Core when it is the executable in use, or null.
  async managedDigest(): Promise<string | null> {
    return (await this.locate()).managedDigest;
  }

  invalidate(): void {
    this.located = null;
    this.candidates = null;
  }

  private candidateExecutables(): Promise<Candidate[]> {
    this.candidates ??= this.gatherCandidates().catch((error: unknown) => {
      this.candidates = null;
      throw error;
    });
    return this.candidates;
  }

  private async gatherCandidates(): Promise<Candidate[]> {
    const configured = vscode.workspace.getConfiguration("runtrol").get<string>("corePath", "").trim();
    const bundled = vscode.Uri.joinPath(
      this.context.extensionUri,
      "resources",
      "core",
      process.platform === "win32" ? "runtrol.exe" : "runtrol",
    ).fsPath;

    const candidates: Candidate[] = [];
    if (configured) {
      if (!path.isAbsolute(configured)) {
        throw new Error("runtrol.corePath must be an absolute path");
      }
      candidates.push({ executable: configured, managedDigest: null });
    } else {
      let bundledExists = false;
      try {
        await access(bundled);
        bundledExists = true;
      } catch (error) {
        if (!isNotFound(error)) {
          throw error;
        }
        // A development build normally has no bundled core yet.
      }
      if (bundledExists) {
        const managed = await materializeManagedCore(bundled, this.context.globalStorageUri.fsPath);
        candidates.push({ executable: managed.executable, managedDigest: managed.digest });
      }
      candidates.push({ executable: "runtrol", managedDigest: null });
    }
    return candidates;
  }

  private async discover(): Promise<LocatedCore> {
    const candidates = await this.candidateExecutables();
    const listed = await this.listedControlEndpoint();
    if (listed && await answers(listed)) {
      const [first] = candidates;
      return { executable: first!.executable, endpoint: listed, managedDigest: first!.managedDigest };
    }
    const failures: string[] = [];
    for (const { executable, managedDigest } of candidates) {
      try {
        const endpoint = await probeEndpoint(executable);
        return { executable, endpoint, managedDigest };
      } catch (error) {
        failures.push(`${executable}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    throw new Error(`no usable runtrol core was found. ${failures.join(" | ")}`);
  }
}

function isNotFound(error: unknown): boolean {
  return Boolean(
    error && typeof error === "object" && "code" in error
      && String((error as NodeJS.ErrnoException).code) === "ENOENT",
  );
}

/// Whether something accepts a connection at the listed endpoint. A stale listing (a generation that died
/// without unpublishing) answers nothing, and then the probe below starts a fresh one as before.
async function answers(endpoint: string): Promise<boolean> {
  try {
    const transport = await FrameTransport.connect(endpoint, 1_000);
    transport.close();
    return true;
  } catch {
    // Not reported: an unanswered listing is the ordinary reason to fall through to the probe, whose own
    // failure is what gets reported.
    return false;
  }
}

function probeEndpoint(executable: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      ["endpoint"],
      // Thirty seconds, not twelve: `endpoint` may be STARTING the first daemon of a home, and that daemon
      // binds first and assembles afterwards, so on a storming disk the greeting legitimately arrives late
      // (measured 2026-08-27 on the CI hosts: first assembly outlived the old probe budget and the extension
      // reported a healthy install as "no usable core").
      { encoding: "utf8", timeout: 30_000, windowsHide: true },
      (error, stdout, stderr) => {
        if (error) {
          // Both halves, always: the exec error says how the probe ended (exit code, timeout kill) and the
          // core's stderr says what it was doing. One without the other read as silence on the CI hosts
          // (measured 2026-08-27: "Command failed: ... endpoint" with the cause invisible).
          const said = stderr.trim();
          reject(new Error(said ? `${error.message.trim()}: ${said}` : error.message));
          return;
        }
        const endpoint = stdout.trim();
        if (!endpoint) {
          reject(new Error("the core returned an empty endpoint"));
          return;
        }
        resolve(endpoint);
      },
    );
  });
}
