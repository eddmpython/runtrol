import { execFile } from "node:child_process";
import { access } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { materializeManagedCore } from "./managedCore";

type LocatedCore = {
  executable: string;
  endpoint: string;
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

    const candidates: string[] = [];
    if (configured) {
      if (!path.isAbsolute(configured)) {
        throw new Error("runtrol.corePath must be an absolute path");
      }
      candidates.push(configured);
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
        candidates.push(managed.executable);
      }
      candidates.push("runtrol");
    }

    const failures: string[] = [];
    for (const executable of candidates) {
      try {
        const endpoint = await probeEndpoint(executable);
        return { executable, endpoint };
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
