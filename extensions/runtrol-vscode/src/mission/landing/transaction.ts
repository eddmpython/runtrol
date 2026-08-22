import { sameBytes, type LandingByteEvidence } from "./model";

export type LandingTransactionIo = {
  readonly beforeWrite: (entry: LandingByteEvidence) => Promise<void>;
  readonly read: (path: string) => Promise<Uint8Array | null>;
  readonly write: (path: string, bytes: Uint8Array, expected: Uint8Array | null) => Promise<void>;
  readonly remove: (path: string, expected: Uint8Array) => Promise<void>;
};

export type LandingDirectoryIo = {
  readonly ensure: (path: string) => Promise<boolean>;
  readonly exists: (path: string) => Promise<boolean>;
  readonly remove: (path: string) => Promise<void>;
};

export class LandingTransactionError extends Error {
  constructor(
    message: string,
    readonly rollbackProblems: readonly string[],
  ) {
    super(message);
    this.name = "LandingTransactionError";
  }
}

/// Apply exact Receipt bytes after the caller has completed its whole preflight. A later write or verification
/// failure restores every earlier existing file and removes every earlier file created by this transaction.
export async function applyLandingTransaction(
  entries: readonly LandingByteEvidence[],
  io: LandingTransactionIo,
): Promise<void> {
  const touched: LandingByteEvidence[] = [];
  try {
    for (const entry of entries) {
      await io.beforeWrite(entry);
      touched.push(entry);
      await io.write(entry.path, entry.sourceBytes, entry.targetBytes);
    }
    for (const entry of entries) {
      const written = await io.read(entry.path);
      if (written === null || !sameBytes(written, entry.sourceBytes)) {
        throw new Error(`written Artifact does not match its Receipt: ${entry.path}`);
      }
    }
  } catch (error) {
    const rollbackProblems: string[] = [];
    for (const entry of touched.reverse()) {
      try {
        const current = await io.read(entry.path);
        const alreadyRestored = entry.targetBytes === null
          ? current === null
          : current !== null && sameBytes(current, entry.targetBytes);
        if (!alreadyRestored) {
          if (current === null || !sameBytes(current, entry.sourceBytes)) {
            throw new Error("Artifact changed after the Landing write and was left untouched");
          }
          if (entry.targetBytes === null) await io.remove(entry.path, entry.sourceBytes);
          else await io.write(entry.path, entry.targetBytes, entry.sourceBytes);
        }
        const restored = await io.read(entry.path);
        if (entry.targetBytes === null ? restored !== null : restored === null || !sameBytes(restored, entry.targetBytes)) {
          throw new Error("restored bytes do not match the reviewed project state");
        }
      } catch (rollbackError) {
        rollbackProblems.push(
          `${entry.path}: ${rollbackError instanceof Error ? rollbackError.message : String(rollbackError)}`,
        );
      }
    }
    throw new LandingTransactionError(
      error instanceof Error ? error.message : String(error),
      rollbackProblems,
    );
  }
}

/// Create only the missing parent directories selected by the caller. If a later creation fails, every directory
/// created by this invocation is removed and the removal is verified before the error is reported as recovered.
export async function createLandingDirectories(
  paths: readonly string[],
  io: LandingDirectoryIo,
): Promise<string[]> {
  const created: string[] = [];
  try {
    for (const directory of paths) {
      if (await io.ensure(directory)) created.push(directory);
    }
    return created;
  } catch (error) {
    const rollbackProblems = await removeLandingDirectories(created, io);
    throw new LandingTransactionError(
      error instanceof Error ? error.message : String(error),
      rollbackProblems,
    );
  }
}

export async function removeLandingDirectories(
  paths: readonly string[],
  io: LandingDirectoryIo,
): Promise<string[]> {
  const problems: string[] = [];
  for (const directory of [...paths].reverse()) {
    try {
      await io.remove(directory);
      if (await io.exists(directory)) throw new Error("directory still exists after removal");
    } catch (error) {
      problems.push(`${directory}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return problems;
}
