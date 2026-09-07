import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { chmod } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { promisify } from "node:util";

const executeFile = promisify(execFile);

export async function makeOwnerOnly(path: string): Promise<void> {
  if (process.platform !== "win32") {
    await chmod(path, 0o600);
    return;
  }
  const systemRoot = process.env.SystemRoot;
  assert.ok(systemRoot && isAbsolute(systemRoot));
  const powershell = join(
    systemRoot,
    "System32",
    "WindowsPowerShell",
    "v1.0",
    "powershell.exe",
  );
  const script = [
    "& { param([string]$TargetPath)",
    "$ErrorActionPreference='Stop'",
    "$acl=[System.IO.File]::GetAccessControl($TargetPath)",
    "$acl.SetAccessRuleProtection($true,$false)",
    "$identity=[Security.Principal.WindowsIdentity]::GetCurrent().User",
    "$rule=New-Object Security.AccessControl.FileSystemAccessRule($identity,'FullControl','Allow')",
    "$acl.SetAccessRule($rule)",
    "[System.IO.File]::SetAccessControl($TargetPath,$acl)",
    "}",
  ].join(";");
  await executeFile(
    powershell,
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script, path],
    { windowsHide: true },
  );
}
