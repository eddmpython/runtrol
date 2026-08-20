import { execFile } from "node:child_process";
import { access } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { materializeManagedCore } from "./managedCore";

type LocatedCore = {
  executable: string;
  endpoint: string;
  // SHA-256 of the managed Core this extension installed, when that is the executable in use.
  // Null for an operator-configured corePath or a PATH fallback: supersession only acts on a
  // binary this extension owns, never on somebody else's build.
  managedDigest: string | null;
};

export class CoreLocator {
  private located: Promise<LocatedCore> | null = null;

  constructor(private readonly context: vscode.ExtensionContext) {}

  locate(): Promise<LocatedCore> {
    this.located ??= this.discover();
    return this.located;
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
  }

  private async discover(): Promise<LocatedCore> {
    const configured = vscode.workspace.getConfiguration("runtrol").get<string>("corePath", "").trim();
    const bundled = vscode.Uri.joinPath(
      this.context.extensionUri,
      "resources",
      "core",
      process.platform === "win32" ? "runtrol.exe" : "runtrol",
    ).fsPath;

    const candidates: { executable: string; managedDigest: string | null }[] = [];
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

function probeEndpoint(executable: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      ["endpoint"],
      { encoding: "utf8", timeout: 12_000, windowsHide: true },
      (error, stdout, stderr) => {
        if (error) {
          reject(new Error(stderr.trim() || error.message));
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
