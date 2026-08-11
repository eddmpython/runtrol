import { createHash, randomUUID } from "node:crypto";
import { createReadStream } from "node:fs";
import {
  chmod,
  copyFile,
  link,
  mkdir,
  opendir,
  rename,
  stat,
  unlink,
} from "node:fs/promises";
import path from "node:path";

const DIGEST_PATTERN = /^[a-f0-9]{64}$/;

export type ManagedCore = {
  executable: string;
  digest: string;
  replaced: boolean;
};

export async function materializeManagedCore(source: string, storageRoot: string): Promise<ManagedCore> {
  const sourceInfo = await stat(source);
  if (!sourceInfo.isFile()) {
    throw new Error(`the bundled Core is not a file: ${source}`);
  }
  const sourceDigest = await fileDigest(source);
  const managedRoot = path.join(storageRoot, "core");
  const executable = path.join(managedRoot, process.platform === "win32" ? "runtrol.exe" : "runtrol");
  await mkdir(managedRoot, { recursive: true });

  const currentDigest = await optionalFileDigest(executable);
  if (currentDigest === sourceDigest) {
    await removeInactiveImages(managedRoot, path.basename(executable));
    return { executable, digest: sourceDigest, replaced: false };
  }

  const incoming = `${executable}.incoming-${process.pid}-${randomUUID()}`;
  let preserved: string | null = null;
  try {
    await copyFile(source, incoming);
    if (process.platform !== "win32") {
      await chmod(incoming, 0o755);
    }
    if (await fileDigest(incoming) !== sourceDigest) {
      throw new Error("the managed Core copy differs from the bundled Core");
    }

    if (currentDigest) {
      preserved = `${executable}.inuse-${currentDigest}`;
      await preserveCurrentImage(executable, preserved, currentDigest);
    }
    await rename(incoming, executable);
    if (await fileDigest(executable) !== sourceDigest) {
      throw new Error("the managed Core differs after atomic replacement");
    }
  } finally {
    await removeIfPresent(incoming);
  }

  await removeInactiveImages(managedRoot, path.basename(executable));
  return { executable, digest: sourceDigest, replaced: currentDigest !== null };
}

async function preserveCurrentImage(executable: string, preserved: string, digest: string): Promise<void> {
  try {
    await link(executable, preserved);
  } catch (error) {
    if (errorCode(error) !== "EEXIST") {
      throw error;
    }
    if (await optionalFileDigest(preserved) !== digest) {
      throw new Error(`the preserved Core image has unexpected contents: ${preserved}`);
    }
  }
}

async function removeInactiveImages(root: string, executableName: string): Promise<void> {
  const prefix = `${executableName}.inuse-`;
  const directory = await opendir(root);
  for await (const entry of directory) {
    if (!entry.isFile() || !entry.name.startsWith(prefix)) {
      continue;
    }
    const digest = entry.name.slice(prefix.length);
    if (!DIGEST_PATTERN.test(digest)) {
      continue;
    }
    try {
      await unlink(path.join(root, entry.name));
    } catch (error) {
      if (!isMappedImageError(error)) {
        throw error;
      }
      // Windows refuses to unlink the preserved image while its daemon is alive. A later activation retries cleanup.
    }
  }
}

async function removeIfPresent(file: string): Promise<void> {
  try {
    await unlink(file);
  } catch (error) {
    if (errorCode(error) !== "ENOENT") {
      throw error;
    }
  }
}

async function optionalFileDigest(file: string): Promise<string | null> {
  try {
    const info = await stat(file);
    if (!info.isFile()) {
      throw new Error(`the managed Core path is not a file: ${file}`);
    }
    return await fileDigest(file);
  } catch (error) {
    if (errorCode(error) === "ENOENT") {
      return null;
    }
    throw error;
  }
}

async function fileDigest(file: string): Promise<string> {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(file)) {
    digest.update(chunk as Buffer);
  }
  return digest.digest("hex");
}

function errorCode(error: unknown): string | undefined {
  return error && typeof error === "object" && "code" in error
    ? String((error as NodeJS.ErrnoException).code)
    : undefined;
}

function isMappedImageError(error: unknown): boolean {
  return process.platform === "win32" && ["EACCES", "EBUSY", "EPERM"].includes(errorCode(error) ?? "");
}
