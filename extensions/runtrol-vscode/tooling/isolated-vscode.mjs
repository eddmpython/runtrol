import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, createReadStream } from "node:fs";
import {
  copyFile,
  link,
  lstat,
  mkdir,
  readFile,
  readdir,
  readlink,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import path from "node:path";

import {
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
  runTests,
} from "@vscode/test-electron";

import { extensionInstallPrefix } from "./extension-manifest.mjs";
import { descendantPids, normalizedExecutable, processRows } from "./process-identity.mjs";

export const isolatedProfileSettings = Object.freeze({
  "extensions.autoCheckUpdates": false,
  "extensions.autoUpdate": false,
  "workbench.startupEditor": "none",
});

export const isolatedLaunchArguments = Object.freeze([
  "--disable-updates",
]);

export async function acquireVSCode(cachePath) {
  const executable = await downloadAndUnzipVSCode({
    version: process.env.RUNTROL_TEST_VSCODE_VERSION || "stable",
    cachePath,
  });
  const [cli] = resolveCliArgsFromVSCodeExecutablePath(executable);
  return { executable, cli };
}

export async function isolateVSCodeProduct(executable, destination) {
  const product = await locateProduct(executable);
  await rm(destination, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  await cloneWithLinks(product.root, destination);
  const isolatedProduct = path.join(destination, product.relativeProduct);
  const manifest = JSON.parse(await readFile(isolatedProduct, "utf8"));
  delete manifest.extensionsGallery;
  await rm(isolatedProduct, { force: true });
  await writeFile(isolatedProduct, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return path.join(destination, product.relativeExecutable);
}

async function locateProduct(executable) {
  const resolvedExecutable = path.resolve(executable);
  let root = path.dirname(resolvedExecutable);
  for (let depth = 0; depth < 6; depth += 1) {
    const relativeProducts = [
      path.join("resources", "app", "product.json"),
      path.join("Resources", "app", "product.json"),
    ];
    const children = await readdir(root, { withFileTypes: true }).catch(() => []);
    for (const child of children) {
      if (child.isDirectory()) {
        relativeProducts.push(
          path.join(child.name, "resources", "app", "product.json"),
          path.join(child.name, "Resources", "app", "product.json"),
        );
      }
    }
    for (const relativeProduct of relativeProducts) {
      const candidate = path.join(root, relativeProduct);
      if (await lstat(candidate).then((entry) => entry.isFile()).catch(() => false)) {
        return {
          root,
          relativeProduct,
          relativeExecutable: path.relative(root, resolvedExecutable),
        };
      }
    }
    const parent = path.dirname(root);
    if (parent === root) break;
    root = parent;
  }
  throw new Error(`cannot locate VS Code product.json above ${resolvedExecutable}`);
}

async function cloneWithLinks(source, destination) {
  const pending = [[source, destination]];
  while (pending.length > 0) {
    const [directory, clonedDirectory] = pending.pop();
    await mkdir(clonedDirectory, { recursive: true });
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const from = path.join(directory, entry.name);
      const to = path.join(clonedDirectory, entry.name);
      if (entry.isDirectory()) {
        pending.push([from, to]);
      } else if (entry.isSymbolicLink()) {
        await symlink(await readlink(from), to);
      } else if (entry.isFile()) {
        await link(from, to).catch(async () => {
          await copyFile(from, to, constants.COPYFILE_FICLONE);
        });
      }
    }
  }
}

function installExtension(cli, source, userData, extensions) {
  const installed = spawnSync(
    cli,
    [
      "--user-data-dir",
      userData,
      "--extensions-dir",
      extensions,
      "--install-extension",
      source,
      "--force",
    ],
    {
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
      shell: process.platform === "win32",
    },
  );
  if (installed.status !== 0) {
    throw new Error(
      `extension installation failed: ${installed.error?.message ?? `exit ${String(installed.status)}`}\n`
      + `${installed.stdout ?? ""}${installed.stderr ?? ""}`,
    );
  }
}

export function installVSIX(cli, archive, userData, extensions) {
  installExtension(cli, archive, userData, extensions);
}

export function installMarketplaceExtension(cli, identifier, userData, extensions) {
  installExtension(cli, identifier, userData, extensions);
}

export function uninstallExtension(cli, identifier, userData, extensions) {
  const uninstalled = spawnSync(
    cli,
    [
      "--user-data-dir",
      userData,
      "--extensions-dir",
      extensions,
      "--uninstall-extension",
      identifier,
    ],
    {
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
      shell: process.platform === "win32",
    },
  );
  if (uninstalled.status !== 0) {
    throw new Error(
      `extension uninstall failed: ${uninstalled.error?.message ?? `exit ${String(uninstalled.status)}`}\n`
      + `${uninstalled.stdout ?? ""}${uninstalled.stderr ?? ""}`,
    );
  }
}

export function runInstalledExtensionTest(options) {
  return runTests({
    vscodeExecutablePath: options.vscodeExecutablePath,
    extensionDevelopmentPath: options.verifierRoot,
    extensionTestsPath: options.testEntry,
    extensionTestsEnv: options.environment,
    launchArgs: [
      options.workspace,
      ...isolatedLaunchArguments,
      "--disable-workspace-trust",
      "--skip-welcome",
      `--user-data-dir=${options.userData}`,
      `--extensions-dir=${options.extensions}`,
    ],
  });
}

export async function findInstalledExtension(root, expectedVersion) {
  const entries = await readdir(root, { withFileTypes: true });
  const matches = entries.filter(
    (entry) => entry.isDirectory() && entry.name.startsWith(extensionInstallPrefix),
  );
  if (!expectedVersion) {
    if (matches.length !== 1) {
      throw new Error(`expected one isolated Runtrol Studio installation, found ${matches.length}`);
    }
    return path.join(root, matches[0].name);
  }
  const expected = matches.filter((entry) => entry.name.includes(`-${expectedVersion}`));
  if (expected.length !== 1) {
    throw new Error(
      `expected one Runtrol Studio ${expectedVersion} directory, found ${expected.length} among ${matches.length}`,
    );
  }
  return path.join(root, expected[0].name);
}

export async function fileDigest(file) {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(file)) {
    digest.update(chunk);
  }
  return digest.digest("hex");
}

export async function terminateExactProcesses(marker, executable) {
  const expected = executable ? normalizedExecutable(executable) : "";
  const normalizedMarker = process.platform === "win32"
    ? marker.toLocaleLowerCase("en-US")
    : marker;
  const matchingIdentities = () => {
    const rows = processRows();
    const roots = rows.filter((row) => {
      const command = process.platform === "win32"
        ? row.command.toLocaleLowerCase("en-US")
        : row.command;
      return command.includes(normalizedMarker)
        || (expected && normalizedExecutable(row.executable) === expected);
    });
    const pids = new Set(roots.map((row) => row.pid));
    for (const root of roots) {
      for (const pid of descendantPids(rows, root.pid)) {
        pids.add(pid);
      }
    }
    return rows.filter((row) => pids.has(row.pid) && row.pid !== process.pid);
  };

  const identities = matchingIdentities();
  signalExactProcesses(identities, "SIGTERM");
  let survivors = await waitForExactProcesses(identities, 5_000);
  if (survivors.length > 0) {
    signalExactProcesses(survivors, "SIGKILL");
    survivors = await waitForExactProcesses(identities, 5_000);
  }
  const late = matchingIdentities();
  if (late.length > 0) {
    signalExactProcesses(late, "SIGKILL");
    await waitForExactProcesses(late, 5_000);
  }
  const unique = new Map();
  for (const identity of [...survivingIdentities(identities), ...survivingIdentities(late)]) {
    unique.set(identity.pid, identity);
  }
  survivors = [...unique.values()];
  if (survivors.length > 0) {
    throw new Error(
      `isolated process cleanup left exact PIDs ${survivors.map((row) => row.pid).join(", ")}`,
    );
  }
}

function signalExactProcesses(identities, signal) {
  for (const identity of identities) {
    try {
      process.kill(identity.pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
}

async function waitForExactProcesses(identities, milliseconds) {
  const deadline = Date.now() + milliseconds;
  let survivors = survivingIdentities(identities);
  while (survivors.length > 0 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    survivors = survivingIdentities(identities);
  }
  return survivors;
}

function survivingIdentities(identities) {
  const current = new Map(processRows().map((row) => [row.pid, row]));
  return identities.filter((identity) => {
    const row = current.get(identity.pid);
    return row
      && row.command === identity.command
      && normalizedExecutable(row.executable) === normalizedExecutable(identity.executable);
  });
}
