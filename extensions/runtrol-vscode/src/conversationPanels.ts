import * as vscode from "vscode";

import { ConversationView } from "./conversationView";
import type { StudioRuntimeClient } from "./runtimeClient";
import type { RuntimeState } from "./state";
import type { SessionLine, WatchCursor } from "./runtimeTypes";
import type { ViewAction } from "./viewActions";

/// One conversation tab per session, with the file-click grammar the operator dictated
/// (memory/uxContract.md): clicking a conversation opens ITS tab beside whatever is already open, exactly
/// like clicking a file, and the tabs split, move, and close under the editor's own rules.
///
/// Each binding owns its panel AND its event watch. A hidden panel's webview dies
/// (`retainContextWhenHidden: false`), which pauses its watch through the visibility callback, so the live
/// cost of many tabs is only the visible ones. There is deliberately no cap on bindings: a binding whose
/// panel closed removes itself, so the map's size is the number of open conversation tabs and nothing else.
export class ConversationPanels implements vscode.Disposable {
  private readonly bindings = new Map<string, ConversationBinding>();
  private focusedId: string | null = null;
  private disposed = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly action: (session: SessionLine, message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string,
    private readonly providerOf: (session: SessionLine) => string,
    /// Told whenever a panel gains focus, with its session; the tree highlight and every
    /// selected-conversation command follow the focused tab.
    private readonly focusChanged: (session: SessionLine) => void,
  ) {}

  /// The binding whose tab currently has focus, or null when none does.
  focused(): ConversationBinding | null {
    return this.focusedId === null ? null : this.bindings.get(this.focusedId) ?? null;
  }

  /// The open binding for one session, or null when it has no tab.
  bindingFor(sessionId: string): ConversationBinding | null {
    return this.bindings.get(sessionId) ?? null;
  }

  /// Open this session's conversation tab: reveal the existing one, or create it beside the others.
  async open(session: SessionLine, preserveFocus = false): Promise<ConversationBinding> {
    const existing = this.bindings.get(session.sessionId);
    if (existing) {
      existing.updateSession(session);
      await existing.view.show(preserveFocus);
      return existing;
    }
    const binding = this.bind(session);
    await binding.view.show(preserveFocus);
    return binding;
  }

  /// Adopt a panel VS Code restored, rebinding it to the session its webview state names.
  async adopt(panel: vscode.WebviewPanel, session: SessionLine): Promise<ConversationBinding> {
    const existing = this.bindings.get(session.sessionId);
    if (existing) {
      // Two tabs for one session would race replay; the restored one wins because it is what the
      // reader sees, and the older binding folds.
      existing.dispose();
    }
    const binding = this.bind(session);
    await binding.view.adopt(panel);
    return binding;
  }

  /// Fan a session's fresh metadata out to its tab, if one is open.
  updateSession(session: SessionLine): void {
    this.bindings.get(session.sessionId)?.updateSession(session);
  }

  /// Every open binding, for whole-surface operations (reconnect pauses, machine-wide refresh).
  all(): ConversationBinding[] {
    return [...this.bindings.values()];
  }

  private bind(session: SessionLine): ConversationBinding {
    const binding = new ConversationBinding(
      session,
      this.extensionUri,
      this.runtime,
      this.state,
      (message) => this.action(binding.session, message),
      this.titleOf,
      this.providerOf,
      (visible) => {
        if (visible) {
          this.focusedId = binding.session.sessionId;
          this.focusChanged(binding.session);
        }
      },
      () => {
        if (this.bindings.get(binding.session.sessionId) === binding) {
          this.bindings.delete(binding.session.sessionId);
        }
        if (this.focusedId === binding.session.sessionId) {
          this.focusedId = null;
        }
      },
    );
    this.bindings.set(session.sessionId, binding);
    return binding;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const binding of this.bindings.values()) {
      binding.dispose();
    }
    this.bindings.clear();
  }
}

/// One session's tab: the view plus the watch that feeds it.
///
/// The watch lives here rather than in the controller because its lifetime IS the tab's visible lifetime:
/// visible tab, running watch; hidden or closed tab, no watch. The controller's one-selected-watch shape
/// could not say that for more than one tab at a time.
export class ConversationBinding implements vscode.Disposable {
  readonly view: ConversationView;
  private watchAbort: AbortController | null = null;
  private watchReady: Promise<void> = Promise.resolve();
  private current: SessionLine;
  private disposed = false;

  constructor(
    session: SessionLine,
    extensionUri: vscode.Uri,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    action: (message: ViewAction) => void,
    titleOf: (session: SessionLine) => string,
    providerOf: (session: SessionLine) => string,
    private readonly visibility: (visible: boolean) => void,
    private readonly closed: () => void,
  ) {
    this.current = session;
    this.view = new ConversationView(
      extensionUri,
      action,
      titleOf,
      (visible) => {
        this.visibility(visible);
        if (visible) {
          // A tab coming back is a reborn empty document (retainContextWhenHidden is false), so the
          // watch replays the daemon's bounded window instead of resuming past what nobody can see.
          this.state.forgetCursor(this.current.sessionId);
          this.ensureWatch();
        } else {
          this.pauseWatch();
        }
        if (!visible && !this.view.isOpen) {
          this.dispose();
        }
      },
      providerOf,
    );
    this.view.reset(session);
  }

  get session(): SessionLine {
    return this.current;
  }

  /// Everything already delivered has painted, and the watch is attached. The journey and the harness
  /// wait on this instead of sleeping.
  async settled(): Promise<void> {
    await this.view.waitForCurrentRender();
    await this.watchReady;
  }

  updateSession(session: SessionLine): void {
    this.current = session;
    this.view.updateSession(session);
  }

  /// Re-arm against a fresh connection: stop the old stream and, if the tab is on screen, replay.
  rewatch(): void {
    this.pauseWatch();
    if (this.view.isVisible) {
      this.state.forgetCursor(this.current.sessionId);
      this.ensureWatch();
    }
  }

  private ensureWatch(): void {
    if (this.disposed || this.watchAbort) return;
    const abort = new AbortController();
    this.watchAbort = abort;
    let ready = () => {};
    this.watchReady = new Promise<void>((resolve) => {
      ready = resolve;
    });
    void this.watchLoop(abort.signal, ready);
  }

  private pauseWatch(): void {
    this.watchAbort?.abort();
    this.watchAbort = null;
    this.watchReady = Promise.resolve();
  }

  private async watchLoop(signal: AbortSignal, ready: () => void): Promise<void> {
    let retryMs = 250;
    const sessionId = this.current.sessionId;
    while (!signal.aborted && !this.disposed) {
      try {
        await this.runtime.watchEvents(
          sessionId,
          this.state.cursor(sessionId),
          {
            started: ready,
            event: (payload: unknown, nextExpected: WatchCursor) => {
              if (this.view.frame(payload)) {
                this.state.advance(sessionId, nextExpected);
                return true;
              }
              this.pauseWatch();
              return false;
            },
            gap: (nextExpected: WatchCursor, message: string) => {
              this.state.advance(sessionId, nextExpected);
              this.view.status(message, "warning");
            },
          },
          signal,
        );
        retryMs = 250;
      } catch (error) {
        if (signal.aborted) {
          return;
        }
        this.view.status(error instanceof Error ? error.message : String(error), "error");
      }
      await abortableDelay(retryMs, signal);
      retryMs = Math.min(retryMs * 2, 5_000);
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.pauseWatch();
    this.view.dispose();
    this.closed();
  }
}

function abortableDelay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve();
      return;
    }
    const timer = setTimeout(done, ms);
    function done(): void {
      signal.removeEventListener("abort", done);
      clearTimeout(timer);
      resolve();
    }
    signal.addEventListener("abort", done, { once: true });
  });
}
