import { writeFile } from "node:fs/promises";
import path from "node:path";

import { UNINSTALL_RECORD, type UninstallRecord } from "../uninstall";

/// Leave the `vscode:uninstall` hook the one fact it cannot derive: which global storage this Studio owned.
///
/// The hook runs on a later VS Code start with no VS Code API and no user-data-dir argument (measured 2026-09-02),
/// so the record is written beside it, in the extension folder, on every activation. A folder that refuses the
/// write (a read-only install) is not an error the operator can act on; the hook then falls back to the default
/// storage locations, so the refusal is reported to the caller and never surfaced as a failure.
export function rememberForUninstall(extensionPath: string, globalStorage: string): Promise<void> {
  const record: UninstallRecord = { schema: 1, globalStorage };
  return writeFile(path.join(extensionPath, UNINSTALL_RECORD), `${JSON.stringify(record)}\n`, "utf8");
}
