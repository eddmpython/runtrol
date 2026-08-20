import * as vscode from "vscode";

import { ConversationView, type AttachmentLabel, type ConversationContext } from "./conversationView";
import type { DraftChips, DraftState } from "./draft";
import type { StudioRuntimeClient } from "./runtimeClient";
import type { RuntimeState } from "./state";
import type { SessionLine, WatchCursor } from "./runtimeTypes";
import { MAX_ATTACHMENTS, type ViewAction } from "./viewActions";

/// One image waiting to ride with the next message. Page memory and this object only: never written anywhere.
export type Attachment = {
  readonly name: string;
  readonly mediaType: string;
  readonly base64Data: string;
};

/// The draft a tab shows before its conversation starts: the record, and the chips drawn from it.
export type DraftRecord = {
  readonly state: DraftState;
  readonly chips: DraftChips;
};

/// One conversation tab per session, with the file-click grammar the operator dictated
/// (memory/uxContract.md): clicking a conversation opens ITS tab beside whatever is already open, exactly
/// like clicking a file, and the tabs split, move, and close under the editor's own rules.
///
/// A tab may also hold a draft: a conversation that has not started, with its project, service, model,
/// effort and mode chips set but nothing said yet. The first message starts the session and the same tab
/// becomes that session's tab, so "New chat" costs no process until somebody actually speaks.
///
/// Each binding owns its panel AND its event watch. A hidden panel's webview dies
/// (`retainContextWhenHidden: false`), which pauses its watch through the visibility callback, so the live
/// cost of many tabs is only the visible ones. There is deliberately no cap on bindings: a binding whose
/// panel closed removes itself, so the map's size is the number of open conversation tabs and nothing else.
export class ConversationPanels implements vscode.Disposable {
  private readonly bindings = new Map<string, ConversationBinding>();
  private focusedKey: string | null = null;
  private disposed = false;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly action: (binding: ConversationBinding, message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string,
    private readonly providerOf: (session: SessionLine) => string,
    /// Told whenever a panel gains focus, with its session (null for a draft); the tree highlight and
    /// every selected-conversation command follow the focused tab.
    private readonly focusChanged: (session: SessionLine | null) => void,
    /// Where a live session runs, for the chips above its composer; asked when a tab shows a session.
    private readonly contextOf: (session: SessionLine) => Promise<ConversationContext>,
  ) {}

  /// The binding whose tab currently has focus, or null when none does.
  focused(): ConversationBinding | null {
    return this.focusedKey === null ? null : this.bindings.get(this.focusedKey) ?? null;
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
    const binding = this.bind(session, null);
    await binding.view.show(preserveFocus);
    return binding;
  }

  /// Open a new tab holding a draft: nothing started, everything ready to be.
  async openDraft(draft: DraftRecord): Promise<ConversationBinding> {
    const binding = this.bind(null, draft);
    await binding.view.show(false);
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
    const binding = this.bind(session, null);
    await binding.view.adopt(panel);
    return binding;
  }

  /// Adopt a restored panel that was showing a draft, with the choices it had stamped.
  async adoptDraft(panel: vscode.WebviewPanel, draft: DraftRecord): Promise<ConversationBinding> {
    const binding = this.bind(null, draft);
    await binding.view.adopt(panel);
    return binding;
  }

  /// A draft's conversation started: the same tab is now that session's tab.
  becomeSession(binding: ConversationBinding, session: SessionLine): void {
    const previous = binding.key;
    binding.becomeSession(session);
    if (this.bindings.get(previous) === binding) this.bindings.delete(previous);
    this.bindings.get(session.sessionId)?.dispose();
    this.bindings.set(session.sessionId, binding);
    if (this.focusedKey === previous) {
      this.focusedKey = session.sessionId;
      this.focusChanged(session);
    }
  }

  /// Fan a session's fresh metadata out to its tab, if one is open.
  updateSession(session: SessionLine): void {
    this.bindings.get(session.sessionId)?.updateSession(session);
  }

  /// Every open binding, for whole-surface operations (reconnect pauses, machine-wide refresh).
  all(): ConversationBinding[] {
    return [...this.bindings.values()];
  }

  private bind(session: SessionLine | null, draft: DraftRecord | null): ConversationBinding {
    const binding = new ConversationBinding(
      session,
      draft,
      this.extensionUri,
      this.runtime,
      this.state,
      (message) => this.action(binding, message),
      this.titleOf,
      this.providerOf,
      this.contextOf,
      (visible) => {
        if (visible) {
          this.focusedKey = binding.key;
          this.focusChanged(binding.session);
        }
      },
      () => {
        if (this.bindings.get(binding.key) === binding) {
          this.bindings.delete(binding.key);
        }
        if (this.focusedKey === binding.key) {
          this.focusedKey = null;
        }
      },
    );
    this.bindings.set(binding.key, binding);
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

/// One tab: the view plus the watch that feeds it, or the draft it holds until a session exists.
///
/// The watch lives here rather than in the controller because its lifetime IS the tab's visible lifetime:
/// visible tab, running watch; hidden or closed tab, no watch. The controller's one-selected-watch shape
/// could not say that for more than one tab at a time.
export class ConversationBinding implements vscode.Disposable {
  readonly view: ConversationView;
  private watchAbort: AbortController | null = null;
  private watchReady: Promise<void> = Promise.resolve();
  private current: SessionLine | null;
  private currentDraft: DraftRecord | null;
  private pendingAttachments: Attachment[] = [];
  private disposed = false;

  constructor(
    session: SessionLine | null,
    draft: DraftRecord | null,
    extensionUri: vscode.Uri,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    action: (message: ViewAction) => void,
    titleOf: (session: SessionLine) => string,
    providerOf: (session: SessionLine) => string,
    private readonly contextOf: (session: SessionLine) => Promise<ConversationContext>,
    private readonly visibility: (visible: boolean) => void,
    private readonly closed: () => void,
  ) {
    this.current = session;
    this.currentDraft = session ? null : draft;
    this.view = new ConversationView(
      extensionUri,
      action,
      titleOf,
      (visible) => {
        this.visibility(visible);
        if (visible) {
          // A tab coming back is a reborn empty document (retainContextWhenHidden is false), so the
          // watch replays the daemon's bounded window instead of resuming past what nobody can see.
          if (this.current) this.state.forgetCursor(this.current.sessionId);
          this.ensureWatch();
          this.view.updateAttachments(this.attachmentLabels());
          void this.refreshContext();
        } else {
          this.pauseWatch();
        }
        if (!visible && !this.view.isOpen) {
          this.dispose();
        }
      },
      providerOf,
    );
    this.view.reset(session, this.currentDraft);
  }

  /// The session this tab shows, or null while it holds a draft.
  get session(): SessionLine | null {
    return this.current;
  }

  /// The draft this tab holds, or null once a session exists.
  get draft(): DraftRecord | null {
    return this.currentDraft;
  }

  /// What this tab is keyed by: its session, or its draft.
  get key(): string {
    return this.current?.sessionId ?? this.currentDraft?.state.id ?? "";
  }

  /// The images waiting to ride with the next message.
  get attachments(): readonly Attachment[] {
    return this.pendingAttachments;
  }

  /// Add an image to the next message. Refused past the protocol's own bound rather than silently dropped.
  addAttachment(attachment: Attachment): boolean {
    if (this.pendingAttachments.length >= MAX_ATTACHMENTS) return false;
    this.pendingAttachments = [...this.pendingAttachments, attachment];
    this.view.updateAttachments(this.attachmentLabels());
    return true;
  }

  removeAttachment(index: number): void {
    this.pendingAttachments = this.pendingAttachments.filter((_item, at) => at !== index);
    this.view.updateAttachments(this.attachmentLabels());
  }

  /// Take every waiting image, leaving none: they ride with exactly one message.
  takeAttachments(): Attachment[] {
    const taken = this.pendingAttachments;
    this.pendingAttachments = [];
    this.view.updateAttachments([]);
    return taken;
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

  /// The draft's choices changed.
  updateDraft(draft: DraftRecord): void {
    if (this.current) return;
    this.currentDraft = draft;
    this.view.updateDraft(draft.chips, draft.state);
  }

  /// Where a live conversation runs, for the chips.
  updateContext(context: ConversationContext): void {
    this.view.updateContext(context);
  }

  /// The draft became a conversation: show the session in this same tab and start watching it.
  becomeSession(session: SessionLine): void {
    this.currentDraft = null;
    this.current = session;
    this.state.forgetCursor(session.sessionId);
    this.view.reset(session);
    if (this.view.isVisible) this.ensureWatch();
    void this.refreshContext();
  }

  /// Ask where the session runs and tell the page, unless the tab moved on meanwhile.
  private async refreshContext(): Promise<void> {
    const session = this.current;
    if (!session) return;
    const context = await this.contextOf(session);
    if (this.current?.sessionId === session.sessionId && !this.disposed) {
      this.view.updateContext(context);
    }
  }

  /// Re-arm against a fresh connection: stop the old stream and, if the tab is on screen, replay.
  rewatch(): void {
    this.pauseWatch();
    if (this.view.isVisible && this.current) {
      this.state.forgetCursor(this.current.sessionId);
      this.ensureWatch();
    }
  }

  private attachmentLabels(): AttachmentLabel[] {
    return this.pendingAttachments.map((attachment) => ({
      name: attachment.name,
      kilobytes: Math.max(1, Math.round(attachment.base64Data.length * 3 / 4 / 1024)),
    }));
  }

  private ensureWatch(): void {
    if (this.disposed || this.watchAbort || !this.current) return;
    const abort = new AbortController();
    this.watchAbort = abort;
    let ready = () => {};
    this.watchReady = new Promise<void>((resolve) => {
      ready = resolve;
    });
    void this.watchLoop(this.current.sessionId, abort.signal, ready);
  }

  private pauseWatch(): void {
    this.watchAbort?.abort();
    this.watchAbort = null;
    this.watchReady = Promise.resolve();
  }

  private async watchLoop(sessionId: string, signal: AbortSignal, ready: () => void): Promise<void> {
    let retryMs = 250;
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
    this.pendingAttachments = [];
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
