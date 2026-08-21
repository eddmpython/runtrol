import { createHash, randomUUID } from "node:crypto";
import { isAbsolute } from "node:path";

import {
  IntegrationCredentials,
  IntegrationIdentity,
  RuntimeConnector,
  RuntimeLocator,
  RuntimeRequestError,
  newMutationRequestId,
  type AppScope,
  type ClientOptions,
  type ControlLease,
  type EventCursor,
  type IntegrationGrant,
  type ManagedSessionList,
  type PendingApproval,
  type ProviderList,
  type PublicInputBlock,
  type RuntimeClient,
  type RuntimeModelCatalog,
  type RuntimeProviderCapabilities,
  type SessionDescriptor,
  type SessionWorkspaceAccess,
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
import type { NativeChatCatalogue, NativeChatLine } from "./runtimeTypes";

const SECRET_KEY = "runtrol.runtime.integration.v1";
const ENROLLMENT_PASSIVE_SETTLE_MS = 250;
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
  private commandTail: Promise<void> = Promise.resolve();
  private controlPersistence: Promise<void> = Promise.resolve();
  private readonly controls = new Map<string, ControlLease>();
  private providerSnapshot: ProviderList | null = null;
  private sessionSnapshot: ManagedSessionList | null = null;
  private runtimeExecutable: string | null = null;
  private providerWatch: symbol | null = null;
  private sessionWatch: symbol | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly locateRuntimeExecutable: () => Promise<string>,
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

  async initialize(): Promise<void> {
    this.reportInitialization("bootstrap");
    const [stored, runtimeExecutable] = await Promise.all([
      this.loadOrCreateIdentity(),
      this.locateRuntimeExecutable(),
    ]);
    this.runtimeExecutable = runtimeExecutable;
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

  /// The Studio's own integration identity, or null before enrollment has ever succeeded.
  ///
  /// Only the identity. Roots and generation age the moment the daemon revises the grant, so anything acting on
  /// them reads the daemon's own row instead of a stored copy.
  integrationId(): string | null {
    return this.stored?.grant?.integrationId ?? null;
  }

  async inventory(): Promise<RuntimeInventory> {
    const providers = this.providerSnapshot;
    const sessions = this.sessionSnapshot;
    if (providers && sessions) return { providers, sessions };
    return this.read(async (runtime) => {
      const nextProviders = this.providerSnapshot ?? await runtime.providers().list();
      const nextSessions = this.sessionSnapshot ?? await runtime.sessions().list();
      if (this.providerWatch) this.providerSnapshot = nextProviders;
      if (this.sessionWatch) this.sessionSnapshot = nextSessions;
      return { providers: nextProviders, sessions: nextSessions };
    });
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
  /// action is the click that chose "shared" (Start here anyway, Resume anyway, asking several services at
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

  async watchProviders(
    snapshot: (providers: ProviderList) => void,
    signal: AbortSignal,
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

  private withRuntimeLocator<T>(operation: (locator: ValidatedLocator) => Promise<T>): Promise<T> {
    const pending = this.locator ??= inspectRuntimeLocator(this.runtimeExecutable);
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
      const passiveDeadline = Math.min(
        receipt.expiresAtMs,
        Date.now() + ENROLLMENT_PASSIVE_SETTLE_MS,
      );
      let decision = await runtime.integrations().watch(receipt.pendingId);
      while (decision.state === "pending" && Date.now() < passiveDeadline) {
        await new Promise((resolve) => setTimeout(resolve, ENROLLMENT_DECISION_POLL_MS));
        decision = await runtime.integrations().watch(receipt.pendingId);
      }
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

async function inspectRuntimeLocator(runtimeExecutable: string | null): Promise<ValidatedLocator> {
  const locator = RuntimeLocator.system(
    process.platform === "win32" && runtimeExecutable && isAbsolute(runtimeExecutable)
      ? { runtimeExecutable }
      : {},
  );
  const deadline = Date.now() + RUNTIME_LOCATOR_SETTLE_MS;
  while (true) {
    const inspected = await locator.inspect();
    if (inspected.state === "running") return inspected.locator;
    if (Date.now() >= deadline) {
      throw new Error("Runtrol Runtime is not installed");
    }
    await new Promise((resolve) => setTimeout(resolve, RUNTIME_LOCATOR_POLL_MS));
  }
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
