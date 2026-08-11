import { access, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

type ExtensionApi = {
  readonly ready: Promise<void>;
  refresh(): Promise<void>;
};

let currentStage = "starting";

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    await verifyInstalledPackage(resultPath);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        stage: currentStage,
      }),
      "utf8",
    );
    throw error;
  }
}

async function verifyInstalledPackage(resultPath: string): Promise<void> {
  const expectedVersion = requiredEnvironment("RUNTROL_TEST_EXTENSION_VERSION");
  const expectedTarget = requiredEnvironment("RUNTROL_TEST_EXTENSION_TARGET");
  const installedRoot = path.resolve(requiredEnvironment("RUNTROL_TEST_INSTALLED_ROOT"));
  const configuredCore = vscode.workspace.getConfiguration("runtrol").get<string>("corePath", "");
  if (configuredCore !== "") {
    throw new Error(`the clean profile unexpectedly configured Core ${configuredCore}`);
  }

  currentStage = "locating-installed-extension";
  const extension = vscode.extensions.getExtension("eddmpython.runtrol-studio");
  if (!extension) {
    throw new Error("the installed Runtrol Studio extension is missing");
  }
  if (extension.packageJSON.version !== expectedVersion) {
    throw new Error(`installed version ${String(extension.packageJSON.version)} is not ${expectedVersion}`);
  }
  const installedPath = path.resolve(extension.extensionPath);
  if (installedPath !== installedRoot && !installedPath.startsWith(`${installedRoot}${path.sep}`)) {
    throw new Error(`the extension loaded outside the isolated installation root: ${installedPath}`);
  }

  const executableName = process.platform === "win32" ? "runtrol.exe" : "runtrol";
  const corePath = path.join(installedPath, "resources", "core", executableName);
  await access(corePath);

  currentStage = "activating-installed-extension";
  const api = await within(extension.activate() as Promise<ExtensionApi>, 10_000, "installed extension activation");
  await within(api.ready, 15_000, "bundled Core discovery");
  currentStage = "refreshing-through-bundled-core";
  await within(api.refresh(), 5_000, "installed extension refresh");
  await within(
    vscode.commands.executeCommand("workbench.view.extension.runtrol"),
    5_000,
    "opening the installed Runtrol view",
  );

  await writeFile(
    resultPath,
    JSON.stringify({
      vscode: vscode.version,
      extensionVersion: expectedVersion,
      target: expectedTarget,
      extensionPath: installedPath,
      corePath,
      configuredCore,
    }),
    "utf8",
  );
}

function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    Promise.resolve(work),
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) {
      clearTimeout(timer);
    }
  });
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}
