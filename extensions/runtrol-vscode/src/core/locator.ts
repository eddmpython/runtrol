import { execFile } from "node:child_process";
import { access } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

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
      try {
        await access(bundled);
        candidates.push(bundled);
      } catch {
        // A development build normally has no bundled core yet.
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

