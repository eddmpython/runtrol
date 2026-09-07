import { createHash } from "node:crypto";

import * as vscode from "vscode";
import type {
  WindowMirrorEndParams,
  WindowMirrorOpenParams,
  WindowMirrorOpened,
  WindowMirrorOutputParams,
  WindowRegisterParams,
  WindowUpdateParams,
  WindowRegistration,
  WatchWindowInputParams,
  WindowInputSubscription,
} from "@runtrol/runtime-client";

import { mirrorChunks, providerOfCommand } from "./observedMirrorState";
import { DEFAULT_VIEW_GEOMETRY } from "./runtimeTerminal";
import { errorKindOf } from "./serviceHelp";
import { WindowRegistryState } from "./windowRegistryState";
import { OwnerText } from "./ownerText";
import { abortableDelay } from "./abortableDelay";

/// Where the window's record and its observed mirrors go.
export type BoundMirrorHandle = {
  readonly terminalId: string;
  readonly closed: boolean;
  output(bytesBase64: string): Promise<void>;
  end(exitCode?: number): Promise<void>;
};
export type WindowGroupHandle = {
  readonly registration: WindowRegistration;
  readonly signal: AbortSignal;
  openMirror(params: Omit<WindowMirrorOpenParams, "registrationGeneration" | "ownerToken">): Promise<BoundMirrorHandle>;
};
export type WindowSnapshot = { readonly register: WindowRegisterParams; readonly update: WindowUpdateParams };
export type WindowOwner = {
  state(): WindowSnapshot;
  reveal(group: WindowGroupHandle, terminalKey: string): boolean;
  input(group: WindowGroupHandle, subscription: WindowInputSubscription): Promise<void>;
  failed(group: WindowGroupHandle, lane: string, error: unknown): void;
};
export type WindowPublisher = {
  setWindowOwner(owner: WindowOwner): { dispose(): void };
  publishWindow(state: WindowSnapshot): Promise<WindowGroupHandle>;
  providerCommandNames(): Promise<ReadonlyMap<string, string>>;
};

/// What this window fed into one observed mirror, for the journey to hold against the Runtime's own view.
export type MirrorEvidence = {
  readonly terminalKey: string;
  readonly executionId: string;
  readonly providerId: string;
  readonly commandLine: string;
  readonly terminalId: string | null;
  readonly refusal: string | null;
  readonly bytes: number;
  readonly chunks: number;
  readonly sha256: string;
  readonly ended: boolean;
  readonly exitCode: number | null;
  readonly startedAtMs: number;
  readonly openedAtMs: number | null;
  readonly firstChunkAtMs: number | null;
};

const MIRROR_HISTORY_LIMIT = 64;

type Mirror = {
  readonly terminalKey: string;
  readonly executionId: string;
  readonly providerId: string;
  readonly commandLine: string;
  terminalId: string | null;
  group: WindowGroupHandle | null;
  handle: BoundMirrorHandle | null;
  refusal: string | null;
  bytes: number;
  chunks: number;
  readonly digest: ReturnType<typeof createHash>;
  ended: boolean;
  /// The end was already sent by the close path; the pump must not send a second one.
  endSent: boolean;
  exitCode: number | null;
  readonly startedAtMs: number;
  openedAtMs: number | null;
  firstChunkAtMs: number | null;
};

/// This window's entry in the Runtime's window registry, kept current by VS Code's own terminal events and
/// nothing else: a terminal opened or closed, shell integration attached, a command started or ended, the
/// workspace folders changed. One publish is in flight at a time and a change that lands meanwhile is sent right
/// after it, so the registry always ends on the latest state without a queue growing behind a slow Runtime.
///
/// A command that starts a provider (its program word is one of the inventory's command names) is also
/// mirrored: the execution's output stream is taken synchronously inside the start event (measured 2026-09-02:
/// taken later it yields nothing), the mirror is opened, and every captured chunk is fed in order until the
/// command ends. The Runtime refuses the open when the transparent shim already brokers that shell, which is the
/// ordinary case for a provider typed by name; the mirror is for a provider started some other way.
export class WindowRegistry implements vscode.Disposable {
  private readonly state: WindowRegistryState;
  private readonly subscriptions: vscode.Disposable[] = [];
  private readonly mirrors = new Map<vscode.Terminal, Mirror>();
  private readonly executions = new WeakMap<vscode.TerminalShellExecution, string>();
  private readonly history: Mirror[] = [];
  private inFlight: Promise<WindowGroupHandle> | null = null;
  private dirty = false;
  private disposed = false;
  private lastReported: string | null = null;
  private commandNames: ReadonlyMap<string, string> | null = null;
  private commandNamesReady: Promise<ReadonlyMap<string, string> | null> = Promise.resolve(null);
  private readonly owners = new Map<WindowGroupHandle, { dispatcher: OwnerText; close(): void }>();
  private readonly lifetime = new AbortController();

  constructor(
    private readonly publisher: WindowPublisher,
    private readonly report: (message: string) => void,
    private readonly updated: (snapshot: WindowSnapshot) => void = () => undefined,
  ) {
    this.state = new WindowRegistryState(
      {
        windowSessionId: vscode.env.sessionId,
        hostGeneration: `${process.pid}-${Date.now()}`,
        vscodeVersion: vscode.version,
        hostPid: process.pid,
      },
      folders(),
    );
  }

  /// Register again with the Runtime that answers now. A generation that just started holds no window at all,
  /// so the registration this window published to its predecessor has to be said again, with every terminal it
  /// holds now; a failure that was already reported once is reported again if it repeats against the new one.
  resync(): void {
    this.lastReported = null;
    this.schedule();
  }

  /// Register now, with every terminal the window already holds, and follow the events from here on.
  start(): void {
    // Initialization may finish before the ordinary window registry exists. Attaching these callbacks is
    // synchronous; the first publish below asks the caller's router to prepare the initial ownership group.
    this.subscriptions.push(this.publisher.setWindowOwner({
      state: () => this.currentState(),
      reveal: (group, key) => this.revealFromGroup(group, key),
      input: (group, subscription) => this.serveGroupInput(group, subscription),
      failed: (_group, _lane, error) => this.reportOnce(error),
    }));
    for (const terminal of vscode.window.terminals) this.track(terminal);
    this.subscriptions.push(
      vscode.window.onDidOpenTerminal((terminal) => {
        this.track(terminal);
        this.schedule();
      }),
      vscode.window.onDidCloseTerminal((terminal) => {
        // A closed terminal ends its mirror now: VS Code may never end the execution's stream for it, and a
        // row for a terminal that no longer exists would be a lie. The end queues behind the chunks already sent.
        this.endMirror(terminal, null, true);
        if (this.state.closed(terminal)) this.schedule();
      }),
      vscode.window.onDidChangeTerminalShellIntegration((change) => {
        if (this.state.shellIntegrationChanged(change.terminal, change.shellIntegration.cwd?.fsPath ?? null)) {
          this.schedule();
        }
      }),
      vscode.window.onDidStartTerminalShellExecution((start) => {
        const commandLine = start.execution.commandLine;
        const executionId = this.state.executionStarted(
          start.terminal,
          commandLine.value,
          commandLine.confidence,
          Date.now(),
        );
        if (executionId === null) return;
        this.executions.set(start.execution, executionId);
        this.schedule();
        if (this.commandNames !== null) {
          const providerId = providerOfCommand(commandLine.value, this.commandNames);
          if (providerId !== null) this.tryBeginMirror(start, executionId, providerId, start.execution.read());
          return;
        }
        // The inventory has not answered yet. The stream can only be taken now, so it is taken for every command
        // and released the moment the names say this one is not a provider.
        const stream = start.execution.read();
        void this.commandNamesReady.then((names) => {
          const providerId = names === null ? null : providerOfCommand(commandLine.value, names);
          if (providerId === null) {
            void drain(stream);
            return;
          }
          this.tryBeginMirror(start, executionId, providerId, stream);
        });
      }),
      vscode.window.onDidEndTerminalShellExecution((end) => {
        const executionId = this.executions.get(end.execution);
        if (executionId === undefined) return;
        this.executions.delete(end.execution);
        this.endMirror(end.terminal, end.exitCode ?? null, false, executionId);
        if (this.state.executionEnded(end.terminal, executionId)) this.schedule();
      }),
      vscode.workspace.onDidChangeWorkspaceFolders(() => {
        this.state.foldersChanged(folders());
        this.schedule();
      }),
    );
    this.commandNamesReady = this.publisher.providerCommandNames().then(
      (names) => {
        this.commandNames = names;
        return names;
      },
      (error: unknown) => {
        this.reportOnce(error);
        return null;
      },
    );
    this.schedule();
  }

  private tryBeginMirror(
    start: vscode.TerminalShellExecutionStartEvent,
    executionId: string,
    providerId: string,
    stream: AsyncIterable<string>,
  ): void {
    if (this.state.executionIdOf(start.terminal) !== executionId) {
      void drain(stream);
      return;
    }
    try {
      this.beginMirror(start.terminal, start.execution, executionId, providerId, stream);
    } catch (error) {
      // Nothing here may throw into a VS Code event handler, and a mirror that never began must still be
      // visible as evidence rather than as silence.
      this.rememberMirror(failedMirror(this.state.terminalKey(start.terminal) ?? "", executionId, providerId, start.execution.commandLine.value, error));
      this.reportOnce(error);
    }
  }

  /// Another window asked for one of this window's terminals: show it here, the way a person would click its
  /// tab. Returns whether the key named a terminal this window still has.
  showTerminal(terminalKey: string): boolean {
    const handle = this.state.handleOf(terminalKey);
    if (handle === null) return false;
    (handle as vscode.Terminal).show(false);
    return true;
  }

  /// The provider command names this window recognises, or null while the inventory has not answered.
  knownCommandNames(): string[] | null {
    return this.commandNames === null ? null : [...this.commandNames.keys()];
  }

  /// Every mirror this window opened or tried to open, oldest first.
  mirrorEvidence(): MirrorEvidence[] {
    return this.history.map((mirror) => ({
      terminalKey: mirror.terminalKey,
      executionId: mirror.executionId,
      providerId: mirror.providerId,
      commandLine: mirror.commandLine,
      terminalId: mirror.terminalId,
      refusal: mirror.refusal,
      bytes: mirror.bytes,
      chunks: mirror.chunks,
      sha256: mirror.digest.copy().digest("hex"),
      ended: mirror.ended,
      exitCode: mirror.exitCode,
      startedAtMs: mirror.startedAtMs,
      openedAtMs: mirror.openedAtMs,
      firstChunkAtMs: mirror.firstChunkAtMs,
    }));
  }

  dispose(): void {
    this.disposed = true;
    this.lifetime.abort();
    for (const owner of [...this.owners.values()]) owner.close();
    this.owners.clear();
    for (const subscription of this.subscriptions) subscription.dispose();
    this.subscriptions.length = 0;
  }

  private rememberMirror(mirror: Mirror): void {
    this.history.push(mirror);
    if (this.history.length > MIRROR_HISTORY_LIMIT) this.history.shift();
  }

  private track(terminal: vscode.Terminal): void {
    // Only ordinary shells are observed terminals. A pseudoterminal is some extension's own surface (Runtrol's
    // conversation tabs among them): it has no shell process, VS Code answers its process id as -1, and the
    // Runtime rightly refused a window update carrying that (measured 2026-09-05: the refusal cost the window its
    // registration and every conversation tab on the connection).
    if ("pty" in terminal.creationOptions) return;
    this.state.opened(terminal, terminal.name);
    if (terminal.shellIntegration) {
      this.state.shellIntegrationChanged(terminal, terminal.shellIntegration.cwd?.fsPath ?? null);
    }
    void terminal.processId.then((processId) => {
      if (this.state.processResolved(terminal, processId)) this.schedule();
    });
  }

  private beginMirror(
    terminal: vscode.Terminal,
    execution: vscode.TerminalShellExecution,
    executionId: string,
    providerId: string,
    stream: AsyncIterable<string>,
  ): void {
    const terminalKey = this.state.terminalKey(terminal) ?? "";
    const cwd = this.state.cwdOf(terminal);
    const processId = this.state.processIdOf(terminal);
    const open: Omit<WindowMirrorOpenParams, "registrationGeneration" | "ownerToken"> = {
      windowSessionId: vscode.env.sessionId,
      terminalKey,
      executionId,
      providerId,
      commandLine: execution.commandLine.value.slice(0, 1024),
      cwd: cwd ?? "",
      ...(processId === null ? {} : { processId }),
      geometry: { columns: DEFAULT_VIEW_GEOMETRY.columns, rows: DEFAULT_VIEW_GEOMETRY.rows },
    };
    const mirror: Mirror = {
      terminalKey,
      executionId,
      providerId,
      commandLine: open.commandLine,
      terminalId: null,
      group: null,
      handle: null,
      refusal: null,
      bytes: 0,
      chunks: 0,
      digest: createHash("sha256"),
      ended: false,
      endSent: false,
      exitCode: null,
      startedAtMs: Date.now(),
      openedAtMs: null,
      firstChunkAtMs: null,
    };
    const previous = this.mirrors.get(terminal);
    if (previous && !previous.ended) previous.ended = true;
    this.mirrors.set(terminal, mirror);
    this.rememberMirror(mirror);
    void (async () => {
      let terminalId: string;
      try {
        if (terminalKey === "") throw new Error("the terminal is not in this window's registry");
        if (cwd === null) throw new Error("shell integration reported no working directory for the terminal");
        if (processId === null) throw new Error("the terminal shell process is not known yet");
        const group = await this.publishCurrent();
        if (!group.registration.ownerToken) throw new Error("This Runtime cannot authorize an observed terminal. Reconnect to the current Runtime.");
        if (this.disposed || this.state.executionIdOf(terminal) !== executionId) {
          throw new Error("the shell execution ended before its mirror opened");
        }
        mirror.group = group;
        mirror.handle = await group.openMirror({ ...open, cwd });
        terminalId = mirror.handle.terminalId;
      } catch (error) {
        // The Runtime declined this mirror: the shell is brokered already, or the mirror table is full. The
        // stream is drained so VS Code does not hold it, and the refusal is evidence, not a warning.
        mirror.refusal = error instanceof Error ? error.message : String(error);
        await drain(stream);
        return;
      }
      mirror.terminalId = terminalId;
      mirror.openedAtMs = Date.now();
      try {
        for await (const text of stream) {
          // A mirror this window has already replaced or ended has no feed left to fill; the Runtime retired it and
          // the stream is drained rather than fed.
          if (mirror.ended && mirror.endSent) break;
          for (const bytesBase64 of mirrorChunks(text)) {
            const chunk = Buffer.from(bytesBase64, "base64");
            mirror.digest.update(chunk);
            mirror.bytes += chunk.length;
            mirror.chunks += 1;
            mirror.firstChunkAtMs ??= Date.now();
            await mirror.handle!.output(bytesBase64);
          }
        }
      } catch (error) {
        mirror.refusal = error instanceof Error ? error.message : String(error);
      }
      if (mirror.endSent) return;
      mirror.endSent = true;
      try {
        await mirror.handle!.end(mirror.exitCode ?? undefined);
      } catch (error) {
        this.reportMirrorEnd(error);
      }
    })();
  }

  private endMirror(terminal: vscode.Terminal, exitCode: number | null, closed = false, executionId?: string): void {
    const mirror = this.mirrors.get(terminal);
    if (!mirror || mirror.ended) return;
    if (executionId !== undefined && mirror.executionId !== executionId) return;
    mirror.ended = true;
    mirror.exitCode = exitCode;
    this.mirrors.delete(terminal);
    if (closed && mirror.terminalId !== null) {
      mirror.endSent = true;
      void mirror.handle?.end().catch((error: unknown) => this.reportMirrorEnd(error));
    }
  }

  /// The last refusal the Runtime gave this window's publish, for the eye passes; null while every publish held.
  lastPublishFailure(): string | null {
    return this.lastReported;
  }

  /** A synchronous latest-state accessor for route preparation; it performs no connection work. */
  currentState(): WindowSnapshot {
    return { register: this.state.register(), update: this.state.update() };
  }

  /** Called from this exact ownership group's reveal hook. No global window subscription is created here. */
  revealFromGroup(group: WindowGroupHandle, terminalKey: string): boolean {
    return !this.disposed && !group.signal.aborted && this.showTerminal(terminalKey);
  }

  /// Exactly what the next publish would send, for the eye passes to hold against a refusal.
  currentUpdate(): WindowUpdateParams {
    return this.state.update();
  }

  /// A mirror the Runtime already retired (the transparent shim's brokered open replaced it, `docs/vscodeSurface.md`)
  /// answers its end with `terminalNotFound`: nothing to report, the row moved on. Any other refusal is reported.
  private reportMirrorEnd(error: unknown): void {
    if (errorKindOf(error) === "terminalNotFound") return;
    this.reportOnce(error);
  }

  private reportOnce(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    if (message === this.lastReported) return;
    this.lastReported = message;
    this.report(message);
  }

  private schedule(): void {
    if (this.disposed) return;
    this.updated(this.currentState());
    void this.publishCurrent().catch((error: unknown) => this.reportOnce(error));
  }

  private publishCurrent(): Promise<WindowGroupHandle> {
    if (this.disposed) return Promise.reject(new Error("the window registry is closed"));
    if (this.inFlight) {
      this.dirty = true;
      return this.inFlight;
    }
    const publishing = (async () => {
      // A registration lives as long as its Runtime connection. When the Runtime restarts, that connection is
      // replaced on its next use, so the first publish after a restart lands on the dead one and throws; a window
      // with no terminal event to follow it would then never register with the new generation (measured 2026-09-05:
      // after a Runtime restart the new generation listed no window until the window itself restarted, so no other
      // window could reveal a terminal here). A failed publish is therefore retried on a short backoff until it
      // holds or the retries run out, rather than waiting for an event that a quiet window never gets.
      let backoffMs = PUBLISH_RETRY_FIRST_MS;
      for (let attempt = 0; !this.disposed; attempt += 1) {
        try {
          let group: WindowGroupHandle;
          do {
            this.dirty = false;
            // Names settle after the shell starts and VS Code raises no event for that; read them at publish time.
            for (const terminal of vscode.window.terminals) this.state.renamed(terminal, terminal.name);
            group = await this.publisher.publishWindow(this.currentState());
          } while (this.dirty && !this.disposed);
          return group;
        } catch (error) {
          // Nothing here may throw into a VS Code event handler, and the same failure is reported once, not on
          // every attempt. After the last attempt the next VS Code event is what tries again.
          this.reportOnce(error);
          if (attempt + 1 >= PUBLISH_RETRY_ATTEMPTS || this.disposed) throw error;
          await abortableDelay(backoffMs, this.lifetime.signal);
          backoffMs = Math.min(backoffMs * 2, PUBLISH_RETRY_MAX_MS);
          this.dirty = true;
        }
      }
      throw new Error("the window registry is closed");
    })();
    this.inFlight = publishing;
    const settled = () => { if (this.inFlight === publishing) this.inFlight = null; };
    void publishing.then(settled, settled);
    return publishing;
  }

  /** One owner dispatcher per exact registration group. The group owns subscription creation and recovery. */
  async serveGroupInput(group: WindowGroupHandle, channel: WindowInputSubscription): Promise<void> {
    if (this.disposed || group.signal.aborted) { channel.close(); return; }
    if (this.owners.has(group)) { channel.close(); throw new Error("this window group already has an input receiver"); }
    const registration = group.registration;
    let active = true;
    const dispatcher = new OwnerText({
      current: (binding) => {
        if (!active || group.signal.aborted || binding.registrationGeneration !== registration.registrationGeneration) return null;
        const handle = this.state.inputTarget(binding) as vscode.Terminal | null;
        const mirror = handle === null ? undefined : this.mirrors.get(handle);
        return mirror && !mirror.ended && mirror.group === group && mirror.handle?.closed === false
          && mirror.terminalId === binding.terminalId ? handle : null;
      },
      authorize: (sequence) => {
        if (!active || group.signal.aborted) return Promise.reject(new Error("the exact owner group ended"));
        return channel.claimInput(sequence);
      },
      complete: (receipt) => {
        if (!active || group.signal.aborted) return Promise.reject(new Error("the exact owner group ended"));
        return channel.inputReceipt(receipt);
      },
    });
    const close = (): void => {
      if (!active) return;
      active = false;
      dispatcher.close();
      channel.close();
      group.signal.removeEventListener("abort", close);
      this.lifetime.signal.removeEventListener("abort", close);
      if (this.owners.get(group)?.close === close) this.owners.delete(group);
    };
    this.owners.set(group, { dispatcher, close });
    group.signal.addEventListener("abort", close, { once: true });
    this.lifetime.signal.addEventListener("abort", close, { once: true });
    try {
      while (active) {
        const notification = await channel.next();
        if (!active) return;
        if (notification.kind === "ended") return;
        await dispatcher.receive(notification.offered);
      }
    } finally { close(); }
  }
}

/// A publish that failed on a just-replaced Runtime connection is retried this many times before it waits for
/// the next VS Code event, on a backoff from the first delay up to the cap.
const PUBLISH_RETRY_ATTEMPTS = 8;
const PUBLISH_RETRY_FIRST_MS = 400;
const PUBLISH_RETRY_MAX_MS = 4_000;

async function drain(stream: AsyncIterable<string>): Promise<void> {
  for await (const _ of stream) {
    /* released */
  }
}

function failedMirror(terminalKey: string, executionId: string, providerId: string, commandLine: string, error: unknown): Mirror {
  return {
    terminalKey,
    executionId,
    providerId,
    commandLine: commandLine.slice(0, 1024),
    terminalId: null,
    group: null,
    handle: null,
    refusal: error instanceof Error ? error.message : String(error),
    bytes: 0,
    chunks: 0,
    digest: createHash("sha256"),
    ended: true,
    endSent: true,
    exitCode: null,
    startedAtMs: Date.now(),
    openedAtMs: null,
    firstChunkAtMs: null,
  };
}

function folders(): string[] {
  return (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
}
