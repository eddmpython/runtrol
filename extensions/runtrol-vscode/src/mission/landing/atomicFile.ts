import { randomUUID } from "node:crypto";
import { open, rename, rm } from "node:fs/promises";
import path from "node:path";

import { sameBytes } from "./model";
import { inspectSafeLocalDirectory } from "./localFile";

/// Prepare exact bytes in the target directory, recheck the caller's CAS guard, then replace one directory entry.
/// A write or verification failure before rename leaves the target untouched.
export async function writeAtomicLandingFile(
  projectRoot: string,
  target: string,
  bytes: Uint8Array,
  priorMode: number | null,
  beforeReplace: () => Promise<void>,
): Promise<void> {
  const parent = path.dirname(target);
  const parentBefore = await inspectSafeLocalDirectory(projectRoot, parent);
  const temporary = path.join(parent, `.runtrol-landing-${process.pid}-${randomUUID()}.tmp`);
  let handle: Awaited<ReturnType<typeof open>> | null = await open(temporary, "wx+", 0o666);
  try {
    let offset = 0;
    while (offset < bytes.byteLength) {
      const written = await handle.write(bytes, offset, bytes.byteLength - offset, offset);
      if (written.bytesWritten === 0) throw new Error("Landing temp write stalled");
      offset += written.bytesWritten;
    }
    if (priorMode !== null) await handle.chmod(priorMode & 0o7777);
    await handle.sync();
    const prepared = new Uint8Array(bytes.byteLength);
    offset = 0;
    while (offset < prepared.byteLength) {
      const read = await handle.read(prepared, offset, prepared.byteLength - offset, offset);
      if (read.bytesRead === 0) throw new Error("Landing temp read ended early");
      offset += read.bytesRead;
    }
    if (!sameBytes(prepared, bytes) || (await handle.stat()).size !== bytes.byteLength) {
      throw new Error("Landing temp bytes differ");
    }
    await handle.close();
    handle = null;

    await beforeReplace();
    const parentAfter = await inspectSafeLocalDirectory(projectRoot, parent);
    if (
      parentAfter.device !== parentBefore.device
      || parentAfter.inode !== parentBefore.inode
      || normalize(parentAfter.realPath) !== normalize(parentBefore.realPath)
      || normalize(parentAfter.rootRealPath) !== normalize(parentBefore.rootRealPath)
    ) throw new Error("Landing parent changed before replace");
    await rename(temporary, target);
  } finally {
    if (handle) await handle.close().catch(() => undefined);
    await rm(temporary, { force: true }).catch(() => undefined);
  }
}

function normalize(value: string): string {
  const resolved = path.normalize(path.resolve(value));
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}
