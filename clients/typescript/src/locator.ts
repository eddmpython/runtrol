import { execFile } from "node:child_process";
import { lstat, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { promisify } from "node:util";

import type { RuntimeGeneration, RuntimeLocatorRecord } from "./generated/protocol.js";
import { RuntimeLocatorError } from "./errors.js";
import { validatePublic } from "./schema.js";

const MAX_LOCATOR_BYTES = 16 * 1024;
const MAX_ENDPOINT_BYTES = 1024;
const MAX_GENERATIONS = 16;
const MAX_SECURITY_OUTPUT_BYTES = 16 * 1024;
const LOCATOR_SCHEMA = 2;
const runtimeLocatorToken = Symbol("Runtime locator path");
const executeFile = promisify(execFile);

export type LocatorState =
  | { readonly state: "notInstalled" }
  | { readonly state: "running"; readonly locator: ValidatedLocator };

export type RuntimeLocatorOptions = {
  /** Exact Runtime executable used for native Windows owner and DACL validation. It is never PATH-resolved. */
  readonly runtimeExecutable?: string;
  /**
   * SHA-256 of the Runtime build this consumer installed. The generation running exactly that build is chosen
   * when it is listed and not draining; otherwise the newest generation that is not draining.
   */
  readonly preferDigest?: string;
};

const validatedLocatorToken = Symbol("validated Runtime locator");

export class ValidatedLocator {
  readonly #validated = true;

  public constructor(
    token: typeof validatedLocatorToken,
    public readonly instanceId: string,
    public readonly endpoint: string,
    public readonly runtimeVersion: string,
    public readonly digest: string,
    public readonly draining: boolean,
    /** Where the same generation answers its owner's administration protocol. Not a Runtime endpoint. */
    public readonly controlEndpoint: string,
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
    private readonly preferDigest?: string,
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
      join(runtrolHome(), "runtime.locator.json"),
      options.runtimeExecutable,
      options.preferDigest,
    );
  }

  /** The chosen generation, or not installed when nothing is listed to connect to. */
  public async inspect(): Promise<LocatorState> {
    // Validate and read again when the two disagree. What that disagreement means is that the file moved between
    // the two, and a daemon publishing its own generation is the ordinary reason for it: on a home whose first
    // daemon is starting, that write lands exactly here (measured 2026-08-26, a new home could never finish
    // enrolling because one such moment ended the attempt for good).
    //
    // The safety property is unchanged. Each attempt still validates and then reads, and only a pair that agrees
    // is accepted, so a swap between the two is still refused. What changes is that a moving file is given a few
    // more chances to hold still instead of being called an attack.
    for (let attempt = 0; ; attempt += 1) {
      const read = await this.read();
      if (!read) return { state: "notInstalled" };
      const chosen = chooseGeneration(read.record, this.preferDigest);
      if (!chosen) return { state: "notInstalled" };
      if (!read.verified || (
        read.verified.instanceId === read.record.instanceId
        && read.verified.endpoint === chosen.endpoint
        && read.verified.runtimeVersion === chosen.runtimeVersion
        && read.verified.digest === chosen.digest
      )) {
        return { state: "running", locator: validated(read.record, chosen) };
      }
      if (attempt >= LOCATOR_SETTLE_ATTEMPTS) {
        throw new RuntimeLocatorError("unsafe", "Runtime locator changed after native validation");
      }
      await new Promise((resolve) => setTimeout(resolve, LOCATOR_SETTLE_DELAY_MS));
    }
  }

  /** Every listed generation, oldest start first. Empty when nothing is installed. */
  public async inspectAll(): Promise<ReadonlyArray<ValidatedLocator>> {
    const read = await this.read();
    if (!read) return [];
    return read.record.generations.map((generation) => validated(read.record, generation));
  }

  async #readRecord(): Promise<RuntimeLocatorRecord> {
    let decoded: unknown;
    try {
      decoded = JSON.parse(await readFile(this.path, "utf8"));
    } catch (error) {
      throw new RuntimeLocatorError("malformed", `Runtime locator is not valid JSON: ${String(error)}`);
    }
    let record: RuntimeLocatorRecord;
    try {
      record = validatePublic<RuntimeLocatorRecord>("RuntimeLocatorRecord", decoded);
    } catch (error) {
      // A locator of another shape (one written before generations, or by a later build) is a locator
      // this SDK cannot choose from, and that is a malformed locator rather than a protocol failure.
      throw new RuntimeLocatorError("malformed", `Runtime locator is not the shape this SDK reads: ${String(error)}`);
    }
    validateLocatorRecord(record, this.path);
    return record;
  }

  private async read(): Promise<{ record: RuntimeLocatorRecord; verified: NativeLocatorObservation | null } | null> {
    let metadata;
    try {
      metadata = await lstat(this.path);
    } catch (error) {
      if (isNodeError(error) && error.code === "ENOENT") return null;
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
      return { record: await this.#readRecord(), verified: null };
    }
    let verified: NativeLocatorObservation | null = null;
    if (this.runtimeExecutable) {
      try {
        verified = await validateWindowsSecurityWithRuntime(this.runtimeExecutable, this.preferDigest);
      } catch {
        await validateWindowsSecurity(this.path);
      }
    } else {
      await validateWindowsSecurity(this.path);
    }
    return { record: await this.#readRecord(), verified };
  }
}

/** The generation running the preferred digest when listed and not draining, else the newest not draining. */
function chooseGeneration(
  record: RuntimeLocatorRecord,
  preferDigest: string | undefined,
): RuntimeGeneration | null {
  const preferred = preferDigest
    ? record.generations.find((generation) => generation.digest === preferDigest && !generation.draining)
    : undefined;
  if (preferred) return preferred;
  let newest: RuntimeGeneration | null = null;
  for (const generation of record.generations) {
    if (generation.draining) continue;
    if (!newest
      || generation.startedAtMs > newest.startedAtMs
      || (generation.startedAtMs === newest.startedAtMs && generation.processId > newest.processId)) {
      newest = generation;
    }
  }
  return newest;
}

function validated(record: RuntimeLocatorRecord, generation: RuntimeGeneration): ValidatedLocator {
  return new ValidatedLocator(
    validatedLocatorToken,
    record.instanceId,
    generation.endpoint,
    generation.runtimeVersion,
    generation.digest,
    generation.draining,
    generation.controlEndpoint,
  );
}

type NativeLocatorObservation = {
  readonly endpoint: string;
  readonly instanceId: string;
  readonly runtimeVersion: string;
  readonly digest: string;
  readonly draining: boolean;
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
  preferDigest: string | undefined,
): Promise<NativeLocatorObservation> {
  const arguments_ = ["runtime-locator"];
  if (preferDigest) arguments_.push("--prefer", preferDigest);
  let decoded: unknown;
  try {
    const result = await executeFile(
      executable,
      arguments_,
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
  if (Object.keys(record).sort().join(",") !== "digest,draining,endpoint,instanceId,runtimeVersion"
    || typeof record.endpoint !== "string"
    || typeof record.instanceId !== "string"
    || typeof record.runtimeVersion !== "string"
    || typeof record.digest !== "string"
    || typeof record.draining !== "boolean") {
    throw new RuntimeLocatorError("unsafe", "native Runtime locator verification returned a malformed record");
  }
  return record as NativeLocatorObservation;
}

export function runtimeLocatorAtForTesting(path: string, preferDigest?: string): RuntimeLocator {
  if (!isAbsolute(path)) throw new RuntimeLocatorError("environment", "locator path is not absolute");
  return new RuntimeLocator(runtimeLocatorToken, path, undefined, preferDigest);
}

export function validatedLocatorForTesting(
  instanceId: string,
  endpoint: string,
  runtimeVersion: string,
  digest: string = "0".repeat(64),
  draining: boolean = false,
  controlEndpoint: string = `${endpoint}-control`,
): ValidatedLocator {
  return new ValidatedLocator(
    validatedLocatorToken,
    instanceId,
    endpoint,
    runtimeVersion,
    digest,
    draining,
    controlEndpoint,
  );
}

function validateLocatorRecord(record: RuntimeLocatorRecord, locatorPath: string): void {
  if (record.schema !== LOCATOR_SCHEMA
    || record.instanceId.length === 0 || record.instanceId.length > 128
    || record.generations.length > MAX_GENERATIONS) {
    throw new RuntimeLocatorError("malformed", "Runtime locator has invalid bounded fields");
  }
  for (const generation of record.generations) {
    validateGeneration(generation, locatorPath);
  }
}

function validateGeneration(generation: RuntimeGeneration, locatorPath: string): void {
  if (generation.processId === 0
    || !/^[0-9a-f]{64}$/u.test(generation.digest)
    || generation.runtimeVersion.length === 0 || generation.runtimeVersion.length > 128
    || generation.endpoint.length === 0 || Buffer.byteLength(generation.endpoint) > MAX_ENDPOINT_BYTES
    || generation.controlEndpoint.length === 0 || Buffer.byteLength(generation.controlEndpoint) > MAX_ENDPOINT_BYTES) {
    throw new RuntimeLocatorError("malformed", "Runtime locator generation has invalid bounded fields");
  }
  if (process.platform === "win32") {
    if (generation.endpointKind !== "namedPipe"
      || !generation.endpoint.startsWith("\\\\.\\pipe\\runtrol-runtime-")) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator does not name its dedicated local pipe");
    }
  } else if (generation.endpointKind !== "unixSocket" || !isAbsolute(generation.endpoint)
    || dirname(generation.endpoint) !== dirname(locatorPath)
    || !/^runtrol-runtime-[0-9a-f]{16}\.sock$/u.test(basename(generation.endpoint))) {
    throw new RuntimeLocatorError(
      "unsafe",
      "Runtime socket escaped its owner-only state directory",
    );
  }
}

/// How many further attempts a locator that moved under validation is given before it is called unsafe.
///
/// Three, because the write that causes this is one daemon publishing one generation: it happens once and it is
/// over in milliseconds. A file that keeps disagreeing after that is not a daemon starting.
const LOCATOR_SETTLE_ATTEMPTS = 3;

/// How long to wait between those attempts.
const LOCATOR_SETTLE_DELAY_MS = 60;

/// The environment variable that names the Runtrol home, when the operator set one.
const HOME_ENVIRONMENT = "RUNTROL_HOME";

/// Where this machine's Runtrol home is, by the same rule the Core itself follows.
///
/// The Runtime reads `RUNTROL_HOME` first and falls back to the platform's own directory. This used to read
/// only the platform directory, so a process that had set `RUNTROL_HOME` found a daemon in one home through
/// its command line and a locator in another home through this SDK. Both halves believed they were talking to
/// the same Runtime, and the enrollment one half created was invisible to the other: measured 2026-08-26, an
/// extension in a chosen home could never finish enrolling and reported that its pending enrollment did not
/// exist, forever.
///
/// One rule, in the one place each side reads it, is the whole fix. An explicit setting is used exactly as
/// given, because writing somewhere other than where the operator said is the one thing it must never do.
function runtrolHome(): string {
  const chosen = process.env[HOME_ENVIRONMENT];
  if (chosen && chosen.length > 0) {
    if (!isAbsolute(chosen)) {
      throw new RuntimeLocatorError("environment", `${HOME_ENVIRONMENT} is not an absolute path`);
    }
    return chosen;
  }
  return join(systemStateRoot(), "runtrol");
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
