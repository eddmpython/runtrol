import * as vscode from "vscode";

import { gridCells, gridLayout } from "./conversationGrid";
import {
  type ConversationSurface,
  type Place,
  emptyPlaceHtml,
  tabSurface,
  viewIdOf,
  viewSurface,
} from "./conversationSurface";
import { ConversationView, type AttachmentLabel, type ConversationContext } from "./conversationView";
import type { DraftChips, DraftState } from "./draft";
import type { StudioRuntimeClient } from "./runtimeClient";
import { SerializedWatch } from "./serializedWatch";
import type { RuntimeState } from "./state";
import type { SessionLine, WatchCursor } from "./runtimeTypes";
import { MAX_ATTACHMENTS, type ViewAction } from "./viewActions";
import { refreshProviderTitleBindings } from "./nativeTitleRefresh";
import type { WatchLifecycleGate } from "./watchLifecycleGate";

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

/// Which conversation each workbench place last showed, remembered across reloads (the view comes back
/// empty otherwise, and the operator put a conversation there on purpose).
export type PlaceMemory = {
  read(place: Exclude<Place, "tab">): string | null;
  write(place: Exclude<Place, "tab">, sessionId: string | null): void;
};

/// What arranging the open tabs into a grid did.
export type GridResult = {
  readonly arranged: number;
  readonly leftInPlace: number;
};

/// How long a workbench view may take to resolve after it is asked to show.
const VIEW_RESOLVE_TIMEOUT_MS = 5_000;

/// One conversation tab per session, with the file-click grammar the product promises
/// (`docs/vscodeSurface.md`): clicking a conversation opens ITS tab beside whatever is already open, exactly
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
///
/// A tab is the default place. The same conversation can instead live in one of the workbench's views (the
/// bottom panel beside the terminals, the secondary side bar beside the code); the binding is the same, only
/// its surface differs, and a conversation is in exactly one place at a time so its watch runs once.
export class ConversationPanels implements vscode.Disposable {
  private readonly bindings = new Map<string, ConversationBinding>();
  private focusedKey: string | null = null;
  private disposed = false;
  /// The workbench views VS Code has resolved, by place, and who is waiting for one to resolve.
  private readonly views = new Map<Exclude<Place, "tab">, vscode.WebviewView>();
  private readonly viewWaiters = new Map<Exclude<Place, "tab">, Array<(view: vscode.WebviewView) => void>>();
  /// The binding shown in each workbench view right now.
  private readonly occupants = new Map<Exclude<Place, "tab">, ConversationBinding>();
  private placeMemory: PlaceMemory = { read: () => null, write: () => {} };

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
    private readonly watchLifecycle: WatchLifecycleGate,
    private readonly action: (binding: ConversationBinding, message: ViewAction) => void,
    private readonly titleOf: (session: SessionLine) => string,
    private readonly providerOf: (session: SessionLine) => string,
    private readonly iconOf: (providerId: string | null) => string,
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

  /// Open this session's conversation where it already is, or in a tab beside the others.
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

  /// Open this session in one of the workbench's places. A tab is `open`; the panel and the side bar are
  /// views VS Code resolves once: the conversation shown there before leaves (as a closed tab would), and
  /// this session's own tab, if it had one, closes, so the conversation is in one place and watched once.
  async openIn(session: SessionLine, place: Place, preserveFocus = false): Promise<ConversationBinding> {
    if (place === "tab") {
      const existing = this.bindings.get(session.sessionId);
      if (existing && existing.view.place !== "tab") {
        // Back to a tab: the view surface goes, a tab is made on the next show.
        existing.view.dispose();
      }
      return this.open(session, preserveFocus);
    }
    const view = await this.ensureView(place);
    const previous = this.occupants.get(place);
    if (previous && previous.session?.sessionId === session.sessionId) {
      previous.updateSession(session);
      await previous.view.show(preserveFocus);
      return previous;
    }
    previous?.dispose();
    const binding = this.bindings.get(session.sessionId) ?? this.bind(session, null);
    binding.updateSession(session);
    this.occupants.set(place, binding);
    this.placeMemory.write(place, session.sessionId);
    await binding.view.adopt(viewSurface(view, place, () => {
      if (this.occupants.get(place) === binding) {
        this.occupants.delete(place);
        this.placeMemory.write(place, null);
      }
      if (this.views.get(place) === view) view.webview.html = emptyPlaceHtml(place);
    }));
    await binding.view.show(preserveFocus);
    return binding;
  }

  /// The provider VS Code calls when one of the two workbench places is shown for the first time.
  viewProvider(
    place: Exclude<Place, "tab">,
    restore: (place: Exclude<Place, "tab">, sessionId: string) => void,
  ): vscode.WebviewViewProvider {
    return {
      resolveWebviewView: (view) => {
        view.webview.options = {
          enableScripts: true,
          localResourceRoots: [
            vscode.Uri.joinPath(this.extensionUri, "dist"),
            // The provider marks the service chip and the service menu draw.
            vscode.Uri.joinPath(this.extensionUri, "resources", "provider-icons"),
          ],
        };
        view.webview.html = emptyPlaceHtml(place);
        this.views.set(place, view);
        view.onDidDispose(() => {
          if (this.views.get(place) === view) this.views.delete(place);
          this.occupants.get(place)?.dispose();
        });
        const waiters = this.viewWaiters.get(place) ?? [];
        this.viewWaiters.delete(place);
        for (const waiter of waiters) waiter(view);
        // Nobody asked for this view just now: it is the window coming back, so the conversation it
        // showed before comes back with it, when that conversation still exists.
        if (waiters.length === 0) {
          const sessionId = this.placeMemory.read(place);
          if (sessionId) restore(place, sessionId);
        }
      },
    };
  }

  /// Remember which conversation each place shows, across reloads.
  rememberPlaces(memory: PlaceMemory): void {
    this.placeMemory = memory;
  }

  /// Spread the open conversation tabs over a grid of editor groups, VS Code doing the arranging.
  async arrangeGrid(): Promise<GridResult> {
    const tabs = [...this.bindings.values()].filter((binding) => binding.view.place === "tab");
    const cells = gridCells(tabs.length);
    if (cells.length === 0) return { arranged: 0, leftInPlace: tabs.length };
    await vscode.commands.executeCommand("vscode.setEditorLayout", gridLayout(tabs.length));
    cells.forEach((cell, index) => {
      tabs[index]?.view.revealIn(cell as vscode.ViewColumn);
    });
    return { arranged: cells.length, leftInPlace: tabs.length - cells.length };
  }

  /// The workbench view for a place, shown and resolved. VS Code contributes a `<viewId>.focus` command for
  /// every view; asking it to focus is what makes the workbench resolve the view through the provider.
  private async ensureView(place: Exclude<Place, "tab">): Promise<vscode.WebviewView> {
    const existing = this.views.get(place);
    if (existing) {
      existing.show(true);
      return existing;
    }
    const resolved = new Promise<vscode.WebviewView>((resolve, reject) => {
      const waiters = this.viewWaiters.get(place) ?? [];
      waiters.push(resolve);
      this.viewWaiters.set(place, waiters);
      setTimeout(
        () => reject(new Error(`VS Code did not show the Runtrol ${place === "panel" ? "panel" : "side bar"} view within ${VIEW_RESOLVE_TIMEOUT_MS} ms`)),
        VIEW_RESOLVE_TIMEOUT_MS,
      );
    });
    await vscode.commands.executeCommand(`${viewIdOf(place)}.focus`);
    return resolved;
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
    await binding.view.adopt(this.restoredTab(panel));
    return binding;
  }

  /// Adopt a restored panel that was showing a draft, with the choices it had stamped.
  async adoptDraft(panel: vscode.WebviewPanel, draft: DraftRecord): Promise<ConversationBinding> {
    const binding = this.bind(null, draft);
    await binding.view.adopt(this.restoredTab(panel));
    return binding;
  }

  private restoredTab(panel: vscode.WebviewPanel): ConversationSurface {
    return tabSurface(panel);
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

  /// Re-read the title for every open conversation owned by one provider.
  ///
  /// Session metadata did not change when a provider's native catalogue learned a title, so the ordinary index
  /// fan-out above has nothing to send. Reusing the current session refreshes only presentation and does not touch
  /// the event watch or its cursor.
  refreshTitles(providerId: string): void {
    refreshProviderTitleBindings(this.bindings.values(), providerId);
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
      this.watchLifecycle,
      (message) => this.action(binding, message),
      this.titleOf,
      this.providerOf,
      this.iconOf,
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
    this.occupants.clear();
    this.views.clear();
    this.viewWaiters.clear();
  }
}

/// One tab: the view plus the watch that feeds it, or the draft it holds until a session exists.
///
/// The watch lives here rather than in the controller because its lifetime IS the tab's visible lifetime:
/// visible tab, running watch; hidden or closed tab, no watch. The controller's one-selected-watch shape
/// could not say that for more than one tab at a time.
export class ConversationBinding implements vscode.Disposable {
  readonly view: ConversationView;
  private readonly watch = new SerializedWatch();
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
    private readonly watchLifecycle: WatchLifecycleGate,
    action: (message: ViewAction) => void,
    titleOf: (session: SessionLine) => string,
    providerOf: (session: SessionLine) => string,
    iconOf: (providerId: string | null) => string,
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
      iconOf,
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
    await this.watch.settled();
  }

  updateSession(session: SessionLine): void {
    const wasHot = this.current?.hot ?? false;
    this.current = session;
    this.view.updateSession(session);
    // A paused conversation that came back (the operator reopened it, or it was heated for a prompt) is
    // watched again from the top: its process is new, so the old cursor names nothing.
    if (session.hot && !wasHot && this.view.isVisible && !this.watch.requested) {
      this.state.forgetCursor(session.sessionId);
      this.view.status("", "info");
      this.ensureWatch();
    }
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
    if (this.disposed || !this.current) return;
    const sessionId = this.current.sessionId;
    this.watch.start(sessionId, (signal, ready) => this.watchLoop(sessionId, signal, ready));
  }

  private pauseWatch(): void {
    void this.watch.pause();
  }

  private async watchLoop(sessionId: string, signal: AbortSignal, ready: () => void): Promise<void> {
    let retryMs = 250;
    while (!signal.aborted && !this.disposed) {
      const releaseOpening = await this.watchLifecycle.acquire("foreground", signal);
      if (!releaseOpening) return;
      let opening = true;
      try {
        await this.runtime.watchEvents(
          sessionId,
          this.state.cursor(sessionId),
          {
            started: () => {
              ready();
              if (!opening) return;
              opening = false;
              releaseOpening();
            },
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
        // A conversation that is not running cannot be watched, and that is not a fault to retry. It was
        // paused: either the operator closed its process, or the Runtime released it to keep the running set
        // small (eight hot processes, the memory contract). Measured in the real window: the tab of a
        // conversation paused under the reader showed "sessionNotFound" in red and retried forever, while
        // the truth was one calm sentence. The watch resumes by itself when the session is hot again.
        if (this.current && !this.current.hot) {
          this.view.status(
            "Paused: this conversation is not running right now. Open it again from the sidebar to continue.",
            "info",
          );
          this.pauseWatch();
          return;
        }
        this.view.status(error instanceof Error ? error.message : String(error), "error");
      } finally {
        if (opening) releaseOpening();
      }
      await abortableDelay(retryMs, signal);
      retryMs = Math.min(retryMs * 2, 5_000);
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    void this.watch.pause();
    this.watch.dispose();
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
