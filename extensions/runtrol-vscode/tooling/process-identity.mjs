import { spawnSync } from "node:child_process";
import path from "node:path";

// Win32_Process includes every process command line. Busy development hosts can exceed Node's 1 MiB spawnSync
// default even though the bounded JSON snapshot is still small enough to inspect safely in memory.
export const WINDOWS_PROCESS_ROWS_MAX_BUFFER_BYTES = 16 * 1024 * 1024;
const PROCESS_START_TOLERANCE_MS = 2_000;

export function processRows() {
  if (process.platform === "win32") {
    return windowsRows();
  }
  return unixRows();
}

export function descendantPids(rows, rootPid) {
  const root = rows.find((row) => row.pid === rootPid);
  if (!root) return new Set();
  const descendants = new Set();
  const admitted = new Map([[rootPid, root]]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      const parent = admitted.get(row.ppid);
      if (parent && startedWithParent(row, parent)) {
        if (!descendants.has(row.pid)) {
          descendants.add(row.pid);
          admitted.set(row.pid, row);
          changed = true;
        }
      }
    }
  }
  return descendants;
}

function startedWithParent(row, parent) {
  if (!Number.isFinite(row.startedAt) || !Number.isFinite(parent.startedAt)) return true;
  return row.startedAt + PROCESS_START_TOLERANCE_MS >= parent.startedAt;
}

export function normalizedExecutable(value) {
  const resolved = path.resolve(value || "");
  return process.platform === "win32" ? resolved.toLocaleLowerCase("en-US") : resolved;
}

function windowsRows() {
  const query = "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,ExecutablePath,CommandLine,"
    + "@{Name='CreationTimeMs';Expression={[DateTimeOffset]$_.CreationDate "
    + "| ForEach-Object { $_.ToUnixTimeMilliseconds() }}} "
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
    startedAt: Number.isFinite(Number(row.CreationTimeMs)) ? Number(row.CreationTimeMs) : null,
  }));
}

function unixRows() {
  const result = spawnSync("ps", ["-axo", "pid=,ppid=,lstart=,command="], {
    encoding: "utf8",
    env: { ...process.env, LC_ALL: "C" },
    timeout: 15_000,
  });
  if (result.status !== 0) {
    throw new Error(`cannot inspect isolated Unix process identities: ${result.stderr}`);
  }
  return result.stdout.split("\n").flatMap((line) => {
    const timed = /^\s*(\d+)\s+(\d+)\s+(\S{3}\s+\S{3}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(.*)$/u.exec(line);
    if (!timed) {
      if (line.trim() === "") return [];
      throw new Error(`cannot parse isolated Unix process identity: ${line}`);
    }
    const command = timed[4];
    const executable = command.startsWith('"')
      ? command.slice(1, command.indexOf('"', 1))
      : command.split(/\s+/u)[0];
    const parsedStart = Date.parse(timed[3]);
    if (!Number.isFinite(parsedStart)) {
      throw new Error(`cannot parse isolated Unix process start time: ${timed[3]}`);
    }
    return [{
      pid: Number(timed[1]),
      ppid: Number(timed[2]),
      executable,
      command,
      startedAt: parsedStart,
    }];
  });
}
