import { spawnSync } from "node:child_process";
import path from "node:path";

// Win32_Process includes every process command line. Busy development hosts can exceed Node's 1 MiB spawnSync
// default even though the bounded JSON snapshot is still small enough to inspect safely in memory.
export const WINDOWS_PROCESS_ROWS_MAX_BUFFER_BYTES = 16 * 1024 * 1024;

export function processRows() {
  if (process.platform === "win32") {
    return windowsRows();
  }
  return unixRows();
}

export function descendantPids(rows, rootPid) {
  const descendants = new Set();
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      if (row.ppid === rootPid || descendants.has(row.ppid)) {
        if (!descendants.has(row.pid)) {
          descendants.add(row.pid);
          changed = true;
        }
      }
    }
  }
  return descendants;
}

export function normalizedExecutable(value) {
  const resolved = path.resolve(value || "");
  return process.platform === "win32" ? resolved.toLocaleLowerCase("en-US") : resolved;
}

function windowsRows() {
  const query = "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine "
    + "| ConvertTo-Json -Compress";
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-Command", query],
    {
      encoding: "utf8",
      timeout: 60_000,
      windowsHide: true,
      maxBuffer: WINDOWS_PROCESS_ROWS_MAX_BUFFER_BYTES,
    },
  );
  if (result.status !== 0) {
    const details = [
      result.error instanceof Error ? result.error.message : "",
      typeof result.stderr === "string" ? result.stderr.trim() : "",
      `status=${String(result.status)}`,
      result.signal ? `signal=${result.signal}` : "",
    ].filter(Boolean).join("; ");
    // stdout contains command lines and must never be copied into a diagnostic.
    throw new Error(`cannot inspect isolated Windows process identities: ${details}`);
  }
  const decoded = JSON.parse(result.stdout || "[]");
  return (Array.isArray(decoded) ? decoded : [decoded]).map((row) => ({
    pid: Number(row.ProcessId),
    ppid: Number(row.ParentProcessId),
    executable: typeof row.ExecutablePath === "string" ? row.ExecutablePath : "",
    command: typeof row.CommandLine === "string" ? row.CommandLine : "",
  }));
}

function unixRows() {
  const result = spawnSync("ps", ["-axo", "pid=,ppid=,command="], {
    encoding: "utf8",
    timeout: 15_000,
  });
  if (result.status !== 0) {
    throw new Error(`cannot inspect isolated Unix process identities: ${result.stderr}`);
  }
  return result.stdout.split("\n").flatMap((line) => {
    const match = /^\s*(\d+)\s+(\d+)\s+(.*)$/u.exec(line);
    if (!match) return [];
    const command = match[3];
    const executable = command.startsWith('"')
      ? command.slice(1, command.indexOf('"', 1))
      : command.split(/\s+/u)[0];
    return [{ pid: Number(match[1]), ppid: Number(match[2]), executable, command }];
  });
}
