import { lstat, open, realpath } from "node:fs/promises";
import * as path from "node:path";

export type SafeLocalFile = {
  readonly path: string;
  readonly size: number;
  readonly mode: number;
  readonly device: number;
  readonly inode: number;
  readonly realPath: string;
  readonly rootRealPath: string;
};

export type SafeLocalDirectory = {
  readonly path: string;
  readonly device: number;
  readonly inode: number;
  readonly realPath: string;
  readonly rootRealPath: string;
};

export async function inspectSafeLocalDirectory(root: string, directory: string): Promise<SafeLocalDirectory> {
  const resolvedRoot = path.resolve(root);
  const resolvedDirectory = path.resolve(directory);
  const relative = path.relative(resolvedRoot, resolvedDirectory);
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error("Landing target parent escaped its project root");
  }
  let current = resolvedRoot;
  let rootRealPath: string | null = null;
  for (const [index, part] of ["", ...relative.split(path.sep).filter(Boolean)].entries()) {
    if (part) current = path.join(current, part);
    const stat = await lstat(current);
    if (stat.isSymbolicLink() || !stat.isDirectory()) {
      throw new Error("Landing target parent is not a safe directory");
    }
    if (index === 0) rootRealPath = await realpath(current);
    if (normalizedLocalPath(current) === normalizedLocalPath(resolvedDirectory)) {
      const realPath = await realpath(current);
      if (!rootRealPath || !pathIsInside(rootRealPath, realPath)) {
        throw new Error("Landing target parent escaped its project root");
      }
      return { path: current, device: stat.dev, inode: stat.ino, realPath, rootRealPath };
    }
  }
  throw new Error("Landing parent inspection failed");
}

export async function inspectSafeLocalFile(
  root: string,
  relative: string,
  required: boolean,
): Promise<SafeLocalFile | null> {
  let current = path.resolve(root);
  let rootRealPath: string | null = null;
  const parts = relative.split("/");
  for (const [index, part] of ["", ...parts].entries()) {
    if (part) current = path.join(current, part);
    let stat;
    try {
      stat = await lstat(current);
    } catch (error) {
      if (isMissing(error)) {
        if (index === 0) throw new Error(`Artifact root is unavailable: ${root}`);
        if (required) throw new Error(`Missing Artifact: ${relative}`);
        return null;
      }
      throw error;
    }
    if (stat.isSymbolicLink()) throw new Error(`Symbolic link in Artifact path: ${relative}`);
    if (index === 0) rootRealPath = await realpath(current);
    const leaf = index === parts.length;
    if (leaf && !stat.isFile()) throw new Error(`Not a file: ${relative}`);
    if (!leaf && !stat.isDirectory()) throw new Error(`Not a directory in Artifact path: ${relative}`);
    if (leaf) {
      if (!Number.isSafeInteger(stat.size) || stat.size < 0) throw new Error(`Invalid Artifact size: ${relative}`);
      const realPath = await realpath(current);
      if (!rootRealPath || !pathIsInside(rootRealPath, realPath)) {
        throw new Error(`Artifact escaped its reviewed root: ${relative}`);
      }
      return {
        path: current,
        size: stat.size,
        mode: stat.mode,
        device: stat.dev,
        inode: stat.ino,
        realPath,
        rootRealPath,
      };
    }
  }
  throw new Error(`Artifact inspection failed: ${relative}`);
}

/// Read exactly the already-inspected length and probe EOF. The function never allocates from a later, larger stat.
export async function readExactLocalFile(
  file: SafeLocalFile,
  maximumBytes: number,
  label: string,
): Promise<Uint8Array> {
  if (file.size > maximumBytes) throw new Error(`${label} exceeds the Landing byte limit`);
  const handle = await open(file.path, "r");
  try {
    const opened = await handle.stat();
    const named = await lstat(file.path);
    const namedRealPath = await realpath(file.path);
    if (
      !opened.isFile()
      || named.isSymbolicLink()
      || !named.isFile()
      || opened.size !== file.size
      || opened.dev !== file.device
      || opened.ino !== file.inode
      || named.dev !== file.device
      || named.ino !== file.inode
      || normalizedLocalPath(namedRealPath) !== normalizedLocalPath(file.realPath)
      || !pathIsInside(file.rootRealPath, namedRealPath)
    ) throw new Error(`${label} changed before its bounded read`);
    const bytes = new Uint8Array(file.size);
    let offset = 0;
    while (offset < bytes.byteLength) {
      const chunk = await handle.read(bytes, offset, bytes.byteLength - offset, offset);
      if (chunk.bytesRead === 0) throw new Error(`${label} changed during its bounded read`);
      offset += chunk.bytesRead;
    }
    const probe = new Uint8Array(1);
    if ((await handle.read(probe, 0, 1, offset)).bytesRead !== 0) {
      throw new Error(`${label} grew during its bounded read`);
    }
    const closed = await handle.stat();
    if (closed.size !== file.size || closed.dev !== file.device || closed.ino !== file.inode) {
      throw new Error(`${label} changed during its bounded read`);
    }
    return bytes;
  } finally {
    await handle.close();
  }
}

function normalizedLocalPath(value: string): string {
  const resolved = path.normalize(path.resolve(value));
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function pathIsInside(root: string, candidate: string): boolean {
  const normalizedRoot = normalizedLocalPath(root);
  const normalizedCandidate = normalizedLocalPath(candidate);
  const relative = path.relative(normalizedRoot, normalizedCandidate);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== ".." && !path.isAbsolute(relative));
}

function isMissing(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}
