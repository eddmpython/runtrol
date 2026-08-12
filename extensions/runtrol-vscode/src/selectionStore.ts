import { mkdir, open, rm, writeFile } from "node:fs/promises";
import path from "node:path";

const FILE_NAME = "selected-session.json";
const MAX_FILE_BYTES = 256;
const MAX_SESSION_BYTES = 128;
const WRITE_ATTEMPTS = 5;
const WRITE_RETRY_DELAY_MS = 10;

type SelectionWriter = (file: string, contents: string) => Promise<void>;

type StoredSelection = {
  schema: 1;
  session: string;
};

export class SelectionStore {
  private readonly file: string;

  constructor(
    private readonly root: string,
    private readonly writer: SelectionWriter = writeSelection,
  ) {
    this.file = path.join(root, FILE_NAME);
  }

  async load(): Promise<string | null> {
    let handle;
    try {
      handle = await open(this.file, "r");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") {
        return null;
      }
      throw error;
    }
    try {
      const metadata = await handle.stat();
      if (metadata.size > MAX_FILE_BYTES) {
        return null;
      }
      const raw = await handle.readFile("utf8");
      const value: unknown = JSON.parse(raw);
      if (value === null || typeof value !== "object" || Array.isArray(value)) {
        return null;
      }
      const stored = value as Partial<StoredSelection>;
      return stored.schema === 1 && validSession(stored.session) ? stored.session : null;
    } catch (error) {
      if (error instanceof SyntaxError) {
        return null;
      }
      throw error;
    } finally {
      await handle.close();
    }
  }

  async save(session: string): Promise<void> {
    if (!validSession(session)) {
      throw new Error("the selected runtrol session identifier is invalid");
    }
    await mkdir(this.root, { recursive: true });
    const stored: StoredSelection = { schema: 1, session };
    await retryTransientWrite(() => this.writer(this.file, JSON.stringify(stored)));
  }

  async clear(): Promise<void> {
    await rm(this.file, { force: true });
  }
}

function writeSelection(file: string, contents: string): Promise<void> {
  return writeFile(file, contents, { encoding: "utf8", mode: 0o600 });
}

async function retryTransientWrite(write: () => Promise<void>): Promise<void> {
  for (let attempt = 1; attempt <= WRITE_ATTEMPTS; attempt += 1) {
    try {
      await write();
      return;
    } catch (error) {
      if (attempt === WRITE_ATTEMPTS || !isTransientWriteError(error)) {
        throw error;
      }
      await delay(WRITE_RETRY_DELAY_MS);
    }
  }
}

function isTransientWriteError(error: unknown): boolean {
  if (!error || typeof error !== "object" || !("code" in error)) {
    return false;
  }
  return ["EACCES", "EBUSY", "EPERM"].includes(String((error as NodeJS.ErrnoException).code));
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function validSession(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && Buffer.byteLength(value, "utf8") <= MAX_SESSION_BYTES
    && !/[\u0000-\u001f\u007f]/u.test(value);
}
