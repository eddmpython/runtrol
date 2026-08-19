import { randomBytes } from "node:crypto";

import type {
  AcquireControlParams,
  AdoptNativeSessionParams,
  AppScope,
  ClientCapabilities,
  ClientInfo,
  ControlLease,
  ControlLeaseParams,
  CoolSessionParams,
  EnrollmentDecision,
  EnrollmentManifest,
  EnrollmentReceipt,
  EventCursor,
  ForgetSessionParams,
  GetProviderCapabilitiesParams,
  GetSessionParams,
  InitializeParams,
  InitializeResult,
  IntegrationAuthentication,
  IntegrationGrant,
  JsonRpcNotification,
  JsonRpcResponse,
  LaggedNotification,
  ListModelsParams,
  ListNativeSessionsParams,
  ListPendingApprovalsParams,
  ManagedSessionList,
  MutationRequestId,
  NativeSessionCatalogue,
  PendingEnrollmentId,
  PendingApprovalList,
  ProviderId,
  ProviderList,
  ProviderUsageList,
  ProviderWatchEndedNotification,
  ProvidersChangedNotification,
  RequestEnrollmentParams,
  RespondApprovalParams,
  ResumeSessionParams,
  RotateIntegrationKeyParams,
  RuntimeEventNotification,
  RuntimeMethod,
  RuntimeModelCatalog,
  RuntimeProviderCapabilities,
  RuntimeSessionId,
  ServerChallenge,
  SessionDescriptor,
  SessionIndexChangedNotification,
  SessionIndexEndedNotification,
  SessionOpenResult,
  StartSessionParams,
  SubmitInputParams,
  WatchEventsParams,
  WatchEventsResult,
  WatchProvidersResult,
  WatchSessionIndexResult,
} from "./generated/protocol.js";
import { FINALIZED_REVISIONS, PUBLIC_LIMITS } from "./generated/protocol.js";
import {
  RuntimeLocatorError,
  RuntimeProtocolError,
  RuntimeRequestError,
  RuntimeTransportError,
} from "./errors.js";
import { IntegrationCredentials, IntegrationIdentity } from "./identity.js";
import { RuntimeLocator, type ValidatedLocator } from "./locator.js";
import { requireEmpty, validatePublic } from "./schema.js";
import {
  connectLocalTransport,
  type RuntimeTransport,
  type RuntimeTransportFactory,
} from "./transport.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const CHALLENGE_CLOCK_SKEW_TOLERANCE_MS = 5_000;

export function newMutationRequestId(): MutationRequestId {
  const bytes = randomBytes(16);
  let timestamp = Date.now();
  for (let index = 5; index >= 0; index -= 1) {
    bytes[index] = timestamp & 0xff;
    timestamp = Math.floor(timestamp / 256);
  }
  bytes[6] = 0x70 | (bytes[6]! & 0x0f);
  bytes[8] = 0x80 | (bytes[8]! & 0x3f);
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export interface ClientOptions {
  readonly name: string;
  readonly version: string;
  readonly identity?: IntegrationIdentity;
  readonly credentials?: IntegrationCredentials;
  readonly capabilities?: ClientCapabilities;
}

export interface EnrollmentProposal {
  readonly clientInstanceId: string;
  readonly manifestDigest: Uint8Array;
  readonly requestedScopes: ReadonlyArray<AppScope>;
  readonly requestedRoots: ReadonlyArray<string>;
}

export interface ReconnectPolicy {
  readonly initialDelayMs?: number;
  readonly maximumDelayMs?: number;
  readonly deadlineMs?: number;
  readonly signal?: AbortSignal;
}

interface RuntimeClientState {
  readonly capabilities: ClientCapabilities;
  readonly challenge: ServerChallenge;
  readonly clientInfo: ClientInfo;
  readonly identity?: IntegrationIdentity;
  readonly supportedRevisions: ReadonlyArray<string>;
  readonly transport: RuntimeTransport;
  nextId: number;
  streaming: boolean;
}

const runtimeClientToken = Symbol("initialized Runtime client");
const runtimeStates = new WeakMap<RuntimeClient, RuntimeClientState>();

export class RuntimeConnector {
  public constructor(
    private readonly transportFactory: RuntimeTransportFactory = connectLocalTransport,
  ) {}

  public async connect(
    locator: ValidatedLocator,
    options: ClientOptions,
    signal?: AbortSignal,
  ): Promise<RuntimeClient> {
    locator.assertSdkValidated();
    const transport = await this.transportFactory(locator.endpoint, signal);
    const abort = (): void => abortTransport(transport);
    signal?.addEventListener("abort", abort, { once: true });
    try {
      signal?.throwIfAborted();
      return await initializeRuntime(transport, locator, options);
    } catch (error) {
      transport.close();
      throw error;
    } finally {
      signal?.removeEventListener("abort", abort);
    }
  }

  public async connectSystem(options: ClientOptions, signal?: AbortSignal): Promise<RuntimeClient> {
    const state = await RuntimeLocator.system().inspect();
    if (state.state === "notInstalled") {
      throw new RuntimeRequestError({
        code: "runtimeNotInstalled",
        message: "Runtrol Runtime is not installed",
        retryable: false,
        correlationId: "local-locator",
      });
    }
    return this.connect(state.locator, options, signal);
  }

  public connectWithRetry(
    locator: ValidatedLocator,
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<RuntimeClient> {
    return retryConnection((signal) => this.connect(locator, options, signal), policy);
  }

  public connectSystemWithRetry(
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<RuntimeClient> {
    return retryConnection((signal) => this.connectSystem(options, signal), policy);
  }

  public async watchEventsWithReconnect(
    locator: ValidatedLocator,
    options: ClientOptions,
    params: WatchEventsParams,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingEventSubscription> {
    const subscription = new ReconnectingEventSubscription(
      this,
      locator,
      options,
      params,
      policy,
    );
    await subscription.initialize();
    return subscription;
  }

  public async watchEventsWithReconnectSystem(
    options: ClientOptions,
    params: WatchEventsParams,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingEventSubscription> {
    const subscription = new ReconnectingEventSubscription(
      this,
      null,
      options,
      params,
      policy,
    );
    await subscription.initialize();
    return subscription;
  }

  public async watchProvidersWithReconnect(
    locator: ValidatedLocator,
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingProviderSubscription> {
    const subscription = new ReconnectingProviderSubscription(
      this,
      locator,
      options,
      policy,
    );
    await subscription.initialize();
    return subscription;
  }

  public async watchProvidersWithReconnectSystem(
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingProviderSubscription> {
    const subscription = new ReconnectingProviderSubscription(this, null, options, policy);
    await subscription.initialize();
    return subscription;
  }

  public async watchSessionIndexWithReconnect(
    locator: ValidatedLocator,
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingSessionIndexSubscription> {
    const subscription = new ReconnectingSessionIndexSubscription(
      this,
      locator,
      options,
      policy,
    );
    await subscription.initialize();
    return subscription;
  }

  public async watchSessionIndexWithReconnectSystem(
    options: ClientOptions,
    policy: ReconnectPolicy = {},
  ): Promise<ReconnectingSessionIndexSubscription> {
    const subscription = new ReconnectingSessionIndexSubscription(this, null, options, policy);
    await subscription.initialize();
    return subscription;
  }
}

export class RuntimeClient {
  public constructor(
    token: typeof runtimeClientToken,
    public readonly initialization: InitializeResult,
    state: RuntimeClientState,
  ) {
    if (token !== runtimeClientToken) {
      throw new RuntimeProtocolError("RuntimeClient was not initialized by this SDK");
    }
    runtimeStates.set(this, state);
  }

  public integrations(): IntegrationClient {
    return new IntegrationClient(this);
  }

  public providers(): ProviderClient {
    return new ProviderClient(this);
  }

  public sessions(): SessionClient {
    return new SessionClient(this);
  }

  public approvals(): ApprovalClient {
    return new ApprovalClient(this);
  }

  public credentials(grant: IntegrationGrant): IntegrationCredentials {
    const identity = runtimeState(this).identity;
    if (!identity) throw new RuntimeProtocolError("connection has no integration identity");
    return new IntegrationCredentials(identity, grant);
  }

  public async panicStop(): Promise<void> {
    requireEmpty(await callRuntime(this, "runtime/panicStop", {}, undefined));
  }

  public close(): void {
    runtimeState(this).transport.close();
  }
}

export class IntegrationClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public async request(proposal: EnrollmentProposal): Promise<EnrollmentReceipt> {
    if (proposal.manifestDigest.byteLength !== 32) {
      throw new RuntimeProtocolError("enrollment manifest digest must be exactly 32 bytes");
    }
    const identity = signingIdentity(this.runtime);
    const manifest: EnrollmentManifest = {
      clientInstanceId: proposal.clientInstanceId,
      publicKey: identity.publicKeyBase64(),
      manifestDigest: Buffer.from(proposal.manifestDigest).toString("base64url"),
      requestedScopes: proposal.requestedScopes,
      requestedRoots: proposal.requestedRoots,
    };
    const params: RequestEnrollmentParams = {
      manifest,
      signature: identity.signBase64(enrollmentPayload(this.runtime, manifest)),
    };
    return callRuntime(this.runtime, "integrations/requestEnrollment", params, "EnrollmentReceipt");
  }

  public watch(pendingId: PendingEnrollmentId): Promise<EnrollmentDecision> {
    return callRuntime(
      this.runtime,
      "integrations/watchEnrollment",
      { pendingId },
      "EnrollmentDecision",
    );
  }

  public grant(): Promise<IntegrationGrant> {
    return callRuntime(this.runtime, "integrations/getGrant", {}, "IntegrationGrant");
  }

  public async rotateKey(
    requestId: MutationRequestId,
    expectedKeyGeneration: number,
    replacement: IntegrationIdentity,
  ): Promise<IntegrationCredentials> {
    const grant = this.runtime.initialization.grant;
    if (!grant) {
      throw new RuntimeProtocolError("integration key rotation requires an authenticated grant");
    }
    const unsigned: RotateIntegrationKeyParams = {
      requestId,
      expectedKeyGeneration,
      newPublicKey: replacement.publicKeyBase64(),
      newKeyProof: "",
    };
    const params: RotateIntegrationKeyParams = {
      ...unsigned,
      newKeyProof: replacement.signBase64(keyRotationSigningPayload(grant, unsigned)),
    };
    const rotated = await callMutation<IntegrationGrant>(
      this.runtime,
      "integrations/rotateKey",
      params,
      "IntegrationGrant",
    );
    return new IntegrationCredentials(replacement, rotated);
  }
}

export class ProviderClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public list(): Promise<ProviderList> {
    return callRuntime(this.runtime, "providers/list", {}, "ProviderList");
  }

  /** Where each account stands against its limits, by each provider's own latest report. */
  public usage(): Promise<ProviderUsageList> {
    return callRuntime(this.runtime, "providers/usage", {}, "ProviderUsageList");
  }

  public async watch(): Promise<ProviderSubscription> {
    const started = await callRuntime<WatchProvidersResult>(
      this.runtime,
      "providers/watch",
      {},
      "WatchProvidersResult",
    );
    return new ProviderSubscription(beginStream(this.runtime), started);
  }

  public getCapabilities(providerId: ProviderId): Promise<RuntimeProviderCapabilities> {
    const params: GetProviderCapabilitiesParams = { providerId };
    return callRuntime(
      this.runtime,
      "providers/getCapabilities",
      params,
      "RuntimeProviderCapabilities",
    );
  }

  public listModels(providerId: ProviderId): Promise<RuntimeModelCatalog> {
    const params: ListModelsParams = { providerId };
    return callRuntime(this.runtime, "providers/listModels", params, "RuntimeModelCatalog");
  }

  public listNativeSessions(params: ListNativeSessionsParams): Promise<NativeSessionCatalogue> {
    return callRuntime(
      this.runtime,
      "providers/listNativeSessions",
      params,
      "NativeSessionCatalogue",
    );
  }
}

export type ProviderNotification =
  | { readonly kind: "changed"; readonly changed: ProvidersChangedNotification }
  | { readonly kind: "ended"; readonly ended: ProviderWatchEndedNotification };

export type ReconnectingProviderNotification =
  | ProviderNotification
  | { readonly kind: "reconnected"; readonly started: WatchProvidersResult };

export class ProviderSubscription {
  public constructor(
    private readonly transport: RuntimeTransport,
    public readonly started: WatchProvidersResult,
  ) {}

  public async next(): Promise<ProviderNotification> {
    const decoded = decodeJson(await this.transport.receive());
    const notification = validatePublic<JsonRpcNotification>("JsonRpcNotification", decoded);
    if (notification.jsonrpc !== "2.0") {
      throw new RuntimeProtocolError("provider notification JSON-RPC version is not 2.0");
    }
    if (notification.method === "providers/changed") {
      const changed = validatePublic<ProvidersChangedNotification>(
        "ProvidersChangedNotification",
        notification.params,
      );
      this.validateTarget(changed.subscriptionId);
      return { kind: "changed", changed };
    }
    if (notification.method === "providers/watchEnded") {
      const ended = validatePublic<ProviderWatchEndedNotification>(
        "ProviderWatchEndedNotification",
        notification.params,
      );
      this.validateTarget(ended.subscriptionId);
      return { kind: "ended", ended };
    }
    throw new RuntimeProtocolError("dedicated provider stream received a different method");
  }

  public close(): void {
    this.transport.close();
  }

  private validateTarget(subscriptionId: string): void {
    if (subscriptionId !== this.started.subscriptionId) {
      throw new RuntimeProtocolError("provider notification target does not match its subscription");
    }
  }
}

export class ReconnectingProviderSubscription {
  readonly #abort = new AbortController();
  readonly #policy: ReconnectPolicy;
  #current: { runtime: RuntimeClient; subscription: ProviderSubscription } | null = null;
  #started: WatchProvidersResult | null = null;
  #terminal = false;

  public constructor(
    private readonly connector: RuntimeConnector,
    private readonly locator: ValidatedLocator | null,
    private readonly options: ClientOptions,
    policy: ReconnectPolicy,
  ) {
    const signal = policy.signal
      ? AbortSignal.any([policy.signal, this.#abort.signal])
      : this.#abort.signal;
    this.#policy = { ...policy, signal };
  }

  public async initialize(): Promise<void> {
    await this.#open();
  }

  public get started(): WatchProvidersResult {
    if (!this.#started) throw new RuntimeProtocolError("provider stream is not initialized");
    return this.#started;
  }

  public async next(): Promise<ReconnectingProviderNotification> {
    if (this.#terminal) throw new RuntimeProtocolError("provider subscription already ended");
    if (!this.#current) {
      const started = await this.#open();
      return { kind: "reconnected", started };
    }
    try {
      const notification = await this.#current.subscription.next();
      if (notification.kind === "ended") {
        this.#terminal = true;
        this.#closeCurrent();
      }
      return notification;
    } catch (error) {
      this.#closeCurrent();
      if (!retryableConnectionFailure(error)) throw error;
      const started = await this.#open();
      return { kind: "reconnected", started };
    }
  }

  public close(): void {
    this.#abort.abort(new RuntimeTransportError("reconnecting provider stream was closed"));
    this.#closeCurrent();
  }

  async #open(): Promise<WatchProvidersResult> {
    const opened = await retryConnection(
      (signal) => openRuntimeSubscription(
        () => connectSelected(this.connector, this.locator, this.options, signal),
        (runtime) => runtime.providers().watch(),
        signal,
      ),
      this.#policy,
    );
    this.#current = opened;
    this.#started = opened.subscription.started;
    return opened.subscription.started;
  }

  #closeCurrent(): void {
    this.#current?.subscription.close();
    this.#current = null;
  }
}

export class SessionClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public list(): Promise<ManagedSessionList> {
    return callRuntime(this.runtime, "sessions/list", {}, "ManagedSessionList");
  }

  public get(sessionId: RuntimeSessionId): Promise<SessionDescriptor> {
    const params: GetSessionParams = { sessionId };
    return callRuntime(this.runtime, "sessions/get", params, "SessionDescriptor");
  }

  public async watchIndex(): Promise<SessionIndexSubscription> {
    const started = await callRuntime<WatchSessionIndexResult>(
      this.runtime,
      "sessions/watchIndex",
      {},
      "WatchSessionIndexResult",
    );
    return new SessionIndexSubscription(beginStream(this.runtime), started);
  }

  public start(params: StartSessionParams): Promise<SessionOpenResult> {
    if (
      params.model !== undefined
      && params.model !== null
      && Buffer.byteLength(params.model, "utf8") > PUBLIC_LIMITS.maxModelSelectionBytes
    ) {
      throw new RuntimeProtocolError("session model selection exceeds the public byte limit");
    }
    return callMutation(this.runtime, "sessions/start", params, "SessionOpenResult");
  }

  public adoptNative(params: AdoptNativeSessionParams): Promise<SessionOpenResult> {
    if (Buffer.byteLength(params.adoptionToken, "utf8") > PUBLIC_LIMITS.maxNativeAdoptionTokenBytes) {
      throw new RuntimeProtocolError("native adoption proof exceeds the public byte limit");
    }
    return callMutation(this.runtime, "sessions/adoptNative", params, "SessionOpenResult");
  }

  public resume(params: ResumeSessionParams): Promise<SessionOpenResult> {
    return callMutation(this.runtime, "sessions/resume", params, "SessionOpenResult");
  }

  public acquireControl(params: AcquireControlParams): Promise<ControlLease> {
    return callMutation(this.runtime, "sessions/acquireControl", params, "ControlLease");
  }

  public renewControl(params: ControlLeaseParams): Promise<ControlLease> {
    return callMutation(this.runtime, "sessions/renewControl", params, "ControlLease");
  }

  public async releaseControl(params: ControlLeaseParams): Promise<void> {
    requireEmpty(await callMutation(this.runtime, "sessions/releaseControl", params, undefined));
  }

  public async submitInput(params: SubmitInputParams): Promise<void> {
    if (Buffer.byteLength(params.input, "utf8") > PUBLIC_LIMITS.maxInputBytes) {
      throw new RuntimeProtocolError("session input exceeds the public byte limit");
    }
    requireEmpty(await callMutation(this.runtime, "sessions/submitInput", params, undefined));
  }

  public async interrupt(params: ControlLeaseParams): Promise<void> {
    requireEmpty(await callMutation(this.runtime, "sessions/interrupt", params, undefined));
  }

  public async cool(params: CoolSessionParams): Promise<void> {
    requireEmpty(await callMutation(this.runtime, "sessions/cool", params, undefined));
  }

  public async forget(params: ForgetSessionParams): Promise<void> {
    requireEmpty(await callMutation(this.runtime, "sessions/forget", params, undefined));
  }

  public async watchEvents(params: WatchEventsParams): Promise<EventSubscription> {
    const started = await callRuntime<WatchEventsResult>(
      this.runtime,
      "sessions/watchEvents",
      params,
      "WatchEventsResult",
    );
    return new EventSubscription(beginStream(this.runtime), started);
  }
}

export class ApprovalClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public listPending(params: ListPendingApprovalsParams): Promise<PendingApprovalList> {
    return callRuntime(
      this.runtime,
      "approvals/listPending",
      params,
      "PendingApprovalList",
    );
  }

  public async respond(params: RespondApprovalParams): Promise<void> {
    if (params.subjectDigest.length !== 32) {
      throw new RuntimeProtocolError("approval subject digest must be exactly 32 bytes");
    }
    requireEmpty(await callMutation(this.runtime, "approvals/respond", params, undefined));
  }
}

export type SessionIndexNotification =
  | { readonly kind: "changed"; readonly changed: SessionIndexChangedNotification }
  | { readonly kind: "ended"; readonly ended: SessionIndexEndedNotification };

export type ReconnectingSessionIndexNotification =
  | SessionIndexNotification
  | { readonly kind: "reconnected"; readonly started: WatchSessionIndexResult };

export class SessionIndexSubscription {
  public constructor(
    private readonly transport: RuntimeTransport,
    public readonly started: WatchSessionIndexResult,
  ) {}

  public async next(): Promise<SessionIndexNotification> {
    const decoded = decodeJson(await this.transport.receive());
    const notification = validatePublic<JsonRpcNotification>("JsonRpcNotification", decoded);
    if (notification.jsonrpc !== "2.0") {
      throw new RuntimeProtocolError("session index notification JSON-RPC version is not 2.0");
    }
    if (notification.method === "sessions/indexChanged") {
      const changed = validatePublic<SessionIndexChangedNotification>(
        "SessionIndexChangedNotification",
        notification.params,
      );
      this.validateTarget(changed.subscriptionId);
      return { kind: "changed", changed };
    }
    if (notification.method === "sessions/indexEnded") {
      const ended = validatePublic<SessionIndexEndedNotification>(
        "SessionIndexEndedNotification",
        notification.params,
      );
      this.validateTarget(ended.subscriptionId);
      return { kind: "ended", ended };
    }
    throw new RuntimeProtocolError("dedicated session index stream received a different method");
  }

  public close(): void {
    this.transport.close();
  }

  private validateTarget(subscriptionId: string): void {
    if (subscriptionId !== this.started.subscriptionId) {
      throw new RuntimeProtocolError("session index notification target does not match its subscription");
    }
  }
}

export class ReconnectingSessionIndexSubscription {
  readonly #abort = new AbortController();
  readonly #policy: ReconnectPolicy;
  #current: { runtime: RuntimeClient; subscription: SessionIndexSubscription } | null = null;
  #started: WatchSessionIndexResult | null = null;
  #terminal = false;

  public constructor(
    private readonly connector: RuntimeConnector,
    private readonly locator: ValidatedLocator | null,
    private readonly options: ClientOptions,
    policy: ReconnectPolicy,
  ) {
    const signal = policy.signal
      ? AbortSignal.any([policy.signal, this.#abort.signal])
      : this.#abort.signal;
    this.#policy = { ...policy, signal };
  }

  public async initialize(): Promise<void> {
    await this.#open();
  }

  public get started(): WatchSessionIndexResult {
    if (!this.#started) throw new RuntimeProtocolError("session index stream is not initialized");
    return this.#started;
  }

  public async next(): Promise<ReconnectingSessionIndexNotification> {
    if (this.#terminal) throw new RuntimeProtocolError("session index subscription already ended");
    if (!this.#current) {
      const started = await this.#open();
      return { kind: "reconnected", started };
    }
    try {
      const notification = await this.#current.subscription.next();
      if (notification.kind === "ended") {
        this.#terminal = true;
        this.#closeCurrent();
      }
      return notification;
    } catch (error) {
      this.#closeCurrent();
      if (!retryableConnectionFailure(error)) throw error;
      const started = await this.#open();
      return { kind: "reconnected", started };
    }
  }

  public close(): void {
    this.#abort.abort(new RuntimeTransportError("reconnecting session index was closed"));
    this.#closeCurrent();
  }

  async #open(): Promise<WatchSessionIndexResult> {
    const opened = await retryConnection(
      (signal) => openRuntimeSubscription(
        () => connectSelected(this.connector, this.locator, this.options, signal),
        (runtime) => runtime.sessions().watchIndex(),
        signal,
      ),
      this.#policy,
    );
    this.#current = opened;
    this.#started = opened.subscription.started;
    return opened.subscription.started;
  }

  #closeCurrent(): void {
    this.#current?.subscription.close();
    this.#current = null;
  }
}

export type SessionNotification =
  | { readonly kind: "event"; readonly event: RuntimeEventNotification }
  | { readonly kind: "lagged"; readonly lagged: LaggedNotification };

export type ReconnectingSessionNotification =
  | SessionNotification
  | { readonly kind: "reconnected"; readonly started: WatchEventsResult };

export class EventSubscription {
  public constructor(
    private readonly transport: RuntimeTransport,
    public readonly started: WatchEventsResult,
  ) {}

  public async next(): Promise<SessionNotification> {
    const decoded = decodeJson(await this.transport.receive());
    const notification = validatePublic<JsonRpcNotification>("JsonRpcNotification", decoded);
    if (notification.jsonrpc !== "2.0") {
      throw new RuntimeProtocolError("session notification JSON-RPC version is not 2.0");
    }
    if (notification.method === "sessions/event") {
      const event = validatePublic<RuntimeEventNotification>(
        "RuntimeEventNotification",
        notification.params,
      );
      this.validateTarget(event.subscriptionId, event.sessionId);
      return { kind: "event", event };
    }
    if (notification.method === "sessions/lagged") {
      const lagged = validatePublic<LaggedNotification>("LaggedNotification", notification.params);
      this.validateTarget(lagged.subscriptionId, lagged.sessionId);
      return { kind: "lagged", lagged };
    }
    throw new RuntimeProtocolError("dedicated session stream received a non-event method");
  }

  public close(): void {
    this.transport.close();
  }

  private validateTarget(subscriptionId: string, sessionId: string): void {
    if (subscriptionId !== this.started.subscriptionId || sessionId !== this.started.sessionId) {
      throw new RuntimeProtocolError("session notification target does not match its subscription");
    }
  }
}

export class ReconnectingEventSubscription {
  readonly #abort = new AbortController();
  readonly #policy: ReconnectPolicy;
  #accepted: EventCursor | null;
  #current: { runtime: RuntimeClient; subscription: EventSubscription } | null = null;
  #pending: EventCursor | null = null;
  #started: WatchEventsResult | null = null;

  public constructor(
    private readonly connector: RuntimeConnector,
    private readonly locator: ValidatedLocator | null,
    private readonly options: ClientOptions,
    private readonly params: WatchEventsParams,
    policy: ReconnectPolicy,
  ) {
    const signal = policy.signal
      ? AbortSignal.any([policy.signal, this.#abort.signal])
      : this.#abort.signal;
    this.#policy = { ...policy, signal };
    this.#accepted = params.after ? copyCursor(params.after) : null;
  }

  public async initialize(): Promise<void> {
    await this.#open();
  }

  public get started(): WatchEventsResult {
    if (!this.#started) throw new RuntimeProtocolError("reconnecting event stream is not initialized");
    return this.#started;
  }

  public accept(nextExpected: EventCursor): void {
    if (!this.#pending || !sameCursor(this.#pending, nextExpected)) {
      throw new RuntimeProtocolError("accepted event cursor does not match the pending event");
    }
    this.#accepted = copyCursor(nextExpected);
    this.#pending = null;
  }

  public async next(): Promise<ReconnectingSessionNotification> {
    if (this.#pending) {
      throw new RuntimeProtocolError("accept the current event before reading another one");
    }
    for (;;) {
      this.#policy.signal?.throwIfAborted();
      if (!this.#current) {
        const started = await this.#open();
        return { kind: "reconnected", started };
      }
      try {
        const notification = await this.#current.subscription.next();
        if (notification.kind === "event") {
          this.#pending = copyCursor(notification.event.nextExpected);
        } else {
          this.#accepted = copyCursor(notification.lagged.nextExpected);
          this.#closeCurrent();
        }
        return notification;
      } catch (error) {
        this.#closeCurrent();
        if (!retryableConnectionFailure(error)) throw error;
        const started = await this.#open();
        return { kind: "reconnected", started };
      }
    }
  }

  public close(): void {
    this.#abort.abort(new RuntimeTransportError("reconnecting event stream was closed"));
    this.#closeCurrent();
  }

  async #open(): Promise<WatchEventsResult> {
    const opened = await retryConnection(
      (signal) => openRuntimeSubscription(
        () => connectSelected(this.connector, this.locator, this.options, signal),
        (runtime) => runtime.sessions().watchEvents({
          sessionId: this.params.sessionId,
          ...(this.#accepted ? { after: this.#accepted } : {}),
        }),
        signal,
      ),
      this.#policy,
    );
    this.#current = opened;
    this.#started = opened.subscription.started;
    return opened.subscription.started;
  }

  #closeCurrent(): void {
    this.#current?.subscription.close();
    this.#current = null;
  }
}

function connectSelected(
  connector: RuntimeConnector,
  locator: ValidatedLocator | null,
  options: ClientOptions,
  signal?: AbortSignal,
): Promise<RuntimeClient> {
  return locator
    ? connector.connect(locator, options, signal)
    : connector.connectSystem(options, signal);
}

async function openRuntimeSubscription<T>(
  connect: () => Promise<RuntimeClient>,
  subscribe: (runtime: RuntimeClient) => Promise<T>,
  signal?: AbortSignal,
): Promise<{ runtime: RuntimeClient; subscription: T }> {
  signal?.throwIfAborted();
  const runtime = await connect();
  const abort = (): void => abortRuntime(runtime);
  signal?.addEventListener("abort", abort, { once: true });
  try {
    signal?.throwIfAborted();
    const subscription = await subscribe(runtime);
    signal?.throwIfAborted();
    return { runtime, subscription };
  } catch (error) {
    runtime.close();
    throw error;
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}

function abortTransport(transport: RuntimeTransport): void {
  if (transport.abort) transport.abort();
  else transport.close();
}

function abortRuntime(runtime: RuntimeClient): void {
  abortTransport(runtimeState(runtime).transport);
}

async function initializeRuntime(
  transport: RuntimeTransport,
  locator: ValidatedLocator,
  options: ClientOptions,
): Promise<RuntimeClient> {
  if (options.identity && options.credentials) {
    throw new RuntimeProtocolError("client options cannot contain both identity and credentials");
  }
  const challenge = await receiveChallenge(transport, locator);
  const clientInfo: ClientInfo = { name: options.name, version: options.version };
  const capabilities: ClientCapabilities = options.capabilities ?? { opaqueEventExtensions: false };
  const identity = options.credentials?.identity ?? options.identity;
  const expectedGrant = options.credentials?.grant;
  let authentication: IntegrationAuthentication | undefined;
  if (expectedGrant) {
    if (!identity) throw new RuntimeProtocolError("approved grant has no signing identity");
    const unsigned: IntegrationAuthentication = {
      integrationId: expectedGrant.integrationId,
      keyGeneration: expectedGrant.keyGeneration,
      grantGeneration: expectedGrant.grantGeneration,
      signature: "",
    };
    authentication = {
      ...unsigned,
      signature: identity.signBase64(initializationSigningPayload(
        challenge,
        FINALIZED_REVISIONS,
        clientInfo,
        capabilities,
        unsigned,
      )),
    };
  }
  const params: InitializeParams = {
    supportedRevisions: FINALIZED_REVISIONS,
    client: clientInfo,
    clientCapabilities: capabilities,
    ...(authentication ? { authentication } : {}),
  };
  const requestState = { nextId: 1 };
  const initialized = await callOn<InitializeResult>(
    transport,
    requestState,
    "runtime/initialize",
    params,
    "InitializeResult",
  );
  validateInitialization(initialized, locator, expectedGrant);
  await notify(transport, "runtime/initialized", {});
  return new RuntimeClient(runtimeClientToken, initialized, {
    capabilities,
    challenge,
    clientInfo,
    ...(identity ? { identity } : {}),
    nextId: requestState.nextId,
    streaming: false,
    supportedRevisions: FINALIZED_REVISIONS,
    transport,
  });
}

function runtimeState(runtime: RuntimeClient): RuntimeClientState {
  const state = runtimeStates.get(runtime);
  if (!state) throw new RuntimeProtocolError("RuntimeClient was not initialized by this SDK");
  return state;
}

async function retryConnection<T>(
  connect: (signal: AbortSignal) => Promise<T>,
  policy: ReconnectPolicy,
): Promise<T> {
  const initialDelayMs = boundedDelay(policy.initialDelayMs ?? 100, "initialDelayMs");
  const maximumDelayMs = boundedDelay(policy.maximumDelayMs ?? 2_000, "maximumDelayMs");
  const deadlineMs = boundedDelay(policy.deadlineMs ?? 30_000, "deadlineMs");
  if (initialDelayMs > maximumDelayMs || maximumDelayMs > deadlineMs) {
    throw new RuntimeProtocolError(
      "reconnect delays must be ordered within the total deadline",
    );
  }
  const deadline = performance.now() + deadlineMs;
  let delayMs = initialDelayMs;
  for (;;) {
    policy.signal?.throwIfAborted();
    const remainingMs = deadline - performance.now();
    if (remainingMs <= 0) {
      throw new RuntimeTransportError("Runtime reconnect deadline expired");
    }
    const attempt = new AbortController();
    const signal = policy.signal
      ? AbortSignal.any([policy.signal, attempt.signal])
      : attempt.signal;
    const timeout = setTimeout(
      () => attempt.abort(new RuntimeTransportError("Runtime connection attempt timed out")),
      remainingMs,
    );
    try {
      return await connect(signal);
    } catch (error) {
      if (!retryableConnectionFailure(error)) throw error;
      const retryRemainingMs = deadline - performance.now();
      if (retryRemainingMs <= 0) throw error;
      await abortableDelay(Math.min(jitteredDelay(delayMs), retryRemainingMs), policy.signal);
      delayMs = Math.min(delayMs * 2, maximumDelayMs);
    } finally {
      clearTimeout(timeout);
    }
  }
}

function sameCursor(left: EventCursor, right: EventCursor): boolean {
  return left.stream === right.stream && left.epoch === right.epoch && left.seq === right.seq;
}

function copyCursor(cursor: EventCursor): EventCursor {
  return { stream: cursor.stream, epoch: cursor.epoch, seq: cursor.seq };
}

function boundedDelay(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value <= 0 || value > 300_000) {
    throw new RuntimeProtocolError(`${name} must be a positive integer no greater than 300000`);
  }
  return value;
}

function retryableConnectionFailure(error: unknown): boolean {
  if (error instanceof RuntimeTransportError) return true;
  if (error instanceof RuntimeLocatorError) return error.code === "io";
  return error instanceof RuntimeRequestError && error.failure.retryable;
}

function jitteredDelay(delayMs: number): number {
  const sample = randomBytes(2).readUInt16LE(0);
  return Math.max(1, Math.floor(delayMs * (0.75 + (sample / 0xffff) * 0.5)));
}

function abortableDelay(delayMs: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    const timeout = setTimeout(done, delayMs);
    const abort = () => {
      clearTimeout(timeout);
      signal?.removeEventListener("abort", abort);
      reject(signal?.reason ?? new RuntimeTransportError("Runtime reconnect was aborted"));
    };
    function done(): void {
      signal?.removeEventListener("abort", abort);
      resolve();
    }
    signal?.addEventListener("abort", abort, { once: true });
  });
}

async function callRuntime<T>(
  runtime: RuntimeClient,
  method: RuntimeMethod,
  params: unknown,
  resultSchema?: string,
): Promise<T> {
  const state = runtimeState(runtime);
  if (state.streaming) {
    throw new RuntimeProtocolError("event subscription owns this Runtime connection");
  }
  return callOn<T>(state.transport, state, method, params, resultSchema);
}

async function callMutation<T>(
  runtime: RuntimeClient,
  method: RuntimeMethod,
  params: { readonly requestId: MutationRequestId },
  resultSchema?: string,
): Promise<T> {
  try {
    return await callRuntime<T>(runtime, method, params, resultSchema);
  } catch (error) {
    if (!(error instanceof RuntimeTransportError)) throw error;
    throw new RuntimeRequestError({
      code: "outcomeUnknown",
      correlationId: params.requestId,
      message: "Runtime connection ended while the mutation outcome was unresolved",
      retryable: false,
    });
  }
}

function beginStream(runtime: RuntimeClient): RuntimeTransport {
  const state = runtimeState(runtime);
  if (state.streaming) throw new RuntimeProtocolError("connection already has an event subscription");
  state.streaming = true;
  return state.transport;
}

function enrollmentPayload(runtime: RuntimeClient, manifest: EnrollmentManifest): Uint8Array {
  const state = runtimeState(runtime);
  return enrollmentSigningPayload(
    state.challenge,
    state.supportedRevisions,
    runtime.initialization.selectedRevision,
    state.clientInfo,
    state.capabilities,
    manifest,
  );
}

function signingIdentity(runtime: RuntimeClient): IntegrationIdentity {
  const identity = runtimeState(runtime).identity;
  if (!identity) {
    throw new RuntimeProtocolError("integration enrollment requires a consumer-owned identity");
  }
  return identity;
}

async function receiveChallenge(
  transport: RuntimeTransport,
  locator: ValidatedLocator,
): Promise<ServerChallenge> {
  const decoded = decodeJson(await transport.receive());
  const notification = validatePublic<JsonRpcNotification>("JsonRpcNotification", decoded);
  if (notification.jsonrpc !== "2.0" || notification.method !== "runtime/challenge") {
    throw new RuntimeProtocolError("first Runtime frame is not the required challenge");
  }
  const challenge = validatePublic<ServerChallenge>("ServerChallenge", notification.params);
  const now = Date.now();
  if (challenge.instanceId !== locator.instanceId) {
    throw new RuntimeProtocolError("Runtime challenge instance does not match the locator");
  }
  if (challenge.expiresAtMs <= now) {
    throw new RuntimeProtocolError("Runtime challenge is already expired");
  }
  if (challenge.expiresAtMs
    > now + PUBLIC_LIMITS.challengeLifetimeMs + CHALLENGE_CLOCK_SKEW_TOLERANCE_MS) {
    throw new RuntimeProtocolError("Runtime challenge exceeds the public lifetime and clock-skew bound");
  }
  if (!/^nonce_[0-9a-f]{32}$/.test(challenge.nonceId)
    || Buffer.from(challenge.nonce, "base64url").byteLength !== 32) {
    throw new RuntimeProtocolError("Runtime challenge nonce is malformed");
  }
  return challenge;
}

function validateInitialization(
  initialized: InitializeResult,
  locator: ValidatedLocator,
  expectedGrant: IntegrationGrant | undefined,
): void {
  if (initialized.runtime.instanceId !== locator.instanceId
    || initialized.runtime.version !== locator.runtimeVersion
    || !FINALIZED_REVISIONS.includes(initialized.selectedRevision as never)
    || !initializationGrantMatches(initialized.grant, expectedGrant)) {
    throw new RuntimeProtocolError("Runtime initialization does not match the locator or credentials");
  }
}

function initializationGrantMatches(
  current: IntegrationGrant | null | undefined,
  expected: IntegrationGrant | undefined,
): boolean {
  if (!expected) return current == null;
  if (!current) return false;
  if (
    current.integrationId !== expected.integrationId
    || current.keyGeneration !== expected.keyGeneration
    || current.grantGeneration < expected.grantGeneration
  ) {
    return false;
  }
  return current.grantGeneration !== expected.grantGeneration
    || JSON.stringify(current) === JSON.stringify(expected);
}

async function callOn<T>(
  transport: RuntimeTransport,
  state: { nextId: number },
  method: RuntimeMethod,
  params: unknown,
  resultSchema?: string,
): Promise<T> {
  if (!Number.isSafeInteger(state.nextId)) {
    throw new RuntimeProtocolError("connection exhausted its safe request identifiers");
  }
  const id = state.nextId;
  state.nextId += 1;
  await transport.send(encoder.encode(JSON.stringify({ jsonrpc: "2.0", id, method, params })));
  const decoded = decodeJson(await transport.receive());
  const response = validatePublic<JsonRpcResponse>("JsonRpcResponse", decoded);
  if (response.jsonrpc !== "2.0" || response.id !== id) {
    throw new RuntimeProtocolError("Runtime response envelope does not match its request");
  }
  if ("error" in response) throw new RuntimeRequestError(response.error);
  return resultSchema ? validatePublic<T>(resultSchema, response.result) : response.result as T;
}

async function notify(
  transport: RuntimeTransport,
  method: RuntimeMethod,
  params: unknown,
): Promise<void> {
  await transport.send(encoder.encode(JSON.stringify({ jsonrpc: "2.0", method, params })));
}

function decodeJson(payload: Uint8Array): unknown {
  try {
    return JSON.parse(decoder.decode(payload));
  } catch (error) {
    throw new RuntimeProtocolError(`Runtime frame is not valid UTF-8 JSON: ${String(error)}`);
  }
}

function initializationSigningPayload(
  challenge: ServerChallenge,
  supportedRevisions: ReadonlyArray<string>,
  client: ClientInfo,
  capabilities: ClientCapabilities,
  authentication: IntegrationAuthentication,
): Uint8Array {
  return encoder.encode(JSON.stringify({
    domain: "runtrol-runtime-initialize-v1",
    challenge: canonicalChallenge(challenge),
    supportedRevisions,
    client,
    clientCapabilities: capabilities,
    integrationId: authentication.integrationId,
    keyGeneration: authentication.keyGeneration,
    grantGeneration: authentication.grantGeneration,
  }));
}

function enrollmentSigningPayload(
  challenge: ServerChallenge,
  supportedRevisions: ReadonlyArray<string>,
  selectedRevision: string,
  client: ClientInfo,
  capabilities: ClientCapabilities,
  manifest: EnrollmentManifest,
): Uint8Array {
  return encoder.encode(JSON.stringify({
    domain: "runtrol-runtime-enrollment-v1",
    challenge: canonicalChallenge(challenge),
    supportedRevisions,
    selectedRevision,
    client,
    clientCapabilities: capabilities,
    manifest,
  }));
}

function canonicalChallenge(challenge: ServerChallenge): ServerChallenge {
  return {
    instanceId: challenge.instanceId,
    nonceId: challenge.nonceId,
    nonce: challenge.nonce,
    expiresAtMs: challenge.expiresAtMs,
  };
}

function keyRotationSigningPayload(
  grant: IntegrationGrant,
  params: RotateIntegrationKeyParams,
): Uint8Array {
  return encoder.encode(JSON.stringify({
    domain: "runtrol-runtime-key-rotation-v1",
    integrationId: grant.integrationId,
    grantGeneration: grant.grantGeneration,
    requestId: params.requestId,
    expectedKeyGeneration: params.expectedKeyGeneration,
    newPublicKey: params.newPublicKey,
  }));
}
