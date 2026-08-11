import { spawnSync } from "node:child_process";
import path from "node:path";

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
    { encoding: "utf8", timeout: 60_000, windowsHide: true },
  );
  if (result.status !== 0) {
    throw new Error(`cannot inspect isolated Windows process identities: ${result.stderr}`);
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
