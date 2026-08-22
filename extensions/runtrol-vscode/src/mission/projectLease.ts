import { createHash } from "node:crypto";
import { lstat, mkdir, readdir, rmdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { workspaceIdentity } from "../workspaceCollision";

const LOCK_ROOT = path.join(os.tmpdir(), "runtrol-project-integration-leases");
const EMPTY_LOCK_GRACE_MS = 30_000;
const ACTIVE_PROCESS_LEASES = new Set<string>();

export class MissionProjectLeases {
  private readonly projects = new Map<string, string>();

  async run<T>(project: string, missionId: string, action: () => Promise<T>): Promise<T> {
    const identity = workspaceIdentity(project);
    const active = this.projects.get(identity);
    if (active) throw new Error(`Project integration is already running for Mission ${active}`);
    this.projects.set(identity, missionId);
    let lease: ProcessLease | null = null;
    try {
      lease = await acquireProcessLease(identity);
      return await action();
    } finally {
      this.projects.delete(identity);
      if (lease) await lease.release();
    }
  }

  clear(): void {
    this.projects.clear();
  }
}

type ProcessLease = { readonly release: () => Promise<void> };

async function acquireProcessLease(projectIdentity: string): Promise<ProcessLease> {
  await mkdir(LOCK_ROOT, { recursive: true });
  const digest = createHash("sha256").update(projectIdentity).digest("hex");
  const lock = path.join(LOCK_ROOT, digest);
  const owner = path.join(lock, `pid-${process.pid}`);
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      await mkdir(lock);
      try {
        await mkdir(owner);
      } catch (error) {
        await rmdir(lock).catch(() => undefined);
        throw error;
      }
      ACTIVE_PROCESS_LEASES.add(lock);
      return {
        release: async () => {
          ACTIVE_PROCESS_LEASES.delete(lock);
          for (let attempt = 0; attempt < 3; attempt += 1) {
            await rmdir(owner).catch(() => undefined);
            if (await rmdir(lock).then(() => true, () => false)) return;
            await new Promise<void>((resolve) => setTimeout(resolve, 25 * (attempt + 1)));
          }
        },
      };
    } catch (error) {
      if (!isAlreadyExists(error) || attempt > 0 || !await recoverStaleLease(lock)) throw leaseError(error);
    }
  }
  throw new Error("Project lease failed");
}

async function recoverStaleLease(lock: string): Promise<boolean> {
  const stat = await lstat(lock);
  if (stat.isSymbolicLink() || !stat.isDirectory()) return false;
  const entries = await readdir(lock, { withFileTypes: true });
  if (entries.length === 0) {
    if (Date.now() - stat.mtimeMs < EMPTY_LOCK_GRACE_MS) return false;
  } else {
    for (const entry of entries) {
      const match = /^pid-(\d+)$/.exec(entry.name);
      if (!match || !entry.isDirectory()) return false;
      const pid = Number(match[1]);
      if (
        !Number.isSafeInteger(pid)
        || (pid === process.pid ? ACTIVE_PROCESS_LEASES.has(lock) : processIsAlive(pid))
      ) return false;
    }
    for (const entry of entries) await rmdir(path.join(lock, entry.name));
  }
  await rmdir(lock);
  return true;
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error instanceof Error && "code" in error && error.code === "EPERM";
  }
}

function isAlreadyExists(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "EEXIST";
}

function leaseError(error: unknown): Error {
  if (isAlreadyExists(error)) return new Error("Another VS Code window is integrating this project");
  return error instanceof Error ? error : new Error(String(error));
}
