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
  type TerminalView,
  type ValidatedLocator,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";

import {
  cachedControlAction,
  restorableControls,
  settleControlPersistence,
  sessionDisappearedAfterCool,
  type StoredControlState,
} from "./runtimeControl";
import { collectNativeChats } from "./nativeChatCatalogue";
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
  private options: ClientOptions | null = null;
  private stored: StoredIntegration | null = null;
  private locator: Promise<ValidatedLocator> | null = null;
  private firstInspection: Promise<ValidatedLocator | null> | null = null;
  private commandTail: Promise<void> = Promise.resolve();
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
  /// On Windows the inspection runs the installed Core once to verify the locator natively, and a Core process
  /// costs a few hundred milliseconds to start (measured 2026-08-27: 250 to 390 ms per spawn on a desktop). Run
  /// from activation, that spawn overlaps the private endpoint probe instead of following it, and `initialize`
  /// finds the answer already in hand. Errors are not reported here: `initialize` re-inspects on its own path and
  /// reports what it finds, so nothing is lost by letting this early attempt fail quietly.
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
    this.reportInitialization("bootstrap");
    const [stored] = await Promise.all([
      this.loadOrCreateIdentity(),
      this.resolveExecutable(this.locateRuntime),
    ]);
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
    return this.read(async (runtime) => {
      // A refresh is also the operator's zero-configuration service discovery trigger. Runtime restamps its local
      // PATH surface and normally answers from the structural provider cache; a newly installed executable changes
      // that stamp and enters the same provider watch without Studio knowing any provider name.
      const nextProviders = await runtime.providers().list();
      const nextSessions = this.sessionSnapshot ?? await runtime.sessions().list();
      if (this.providerWatch) this.providerSnapshot = nextProviders;
      if (this.sessionWatch) this.sessionSnapshot = nextSessions;
      return { providers: nextProviders, sessions: nextSessions };
    });
  }

  /// Every hosted terminal the Runtime lists right now, with what each process holds in memory.
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
    return this.read((runtime) => runtime.providers().nativeActivity(providerId));
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

  /// Follow the daemon's hosted-terminal registry as an event stream.
  ///
  /// This is the discovery hot path for provider processes launched through a transparent terminal bridge. Process
  /// birth and exit are structural facts and reach the sidebar without waiting for either the activity clock or
  /// the memory sampling clock.
  async watchTerminals(
    snapshot: (terminals: TerminalIndexSnapshot) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const watch = Symbol("terminal watch");
    this.terminalWatch = watch;
    await this.withRuntimeLocator(async (locator) => {
      const runtime = await this.connector.connectWithRetry(locator, this.requireOptions(), { signal });
      let subscription: Awaited<ReturnType<ReturnType<RuntimeClient["terminals"]>["watchIndex"]>> | null = null;
      const close = (): void => subscription?.close();
      signal.addEventListener("abort", close, { once: true });
      try {
        subscription = await runtime.terminals().watchIndex();
        if (this.terminalWatch === watch) snapshot(subscription.started.snapshot);
        while (!signal.aborted) {
          const notification = await subscription.next();
          if (notification.kind === "changed") {
            if (this.terminalWatch === watch) snapshot(notification.changed.snapshot);
          } else {
            throw new Error(`the Runtime terminal stream ended: ${notification.ended.reason}`);
          }
        }
      } catch (error) {
        if (!signal.aborted) throw error;
      } finally {
        signal.removeEventListener("abort", close);
        subscription?.close();
        runtime.close();
        if (this.terminalWatch === watch) this.terminalWatch = null;
      }
    });
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
        runtime.close();
        this.command = null;
        throw error;
      }
    });
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
