import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, watch } from "node:fs";
import { watchValidatedGenerations } from "./generationWatch.js";
import { lstat, open, readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";

import type { RuntimeGeneration, RuntimeLocatorRecord } from "./generated/protocol.js";
import { RuntimeLocatorError } from "./errors.js";
import { validatePublic } from "./schema.js";

const MAX_LOCATOR_BYTES = 16 * 1024;
const MAX_ENDPOINT_BYTES = 1024;
const MAX_GENERATIONS = 16;
const MAX_SECURITY_OUTPUT_BYTES = 16 * 1024;
const LOCATOR_SCHEMA = 2;
/** The schema Runtimes published before generations (Marketplace 0.1.20 to 0.1.22) wrote. */
const LEGACY_LOCATOR_SCHEMA = 1;
/** The digest a pre-generation Runtime never named. All zeros, so it matches no build's preference. */
export const LEGACY_DIGEST = "0".repeat(64);
const runtimeLocatorToken = Symbol("Runtime locator path");
const executeFile = promisify(execFile);

export type LocatorState =
  | { readonly state: "notInstalled" }
  | { readonly state: "running"; readonly locator: ValidatedLocator };

export type RuntimeGenerationSnapshot = {
  readonly current: LocatorState;
  /** Opaque equality token for the selected verified route incarnation. Not process-control authority. */
  readonly currentRevision: string | null;
  readonly generations: readonly ValidatedLocator[];
};

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
    /** Opaque incarnation equality only. Not a credential, PID, or OS process-control authority. */
    public readonly revision: string,
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
    return (await this.inspectSnapshot()).value.current;
  }

  /** One verified read supplies both the selected route and the complete generation fleet. */
  private async inspectSnapshot(signal?: AbortSignal): Promise<{ value: RuntimeGenerationSnapshot; fingerprint: string }> {
    // Validate and read again when the two disagree. What that disagreement means is that the file moved between
    // the two, and a daemon publishing its own generation is the ordinary reason for it: on a home whose first
    // daemon is starting, that write lands exactly here (measured 2026-08-26, a new home could never finish
    // enrolling because one such moment ended the attempt for good).
    //
    // The safety property is unchanged. Each attempt still validates and then reads, and only a pair that agrees
    // is accepted, so a swap between the two is still refused. What changes is that a moving file is given a few
    // more chances to hold still instead of being called an attack.
    for (let attempt = 0; ; attempt += 1) {
      signal?.throwIfAborted();
      let read;
      try {
        read = await this.read(signal);
      } catch (error) {
        // A locator mid-replace: four coexisting generations rewrite the file often, and an ACL or native
        // verification probe that lands inside the atomic rename window fails with a command error, not a
        // security verdict (measured 2026-08-27 21:45 on the operator machine: two "not installed" toasts
        // while the daemon was healthy). Only the probe-failed shape retries; a real DACL verdict, and a
        // probe that keeps failing, still refuse.
        if (
          attempt < LOCATOR_SETTLE_ATTEMPTS
          && error instanceof RuntimeLocatorError
          && String(error.message).includes("could not verify Runtime locator")
        ) {
          await delay(120, undefined, { signal });
          continue;
        }
        throw error;
      }
      if (!read) return { value: { current: { state: "notInstalled" }, currentRevision: null, generations: [] }, fingerprint: "absent" };
      const chosen = chooseGeneration(read.record, this.preferDigest);
      if (!chosen) return { value: { current: { state: "notInstalled" }, currentRevision: null, generations: read.record.generations.map(generation => validated(read.record, generation)) }, fingerprint: routingFingerprint(read.record) };
      if (!read.verified || (
        read.verified.instanceId === read.record.instanceId
        && read.verified.endpoint === chosen.endpoint
        && read.verified.runtimeVersion === chosen.runtimeVersion
        && read.verified.digest === chosen.digest
      )) {
        const current = validated(read.record, chosen);
        return { value: { current: { state: "running", locator: current }, currentRevision: current.revision, generations: read.record.generations.map(generation => validated(read.record, generation)) }, fingerprint: routingFingerprint(read.record) };
      }
      if (attempt >= LOCATOR_SETTLE_ATTEMPTS) {
        throw new RuntimeLocatorError("unsafe", "Runtime locator changed after native validation");
      }
      await delay(LOCATOR_SETTLE_DELAY_MS, undefined, { signal });
    }
  }

  /**
   * Publish verified routing changes without polling. Callback work is awaited; intermediate changes coalesce.
   * Filesystem or validation failure ends this watch. Existing views remain owned by their callers.
   * Cancellation closes watcher handles immediately and suppresses pending inspection delivery.
   */
  public watchGenerations(
    publish: (snapshot: RuntimeGenerationSnapshot) => void | Promise<void>,
    options: { readonly signal?: AbortSignal } = {},
  ): Promise<void> {
    const parent = dirname(this.path);
    let parentIdentity: { dev: number; ino: number } | null = null;
    const ensureParent = async (): Promise<void> => {
      const current = await lstat(parent);
      if (!current.isDirectory() || (parentIdentity !== null
        && (current.dev !== parentIdentity.dev || current.ino !== parentIdentity.ino))) {
        throw new RuntimeLocatorError("io", "Runtime home directory was replaced");
      }
    };
    return watchValidatedGenerations(
      (changed, failed) => {
        const handles: ReturnType<typeof watch>[] = [];
        let closing = false;
        const close = (): void => { closing = true; for (const handle of handles) handle.close(); };
        try {
          parentIdentity = lstatSync(parent);
          const contents = watch(parent, (event, filename) => {
            if (filename === null || filename.toString() === basename(this.path)) changed();
          });
          handles.push(contents);
          // Watching the parent's entry also catches a Runtime home rename, even if its old handle stays alive.
          const home = watch(dirname(parent), (_event, filename) => {
            if (filename === null || filename.toString() === basename(parent)) {
              // The same bounded pump validates parent identity before hints and full inspection.
              changed();
            }
          });
          handles.push(home);
          for (const handle of handles) {
            handle.on("error", failed);
            handle.on("close", () => { if (!closing) failed(new RuntimeLocatorError("io", "Runtime locator watcher closed")); });
          }
          return close;
        } catch (error) { close(); throw error; }
      },
      async (signal) => {
        signal.throwIfAborted();
        await ensureParent();
        let metadata;
        try { metadata = await lstat(this.path); }
        catch (error) { if (isNodeError(error) && error.code === "ENOENT") return "absent"; throw error; }
        if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > MAX_LOCATOR_BYTES) {
          throw new RuntimeLocatorError("unsafe", "Runtime locator hint is not a bounded regular file");
        }
        return routingFingerprint(await this.#readRecord());
      },
      async (signal) => { await ensureParent(); return this.inspectSnapshot(signal); },
      publish,
      options.signal,
    );
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
      decoded = JSON.parse(await boundedLocatorText(this.path));
    } catch (error) {
      throw new RuntimeLocatorError("malformed", `Runtime locator is not valid JSON: ${String(error)}`);
    }
    let record: RuntimeLocatorRecord;
    try {
      record = liftLegacyRecord(decoded) ?? validatePublic<RuntimeLocatorRecord>("RuntimeLocatorRecord", decoded);
    } catch (error) {
      // A locator of another shape (one written before generations, or by a later build) is a locator
      // this SDK cannot choose from, and that is a malformed locator rather than a protocol failure.
      throw new RuntimeLocatorError("malformed", `Runtime locator is not the shape this SDK reads: ${String(error)}`);
    }
    validateLocatorRecord(record, this.path);
    return record;
  }

  private async read(signal?: AbortSignal): Promise<{ record: RuntimeLocatorRecord; verified: NativeLocatorObservation | null } | null> {
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
        verified = await validateWindowsSecurityWithRuntime(this.runtimeExecutable, this.preferDigest, signal);
      } catch {
        signal?.throwIfAborted();
        await validateWindowsSecurity(this.path, signal);
      }
    } else {
      await validateWindowsSecurity(this.path, signal);
    }
    signal?.throwIfAborted();
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
    incarnationRevision(record, generation),
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

// Abort can reject execFile before its close event. Retain admission until the native child and stdio end.
async function executeVerifier(executable: string, args: string[], signal?: AbortSignal): Promise<{ stdout: string; stderr: string }> {
  signal?.throwIfAborted();
  const result = executeFile(executable, args, {
    encoding: "utf8", maxBuffer: MAX_SECURITY_OUTPUT_BYTES, timeout: 5_000, windowsHide: true,
    ...(signal ? { signal } : {}),
  });
  const closed = new Promise<void>((resolve) => result.child.once("close", () => resolve()));
  try { return await result; } finally { await closed; }
}

async function validateWindowsSecurity(path: string, signal?: AbortSignal): Promise<void> {
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
    const result = await executeVerifier(
      powershell,
      ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script, path],
      signal,
    );
    decoded = JSON.parse(result.stdout) as WindowsSecurityObservation;
  } catch (error) {
    signal?.throwIfAborted();
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
  signal?: AbortSignal,
): Promise<NativeLocatorObservation> {
  const arguments_ = ["runtime-locator"];
  if (preferDigest) arguments_.push("--prefer", preferDigest);
  let decoded: unknown;
  try {
    const result = await executeVerifier(
      executable,
      arguments_,
      signal,
    );
    decoded = JSON.parse(result.stdout);
  } catch (error) {
    signal?.throwIfAborted();
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
  revision: string = createHash("sha256").update(JSON.stringify([instanceId, endpoint, runtimeVersion, digest, controlEndpoint])).digest("hex"),
): ValidatedLocator {
  return new ValidatedLocator(
    validatedLocatorToken,
    instanceId,
    endpoint,
    runtimeVersion,
    digest,
    draining,
    controlEndpoint,
    revision,
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

/**
 * A record from a Runtime that predates generations, lifted into the shape this SDK reads.
 *
 * Those Runtimes (Marketplace 0.1.20 to 0.1.22) are installed on real machines, and the client that
 * replaces them meets their locator first: the newer Core starts beside the older daemon and drains it,
 * but until it has published its own generation the file on disk is the old one. Refusing it stranded
 * every such machine at "malformed" (measured 2026-08-27 by the shipped-Runtime interop gate). The old
 * record names one daemon and no digest, so it becomes one generation with the all-zero digest that no
 * build prefers and an empty control endpoint that nothing connects to; only its public endpoint is used.
 */
function liftLegacyRecord(decoded: unknown): RuntimeLocatorRecord | null {
  if (!decoded || typeof decoded !== "object" || Array.isArray(decoded)) return null;
  const record = decoded as Record<string, unknown>;
  if (record.schema !== LEGACY_LOCATOR_SCHEMA) return null;
  const keys = Object.keys(record).sort().join(",");
  if (keys !== "endpoint,endpointKind,instanceId,processId,runtimeVersion,schema"
    || typeof record.instanceId !== "string"
    || (record.endpointKind !== "namedPipe" && record.endpointKind !== "unixSocket")
    || typeof record.endpoint !== "string"
    || typeof record.runtimeVersion !== "string"
    || typeof record.processId !== "number" || !Number.isInteger(record.processId)) {
    throw new RuntimeLocatorError("malformed", "a pre-generation Runtime locator is not the shape those Runtimes wrote");
  }
  return {
    schema: LOCATOR_SCHEMA,
    instanceId: record.instanceId,
    generations: [{
      digest: LEGACY_DIGEST,
      endpointKind: record.endpointKind,
      endpoint: record.endpoint,
      controlEndpoint: "",
      runtimeVersion: record.runtimeVersion,
      processId: record.processId,
      startedAtMs: 0,
      liveSessions: 0,
      draining: false,
    }],
  };
}

function validateGeneration(generation: RuntimeGeneration, locatorPath: string): void {
  const legacy = generation.digest === LEGACY_DIGEST;
  if (generation.processId === 0
    || !/^[0-9a-f]{64}$/u.test(generation.digest)
    || generation.runtimeVersion.length === 0 || generation.runtimeVersion.length > 128
    || generation.endpoint.length === 0 || Buffer.byteLength(generation.endpoint) > MAX_ENDPOINT_BYTES
    || (!legacy && generation.controlEndpoint.length === 0)
    || Buffer.byteLength(generation.controlEndpoint) > MAX_ENDPOINT_BYTES) {
    throw new RuntimeLocatorError("malformed", "Runtime locator generation has invalid bounded fields");
  }
  if (process.platform === "win32") {
    if (generation.endpointKind !== "namedPipe"
      || !generation.endpoint.startsWith("\\\\.\\pipe\\runtrol-runtime-")) {
      throw new RuntimeLocatorError("unsafe", "Runtime locator does not name its dedicated local pipe");
    }
  } else if (generation.endpointKind !== "unixSocket" || !isAbsolute(generation.endpoint)
    || dirname(generation.endpoint) !== dirname(locatorPath)
    || !(legacy
      ? basename(generation.endpoint) === "runtrol-runtime.sock"
      : /^runtrol-runtime-[0-9a-f]{16}\.sock$/u.test(basename(generation.endpoint)))) {
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

/** Observation hint only: equality may suppress redundant inspection but grants no Runtime authority. */
function routingFingerprint(record: RuntimeLocatorRecord): string {
  return JSON.stringify({ schema: record.schema, instanceId: record.instanceId,
    generations: record.generations.map(routingEntry) });
}

function routingEntry({ liveSessions: _liveSessions, ...routing }: RuntimeGeneration): object {
  return Object.fromEntries(Object.entries(routing).sort(([a], [b]) => a.localeCompare(b)));
}

/** Only compare for equality. This token is neither a credential nor an OS process identity. */
function incarnationRevision(record: RuntimeLocatorRecord, generation: RuntimeGeneration): string {
  // Draining changes selection, not process incarnation. Retained mirrors must survive that role change.
  const { liveSessions: _liveSessions, draining: _draining, ...incarnation } = generation;
  return createHash("sha256").update(JSON.stringify({ schema: record.schema, instanceId: record.instanceId,
    incarnation: Object.fromEntries(Object.entries(incarnation).sort(([a], [b]) => a.localeCompare(b))) })).digest("hex");
}

async function boundedLocatorText(path: string): Promise<string> {
  const file = await open(path, "r");
  try {
    const buffer = Buffer.alloc(MAX_LOCATOR_BYTES + 1);
    let used = 0;
    while (used < buffer.length) {
      const { bytesRead } = await file.read(buffer, used, buffer.length - used, used);
      if (bytesRead === 0) break;
      used += bytesRead;
    }
    if (used > MAX_LOCATOR_BYTES) throw new RuntimeLocatorError("unsafe", "Runtime locator exceeds its byte limit");
    return buffer.toString("utf8", 0, used);
  } finally { await file.close(); }
}
