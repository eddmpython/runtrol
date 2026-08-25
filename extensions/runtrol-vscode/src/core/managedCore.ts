import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { chmod, copyFile, mkdir, opendir, readFile, rename, stat, unlink, writeFile } from "node:fs/promises";
import path from "node:path";

const DIGEST_PATTERN = /^[a-f0-9]{64}$/;
/// How much of the digest names the file. Sixteen hex digits: no two builds collide, and a directory
/// listing stays readable.
const NAME_DIGEST_LENGTH = 16;

export type ManagedCore = {
  executable: string;
  digest: string;
  replaced: boolean;
};

/// Put the bundled Core where the daemon runs from, named by its content.
///
/// Every build gets its own file (`runtrol-<digest>.exe`), and an existing file is never written over.
/// Measured 2026-08-25 on Windows: renaming a new image over the one the running daemon was started from
/// fails with EPERM, so a content-addressed name is the only replacement that cannot fail while a daemon
/// runs. The daemon started from the previous file keeps it mapped as its own generation; the new build starts
/// beside it from the new file and the old generation drains; the previous file is removed once nothing maps it.
export async function materializeManagedCore(source: string, storageRoot: string): Promise<ManagedCore> {
  const sourceInfo = await stat(source);
  if (!sourceInfo.isFile()) {
    throw new Error(`the bundled Core is not a file: ${source}`);
  }
  const managedRoot = path.join(storageRoot, "core");
  await mkdir(managedRoot, { recursive: true });
  const remembered = await readDigests(managedRoot);
  const sourceDigest = await rememberedDigest(source, sourceInfo, remembered);
  const executable = path.join(managedRoot, imageName(sourceDigest));

  const others = await otherImages(managedRoot, executable);
  if (await optionalRememberedDigest(executable, remembered) === sourceDigest) {
    await writeDigests(managedRoot, remembered);
    await removeInactiveImages(others);
    return { executable, digest: sourceDigest, replaced: false };
  }

  // Copied beside its final name and moved into place whole, so a reader never sees a half-written image
  // under the name a daemon would be started from.
  const incoming = `${executable}.incoming-${process.pid}`;
  try {
    await copyFile(source, incoming);
    if (process.platform !== "win32") {
      await chmod(incoming, 0o755);
    }
    if (await fileDigest(incoming) !== sourceDigest) {
      throw new Error("the managed Core copy differs from the bundled Core");
    }
    await rename(incoming, executable);
    const placed = await stat(executable);
    if (await rememberedDigest(executable, placed, remembered, true) !== sourceDigest) {
      throw new Error("the managed Core differs after placement");
    }
  } finally {
    await removeIfPresent(incoming);
  }
  await writeDigests(managedRoot, remembered);
  await removeInactiveImages(others);
  return { executable, digest: sourceDigest, replaced: others.length > 0 };
}

/// Digests remembered by file identity (size and modification time), so an activation that finds the
/// same bundled Core and the same installed image reads two stats instead of hashing thirty megabytes.
///
/// Measured 2026-08-25: one sha256 of the 15 MB Core took 60 ms, and every activation did it twice (the
/// bundled source and the installed image) before the sidebar could draw. A changed file changes its
/// identity and is hashed again; the hash after a fresh copy is always computed, never remembered.
type Digests = Record<string, { size: number; mtimeMs: number; digest: string }>;

const DIGESTS_NAME = "digests.json";

async function readDigests(managedRoot: string): Promise<Digests> {
  try {
    const parsed: unknown = JSON.parse(await readFile(path.join(managedRoot, DIGESTS_NAME), "utf8"));
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const digests: Digests = {};
    for (const [file, entry] of Object.entries(parsed as Record<string, unknown>)) {
      if (
        entry && typeof entry === "object"
        && typeof (entry as { size?: unknown }).size === "number"
        && typeof (entry as { mtimeMs?: unknown }).mtimeMs === "number"
        && typeof (entry as { digest?: unknown }).digest === "string"
        && DIGEST_PATTERN.test((entry as { digest: string }).digest)
      ) {
        digests[file] = entry as Digests[string];
      }
    }
    return digests;
  } catch (error) {
    if (errorCode(error) === "ENOENT") return {};
    // A damaged memory is only a memory: everything is hashed again and the file is rewritten.
    if (error instanceof SyntaxError) return {};
    throw error;
  }
}

async function writeDigests(managedRoot: string, digests: Digests): Promise<void> {
  await writeFile(path.join(managedRoot, DIGESTS_NAME), JSON.stringify(digests), "utf8");
}

/// The digest of `file`, remembered under its identity; `fresh` forces a hash (a file just written).
async function rememberedDigest(
  file: string,
  info: { size: number; mtimeMs: number },
  digests: Digests,
  fresh = false,
): Promise<string> {
  const known = digests[file];
  if (!fresh && known && known.size === info.size && known.mtimeMs === info.mtimeMs) {
    return known.digest;
  }
  const digest = await fileDigest(file);
  digests[file] = { size: info.size, mtimeMs: info.mtimeMs, digest };
  return digest;
}

async function optionalRememberedDigest(file: string, digests: Digests): Promise<string | null> {
  try {
    const info = await stat(file);
    if (!info.isFile()) {
      throw new Error(`the managed Core path is not a file: ${file}`);
    }
    return await rememberedDigest(file, info, digests);
  } catch (error) {
    if (errorCode(error) === "ENOENT") {
      return null;
    }
    throw error;
  }
}

/// The directory every managed Core image lives in; the one identity a restart may match processes by.
export function managedCoreDirectory(executable: string): string {
  return path.dirname(executable);
}

function imageName(digest: string): string {
  const stem = `runtrol-${digest.slice(0, NAME_DIGEST_LENGTH)}`;
  return process.platform === "win32" ? `${stem}.exe` : stem;
}

/// Every managed image other than `current`: previous builds, and the single-name image older
/// extensions installed as `runtrol.exe` before images were named by content.
async function otherImages(root: string, current: string): Promise<string[]> {
  const found: string[] = [];
  const directory = await opendir(root);
  for await (const entry of directory) {
    if (!entry.isFile()) continue;
    const full = path.join(root, entry.name);
    if (full === current) continue;
    const legacy = /^runtrol(\.exe)?(\.inuse-[a-f0-9]{64})?$/u.test(entry.name) || entry.name.includes(".incoming-");
    const content = /^runtrol-[a-f0-9]{16}(\.exe)?$/u.test(entry.name);
    if (legacy || content) found.push(full);
  }
  return found;
}

/// Remove previous images. Windows refuses to unlink an image while its daemon is alive; that image is
/// left for a later activation, after the generation that maps it has drained and exited.
async function removeInactiveImages(images: readonly string[]): Promise<void> {
  for (const image of images) {
    try {
      await unlink(image);
    } catch (error) {
      if (!isMappedImageError(error) && errorCode(error) !== "ENOENT") {
        throw error;
      }
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

async function fileDigest(file: string): Promise<string> {
  const digest = createHash("sha256");
  for await (const chunk of createReadStream(file)) {
    digest.update(chunk as Buffer);
  }
  const hex = digest.digest("hex");
  if (!DIGEST_PATTERN.test(hex)) {
    throw new Error("the Core digest did not render as sha256 hex");
  }
  return hex;
}

function errorCode(error: unknown): string | undefined {
  return error && typeof error === "object" && "code" in error
    ? String((error as NodeJS.ErrnoException).code)
    : undefined;
}

function isMappedImageError(error: unknown): boolean {
  return process.platform === "win32" && ["EACCES", "EBUSY", "EPERM"].includes(errorCode(error) ?? "");
}
