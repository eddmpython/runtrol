import { createHash, randomUUID } from "node:crypto";

import {
  IntegrationCredentials,
  IntegrationIdentity,
  RuntimeConnector,
  type AppScope,
  type ClientOptions,
  type EventCursor,
  type IntegrationGrant,
  type ManagedSessionList,
  type ProviderList,
  type RuntimeClient,
  type RuntimeModelCatalog,
} from "@runtrol/runtime-client";
import * as vscode from "vscode";

const SECRET_KEY = "runtrol.runtime.integration.v1";
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

export class StudioRuntimeClient implements vscode.Disposable {
  private readonly connector = new RuntimeConnector();
  private command: RuntimeClient | null = null;
  private options: ClientOptions | null = null;
  private commandTail: Promise<void> = Promise.resolve();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly ensureRuntimeReady: () => Promise<void>,
    private readonly approveEnrollment: (pendingId: string) => Promise<boolean>,
  ) {}

  async initialize(): Promise<void> {
    await this.ensureRuntimeReady();
    const stored = await this.loadOrCreateIdentity();
    const identity = IntegrationIdentity.fromPkcs8(Buffer.from(stored.privateKeyPkcs8, "base64url"));
    const grant = stored.grant ?? await this.enroll(stored, identity);
    this.options = {
      name: "Runtrol Studio",
      version: extensionVersion(this.context),
      credentials: new IntegrationCredentials(identity, grant),
    };
    this.command = await this.connector.connectSystemWithRetry(this.options);
  }

  async inventory(): Promise<RuntimeInventory> {
    return this.read(async (runtime) => ({
      providers: await runtime.providers().list(),
      sessions: await runtime.sessions().list(),
    }));
  }

  async models(providerId: string): Promise<RuntimeModelCatalog> {
    return this.read((runtime) => runtime.providers().listModels(providerId));
  }

  async watchProviders(
    snapshot: (providers: ProviderList) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const subscription = await this.connector.watchProvidersWithReconnectSystem(
      this.requireOptions(),
      { signal },
    );
    try {
      snapshot(subscription.started.snapshot);
      while (!signal.aborted) {
        const notification = await subscription.next();
        if (notification.kind === "changed") {
          snapshot(notification.changed.snapshot);
        } else if (notification.kind === "reconnected") {
          snapshot(notification.started.snapshot);
        } else {
          throw new Error(`the Runtime provider stream ended: ${notification.ended.reason}`);
        }
      }
    } catch (error) {
      if (!signal.aborted) throw error;
    } finally {
      subscription.close();
    }
  }

  async watchSessions(
    snapshot: (sessions: ManagedSessionList) => void,
    signal: AbortSignal,
  ): Promise<void> {
    const subscription = await this.connector.watchSessionIndexWithReconnectSystem(
      this.requireOptions(),
      { signal },
    );
    try {
      snapshot(subscription.started.snapshot);
      while (!signal.aborted) {
        const notification = await subscription.next();
        if (notification.kind === "changed") {
          snapshot(notification.changed.snapshot);
        } else if (notification.kind === "reconnected") {
          snapshot(notification.started.snapshot);
        } else {
          throw new Error(`the Runtime session stream ended: ${notification.ended.reason}`);
        }
      }
    } catch (error) {
      if (!signal.aborted) throw error;
    } finally {
      subscription.close();
    }
  }

  async watchEvents(
    sessionId: string,
    after: EventCursor | null,
    handlers: RuntimeEventHandlers,
    signal: AbortSignal,
  ): Promise<void> {
    const subscription = await this.connector.watchEventsWithReconnectSystem(
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
    } catch (error) {
      if (!signal.aborted) throw error;
    } finally {
      subscription.close();
    }
  }

  async reset(): Promise<void> {
    await this.serial(async () => {
      this.command?.close();
      this.command = null;
      if (this.options) {
        this.command = await this.connector.connectSystemWithRetry(this.options);
      }
    });
  }

  dispose(): void {
    this.command?.close();
    this.command = null;
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

  private async commandClient(): Promise<RuntimeClient> {
    this.command ??= await this.connector.connectSystemWithRetry(this.requireOptions());
    return this.command;
  }

  private requireOptions(): ClientOptions {
    if (!this.options) throw new Error("Runtrol Studio has not initialized its Runtime integration");
    return this.options;
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
    if (stored) return stored;
    const identity = IntegrationIdentity.generate();
    const created: StoredIntegration = {
      schema: 1,
      clientInstanceId: randomUUID(),
      privateKeyPkcs8: Buffer.from(identity.exportPkcs8()).toString("base64url"),
    };
    await this.context.secrets.store(SECRET_KEY, JSON.stringify(created));
    return created;
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
    const runtime = await this.connector.connectSystemWithRetry(options);
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
      if (!await this.approveEnrollment(receipt.pendingId)) {
        throw new Error("Runtrol Studio Runtime access was not approved");
      }
      const decision = await runtime.integrations().watch(receipt.pendingId);
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
