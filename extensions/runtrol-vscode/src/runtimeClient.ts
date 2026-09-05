import { createHash, randomUUID } from "node:crypto";
import { isAbsolute } from "node:path";

import {
  IntegrationCredentials,
  IntegrationIdentity,
  RuntimeConnector,
  RuntimeLocator,
  RuntimeRequestError,
  TerminalClient,
  newMutationRequestId,
  type AppScope,
  type ClientOptions,
  type ControlLease,
  type EventCursor,
  type IntegrationGrant,
  type ManagedSessionList,
  type NativeActivity,
  type PendingApproval,
  type ProviderList,
  type PublicInputBlock,
  type RuntimeClient,
  type RuntimeModelCatalog,
  type RuntimeProviderCapabilities,
  type SessionDescriptor,
  type SessionWorkspaceAccess,
  type TerminalOpenParams,
  type TerminalDescriptor,
  type TerminalIndexSnapshot,
  type WindowMirrorEndParams,
  type NativeFocusParams,
  type NativeFocusResult,
  type WindowRevealParams,
  type WindowRevealResult,
  type WindowMirrorOpenParams,
  type WindowMirrorOpened,
  type WindowMirrorOutputParams,
  type WindowRegisterParams,
  type WindowUpdateParams,
  type TerminalIndexSubscription,
  type TerminalView,
  type ValidatedLocator,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";

import { abortableDelay } from "./abortableDelay";
import { readDialogueGuide, setTerminalDialogue } from "./dialogueActivation";
import {
  cachedControlAction,
  restorableControls,
  settleControlPersistence,
  sessionDisappearedAfterCool,
  type StoredControlState,
} from "./runtimeControl";
import { collectNativeChats } from "./nativeChatCatalogue";
import { providerCommandNames } from "./observedMirrorState";
import { TerminalFleet } from "./terminalFleet";
import { errorKindOf } from "./serviceHelp";
import { workspaceIdentity } from "./workspaceCollision";
import type { NativeChatCatalogue, NativeChatLine } from "./runtimeTypes";

const SECRET_KEY = "runtrol.runtime.integration.v1";

/// Which Core file the public locator verifies with, and which generation it should prefer.
export type RuntimeSource = { runtimeExecutable: string; preferDigest: string | null };
const ENROLLMENT_DECISION_SETTLE_MS = 5_000;
const ENROLLMENT_DECISION_POLL_MS = 50;
const RUNTIME_LOCATOR_SETTLE_MS = 12_000;
const RUNTIME_LOCATOR_POLL_MS = 25;
// The in-memory lease is authoritative while the Extension Host is alive. Persisting it only
// improves reload recovery, so SecretStorage latency must not delay an interactive session action.
const CONTROL_PERSISTENCE_INLINE_MS = 0;
const ALL_STUDIO_SCOPES: readonly AppScope[] = [
  "provider.read",
  "model.read",
  "session.list",
  "session.native.discover",
  "session.output.read",
  "session.start",
  "session.resume",
  "session.input.write",
  "session.stop",
  "approval.respond.low",
  "approval.respond.high",
  "session.delete",
];

type StoredIntegration = {
  schema: 1;
  clientInstanceId: string;
  privateKeyPkcs8: string;
  grant?: IntegrationGrant;
  controlState?: StoredControlState;
};

export type RuntimeInventory = {
  sessions: ManagedSessionList;
  providers: ProviderList;
};

export type RuntimeEventHandlers = {
  started(): void;
  event(payload: unknown, nextExpected: EventCursor): boolean;
  gap(nextExpected: EventCursor, message: string): void;
};

export type RuntimeSessionAction = {
  sessionId: string;
  lifecycle: "hotIdle" | "hotRunning" | "cold" | "failed";
  generation: number;
  workspace: string;
};

export class StudioRuntimeClient implements vscode.Disposable {
  private readonly connector = new RuntimeConnector();
  private command: RuntimeClient | null = null;
  /// The command connection this window's registration was made on; a new connection needs a new one.
  private windowRegisteredOn: RuntimeClient | null = null;
  /// The 250 ms process-roster clock must never queue in front of operator commands.
  ///
  /// A provider can take tens of milliseconds to validate its structural activity surface on Windows. Sharing
  /// `commandTail` made a manual refresh, input or stop wait behind that observation even though neither operation
  /// depends on it. One persistent authenticated connection and one narrow queue preserve ordering inside the
  /// activity lane without adding a connection per tick or delaying the command lane.
  private activity: RuntimeClient | null = null;
  /// The observed mirror's own connection. It carries only this window's captured provider output, so a chunk
  /// the Runtime refuses closes this and nothing else: not a person's click, and not the command connection
  /// that holds this window's registration in the Runtime's window registry.
  private mirror: RuntimeClient | null = null;
  private mirrorTail: Promise<void> = Promise.resolve();
  private options: ClientOptions | null = null;
  private stored: StoredIntegration | null = null;
  private locator: Promise<ValidatedLocator> | null = null;
  private firstInspection: Promise<ValidatedLocator | null> | null = null;
  private commandTail: Promise<void> = Promise.resolve();
  private activityTail: Promise<void> = Promise.resolve();
  private controlPersistence: Promise<void> = Promise.resolve();
  private readonly controls = new Map<string, ControlLease>();
  private providerSnapshot: ProviderList | null = null;
  private sessionSnapshot: ManagedSessionList | null = null;
  private runtimeExecutable: string | null = null;
  private preferDigest: string | null = null;
  private providerWatch: symbol | null = null;
  private sessionWatch: symbol | null = null;
  private terminalWatch: symbol | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    /// The Core the endpoint probe settled on, with the digest of the Core this extension installed when that
    /// is the one in use (null for an operator corePath or a PATH build, so the newest generation not draining
    /// is chosen).
    private readonly locateRuntime: () => Promise<RuntimeSource>,
    /// The Core the probe is expected to settle on, known before it answers; see `warmLocator`.
    private readonly expectedRuntime: () => Promise<RuntimeSource>,
    private readonly selfApprove: (pendingId: string, signature: string) => Promise<boolean>,
    private readonly confirmForget: (
      confirmationId: string,
      sessionId: string,
    ) => Promise<boolean>,
    private readonly confirmSharedOpen: (
      confirmationId: string,
      workspace: string,
    ) => Promise<boolean>,
    private readonly additionalEnrollmentRoots: readonly string[] = [],
    private readonly reportInitialization: (stage: string) => void = () => undefined,
  ) {}

  /// Begin the locator inspection now, before anything waits on it.
  ///
  /// On POSIX the inspection is an owner and mode checked locator read. On Windows it runs the installed Core once
  /// to verify the locator natively, and a Core process costs a few hundred milliseconds to start (measured
  /// 2026-08-27: 250 to 390 ms per spawn on a desktop). Errors are not reported here: `initialize` re-inspects on
  /// its own path and reports what it finds, so nothing is lost by letting this early attempt fail quietly.
  warmLocator(): Promise<ValidatedLocator | null> {
    this.firstInspection ??= this.resolveExecutable(this.expectedRuntime)
      .then(() => this.inspectOnce())
      .catch(() => null);
    return this.firstInspection;
  }

  /// One look at the locator, without waiting for a generation to appear.
  ///
  /// A running generation of the installed build is remembered as the settled locator, so `initialize` does not
  /// inspect again. Anything else answers null and leaves the settling to `initialize`'s own loop.
  private async inspectOnce(): Promise<ValidatedLocator | null> {
    const inspected = await RuntimeLocator.system({
      ...(process.platform === "win32" && this.runtimeExecutable && isAbsolute(this.runtimeExecutable)
        ? { runtimeExecutable: this.runtimeExecutable }
        : {}),
      ...(this.preferDigest ? { preferDigest: this.preferDigest } : {}),
    }).inspect();
    if (inspected.state !== "running") return null;
    if (this.preferDigest && inspected.locator.digest !== this.preferDigest) return null;
    this.locator ??= Promise.resolve(inspected.locator);
    return inspected.locator;
  }

  async initialize(): Promise<void> {
    const pendingStages = new Set(["identity", "core"]);
    const stageSettled = (stage: string): void => {
      pendingStages.delete(stage);
      if (pendingStages.size > 0) this.reportInitialization([...pendingStages].join("+"));
    };
    this.reportInitialization("identity+core");
    const identity = this.loadOrCreateIdentity().finally(() => stageSettled("identity"));
    const core = this.resolveExecutable(this.locateRuntime).finally(() => stageSettled("core"));
    const [stored] = await Promise.all([identity, core]);
    this.reportInitialization("locator");
    await this.withRuntimeLocator(async () => undefined);
    this.reportInitialization("integration");
    try {
      await this.useIntegration(stored);
    } catch (error) {
      if (!stored.grant || !recoverableAuthenticationFailure(error)) throw error;
      await this.replaceIntegration();
    }
  }

  /// Where each account stands against its limits, by each provider's own latest report.
  async providersUsage(): Promise<import("@runtrol/runtime-client").ProviderUsageList> {
    return this.read(async (runtime) => runtime.providers().usage());
  }

  /// Open one provider-faithful terminal on its own public Runtime connection.
  async openTerminal(params: TerminalOpenParams): Promise<TerminalView> {
    await this.commandClient();
    const dedicated = await this.withRuntimeLocator((locator) => this.connector.connectWithRetry(
      locator,
      this.requireOptions(),
    ));
    try {
      return await dedicated.terminals().open(params);
    } catch (error) {
      dedicated.close();
      if (
        params.target.kind === "native"
        && error instanceof RuntimeRequestError
        && error.failure.code === "terminalAlreadyLive"
      ) {
        const existing = await this.findTerminal(
          params.providerId,
          params.target.nativeSessionId,
          params.workspace,
        );
        if (existing) {
          return this.attachTerminal(existing.runtimeGeneration, existing.terminalId);
        }
      }
      throw error;
    }
  }

  /// Reattach one view to the exact generation that owns an already live terminal.
  async attachTerminal(runtimeGeneration: string, terminalId: string): Promise<TerminalView> {
    await this.commandClient();
    return TerminalClient.attachInGeneration(
      this.connector,
      this.runtimeLocator(),
      this.requireOptions(),
      runtimeGeneration,
      terminalId,
    );
  }

  private async findTerminal(
    providerId: string,
    nativeSessionId: string,
    workspace: string,
  ): Promise<TerminalDescriptor | null> {
    const fleet = await TerminalClient.listAllGenerations(
      this.connector,
      this.runtimeLocator(),
      this.requireOptions(),
    );
    // The folder is compared as an identity, not as a string. The Runtime canonicalises the path it stores and
    // Windows hands the same folder back with a different drive case, so the raw comparison found nothing and
    // the person was shown the refusal instead of their own running conversation (2026-08-28).
    const wanted = workspaceIdentity(workspace);
    const matches = fleet.flatMap((entry) => entry.outcome.kind === "listed"
      ? entry.outcome.snapshot.terminals.filter((terminal) => (
        terminal.providerId === providerId
        && terminal.nativeSessionId === nativeSessionId
        && workspaceIdentity(terminal.workspace) === wanted
      ))
      : []);
    // More than one generation can name the same conversation while a handover is in flight, which is exactly
    // the moment this lookup exists for. Refusing then sent the person back to the error; take the one that is
    // still running, and the newest of those.
    const live = matches.filter((terminal) => terminal.processState === "running");
    const candidates = live.length > 0 ? live : matches;
    return candidates.reduce<TerminalDescriptor | null>(
      (best, terminal) => (best === null || terminal.openedAtMs > best.openedAtMs ? terminal : best),
      null,
    );
  }

  private runtimeLocator(): RuntimeLocator {
    return RuntimeLocator.system({
      ...(process.platform === "win32" && this.runtimeExecutable && isAbsolute(this.runtimeExecutable)
        ? { runtimeExecutable: this.runtimeExecutable }
        : {}),
      ...(this.preferDigest ? { preferDigest: this.preferDigest } : {}),
    });
  }

  /// The Studio's own integration identity, or null before enrollment has ever succeeded.
  ///
  /// Only the identity. Roots and generation age the moment the daemon revises the grant, so anything acting on
  /// them reads the daemon's own row instead of a stored copy.
  integrationId(): string | null {
    return this.stored?.grant?.integrationId ?? null;
  }

  async inventory(): Promise<RuntimeInventory> {
    const sessions = this.sessionSnapshot;
    const providers = this.providerSnapshot;
    if (sessions && providers) return { providers, sessions };
    return this.read(async (runtime) => {
      const nextProviders = this.providerSnapshot ?? await runtime.providers().list();
      const nextSessions = this.sessionSnapshot ?? await runtime.sessions().list();
      if (this.providerWatch) this.providerSnapshot = nextProviders;
      if (this.sessionWatch) this.sessionSnapshot = nextSessions;
      return { providers: nextProviders, sessions: nextSessions };
    });
  }

  /// Ask Runtime to restamp its executable search surface without making a normal sidebar repaint wait for it.
  ///
  /// Provider and session watches already own the current inventory. A list request is still the explicit
  /// zero-configuration discovery trigger, but its answer is the current snapshot and the potentially expensive
  /// search happens behind the provider watch. Keeping that request separate lets ordinary refreshes paint from
  /// the pushed snapshots instead of paying one audited round trip for facts already in memory.
  async refreshProviderInventory(): Promise<void> {
    const nextProviders = await this.read((runtime) => runtime.providers().list());
    if (this.providerWatch) this.providerSnapshot = nextProviders;
  }

  /// The provider command names the Runtime's inventory declares, lowercase, to the provider each belongs to.
  async providerCommandNames(): Promise<ReadonlyMap<string, string>> {
    const known = providerCommandNames((await this.inventory()).providers.providers);
    if (known.size > 0) return known;
    // The first inventory can be the watch's empty opening snapshot; the Runtime's own list never is.
    return providerCommandNames((await this.read((runtime) => runtime.providers().list())).providers);
  }

  /// Open a mirror of a terminal this window observes, on the mirror's own connection.
  mirrorOpen(params: WindowMirrorOpenParams): Promise<WindowMirrorOpened> {
    return this.mirrorLane((runtime) => runtime.windows().mirrorOpen(params));
  }

  /// One chunk of the observed execution's output, in order with every other chunk and behind nothing else.
  mirrorOutput(params: WindowMirrorOutputParams): Promise<void> {
    return this.mirrorLane((runtime) => runtime.windows().mirrorOutput(params));
  }

  mirrorEnd(params: WindowMirrorEndParams): Promise<void> {
    return this.mirrorLane((runtime) => runtime.windows().mirrorEnd(params));
  }

  /// Ask the window that owns a terminal to show it and come forward.
  revealAtOwner(params: WindowRevealParams): Promise<WindowRevealResult> {
    return this.read((runtime) => runtime.windows().reveal(params));
  }

  /// Ask the window that owns a live conversation's terminal to show it and come forward. The Runtime knows which
  /// window that is; this window never learns it.
  focusNative(params: NativeFocusParams): Promise<NativeFocusResult> {
    return this.read((runtime) => runtime.providers().focusNative(params));
  }

  /// Follow reveal requests for this window on a dedicated connection until `signal` aborts, reconnecting after
  /// a Runtime restart; `onRequest` gets the key of the terminal to show.
  async watchWindowReveals(
    windowSessionId: string,
    onRequest: (terminalKey: string) => void,
    signal: AbortSignal,
  ): Promise<void> {
    while (!signal.aborted) {
      let runtime: RuntimeClient | null = null;
      try {
        runtime = await this.withRuntimeLocator(
          (locator) => this.connector.connectWithRetry(locator, this.requireOptions()),
        );
        const subscription = await runtime.windows().watchReveals({ windowSessionId });
        while (!signal.aborted) {
          const notification = await subscription.next();
          if (notification.kind !== "requested") break;
          onRequest(notification.requested.terminalKey);
        }
      } catch (error) {
        if (signal.aborted) return;
        // The Runtime went away or refused (a registration not made yet); the next round asks again.
        void error;
      } finally {
        runtime?.close();
      }
      if (!signal.aborted) await abortableDelay(2_000, signal);
    }
  }

  /// Every hosted terminal the Runtime lists right now, with what each process holds in memory.
  /// Register this window and publish the terminals it observes, on the persistent command connection. A
  /// registration lives as long as its connection, so a fresh connection registers again before it updates.
  async publishWindow(register: WindowRegisterParams, update: WindowUpdateParams): Promise<void> {
    await this.read(async (runtime) => {
      if (this.windowRegisteredOn !== runtime) {
        await runtime.windows().register(register);
        this.windowRegisteredOn = runtime;
      }
      await runtime.windows().update(update);
    });
  }

  async listTerminals(): Promise<TerminalIndexSnapshot> {
    return this.read((runtime) => runtime.terminals().list());
  }

  /// The managed sessions as the Runtime lists them right now, memory figures included. The watch delivers
  /// structural changes; a figure that moves without a structural change needs to be asked for.
  async listSessionsNow(): Promise<ManagedSessionList> {
    return this.read((runtime) => runtime.sessions().list());
  }

  /// The live conversations of one service and the subset with a model answering right now.
  ///
  /// This is the bounded compatibility path for processes that began outside the transparent broker. The provider
  /// answers from its small process roster, and Studio asks it on the dedicated fast activity clock rather than
  /// walking the stored conversation catalogue or sharing the memory sampling clock.
  async nativeActivity(providerId: string): Promise<NativeActivity> {
    return this.activityRead((runtime) => runtime.providers().nativeActivity(providerId));
  }

  async models(providerId: string): Promise<RuntimeModelCatalog> {
    return this.read((runtime) => runtime.providers().listModels(providerId));
  }

  async capabilities(providerId: string): Promise<RuntimeProviderCapabilities> {
    return this.read((runtime) => runtime.providers().getCapabilities(providerId));
  }

  async verifyProvider(providerId: string): Promise<void> {
    await this.read(async (runtime) => {
      await runtime.providers().getCapabilities(providerId);
    });
  }

  async nativeChats(providerId: string, signal?: AbortSignal): Promise<NativeChatCatalogue> {
    signal?.throwIfAborted();
    if (!this.options) {
      return nativeCatalogueFailure(
        providerId,
        "Existing chat discovery is not approved for this Runtrol Studio integration.",
      );
    }
    return this.withRuntimeLocator(async (locator) => {
      const runtime = await this.connector.connectWithRetry(locator, this.requireOptions());
      const close = (): void => runtime.close();
      signal?.addEventListener("abort", close, { once: true });
      try {
        signal?.throwIfAborted();
        // The grant as the daemon holds it NOW, answered on this very connection's initialization. The
        // stored snapshot refreshes on the command path's connects, so reading it here raced every root
        // widening: a folder opened a moment ago was invisible to discovery until some later reconnect.
        const grant = runtime.initialization.grant;
        if (!grant?.scopes.includes("session.native.discover")) {
          return nativeCatalogueFailure(
            providerId,
            "Existing chat discovery is not approved for this Runtrol Studio integration.",
          );
        }
        // The machine, in one question. Measured 2026-08-20 against the installed CLIs: four of the
        // five answer without a folder filter and every row they return carries its own folder, so
        // asking per approved root was narrowing a list that the providers were already willing to
        // give whole. That narrowing is what left yesterday's conversation invisible unless this
        // window happened to have opened its folder.
        const machineWide = await collectNativeChats(
          runtime.providers(),
          providerId,
          [null],
          Date.now,
          signal,
        );
        if (!needsPerRootDiscovery(machineWide)) return machineWide;

        // This provider only answers about one folder at a time, and says so by name. Approved
        // roots are all this surface can offer it, and the catalogue's own coverage tells the
        // reader that the answer is partial.
        const roots = [...new Set(grant.roots)];
        if (roots.length === 0) {
          return nativeCatalogueFailure(
            providerId,
            "This service lists conversations one workspace root at a time, and none is approved yet.",
          );
        }
        return await collectNativeChats(runtime.providers(), providerId, roots, Date.now, signal);
      } finally {
        signal?.removeEventListener("abort", close);
        runtime.close();
      }
    });
  }

  async start(
    providerId: string,
    workspace: string,
    access: SessionWorkspaceAccess,
    model: string | null,
    reasoningEffort: string | null,
    permission: string | null = null,
  ): Promise<SessionDescriptor> {
    return this.mutate(async (runtime) => {
      const params = {
        requestId: newMutationRequestId(),
        providerId,
        workspace,
        access,
        ...(model ? { model } : {}),
        ...(reasoningEffort ? { reasoningEffort } : {}),
        ...(permission ? { permission } : {}),
      };
      const opened = await this.openConfirmingShared(access, workspace, () => runtime.sessions().start(params));
      await this.rememberControl(opened.control);
      return opened.session;
    });
  }

  async resume(session: RuntimeSessionAction, access: SessionWorkspaceAccess): Promise<SessionDescriptor> {
    return this.mutate(async (runtime) => {
      const params = {
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        expectedLifecycle: session.lifecycle,
        expectedSessionGeneration: session.generation,
        workspace: session.workspace,
        access,
      };
      const opened = await this.openConfirmingShared(access, session.workspace, () => runtime.sessions().resume(params));
      await this.rememberControl(opened.control);
      return opened.session;
    });
  }

  /// One public open, sent again once after the person's own shared-writer choice is confirmed at the machine.
  ///
  /// The public Runtime grants a second writer in a working tree only after a local action. For the Studio that
  /// action is the click that chose "shared" (Start here anyway, keeping both chats working, asking services at
  /// once), so the confirmation is made on the person's behalf and the same request, same mutation identity, is
  /// sent again. An exclusive open that says presenceRequired is something else (a service asking to be signed
  /// in) and is left to the caller.
  private async openConfirmingShared<Opened>(
    access: SessionWorkspaceAccess,
    workspace: string,
    open: () => Promise<Opened>,
  ): Promise<Opened> {
    try {
      return await open();
    } catch (error) {
      if (access !== "shared" || !presenceConfirmation(error)) throw error;
      if (!await this.confirmSharedOpen(error.failure.correlationId, workspace)) {
        throw new Error("the exact shared-writer session open was not confirmed locally");
      }
      return open();
    }
  }

  async adoptNative(
    native: NativeChatLine,
    access: SessionWorkspaceAccess,
  ): Promise<SessionDescriptor> {
    const adoptionToken = native.adoptionToken;
    if (!adoptionToken) {
      throw new Error("that existing chat has no current Runtime adoption proof");
    }
    return this.mutate(async (runtime) => {
      const params = {
        requestId: newMutationRequestId(),
        providerId: native.providerId,
        nativeSessionId: native.nativeSessionId,
        workspace: native.cwd,
        access,
        adoptionToken,
      };
      const opened = await this.openConfirmingShared(access, native.cwd, () => runtime.sessions().adoptNative(params));
      await this.rememberControl(opened.control);
      return opened.session;
    });
  }

  async submitInput(session: RuntimeSessionAction, input: string): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      await runtime.sessions().submitInput({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        input,
      });
    });
  }

  /// One message made of several blocks (text and images), under the same lease as plain input.
  ///
  /// The bytes travel once and are never kept: the image rides this request and nothing else. Whether a
  /// service accepts images is its own published capability; a refusal arrives as the daemon's error.
  async submitBlocks(session: RuntimeSessionAction, blocks: readonly PublicInputBlock[]): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      await runtime.sessions().submitBlocks({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        blocks,
      });
    });
  }

  /// Relay the operator's model choice through the provider's own switch surface, under the same lease as
  /// input. What the session actually answers with stays the provider's word, arriving on the event stream.
  async setModel(
    session: RuntimeSessionAction,
    model: string,
    reasoningEffort?: string,
  ): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      await runtime.sessions().setModel({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        model,
        ...(reasoningEffort ? { reasoningEffort } : {}),
      });
    });
  }

  /// Relay the operator's mode choice under the same lease as input. Whether it changed stays the
  /// provider's word, arriving on the event stream.
  async setMode(session: RuntimeSessionAction, mode: string): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      await runtime.sessions().setMode({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        mode,
      });
    });
  }

  async interrupt(session: RuntimeSessionAction): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      await runtime.sessions().interrupt({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
    });
  }

  /// Stop holding one provider process while keeping Runtrol's durable pointer to its conversation.
  ///
  /// This is the lifecycle operation a conversation switch needs. `close` goes one step further and forgets
  /// the pointer, which made it impossible to switch away from an idle chat without either leaving two writers
  /// alive or making the old chat disappear from Runtrol.
  async cool(session: RuntimeSessionAction, interruptRunning: boolean): Promise<void> {
    await this.mutate(async (runtime) => {
      let current = await runtime.sessions().get(session.sessionId);
      if (current.lifecycle === "hotRunning") {
        if (!interruptRunning) {
          throw new Error("the running turn must be interrupted before this session can cool");
        }
        const lease = await this.ensureControl(runtime, runtimeAction(current));
        await runtime.sessions().interrupt({
          requestId: newMutationRequestId(),
          sessionId: current.sessionId,
          leaseId: lease.leaseId,
          leaseGeneration: lease.leaseGeneration,
        });
        current = await waitForIdleSession(runtime, current.sessionId);
      }
      if (current.lifecycle === "cold") return;
      if (current.lifecycle !== "hotIdle") {
        throw new Error(`the session cannot cool from Runtime lifecycle ${current.lifecycle}`);
      }
      const lease = await this.ensureControl(runtime, runtimeAction(current));
      await runtime.sessions().cool({
        requestId: newMutationRequestId(),
        sessionId: current.sessionId,
        expectedSessionGeneration: current.sessionGeneration,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
      await this.forgetControl(current.sessionId);
    });
  }

  async close(session: RuntimeSessionAction, interruptRunning: boolean): Promise<void> {
    await this.mutate(async (runtime) => {
      let current = await runtime.sessions().get(session.sessionId);
      if (current.lifecycle === "hotRunning") {
        if (!interruptRunning) {
          throw new Error("the running turn must be interrupted before this session can close");
        }
        const lease = await this.ensureControl(runtime, runtimeAction(current));
        await runtime.sessions().interrupt({
          requestId: newMutationRequestId(),
          sessionId: current.sessionId,
          leaseId: lease.leaseId,
          leaseGeneration: lease.leaseGeneration,
        });
        current = await waitForIdleSession(runtime, current.sessionId);
      }
      if (current.lifecycle === "hotIdle") {
        const lease = await this.ensureControl(runtime, runtimeAction(current));
        await runtime.sessions().cool({
          requestId: newMutationRequestId(),
          sessionId: current.sessionId,
          expectedSessionGeneration: current.sessionGeneration,
          leaseId: lease.leaseId,
          leaseGeneration: lease.leaseGeneration,
        });
        await this.forgetControl(current.sessionId);
        try {
          current = await runtime.sessions().get(current.sessionId);
        } catch (error) {
          if (sessionDisappearedAfterCool(error)) return;
          throw error;
        }
      }
      if (current.lifecycle !== "cold") {
        throw new Error(`the session cannot close from Runtime lifecycle ${current.lifecycle}`);
      }
      const forget = {
        requestId: newMutationRequestId(),
        sessionId: current.sessionId,
        expectedSessionGeneration: current.sessionGeneration,
      };
      try {
        await runtime.sessions().forget(forget);
      } catch (error) {
        if (!presenceConfirmation(error)) throw error;
        if (!await this.confirmForget(error.failure.correlationId, current.sessionId)) {
          throw new Error("the exact Runtime session removal was not confirmed locally");
        }
        await runtime.sessions().forget(forget);
      }
      await this.forgetControl(current.sessionId);
    });
  }

  /// Ask the provider that owns a stored conversation to delete it, through the Runtime's relay.
  ///
  /// The Runtime stores nothing and deletes nothing itself; it hands the request to the CLI that owns the
  /// conversation, and a CLI with no such surface refuses as `capabilityUnavailable`. No lease: the act is
  /// about a conversation nobody supervises.
  async deleteNative(native: NativeChatLine): Promise<void> {
    await this.mutate(async (runtime) => {
      await runtime.sessions().deleteNative({
        requestId: newMutationRequestId(),
        providerId: native.providerId,
        nativeSessionId: native.nativeSessionId,
        workspace: native.cwd,
      });
    });
  }

  /// The questions a conversation is waiting on, with the provider's own options, for answering from the
  /// sidebar row without opening the page. Read under the same control lease an answer takes.
  async listPendingApprovals(session: RuntimeSessionAction): Promise<readonly PendingApproval[]> {
    let approvals: readonly PendingApproval[] = [];
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      const pending = await runtime.approvals().listPending({
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
      approvals = pending.approvals;
    });
    return approvals;
  }

  async answerApproval(
    session: RuntimeSessionAction,
    approvalId: string,
    optionId: number,
    subjectDigest: readonly number[],
  ): Promise<void> {
    await this.mutate(async (runtime) => {
      const lease = await this.ensureControl(runtime, session);
      const pending = await runtime.approvals().listPending({
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
      const approval = pending.approvals.find((candidate) => candidate.approvalId === approvalId);
      if (!approval || !sameBytes(approval.subjectDigest, subjectDigest)) {
        throw new Error("the selected approval is no longer pending with the same subject");
      }
      if (!approval.options.some((option) => option.optionId === optionId && option.unavailable == null)) {
        throw new Error("the selected approval option is no longer available");
      }
      await runtime.approvals().respond({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
        approvalId,
        optionId,
        subjectDigest: [...subjectDigest],
      });
    });
  }

  /// Follow the provider inventory and, on the same subscription, every account's usage position.
  ///
  /// Usage arrives pushed: once when the subscription starts and again whenever a turn or a probe moves it.
  /// Nothing here polls `providers/usage`.
  async watchProviders(
    snapshot: (providers: ProviderList) => void,
    signal: AbortSignal,
    usage: (usage: import("@runtrol/runtime-client").ProviderUsageList) => void = () => undefined,
  ): Promise<void> {
    const watch = Symbol("provider watch");
    this.providerWatch = watch;
    const publish = (providers: ProviderList): void => {
      if (this.providerWatch !== watch) return;
      this.providerSnapshot = providers;
      snapshot(providers);
    };
    try {
      await this.withRuntimeLocator(async (locator) => {
        try {
          const subscription = await this.connector.watchProvidersWithReconnect(
            locator,
            this.requireOptions(),
            { signal },
          );
          try {
            publish(subscription.started.snapshot);
            while (!signal.aborted) {
              const notification = await subscription.next();
              if (notification.kind === "changed") {
                publish(notification.changed.snapshot);
              } else if (notification.kind === "usageChanged") {
                if (this.providerWatch === watch) usage(notification.usageChanged.snapshot);
              } else if (notification.kind === "reconnected") {
                publish(notification.started.snapshot);
              } else {
                throw new Error(`the Runtime provider stream ended: ${notification.ended.reason}`);
              }
            }
          } finally {
            subscription.close();
          }
        } catch (error) {
          if (!signal.aborted) throw error;
        }
      });
    } finally {
      if (this.providerWatch === watch) {
        this.providerWatch = null;
        this.providerSnapshot = null;
      }
    }
  }

  async watchSessions(
    snapshot: (sessions: ManagedSessionList) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const watch = Symbol("session watch");
    this.sessionWatch = watch;
    const publish = (sessions: ManagedSessionList): void => {
      if (this.sessionWatch !== watch) return;
      this.sessionSnapshot = sessions;
      snapshot(sessions);
    };
    try {
      await this.withRuntimeLocator(async (locator) => {
        try {
          const subscription = await this.connector.watchSessionIndexWithReconnect(
            locator,
            this.requireOptions(),
            { signal },
          );
          try {
            publish(subscription.started.snapshot);
            while (!signal.aborted) {
              const notification = await subscription.next();
              if (notification.kind === "changed") {
                publish(notification.changed.snapshot);
              } else if (notification.kind === "reconnected") {
                publish(notification.started.snapshot);
              } else {
                throw new Error(`the Runtime session stream ended: ${notification.ended.reason}`);
              }
            }
          } finally {
            subscription.close();
          }
        } catch (error) {
          if (!signal.aborted) throw error;
        }
      });
    } finally {
      if (this.sessionWatch === watch) {
        this.sessionWatch = null;
        this.sessionSnapshot = null;
      }
    }
  }

  /// Follow the hosted-terminal registry of every Runtime generation as one event stream.
  ///
  /// This is the discovery hot path for provider processes launched through a transparent terminal bridge. Process
  /// birth and exit are structural facts and reach the sidebar without waiting for either the activity clock or
  /// the memory sampling clock.
  ///
  /// The generation this window commands is the anchor: losing its stream is losing the Core, and the error says
  /// so. Every other generation the locator lists is followed beside it, because an update leaves the old
  /// generation draining next to the new one for as long as its conversations run, and a conversation's terminal
  /// lives in the exact generation that opened it (`docs/terminalSurface.md`, generation continuity). A row that
  /// cannot see that terminal cannot attach to it, and this window then took the conversation for one running
  /// outside Runtrol (measured 2026-08-29: eight conversations in five draining generations, none openable). A
  /// generation that cannot be followed is named in the merged snapshot's warnings and tried again later; it is
  /// never read as the Core going away.
  async watchTerminals(
    snapshot: (terminals: TerminalIndexSnapshot) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const watch = Symbol("terminal watch");
    this.terminalWatch = watch;
    const fleet = new TerminalFleet();
    const publish = (): void => {
      if (this.terminalWatch === watch) snapshot(fleet.merged());
    };
    await this.withRuntimeLocator(async (anchor) => {
      const others = new AbortController();
      const stopOthers = (): void => others.abort();
      signal.addEventListener("abort", stopOthers, { once: true });
      const following = fleet.followOtherGenerations(
        anchor.digest,
        () => this.listedGenerations(),
        (generation, followingSignal) => this.followGeneration(generation, fleet, publish, followingSignal),
        publish,
        others.signal,
      );
      try {
        // The anchor stream ending is not the Core going away: the grant generation moving (a deploy or a
        // re-enrollment) or this generation draining ends the stream, and the window re-reads the locator and
        // re-subscribes rather than showing an error and going unreachable (operator, 2026-08-29: "the Runtime
        // terminal stream ended: authorityChanged" surfaced during a deploy). Only a revoked integration is a
        // real stop, and a transport error still throws to the outer retry.
        while (!signal.aborted) {
          const runtime = await this.connector.connectWithRetry(anchor, this.requireOptions(), { signal });
          let ended: TerminalStreamEnd;
          try {
            ended = await followTerminalIndex(runtime, anchor.digest, fleet, publish, signal);
          } finally {
            runtime.close();
          }
          if (signal.aborted || ended === null || ended === "integrationRevoked") {
            if (ended === "integrationRevoked") throw new Error("Runtime access was revoked for this window");
            break;
          }
          // authorityChanged or runtimeUnavailable: a beat, then reconnect through the locator.
          await abortableDelay(250, signal);
        }
      } finally {
        signal.removeEventListener("abort", stopOthers);
        others.abort();
        await following;
        if (this.terminalWatch === watch) this.terminalWatch = null;
      }
    });
  }

  /// Read one listed generation's terminal index until that generation ends or the watch stops.
  private async followGeneration(
    generation: ValidatedLocator,
    fleet: TerminalFleet,
    publish: () => void,
    signal: AbortSignal,
  ): Promise<void> {
    const runtime = await this.connector.connect(generation, this.requireOptions(), signal);
    try {
      if (!runtime.initialization.serverCapabilities.terminalSurface) {
        throw new Error("this Runtime generation has no public terminal surface");
      }
      await followTerminalIndex(runtime, generation.digest, fleet, publish, signal);
    } finally {
      runtime.close();
    }
  }

  /// Every generation the locator lists right now, or none while the locator is being rewritten.
  ///
  /// A locator mid-replacement (an update publishing, a generation leaving) reads as malformed for a moment.
  /// That moment is not a fleet of zero: the generations already followed keep streaming, and the next listing
  /// sees the settled file. So a failed read changes nothing and the error is not raised, deliberately.
  private async listedGenerations(): Promise<ReadonlyArray<ValidatedLocator>> {
    try {
      return await this.runtimeLocator().inspectAll();
    } catch {
      return [];
    }
  }

  /// End the provider process behind one hosted terminal, in the exact generation that owns it.
  ///
  /// Stopping needs the terminal's control lease, so this attaches a view, takes control, and stops through it.
  /// The view is closed again at once: this is a sidebar action on a conversation nobody has open here, or one
  /// whose tab will learn of the exit from its own stream.
  async stopTerminal(terminal: TerminalDescriptor): Promise<void> {
    await this.commandClient();
    const view = await this.attachTerminal(terminal.runtimeGeneration, terminal.terminalId);
    try {
      const lease = await view.acquireControl({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        expectedTerminalGeneration: view.opened.terminal.terminalGeneration,
      });
      await view.stop({
        requestId: newMutationRequestId(),
        terminalId: view.opened.terminal.terminalId,
        leaseId: lease.leaseId,
        leaseGeneration: lease.leaseGeneration,
      });
    } finally {
      view.close();
    }
  }

  async setTerminalDialogue(terminal: TerminalDescriptor, enabled: boolean): Promise<void> {
    await this.commandClient();
    const source = await this.locateRuntime();
    const view = await this.attachTerminal(terminal.runtimeGeneration, terminal.terminalId);
    try {
      const guide = enabled ? await readDialogueGuide(source.runtimeExecutable, view.opened.terminal) : null;
      await setTerminalDialogue(view, enabled, guide);
    } finally {
      view.close();
    }
  }

  async watchEvents(
    sessionId: string,
    after: EventCursor | null,
    handlers: RuntimeEventHandlers,
    signal: AbortSignal,
  ): Promise<void> {
    await this.withRuntimeLocator(async (locator) => {
      try {
        const subscription = await this.connector.watchEventsWithReconnect(
          locator,
          this.requireOptions(),
          { sessionId, ...(after ? { after } : {}) },
          { signal },
        );
        try {
          handlers.started();
          if (subscription.started.gap) {
            handlers.gap(subscription.started.startsAt, "The bounded replay window has a gap.");
          }
          while (!signal.aborted) {
            const notification = await subscription.next();
            if (notification.kind === "event") {
              if (!handlers.event(notification.event.event, notification.event.nextExpected)) return;
              subscription.accept(notification.event.nextExpected);
            } else if (notification.kind === "lagged") {
              handlers.gap(
                notification.lagged.nextExpected,
                "The active view fell behind the bounded stream.",
              );
            } else if (notification.started.gap) {
              handlers.gap(
                notification.started.startsAt,
                "The bounded replay window has a gap after reconnecting.",
              );
            }
          }
        } finally {
          subscription.close();
        }
      } catch (error) {
        if (!signal.aborted) throw error;
      }
    });
  }

  async reset(): Promise<void> {
    this.invalidateInventory();
    this.closeActivity();
    await this.serial(async () => {
      this.command?.close();
      this.command = null;
      if (this.options) {
        try {
          await this.commandClient();
        } catch (error) {
          if (!recoverableAuthenticationFailure(error)) throw error;
          await this.replaceIntegration();
        }
      }
    });
  }

  dispose(): void {
    this.closeActivity();
    this.closeMirror();
    this.command?.close();
    this.command = null;
    this.controls.clear();
    this.invalidateInventory();
  }

  private read<T>(operation: (runtime: RuntimeClient) => Promise<T>): Promise<T> {
    return this.serial(async () => {
      const runtime = await this.commandClient();
      try {
        return await operation(runtime);
      } catch (error) {
        // An answered refusal keeps the connection: the Runtime said no to one request, and the window
        // registration and the control this connection holds are still good. Only a failure with no Runtime
        // answer replaces it (measured 2026-09-05: one refused window update closed the connection, the
        // Runtime forgot the window, and the conversation tabs reopened fresh providers of their own).
        if (errorKindOf(error) === undefined) {
          runtime.close();
          this.command = null;
        }
        throw error;
      }
    });
  }

  private activityRead<T>(operation: (runtime: RuntimeClient) => Promise<T>): Promise<T> {
    const action = async (): Promise<T> => {
      const runtime = await this.activityClient();
      try {
        return await operation(runtime);
      } catch (error) {
        runtime.close();
        if (this.activity === runtime) this.activity = null;
        throw error;
      }
    };
    const result = this.activityTail.then(action);
    this.activityTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private mutate<T>(operation: (runtime: RuntimeClient) => Promise<T>): Promise<T> {
    return this.serial(async () => {
      this.sessionSnapshot = null;
      const runtime = await this.commandClient();
      try {
        const result = await operation(runtime);
        this.sessionSnapshot = null;
        return result;
      } catch (error) {
        this.sessionSnapshot = null;
        runtime.close();
        this.command = null;
        throw error;
      }
    });
  }

  /// Ask the provider that owns a stored conversation to archive it through Runtime's relay.
  async archiveNative(native: NativeChatLine): Promise<void> {
    await this.mutate(async (runtime) => {
      await runtime.sessions().archiveNative({
        requestId: newMutationRequestId(),
        providerId: native.providerId,
        nativeSessionId: native.nativeSessionId,
        workspace: native.cwd,
      });
    });
  }

  private async ensureControl(
    runtime: RuntimeClient,
    session: RuntimeSessionAction,
  ): Promise<ControlLease> {
    const current = this.controls.get(session.sessionId);
    const action = cachedControlAction(current, Date.now());
    if (action === "reuse" && current) return current;
    if (action === "renew" && current) {
      const renewed = await runtime.sessions().renewControl({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: current.leaseId,
        leaseGeneration: current.leaseGeneration,
      });
      await this.rememberControl(renewed);
      return renewed;
    }
    await this.forgetControl(session.sessionId);
    const acquired = await runtime.sessions().acquireControl({
      requestId: newMutationRequestId(),
      sessionId: session.sessionId,
      expectedLifecycle: session.lifecycle,
      expectedSessionGeneration: session.generation,
    });
    await this.rememberControl(acquired);
    return acquired;
  }

  private async commandClient(): Promise<RuntimeClient> {
    if (this.command) return this.command;
    const expectedInstance = this.stored?.controlState?.runtimeInstanceId;
    const connected = await this.connectCommand();
    this.command = connected;
    if (
      expectedInstance
      && expectedInstance !== connected.initialization.runtime.instanceId
    ) {
      this.controls.clear();
      await this.persistControls();
    }
    return this.command;
  }

  private async activityClient(): Promise<RuntimeClient> {
    if (this.activity) return this.activity;
    this.activity = await this.withRuntimeLocator(
      (locator) => this.connector.connectWithRetry(locator, this.requireOptions()),
    );
    return this.activity;
  }

  private closeActivity(): void {
    this.activity?.close();
    this.activity = null;
  }

  private async mirrorClient(): Promise<RuntimeClient> {
    if (this.mirror) return this.mirror;
    this.mirror = await this.withRuntimeLocator(
      (locator) => this.connector.connectWithRetry(locator, this.requireOptions()),
    );
    return this.mirror;
  }

  /// The mirror's lane: chunks keep their order among themselves, never queue behind a person's command, and a
  /// failure tears down only this connection. The Runtime ends the mirrors of a connection that goes away, which is
  /// the honest outcome: the feed really has stopped.
  private mirrorLane<T>(operation: (runtime: RuntimeClient) => Promise<T>): Promise<T> {
    const action = async (): Promise<T> => {
      const runtime = await this.mirrorClient();
      try {
        return await operation(runtime);
      } catch (error) {
        runtime.close();
        if (this.mirror === runtime) this.mirror = null;
        throw error;
      }
    };
    const result = this.mirrorTail.then(action);
    this.mirrorTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private closeMirror(): void {
    this.mirror?.close();
    this.mirror = null;
  }

  private async rememberControl(control: ControlLease): Promise<void> {
    this.controls.set(control.sessionId, control);
    await this.persistControls();
  }

  private async forgetControl(sessionId: string): Promise<void> {
    if (!this.controls.delete(sessionId)) return;
    await this.persistControls();
  }

  private async persistControls(): Promise<void> {
    const stored = this.stored;
    const command = this.command;
    if (!stored || !command) return;
    const now = Date.now();
    const leases = [...this.controls.values()].filter((lease) => lease.expiresAtMs > now);
    const next: StoredIntegration = {
      ...stored,
      controlState: {
        runtimeInstanceId: command.initialization.runtime.instanceId,
        leases,
      },
    };
    this.stored = next;
    const previous = this.controlPersistence;
    const persistence = previous
      .catch(() => undefined)
      .then(() => this.context.secrets.store(SECRET_KEY, JSON.stringify(next)));
    this.controlPersistence = persistence;
    await settleControlPersistence(persistence, CONTROL_PERSISTENCE_INLINE_MS);
  }

  private async connectCommand(): Promise<RuntimeClient> {
    const options = this.requireOptions();
    const runtime = await this.withRuntimeLocator(
      (locator) => this.connector.connectWithRetry(locator, options),
    );
    const current = runtime.initialization.grant;
    const credentials = options.credentials;
    if (!current || !credentials) {
      runtime.close();
      throw new Error("Runtrol Studio Runtime authentication returned no integration grant");
    }
    if (JSON.stringify(current) !== JSON.stringify(credentials.grant)) {
      const stored = this.stored;
      if (!stored) {
        runtime.close();
        throw new Error("Runtrol Studio has no integration identity to update");
      }
      const next = { ...stored, grant: current };
      await this.context.secrets.store(SECRET_KEY, JSON.stringify(next));
      this.stored = next;
      this.options = {
        ...options,
        credentials: new IntegrationCredentials(credentials.identity, current),
      };
      this.closeActivity();
    }
    return runtime;
  }

  private requireOptions(): ClientOptions {
    if (!this.options) throw new Error("Runtrol Studio has not initialized its Runtime integration");
    return this.options;
  }

  /// The executable and installed digest the locator inspection is keyed on.
  ///
  /// The warm pass keys on what the probe is expected to settle on; `initialize` keys on what it did settle
  /// on. When the two differ (the installed Core could not answer and the probe fell back to a PATH build), the
  /// early inspection preferred a generation that will never appear, so it is dropped and taken again.
  private async resolveExecutable(source: () => Promise<RuntimeSource>): Promise<void> {
    const { runtimeExecutable, preferDigest } = await source();
    if (this.runtimeExecutable !== runtimeExecutable || this.preferDigest !== preferDigest) {
      this.locator = null;
    }
    this.runtimeExecutable = runtimeExecutable;
    this.preferDigest = preferDigest;
  }

  private withRuntimeLocator<T>(operation: (locator: ValidatedLocator) => Promise<T>): Promise<T> {
    const pending = this.locator ??= inspectRuntimeLocator(this.runtimeExecutable, this.preferDigest);
    return pending.then(operation).catch((error: unknown) => {
      if (this.locator === pending) this.locator = null;
      throw error;
    });
  }

  private serial<T>(action: () => Promise<T>): Promise<T> {
    const result = this.commandTail.then(action);
    this.commandTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private async loadOrCreateIdentity(): Promise<StoredIntegration> {
    const raw = await this.context.secrets.get(SECRET_KEY);
    const stored = parseStoredIntegration(raw);
    if (stored) {
      try {
        IntegrationIdentity.fromPkcs8(Buffer.from(stored.privateKeyPkcs8, "base64url"));
        return stored;
      } catch {
        return this.createIdentity();
      }
    }
    return this.createIdentity();
  }

  private async createIdentity(): Promise<StoredIntegration> {
    const identity = IntegrationIdentity.generate();
    return {
      schema: 1,
      clientInstanceId: randomUUID(),
      privateKeyPkcs8: Buffer.from(identity.exportPkcs8()).toString("base64url"),
    };
  }

  private async useIntegration(stored: StoredIntegration): Promise<void> {
    const identity = IntegrationIdentity.fromPkcs8(
      Buffer.from(stored.privateKeyPkcs8, "base64url"),
    );
    if (!stored.grant) this.reportInitialization("enrollment");
    const grant = stored.grant ?? await this.enroll(stored, identity);
    this.stored = { ...stored, grant };
    this.options = {
      name: "Runtrol Studio",
      version: extensionVersion(this.context),
      credentials: new IntegrationCredentials(identity, grant),
    };
    this.reportInitialization("command");
    this.command = await this.commandClient();
    const controls = restorableControls(
      this.stored?.controlState,
      this.command.initialization.runtime.instanceId,
      Date.now(),
    );
    for (const lease of controls) {
      this.controls.set(lease.sessionId, lease);
    }
  }

  private async replaceIntegration(): Promise<void> {
    this.closeActivity();
    this.command?.close();
    this.command = null;
    this.controls.clear();
    this.options = null;
    this.stored = null;
    this.invalidateInventory();
    await this.useIntegration(await this.createIdentity());
  }

  private invalidateInventory(): void {
    this.providerWatch = null;
    this.sessionWatch = null;
    this.terminalWatch = null;
    this.providerSnapshot = null;
    this.sessionSnapshot = null;
  }

  private async enroll(
    stored: StoredIntegration,
    identity: IntegrationIdentity,
  ): Promise<IntegrationGrant> {
    const options: ClientOptions = {
      name: "Runtrol Studio",
      version: extensionVersion(this.context),
      identity,
    };
    const runtime = await this.withRuntimeLocator(
      (locator) => this.connector.connectWithRetry(locator, options),
    );
    try {
      const roots = [...new Set([
        ...(vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
        ...this.additionalEnrollmentRoots,
      ])];
      const receipt = await runtime.integrations().request({
        clientInstanceId: stored.clientInstanceId,
        manifestDigest: createHash("sha256")
          .update(JSON.stringify({ product: "runtrol-studio", scopes: ALL_STUDIO_SCOPES }))
          .digest(),
        requestedScopes: ALL_STUDIO_SCOPES,
        requestedRoots: roots,
      });
      // Studio approves its own request at once: it is the person's window, and a quarter second spent
      // waiting for somebody else to approve it was a quarter second of every activation (measured 2026-08-27,
      // 250 ms of a 2.3 s activation). A request already decided (a policy, or an earlier attempt) is honoured.
      let decision = await runtime.integrations().watch(receipt.pendingId);
      if (decision.state === "pending") {
        const approved = await this.selfApprove(
          receipt.pendingId,
          identity.signBase64(selfApprovalPayload(receipt.pendingId)),
        );
        if (!approved) {
          throw new Error("Runtrol Studio Runtime access was not approved");
        }
      }
      const decisionDeadline = Math.min(
        receipt.expiresAtMs,
        Date.now() + ENROLLMENT_DECISION_SETTLE_MS,
      );
      while (decision.state === "pending" && Date.now() < decisionDeadline) {
        await new Promise((resolve) => setTimeout(resolve, ENROLLMENT_DECISION_POLL_MS));
        decision = await runtime.integrations().watch(receipt.pendingId);
      }
      if (decision.state !== "approved") {
        throw new Error(`Runtrol Studio Runtime enrollment ended as ${decision.state}`);
      }
      const next = { ...stored, grant: decision.grant };
      await this.context.secrets.store(SECRET_KEY, JSON.stringify(next));
      return decision.grant;
    } finally {
      runtime.close();
    }
  }
}

/// The exact bytes the Core signs over for a self-approval, which must match
/// `self_approval_signing_payload` in `crates/runtrol-runtime-protocol/src/integration.rs` byte for byte.
/// Field order is the Rust struct's field order and both sides emit compact JSON, so the two agree.
function selfApprovalPayload(pendingId: string): Uint8Array {
  return Buffer.from(
    JSON.stringify({ domain: "runtrol-runtime-self-approval-v1", pendingId }),
    "utf8",
  );
}

/// Whether a machine-wide answer came back empty because the question itself was refused.
///
/// Deliberately not matched against the daemon's wording. A sentence duplicated on both sides of a
/// boundary is a contract nothing enforces: the day the refusal is reworded, the fallback stops
/// happening and the sidebar quietly loses conversations with every gate still green. What is
/// checked instead is the only thing that matters here, that nothing was found and something went
/// wrong, and the folder-by-folder attempt that follows costs one more question in the case where
/// the machine genuinely holds no conversations for this service.
function needsPerRootDiscovery(catalogue: NativeChatCatalogue): boolean {
  return catalogue.chats.length === 0 && catalogue.warning !== null;
}

function nativeCatalogueFailure(providerId: string, warning: string): NativeChatCatalogue {
  return {
    providerId,
    coverage: null,
    chats: [],
    loadedAtMs: Date.now(),
    warning,
  };
}

/// Why a terminal stream stopped: the reason the Runtime gave, or null when this window told it to stop.
type TerminalStreamEnd = "integrationRevoked" | "authorityChanged" | "runtimeUnavailable" | null;

/// Read one generation's terminal index into the fleet until the stream ends or the watch is stopped.
///
/// Returns why it ended rather than throwing on it, because most reasons are recoverable and only the caller
/// knows what to do about them: the grant generation moving (`authorityChanged`) or the Runtime going away
/// (`runtimeUnavailable`) mean reconnect for the anchor and stop-following for a draining peer, and neither is
/// a fault to show a person. Only a transport error throws. The generation's fleet entry is removed when this
/// exact stream ends, because a disconnected stream cannot keep proving that its last descriptors are alive.
async function followTerminalIndex(
  runtime: RuntimeClient,
  generation: string,
  fleet: TerminalFleet,
  publish: () => void,
  signal: AbortSignal,
): Promise<TerminalStreamEnd> {
  let subscription: TerminalIndexSubscription | null = null;
  const close = (): void => subscription?.close();
  signal.addEventListener("abort", close, { once: true });
  try {
    subscription = await runtime.terminals().watchIndex();
    fleet.set(generation, subscription.started.snapshot);
    publish();
    while (!signal.aborted) {
      const notification = await subscription.next();
      if (notification.kind === "changed") {
        fleet.set(generation, notification.changed.snapshot);
        publish();
      } else {
        return notification.ended.reason;
      }
    }
    return null;
  } catch (error) {
    // Told to stop: a stream failing because its socket was closed under it is the stop, not a fault.
    if (!signal.aborted) throw error;
    return null;
  } finally {
    signal.removeEventListener("abort", close);
    subscription?.close();
    // A descriptor is proof only while this exact generation's stream is live. Keeping the last snapshot
    // across authorityChanged or runtimeUnavailable made an exited provider process look openable until a
    // later connection happened to replace it.
    fleet.delete(generation);
    publish();
  }
}

async function inspectRuntimeLocator(
  runtimeExecutable: string | null,
  preferDigest: string | null,
): Promise<ValidatedLocator> {
  const locator = RuntimeLocator.system({
    ...(process.platform === "win32" && runtimeExecutable && isAbsolute(runtimeExecutable)
      ? { runtimeExecutable }
      : {}),
    ...(preferDigest ? { preferDigest } : {}),
  });
  const deadline = Date.now() + RUNTIME_LOCATOR_SETTLE_MS;
  while (true) {
    const inspected = await locator.inspect();
    if (inspected.state === "running") {
      // The generation this window installed, when it is serving. Otherwise the newest one that is, which is
      // what the locator already chose and what `runtrol endpoint` itself follows.
      //
      // Waiting for our own digest is right while the locator is still settling and wrong the moment it says
      // our generation is draining: that is settled information, and it happens on every rollback to a build
      // that is still finishing the conversations it started. Insisting then meant a window that could never
      // attach to anything (measured 2026-08-26 by the upgrade journey, which found its own generation listed
      // as draining beside a healthy one and gave up on both).
      if (!preferDigest || inspected.locator.digest === preferDigest || ownGenerationIsDraining(
        await locator.inspectAll().catch(() => []),
        preferDigest,
      )) {
        return inspected.locator;
      }
    }
    if (Date.now() >= deadline) {
      // The window installed one build but a different one is serving, and the settle window passed without our
      // own generation appearing. A running Runtime speaks the version-negotiated public protocol whichever
      // build published it, so a healthy one that is not ours is still a Runtime this window can use: taking it
      // is right, and refusing it stranded the sidebar at "not installed" while a healthy daemon answered
      // (measured 2026-08-28 on the operator machine, four older generations still alive from repeated
      // installs kept a just-installed window from ever seeing its own digest).
      if (inspected.state === "running") {
        return inspected.locator;
      }
      // Nothing is serving at all. Named, because "not installed" fits more than one situation: nothing is
      // published, or only a draining generation is. Whoever reads this next should not have to guess which.
      const listed = await locator.inspectAll().catch(() => []);
      const seen = listed.map((entry) => `${entry.digest.slice(0, 16)}${entry.draining ? " draining" : ""}`);
      throw new Error(
        `Runtrol Runtime is not installed: ${locator.path} lists ${
          seen.length === 0 ? "no generation" : seen.join(", ")
        }${preferDigest ? `, and this window installed ${preferDigest.slice(0, 16)}` : ""}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, RUNTIME_LOCATOR_POLL_MS));
  }
}

/// Whether the build this window installed is listed and draining, which settles the wait for it.
///
/// Absent is not draining: a generation that has not been published yet is still on its way, and that is exactly
/// the case the settle loop exists for.
function ownGenerationIsDraining(
  listed: ReadonlyArray<{ digest: string; draining: boolean }>,
  preferDigest: string,
): boolean {
  return listed.some((entry) => entry.digest === preferDigest && entry.draining);
}

function extensionVersion(context: vscode.ExtensionContext): string {
  const value = (context.extension.packageJSON as { version?: unknown }).version;
  return typeof value === "string" && value.length > 0 ? value : "0.0.0";
}

function parseStoredIntegration(raw: string | undefined): StoredIntegration | null {
  if (!raw || raw.length > 16 * 1024) return null;
  try {
    const value = JSON.parse(raw) as Partial<StoredIntegration>;
    if (
      value.schema !== 1
      || typeof value.clientInstanceId !== "string"
      || value.clientInstanceId.length > 128
      || typeof value.privateKeyPkcs8 !== "string"
      || value.privateKeyPkcs8.length > 512
      || (value.grant !== undefined && !validGrant(value.grant))
      || (value.controlState !== undefined && !validControlState(value.controlState))
    ) {
      return null;
    }
    return value as StoredIntegration;
  } catch {
    return null;
  }
}

function validControlState(value: unknown): value is NonNullable<StoredIntegration["controlState"]> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const state = value as Partial<NonNullable<StoredIntegration["controlState"]>>;
  return typeof state.runtimeInstanceId === "string"
    && state.runtimeInstanceId.length > 0
    && state.runtimeInstanceId.length <= 128
    && Array.isArray(state.leases)
    && state.leases.length <= 32
    && state.leases.every(validControlLease);
}

function validControlLease(value: unknown): value is ControlLease {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const lease = value as Partial<ControlLease>;
  return typeof lease.leaseId === "string"
    && lease.leaseId.length > 0
    && lease.leaseId.length <= 128
    && typeof lease.sessionId === "string"
    && lease.sessionId.length > 0
    && lease.sessionId.length <= 128
    && validUint(lease.sessionGeneration)
    && validUint(lease.leaseGeneration)
    && validUint(lease.expiresAtMs);
}

function validUint(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function validGrant(value: unknown): value is IntegrationGrant {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const grant = value as Partial<IntegrationGrant>;
  return typeof grant.integrationId === "string"
    && Array.isArray(grant.scopes)
    && grant.scopes.every((scope) => ALL_STUDIO_SCOPES.includes(scope))
    && Array.isArray(grant.roots)
    && grant.roots.every((root) => typeof root === "string")
    && Number.isSafeInteger(grant.keyGeneration)
    && Number.isSafeInteger(grant.grantGeneration);
}

function sameBytes(left: readonly number[], right: readonly number[]): boolean {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

/// The public Runtime holding one exact mutation for a decision at this machine (forget, shared open).
function presenceConfirmation(error: unknown): error is RuntimeRequestError {
  return error instanceof RuntimeRequestError
    && error.failure.code === "presenceRequired"
    && error.failure.operatorAction === "reviewRuntimeRequestsInRuntrolStudio";
}

function recoverableAuthenticationFailure(error: unknown): boolean {
  return error instanceof RuntimeRequestError
    && (error.failure.code === "unauthenticated"
      || error.failure.code === "integrationRevoked");
}

function runtimeAction(session: SessionDescriptor): RuntimeSessionAction {
  return {
    sessionId: session.sessionId,
    lifecycle: session.lifecycle,
    generation: session.sessionGeneration,
    workspace: session.workspace,
  };
}

async function waitForIdleSession(
  runtime: RuntimeClient,
  sessionId: string,
): Promise<SessionDescriptor> {
  const deadline = Date.now() + 10_000;
  while (true) {
    const current = await runtime.sessions().get(sessionId);
    if (current.lifecycle !== "hotRunning") return current;
    if (Date.now() >= deadline) {
      throw new Error("the provider did not finish interrupting within 10000 ms");
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}
