import { mkdir, open, rm, writeFile } from "node:fs/promises";
import path from "node:path";

const FILE_NAME = "selected-session.json";
const MAX_FILE_BYTES = 256;
const MAX_SESSION_BYTES = 128;

type StoredSelection = {
  schema: 1;
  session: string;
};

export class SelectionStore {
  private readonly file: string;

  constructor(private readonly root: string) {
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
    await writeFile(this.file, JSON.stringify(stored), { encoding: "utf8", mode: 0o600 });
  }

  async clear(): Promise<void> {
    await rm(this.file, { force: true });
  }
}

function validSession(value: unknown): value is string {
  return typeof value === "string"
    && value.length > 0
    && Buffer.byteLength(value, "utf8") <= MAX_SESSION_BYTES
    && !/[\u0000-\u001f\u007f]/u.test(value);
}
