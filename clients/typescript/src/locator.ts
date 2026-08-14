import { execFile } from "node:child_process";
import { lstat, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { promisify } from "node:util";

import type { RuntimeLocatorRecord } from "./generated/protocol.js";
import { RuntimeLocatorError } from "./errors.js";
import { validatePublic } from "./schema.js";

const MAX_LOCATOR_BYTES = 8 * 1024;
const MAX_ENDPOINT_BYTES = 1024;
const MAX_SECURITY_OUTPUT_BYTES = 16 * 1024;
const runtimeLocatorToken = Symbol("Runtime locator path");
const executeFile = promisify(execFile);

export type LocatorState =
  | { readonly state: "notInstalled" }
  | { readonly state: "running"; readonly locator: ValidatedLocator };

export type RuntimeLocatorOptions = {
  /** Exact Runtime executable used for native Windows owner and DACL validation. It is never PATH-resolved. */
  readonly runtimeExecutable?: string;
};

const validatedLocatorToken = Symbol("validated Runtime locator");

export class ValidatedLocator {
  readonly #validated = true;

  public constructor(
    token: typeof validatedLocatorToken,
    public readonly instanceId: string,
    public readonly endpoint: string,
    public readonly runtimeVersion: string,
  ) {
    if (token !== validatedLocatorToken) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator was not validated by this SDK");
    }
  }

  public assertSdkValidated(): void {
    if (!this.#validated) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator was not validated by this SDK");
    }
  }
}

export class RuntimeLocator {
  public constructor(
    token: typeof runtimeLocatorToken,
    public readonly path: string,
    private readonly runtimeExecutable?: string,
  ) {
    if (token !== runtimeLocatorToken) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator path was not derived by this SDK");
    }
    if (runtimeExecutable !== undefined && !isAbsolute(runtimeExecutable)) {
      throw new RuntimeLocatorError("environment", "Runtime verifier executable is not absolute");
    }
  }

  public static system(options: RuntimeLocatorOptions = {}): RuntimeLocator {
    return new RuntimeLocator(
      runtimeLocatorToken,
      join(systemStateRoot(), "runtrol", "runtime.locator.json"),
      options.runtimeExecutable,
    );
  }

  public async inspect(): Promise<LocatorState> {
    let metadata;
    try {
      metadata = await lstat(this.path);
    } catch (error) {
      if (isNodeError(error) && error.code === "ENOENT") return { state: "notInstalled" };
      throw new RuntimeLocatorError("io", `could not inspect Runtime locator: ${String(error)}`);
    }
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator is not a regular file");
    }
    if (metadata.size > MAX_LOCATOR_BYTES) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator exceeds its byte limit");
    }
    if (process.platform !== "win32") {
      if ((metadata.mode & 0o077) !== 0) {
        throw new RuntimeLocatorError("unsafe", "Runtime locator is readable outside its owner");
      }
      if (typeof process.getuid === "function" && metadata.uid !== process.getuid()) {
        throw new RuntimeLocatorError("unsafe", "Runtime locator is not owned by the current user");
      }
    } else {
      let verified: NativeLocatorObservation | null = null;
      if (this.runtimeExecutable) {
        try {
          verified = await validateWindowsSecurityWithRuntime(this.runtimeExecutable);
        } catch {
          await validateWindowsSecurity(this.path);
        }
      } else {
        await validateWindowsSecurity(this.path);
      }
      let decoded: unknown;
      try {
        decoded = JSON.parse(await readFile(this.path, "utf8"));
      } catch (error) {
        throw new RuntimeLocatorError("malformed", `Runtime locator is not valid JSON: ${String(error)}`);
      }
      const record = validatePublic<RuntimeLocatorRecord>("RuntimeLocatorRecord", decoded);
      validateLocatorRecord(record, this.path);
      if (verified && (
        verified.instanceId !== record.instanceId
        || verified.endpoint !== record.endpoint
        || verified.runtimeVersion !== record.runtimeVersion
      )) {
        throw new RuntimeLocatorError("unsafe", "Runtime locator changed after native validation");
      }
      return {
        state: "running",
        locator: new ValidatedLocator(
          validatedLocatorToken,
          record.instanceId,
          record.endpoint,
          record.runtimeVersion,
        ),
      };
    }
    let decoded: unknown;
    try {
      decoded = JSON.parse(await readFile(this.path, "utf8"));
    } catch (error) {
      throw new RuntimeLocatorError("malformed", `Runtime locator is not valid JSON: ${String(error)}`);
    }
    const record = validatePublic<RuntimeLocatorRecord>("RuntimeLocatorRecord", decoded);
    validateLocatorRecord(record, this.path);
    return {
      state: "running",
      locator: new ValidatedLocator(
        validatedLocatorToken,
        record.instanceId,
        record.endpoint,
        record.runtimeVersion,
      ),
    };
  }
}

type NativeLocatorObservation = {
  readonly endpoint: string;
  readonly instanceId: string;
  readonly runtimeVersion: string;
};

interface WindowsSecurityObservation {
  readonly current: string;
  readonly owner: string;
  readonly protected: boolean;
  readonly rules: ReadonlyArray<{
    readonly inherited: boolean;
    readonly rights: number;
    readonly sid: string;
    readonly type: number;
  }>;
}

async function validateWindowsSecurity(path: string): Promise<void> {
  const systemRoot = process.env.SystemRoot;
  if (!systemRoot || !isAbsolute(systemRoot)) {
    throw new RuntimeLocatorError("environment", "SystemRoot is unavailable or not absolute");
  }
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
    "$current=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
    "$owner=$acl.GetOwner([Security.Principal.SecurityIdentifier]).Value",
    "$rules=@($acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]) | ForEach-Object {",
    "[pscustomobject]@{sid=$_.IdentityReference.Value;type=[int]$_.AccessControlType;inherited=$_.IsInherited;rights=[int64]$_.FileSystemRights}",
    "})",
    "[pscustomobject]@{current=$current;owner=$owner;protected=$acl.AreAccessRulesProtected;rules=$rules} | ConvertTo-Json -Compress -Depth 4",
    "}",
  ].join(";");
  let decoded: WindowsSecurityObservation;
  try {
    const result = await executeFile(
      powershell,
      ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script, path],
      { encoding: "utf8", maxBuffer: MAX_SECURITY_OUTPUT_BYTES, timeout: 5_000, windowsHide: true },
    );
    decoded = JSON.parse(result.stdout) as WindowsSecurityObservation;
  } catch (error) {
    throw new RuntimeLocatorError("unsafe", `could not verify Runtime locator ACL: ${String(error)}`);
  }
  const rule = Array.isArray(decoded.rules) && decoded.rules.length === 1
    ? decoded.rules[0]
    : undefined;
  if (typeof decoded.current !== "string" || decoded.current.length === 0
    || decoded.owner !== decoded.current || decoded.protected !== true
    || !rule || rule.sid !== decoded.current || rule.type !== 0
    || rule.inherited !== false || rule.rights !== 2_032_127) {
    throw new RuntimeLocatorError(
      "unsafe",
      "Runtime locator owner or DACL is not current-user-only",
    );
  }
}

async function validateWindowsSecurityWithRuntime(
  executable: string,
): Promise<NativeLocatorObservation> {
  let decoded: unknown;
  try {
    const result = await executeFile(
      executable,
      ["runtime-locator"],
      { encoding: "utf8", maxBuffer: MAX_SECURITY_OUTPUT_BYTES, timeout: 5_000, windowsHide: true },
    );
    decoded = JSON.parse(result.stdout);
  } catch (error) {
    throw new RuntimeLocatorError("unsafe", `could not verify Runtime locator natively: ${String(error)}`);
  }
  if (!decoded || typeof decoded !== "object" || Array.isArray(decoded)) {
    throw new RuntimeLocatorError("unsafe", "native Runtime locator verification returned no record");
  }
  const record = decoded as Partial<NativeLocatorObservation>;
  if (Object.keys(record).sort().join(",") !== "endpoint,instanceId,runtimeVersion"
    || typeof record.endpoint !== "string"
    || typeof record.instanceId !== "string"
    || typeof record.runtimeVersion !== "string") {
    throw new RuntimeLocatorError("unsafe", "native Runtime locator verification returned a malformed record");
  }
  return record as NativeLocatorObservation;
}

export function runtimeLocatorAtForTesting(path: string): RuntimeLocator {
  if (!isAbsolute(path)) throw new RuntimeLocatorError("environment", "locator path is not absolute");
  return new RuntimeLocator(runtimeLocatorToken, path);
}

export function validatedLocatorForTesting(
  instanceId: string,
  endpoint: string,
  runtimeVersion: string,
): ValidatedLocator {
  return new ValidatedLocator(validatedLocatorToken, instanceId, endpoint, runtimeVersion);
}

function validateLocatorRecord(record: RuntimeLocatorRecord, locatorPath: string): void {
  if (record.schema !== 1 || record.processId === 0
    || record.instanceId.length === 0 || record.instanceId.length > 128
    || record.runtimeVersion.length === 0 || record.runtimeVersion.length > 128
    || record.endpoint.length === 0 || Buffer.byteLength(record.endpoint) > MAX_ENDPOINT_BYTES) {
    throw new RuntimeLocatorError("malformed", "Runtime locator has invalid bounded fields");
  }
  if (process.platform === "win32") {
    if (record.endpointKind !== "namedPipe"
      || !record.endpoint.startsWith("\\\\.\\pipe\\runtrol-runtime-")) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator does not name its dedicated local pipe");
    }
  } else if (record.endpointKind !== "unixSocket" || !isAbsolute(record.endpoint)
    || dirname(record.endpoint) !== dirname(locatorPath)
    || basename(record.endpoint) !== "runtrol-runtime.sock") {
    throw new RuntimeLocatorError(
      "unsafe",
      "Runtime socket escaped its owner-only state directory",
    );
  }
}

function systemStateRoot(): string {
  if (process.platform === "win32") return absoluteEnvironment("LOCALAPPDATA");
  if (process.platform === "darwin") return join(absoluteEnvironment("HOME"), "Library", "Application Support");
  const configured = process.env.XDG_STATE_HOME;
  if (configured && isAbsolute(configured)) return configured;
  const home = process.env.HOME || homedir();
  if (!home || !isAbsolute(home)) {
    throw new RuntimeLocatorError("environment", "HOME is unavailable or not absolute");
  }
  return join(home, ".local", "state");
}

function absoluteEnvironment(name: string): string {
  const value = process.env[name];
  if (!value || !isAbsolute(value)) {
    throw new RuntimeLocatorError("environment", `${name} is unavailable or not absolute`);
  }
  return value;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}
