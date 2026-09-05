import { createHash } from "node:crypto";

import {
  PUBLIC_LIMITS,
  RuntimeTransportError,
  newMutationRequestId,
  type TerminalControlLease,
  type TerminalDescriptor,
  type TerminalView,
} from "@runtrol/runtime-client";
import type * as vscode from "vscode";

import type { Conversation } from "./conversationList";
import { MouseModeFilter } from "./mouseModeFilter";
import type { StudioRuntimeClient } from "./runtimeClient";

const MAX_PENDING_INPUT_ACTIONS = 256;

export type JourneyInputTiming = {
  receivedAtMs: number;
  dispatchedAtMs: number;
  acknowledgedAtMs: number;
};

type InputMeasurement = {
  receivedAtMs: number;
  resolve(timing: JourneyInputTiming): void;
  reject(error: unknown): void;
};

/// The conversation surface: the coding service's own terminal interface, hosted by the Core on a pseudo
/// terminal it owns, shown here as an editor-area tab (`docs/terminalSurface.md`).
///
/// One tab per conversation. The tab is a VS Code terminal whose pseudoterminal is a public Runtime
/// connection: what the service draws comes down as bytes and is written to the tab as written; what the
/// person types goes up as bytes. Nothing here reads either. Splitting, grids and full screen are VS
/// Code's own editor-tab behaviour, which is the point of using its terminal rather than a page of ours.
///
/// The Core answers the terminal's own questions and translates the mouse, so two viewers (this tab and
/// a phone) share one screen and one keyboard.

/// Where a tab shows what is not provider output: the pane carries provider bytes only (`terminalTransportIntegrity`,
/// Studio presentation), so opening, ending, and failing are told to the workbench instead.
export type TerminalPresentation = {
  /// The terminal is opening; `work` settles when the first view is connected or the open failed.
  opening(work: Promise<void>): void;
  /// The provider process ended with this code and the pane keeps its last screen.
  ended(code: number): void;
  /// The view failed for this reason and the pane keeps whatever it had.
  failed(message: string): void;
};

/// The smallest event source a `vscode.Pseudoterminal` needs, so this module runs with no VS Code runtime import
/// and its byte stream can be captured in a unit test exactly as VS Code would receive it.
class Emitter<T> {
  private readonly listeners = new Set<(value: T) => unknown>();

  readonly event: vscode.Event<T> = (listener, thisArgs, disposables) => {
    const bound = thisArgs === undefined ? listener : listener.bind(thisArgs);
    this.listeners.add(bound);
    const disposable = { dispose: () => { this.listeners.delete(bound); } };
    disposables?.push(disposable);
    return disposable;
  };

  fire(value: T): void {
    for (const listener of [...this.listeners]) listener(value);
  }

  dispose(): void {
    this.listeners.clear();
  }
}

export type Target = {
  provider: string;
  native: { nativeSessionId: string; adoptionToken: string } | null;
  hosted: { runtimeGeneration: string; terminalId: string } | null;
  workspace: string;
  blocked: string | null;
};

export function targetOf(conversation: Conversation): Target {
  const native = conversation.native?.adoptionToken
    ? {
        nativeSessionId: conversation.native.nativeSessionId,
        adoptionToken: conversation.native.adoptionToken,
      }
    : null;
  return {
    provider: conversation.providerId,
    native,
    hosted: conversation.hostedTerminal
      ? {
          runtimeGeneration: conversation.hostedTerminal.runtimeGeneration,
          terminalId: conversation.hostedTerminal.terminalId,
        }
      : null,
    workspace: conversation.workspace,
    blocked: !conversation.hostedTerminal && conversation.session?.nativeSessionId && !native
      ? "This conversation has no current provider resume proof for the public Runtime terminal."
      : null,
  };
}

/// The Runtime's own words when a row's resume proof no longer verifies (`runtime_terminal/mod.rs`).
const STALE_PROOF = "native catalogue observation expired";

/// A VS Code pseudoterminal whose other end is the Core's hosted terminal.
///
/// Output is decoded as streaming UTF-8 (a multi-byte character may straddle two chunks). Input is sent as
/// the UTF-8 bytes of what VS Code hands over, which for mouse reports and special keys is already the
/// terminal's own escape vocabulary.
/// Whether a refusal is the control lease being gone rather than the action being wrong.
///
/// Named by the Runtime, so the word is matched rather than guessed at. Leases are independent per view, but this
/// view can still hold an expired generation or race its own reconnect. Both are answered by asking again.
function leaseLost(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("leaseExpired")
    || message.includes("the terminal control lease expired or was released")
    // A reconnect or overlapping mutation in this same view may leave an older lease generation behind.
    || message.includes("controlConflict");
}

function sameGeometry(
  left: { columns: number; rows: number },
  right: { columns: number; rows: number },
): boolean {
  return left.columns === right.columns && left.rows === right.rows;
}

/// A journey's digest of one stretch of raw output chunks.
export type OutputRecord = { chunks: number; bytes: number; digest: string };

export function terminalIdentity(runtimeGeneration: string, terminalId: string): string {
  return `${runtimeGeneration}:${terminalId}`;
}

/// The geometry a view starts with before its panel reports one; an observed mirror keeps it, since its owner
/// window's real size is not in the stable VS Code API.
export const DEFAULT_VIEW_GEOMETRY: { readonly columns: number; readonly rows: number } = { columns: 120, rows: 40 };

export class RuntimeTerminal implements vscode.Pseudoterminal {
  private readonly writeEmitter = new Emitter<string>();
  private readonly closeEmitter = new Emitter<number | void>();
  private readonly nameEmitter = new Emitter<string>();
  /// Every output chunk exactly as the service delivered it, before this tab's own mouse-mode filter and before
  /// decoding: what a journey digests to prove two windows received one ordered raw stream.
  private readonly receivedEmitter = new Emitter<Uint8Array>();
  readonly onDidWrite = this.writeEmitter.event;
  readonly onDidClose = this.closeEmitter.event;
  readonly onDidChangeName = this.nameEmitter.event;
  readonly onDidReceive = this.receivedEmitter.event;
  private view: TerminalView | null = null;
  private lease: TerminalControlLease | null = null;
  private decoder = new TextDecoder("utf-8");
  /// The one control family this tab takes out of the service's bytes: the switches that would give the CLI
  /// this terminal's mouse. Everything else the service drew passes through exactly.
  private mouseModes = new MouseModeFilter();
  private closed = false;
  private commandTail = Promise.resolve();
  private dimensions = { ...DEFAULT_VIEW_GEOMETRY };
  private lastResize = { ...DEFAULT_VIEW_GEOMETRY };
  private resizeScheduled = false;
  /// Input typed before the connection is up is kept and sent once it is: a person who starts typing while
  /// the tab opens must not lose the first keys.
  private pending: Uint8Array[] = [];
  private pendingBytes = 0;
  private queuedInputBytes = 0;
  private queuedInputActions = 0;
  private nextInputMeasurement: Omit<InputMeasurement, "receivedAtMs"> | null = null;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    private target: Target,
    /// Told whenever the service writes, for bookkeeping that does not infer model activity.
    private readonly output: () => void,
    private readonly refreshTarget: () => Promise<Target | null>,
    /// The exact hosted terminal identity returned by Runtime, used to name a fresh placeholder safely.
    private readonly connected: (terminal: TerminalDescriptor) => void,
    /// Withdraw this dead route while leaving its final editor contents visible.
    private readonly disconnected: (reason: string) => void,
    /// Where opening, ending, and failing are shown, since the pane holds provider bytes only.
    private readonly presentation: TerminalPresentation,
  ) {}

  /// Every failure below leaves the tab standing, with whatever the service wrote still in it, and tells the
  /// workbench the reason. Closing the tab on failure took the trace away in the same frame (measured
  /// 2026-08-26); writing the reason into the pane put Runtrol's words among the service's, which the raw
  /// lane forbids.
  open(initialDimensions: vscode.TerminalDimensions | undefined): void {
    this.dimensions = {
      columns: initialDimensions?.columns ?? 120,
      rows: initialDimensions?.rows ?? 40,
    };
    const connecting = this.connect();
    this.presentation.opening(connecting);
    void connecting.catch((error: unknown) => this.fail(error));
  }

  close(): void {
    this.detach(true);
  }

  /// Change the VS Code tab name without focusing it or replacing this view.
  setName(name: string): void {
    if (!this.closed) this.nameEmitter.fire(name);
  }

  descriptor(): TerminalDescriptor | null {
    return this.view?.opened.terminal ?? null;
  }

  waitForOutput(text: string, deadlineMs: number): Promise<number> {
    if (!text) return Promise.reject(new Error("the terminal output marker is empty"));
    return new Promise<number>((resolve, reject) => {
      let tail = "";
      let timer: NodeJS.Timeout | undefined;
      const finish = (result: number | Error): void => {
        if (timer) clearTimeout(timer);
        subscription.dispose();
        if (result instanceof Error) reject(result);
        else resolve(result);
      };
      const subscription = this.onDidWrite((chunk) => {
        const candidate = tail + chunk;
        if (candidate.includes(text)) {
          finish(Date.now());
          return;
        }
        // Only the boundary needed to match a split marker is retained. This is a test latch, not a transcript.
        tail = candidate.slice(-Math.max(0, text.length - 1));
      });
      timer = setTimeout(
        () => finish(new Error(`terminal output did not contain ${JSON.stringify(text)} within ${deadlineMs} ms`)),
        deadlineMs,
      );
    });
  }

  handleInput(data: string): void {
    const measurement = this.takeInputMeasurement();
    if (this.closed) {
      measurement?.reject(new Error("the terminal closed before measured input arrived"));
      return;
    }
    const bytes = Buffer.from(data, "utf8");
    if (bytes.byteLength === 0) {
      measurement?.reject(new Error("the measured terminal input was empty"));
      return;
    }
    if (!this.view) {
      measurement?.reject(new Error("the measured terminal input arrived before the Runtime view connected"));
      if (this.pendingBytes + bytes.byteLength > PUBLIC_LIMITS.maxTerminalWriteBytes) {
        this.fail(new Error("Input entered while the terminal opened exceeded the Runtime input bound."));
        return;
      }
      this.pending.push(bytes);
      this.pendingBytes += bytes.byteLength;
      return;
    }
    this.queueInput(bytes, measurement);
  }

  measureNextInput(): Promise<JourneyInputTiming> {
    if (this.nextInputMeasurement) {
      return Promise.reject(new Error("a measured terminal input is already armed"));
    }
    return new Promise<JourneyInputTiming>((resolve, reject) => {
      this.nextInputMeasurement = { resolve, reject };
    });
  }

  handleMeasuredInput(data: string): Promise<JourneyInputTiming> {
    const measured = this.measureNextInput();
    this.handleInput(data);
    return measured;
  }

  private takeInputMeasurement(): InputMeasurement | null {
    const pending = this.nextInputMeasurement;
    this.nextInputMeasurement = null;
    return pending ? { ...pending, receivedAtMs: Date.now() } : null;
  }

  setDimensions(dimensions: vscode.TerminalDimensions): void {
    const next = { columns: dimensions.columns, rows: dimensions.rows };
    if (sameGeometry(this.dimensions, next)) return;
    this.dimensions = next;
    this.scheduleResize();
  }


  /// Everything the pane receives goes through here, and it is the service's bytes and nothing else: no
  /// loading frame, no clear before a checkpoint, no exit or error sentence. The checkpoint the Runtime sends
  /// already begins by clearing the screen, so a replacement lands on a clean page by its own bytes.
  /// Digest the raw output chunks from the one carrying `startText` through the one carrying `endText`, both
  /// inclusive. Two windows on one terminal receive the same chunks at the same boundaries, so their digests over
  /// the same two markers are equal exactly when they received one ordered raw stream.
  recordOutput(startText: string, endText: string, deadlineMs: number): Promise<OutputRecord> {
    if (!startText || !endText) return Promise.reject(new Error("the output record markers are empty"));
    return new Promise<OutputRecord>((resolve, reject) => {
      const hash = createHash("sha256");
      const decoder = new TextDecoder("utf-8");
      let tail = "";
      let recording = false;
      let chunks = 0;
      let bytes = 0;
      let timer: NodeJS.Timeout | undefined;
      const finish = (result: OutputRecord | Error): void => {
        if (timer) clearTimeout(timer);
        subscription.dispose();
        if (result instanceof Error) reject(result);
        else resolve(result);
      };
      const subscription = this.onDidReceive((chunk) => {
        const candidate = tail + decoder.decode(chunk, { stream: true });
        if (!recording && candidate.includes(startText)) recording = true;
        if (recording) {
          hash.update(chunk);
          chunks += 1;
          bytes += chunk.byteLength;
          if (candidate.includes(endText)) finish({ chunks, bytes, digest: hash.digest("hex") });
        }
        tail = candidate.slice(-Math.max(startText.length, endText.length));
      });
      timer = setTimeout(() => finish(new Error(`the output record did not close within ${deadlineMs} ms`)), deadlineMs);
    });
  }

  private writeFromService(raw: string): void {
    const text = this.mouseModes.filter(raw);
    // Feed VS Code's terminal buffer before rebuilding sidebar state. The activity callback is bookkeeping;
    // it must never sit in front of bytes a person is waiting to see.
    this.writeEmitter.fire(text);
    if (text.length > 0) this.output();
  }


  private async connect(): Promise<void> {
    if (this.target.blocked) throw new Error(this.target.blocked);
    const geometry = this.dimensions;
    const view = await this.openOnce(geometry).catch(async (error: unknown) => {
      // A resume proof signed by an earlier Runtime generation. Read the catalogue again and try once with the
      // proof it hands back now; anything else, or a second refusal, is reported as it came.
      if (!(error instanceof Error) || !error.message.includes(STALE_PROOF)) throw error;
      const fresh = await this.refreshTarget();
      if (!fresh?.native) throw error;
      this.target = fresh;
      return this.openOnce(geometry);
    });
    if (this.closed) {
      view.close();
      return;
    }
    this.view = view;
    this.connected(view.opened.terminal);
    this.lease = view.opened.controlLease ?? null;
    this.lastResize = geometry;
    this.writeFromService(this.decoder.decode(view.initialScreen, { stream: true }));
    if (
      this.dimensions.columns !== geometry.columns
      || this.dimensions.rows !== geometry.rows
    ) {
      this.scheduleResize();
    }
    const pending = this.pending;
    this.pending = [];
    this.pendingBytes = 0;
    for (const bytes of pending) this.queueInput(bytes);
    // Opening ends at connection. The provider's lifetime is supervised separately; waiting for its
    // output pump here kept the workbench's opening indicator alive for the whole conversation.
    void this.pump(view).catch((error: unknown) => this.fail(error));
  }

  /// Preserve exact keystroke order while bounding both byte ownership and Promise ownership if a Runtime stalls.
  private queueInput(bytes: Uint8Array, measurement: InputMeasurement | null = null): void {
    if (
      this.queuedInputBytes + bytes.byteLength > PUBLIC_LIMITS.maxTerminalWriteBytes
      || this.queuedInputActions >= MAX_PENDING_INPUT_ACTIONS
    ) {
      const error = new Error("Pending terminal input exceeded the bounded control queue.");
      measurement?.reject(error);
      this.fail(error);
      return;
    }
    this.queuedInputBytes += bytes.byteLength;
    this.queuedInputActions += 1;
    let dispatchedAtMs = 0;
    this.queueControl(async (view, lease) => {
      dispatchedAtMs = Date.now();
      await view.write({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        bytesBase64: Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString("base64"),
      });
    }, (error) => {
      this.queuedInputBytes = Math.max(0, this.queuedInputBytes - bytes.byteLength);
      this.queuedInputActions = Math.max(0, this.queuedInputActions - 1);
      if (!measurement) return;
      if (error) {
        measurement.reject(error);
      } else if (dispatchedAtMs === 0) {
        measurement.reject(new Error("the terminal closed before measured input dispatch"));
      } else {
        measurement.resolve({
          receivedAtMs: measurement.receivedAtMs,
          dispatchedAtMs,
          acknowledgedAtMs: Date.now(),
        });
      }
    });
  }

  /// Collapse a resize burst to one in-flight request and the latest pending geometry. There is no delay: the
  /// current size enters the control lane immediately, while obsolete intermediate sizes consume no queue or PTY I/O.
  private scheduleResize(): void {
    if (
      this.closed
      || !this.view
      || this.resizeScheduled
      || sameGeometry(this.dimensions, this.lastResize)
    ) return;
    this.resizeScheduled = true;
    let geometry: { columns: number; rows: number } | null = null;
    this.queueControl(async (view, lease) => {
      geometry ??= this.dimensions;
      await view.resize({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        geometry,
      });
      this.lastResize = geometry;
    }, () => {
      this.resizeScheduled = false;
      // A follower keeps its pending size for the moment it types and takes control; nothing to repeat now.
      if (this.holdsControl()) this.scheduleResize();
    }, false);
  }

  /// Read the view until the service ends. A transport break reattaches only to the exact recorded generation and
  /// starts again from its replacement screen snapshot. It never repeats terminal input or redirects the identity.
  private openOnce(geometry: { columns: number; rows: number }): Promise<TerminalView> {
    if (this.target.hosted) {
      return this.runtime.attachTerminal(
        this.target.hosted.runtimeGeneration,
        this.target.hosted.terminalId,
      );
    }
    return this.runtime.openTerminal({
      requestId: newMutationRequestId(),
      providerId: this.target.provider,
      workspace: this.target.workspace,
      target: this.target.native
        ? { kind: "native", ...this.target.native }
        : { kind: "fresh" },
      geometry,
    });
  }

  private async pump(initialView: TerminalView): Promise<void> {
    let view = initialView;
    for (;;) {
      try {
        const notification = await view.next();
        switch (notification.kind) {
          case "output":
            this.receivedEmitter.fire(notification.bytes);
            this.writeFromService(this.decoder.decode(notification.bytes, { stream: true }));
            break;
          case "lagged":
            // The Core re-sends the whole screen next; clear so the redraw lands on a clean page, and start
            // decoding afresh so a multibyte tail cut off by the lag never bleeds into it.
            this.decoder = new TextDecoder("utf-8");
            this.mouseModes.reset();
            this.writeFromService(this.decoder.decode(notification.screen, { stream: true }));
            break;
          case "exited":
            // A clean exit closes the tab like a shell's would. Anything else keeps the tab, with the
            // service's own last words on it: a resume the service refused (measured: an empty stored
            // conversation exits at once) must not vanish before the person can read why.
            if (notification.exitCode === 0) {
              this.end(0);
            } else {
              this.presentation.ended(notification.exitCode);
              this.detach(false, `provider terminal exited with code ${notification.exitCode}`);
            }
            return;
        }
      } catch {
        if (this.closed) return;
        view.close();
        view = await this.runtime.attachTerminal(
          view.opened.terminal.runtimeGeneration,
          view.opened.terminal.terminalId,
        );
        if (this.closed) {
          view.close();
          return;
        }
        this.view = view;
        this.connected(view.opened.terminal);
        this.lease = null;
        this.decoder = new TextDecoder("utf-8");
        this.mouseModes.reset();
        this.writeFromService(this.decoder.decode(view.initialScreen, { stream: true }));
      }
    }
  }

  /// Run one action under this view's control lease.
  ///
  /// Exactly one view holds input and resize authority (`terminalTransportIntegrity`, input and geometry). Typing
  /// takes it: a lease lost to another window, or expired after a quiet thirty seconds, is acquired again and the
  /// action runs once under the new lease (the Runtime refused the first attempt, so nothing was applied twice).
  /// A resize only follows: with `takeOver` false the action runs solely while this view already holds control,
  /// so a follower window never resizes the shared process from under the window that is typing.
  private queueControl(
    action: (view: TerminalView, lease: TerminalControlLease) => Promise<void>,
    settled: (error?: unknown) => void = () => undefined,
    takeOver = true,
  ): void {
    const command = this.commandTail.then(async () => {
      if (this.closed) return;
      const view = this.view;
      if (!view) throw new Error("The public Runtime terminal is not connected.");
      if (!takeOver && !this.holdsControl()) return;
      try {
        await action(view, await this.ensureControl(view));
      } catch (error: unknown) {
        // The lease lives thirty seconds and is renewed when something is sent, so a conversation nobody typed
        // into for longer answers the next keystroke with `leaseExpired`. That is recoverable and used to reach
        // the person as a red line in their conversation instead (operator, 2026-08-28, with a picture). Another
        // window may also have taken control since, and asking again is the exact, visible transfer back.
        if (!leaseLost(error)) throw error;
        this.lease = null;
        if (!takeOver) return;
        await action(view, await this.ensureControl(view));
      }
    });
    const finish = (error?: unknown): void => {
      try {
        settled(error);
      } catch (settleError: unknown) {
        this.fail(settleError);
      }
    };
    this.commandTail = command.then(
      () => finish(),
      (error: unknown) => finish(error),
    );
    void command.catch((error: unknown) => this.fail(error));
  }

  private async ensureControl(view: TerminalView): Promise<TerminalControlLease> {
    const lease = this.lease;
    if (lease && lease.expiresAtMs > Date.now() + 5_000) return lease;
    // A lease whose time is already up cannot be renewed: the Runtime retires it before answering, so asking
    // to renew is a round trip that fails by construction. Past that moment the only move is to ask for
    // control again, which is what a person typing after a quiet minute is entitled to.
    const renewable = lease !== null && lease.expiresAtMs > Date.now();
    if (renewable && lease) {
      this.lease = await view.renewControl({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
      return this.lease;
    }
    this.lease = await view.acquireControl({
      requestId: newMutationRequestId(),
      terminalId: view.opened.terminal.terminalId,
      expectedTerminalGeneration: view.opened.terminal.terminalGeneration,
    });
    // Geometry follows the lease holder: the process was sized for whoever held control before, so this view's
    // own size is sent once, after this action, now that it is the one typing.
    this.lastResize = { columns: 0, rows: 0 };
    this.scheduleResize();
    return this.lease;
  }

  /// Whether this view holds a control lease that has not run out.
  private holdsControl(): boolean {
    return this.lease !== null && this.lease.expiresAtMs > Date.now();
  }

  private fail(error: unknown): void {
    if (this.closed) return;
    const message = error instanceof Error ? error.message : String(error);
    // The Runtime going away is not this conversation failing. When the daemon stops, an attached tab's stream
    // ends with a raw transport phrase ("Runtime closed during a frame"), and raising it as a red toast put a
    // protocol sentence in front of the person on top of the sidebar's own "Cannot reach the Runtime Core"
    // notice (measured 2026-09-05, the daemon killed under an open tab). Reachability is the index watch's to
    // say; here the view only detaches quietly, and the tab reattaches to whatever generation answers next.
    if (error instanceof RuntimeTransportError) {
      this.detach(false, `Runtime terminal lost its transport: ${message}`);
      return;
    }
    this.presentation.failed(message);
    this.detach(false, `Runtime terminal failed: ${message}`);
  }

  private end(code?: number): void {
    if (this.closed) return;
    this.detach(false, `provider terminal exited with code ${code ?? "unknown"}`);
    this.closeEmitter.fire(code);
  }

  /// Stop carrying the view but leave the tab open, so what the service wrote last stays readable.
  private detach(notifyRuntime: boolean, reason = "terminal view disconnected"): void {
    if (this.closed) return;
    this.closed = true;
    const view = this.view;
    this.view = null;
    this.lease = null;
    if (!notifyRuntime) this.disconnected(reason);
    if (!view) return;
    if (notifyRuntime) {
      void view.detach({
        terminalId: view.opened.terminal.terminalId,
        viewId: view.opened.viewId,
      }).catch(() => view.close());
    } else {
      view.close();
    }
  }
}
