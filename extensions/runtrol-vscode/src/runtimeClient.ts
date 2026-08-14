import { createHash, randomUUID } from "node:crypto";

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
  type ProviderList,
  type RuntimeClient,
  type RuntimeModelCatalog,
  type SessionDescriptor,
  type SessionWorkspaceAccess,
  type ValidatedLocator,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";

const SECRET_KEY = "runtrol.runtime.integration.v1";
const ENROLLMENT_PASSIVE_SETTLE_MS = 250;
const ENROLLMENT_DECISION_SETTLE_MS = 5_000;
const ENROLLMENT_DECISION_POLL_MS = 50;
const RUNTIME_LOCATOR_SETTLE_MS = 5_000;
const RUNTIME_LOCATOR_POLL_MS = 25;
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
  private readonly controls = new Map<string, ControlLease>();
  private providerSnapshot: ProviderList | null = null;
  private sessionSnapshot: ManagedSessionList | null = null;
  private providerWatch: symbol | null = null;
  private sessionWatch: symbol | null = null;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly ensureRuntimeReady: () => Promise<void>,
    private readonly approveEnrollment: (pendingId: string) => Promise<boolean>,
    private readonly confirmForget: (
      confirmationId: string,
      sessionId: string,
    ) => Promise<boolean>,
  ) {}

  async initialize(): Promise<void> {
    await this.ensureRuntimeReady();
    const stored = await this.loadOrCreateIdentity();
    try {
      await this.useIntegration(stored);
    } catch (error) {
      if (!stored.grant || !recoverableAuthenticationFailure(error)) throw error;
      await this.replaceIntegration();
    }
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

  async start(
    providerId: string,
    workspace: string,
    access: SessionWorkspaceAccess,
    model: string | null,
  ): Promise<SessionDescriptor> {
    return this.mutate(async (runtime) => {
      const opened = await runtime.sessions().start({
        requestId: newMutationRequestId(),
        providerId,
        workspace,
        access,
        ...(model ? { model } : {}),
      });
      this.controls.set(opened.session.sessionId, opened.control);
      return opened.session;
    });
  }

  async resume(session: RuntimeSessionAction, access: SessionWorkspaceAccess): Promise<SessionDescriptor> {
    return this.mutate(async (runtime) => {
      const opened = await runtime.sessions().resume({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        expectedLifecycle: session.lifecycle,
        expectedSessionGeneration: session.generation,
        workspace: session.workspace,
        access,
      });
      this.controls.set(opened.session.sessionId, opened.control);
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
        this.controls.delete(current.sessionId);
        current = await runtime.sessions().get(current.sessionId);
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
        if (!forgetConfirmation(error)) throw error;
        if (!await this.confirmForget(error.failure.correlationId, current.sessionId)) {
          throw new Error("the exact Runtime session removal was not confirmed locally");
        }
        await runtime.sessions().forget(forget);
      }
      this.controls.delete(current.sessionId);
    });
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
      this.controls.clear();
      if (this.options) {
        try {
          this.command = await this.connectCommand();
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
        return await operation(runtime);
      } catch (error) {
        runtime.close();
        this.command = null;
        this.controls.clear();
        throw error;
      }
    });
  }

  private async ensureControl(
    runtime: RuntimeClient,
    session: RuntimeSessionAction,
  ): Promise<ControlLease> {
    const current = this.controls.get(session.sessionId);
    if (
      current
      && current.sessionGeneration === session.generation
      && current.expiresAtMs > Date.now() + 5_000
    ) {
      return current;
    }
    if (
      current
      && current.sessionGeneration === session.generation
      && current.expiresAtMs > Date.now()
    ) {
      const renewed = await runtime.sessions().renewControl({
        requestId: newMutationRequestId(),
        sessionId: session.sessionId,
        leaseId: current.leaseId,
        leaseGeneration: current.leaseGeneration,
      });
      this.controls.set(session.sessionId, renewed);
      return renewed;
    }
    this.controls.delete(session.sessionId);
    const acquired = await runtime.sessions().acquireControl({
      requestId: newMutationRequestId(),
      sessionId: session.sessionId,
      expectedLifecycle: session.lifecycle,
      expectedSessionGeneration: session.generation,
    });
    this.controls.set(session.sessionId, acquired);
    return acquired;
  }

  private async commandClient(): Promise<RuntimeClient> {
    this.command ??= await this.connectCommand();
    return this.command;
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
    const pending = this.locator ??= inspectRuntimeLocator();
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
    const stored = parseStoredIntegration(await this.context.secrets.get(SECRET_KEY));
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
    const created: StoredIntegration = {
      schema: 1,
      clientInstanceId: randomUUID(),
      privateKeyPkcs8: Buffer.from(identity.exportPkcs8()).toString("base64url"),
    };
    await this.context.secrets.store(SECRET_KEY, JSON.stringify(created));
    return created;
  }

  private async useIntegration(stored: StoredIntegration): Promise<void> {
    const identity = IntegrationIdentity.fromPkcs8(
      Buffer.from(stored.privateKeyPkcs8, "base64url"),
    );
    const grant = stored.grant ?? await this.enroll(stored, identity);
    this.stored = { ...stored, grant };
    this.options = {
      name: "Runtrol Studio",
      version: extensionVersion(this.context),
      credentials: new IntegrationCredentials(identity, grant),
    };
    this.command = await this.connectCommand();
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
      const roots = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
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
      if (decision.state === "pending" && !await this.approveEnrollment(receipt.pendingId)) {
        throw new Error("Runtrol Studio Runtime access was not approved");
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

async function inspectRuntimeLocator(): Promise<ValidatedLocator> {
  const locator = RuntimeLocator.system();
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
    ) {
      return null;
    }
    return value as StoredIntegration;
  } catch {
    return null;
  }
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

function forgetConfirmation(error: unknown): error is RuntimeRequestError {
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
