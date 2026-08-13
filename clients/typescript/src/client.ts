import type {
  AcquireControlParams,
  AppScope,
  ClientCapabilities,
  ClientInfo,
  ControlLease,
  ControlLeaseParams,
  EnrollmentDecision,
  EnrollmentManifest,
  EnrollmentReceipt,
  InitializeParams,
  InitializeResult,
  IntegrationAuthentication,
  IntegrationGrant,
  JsonRpcNotification,
  JsonRpcResponse,
  LaggedNotification,
  ListModelsParams,
  ListNativeSessionsParams,
  ManagedSessionList,
  NativeSessionCatalogue,
  PendingEnrollmentId,
  ProviderId,
  ProviderList,
  RequestEnrollmentParams,
  RuntimeEventNotification,
  RuntimeMethod,
  RuntimeModelCatalog,
  ServerChallenge,
  SubmitInputParams,
  WatchEventsParams,
  WatchEventsResult,
} from "./generated/protocol.js";
import { FINALIZED_REVISIONS, PUBLIC_LIMITS } from "./generated/protocol.js";
import {
  RuntimeProtocolError,
  RuntimeRequestError,
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

  public async connect(locator: ValidatedLocator, options: ClientOptions): Promise<RuntimeClient> {
    locator.assertSdkValidated();
    const transport = await this.transportFactory(locator.endpoint);
    try {
      return await initializeRuntime(transport, locator, options);
    } catch (error) {
      transport.close();
      throw error;
    }
  }

  public async connectSystem(options: ClientOptions): Promise<RuntimeClient> {
    const state = await RuntimeLocator.system().inspect();
    if (state.state === "notInstalled") {
      throw new RuntimeRequestError({
        code: "runtimeNotInstalled",
        message: "Runtrol Runtime is not installed",
        retryable: false,
        correlationId: "local-locator",
      });
    }
    return this.connect(state.locator, options);
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
}

export class ProviderClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public list(): Promise<ProviderList> {
    return callRuntime(this.runtime, "providers/list", {}, "ProviderList");
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

export class SessionClient {
  public constructor(private readonly runtime: RuntimeClient) {}

  public list(): Promise<ManagedSessionList> {
    return callRuntime(this.runtime, "sessions/list", {}, "ManagedSessionList");
  }

  public acquireControl(params: AcquireControlParams): Promise<ControlLease> {
    return callRuntime(this.runtime, "sessions/acquireControl", params, "ControlLease");
  }

  public renewControl(params: ControlLeaseParams): Promise<ControlLease> {
    return callRuntime(this.runtime, "sessions/renewControl", params, "ControlLease");
  }

  public async releaseControl(params: ControlLeaseParams): Promise<void> {
    requireEmpty(await callRuntime(this.runtime, "sessions/releaseControl", params, undefined));
  }

  public async submitInput(params: SubmitInputParams): Promise<void> {
    if (Buffer.byteLength(params.input, "utf8") > PUBLIC_LIMITS.maxInputBytes) {
      throw new RuntimeProtocolError("session input exceeds the public byte limit");
    }
    requireEmpty(await callRuntime(this.runtime, "sessions/submitInput", params, undefined));
  }

  public async interrupt(params: ControlLeaseParams): Promise<void> {
    requireEmpty(await callRuntime(this.runtime, "sessions/interrupt", params, undefined));
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

export type SessionNotification =
  | { readonly kind: "event"; readonly event: RuntimeEventNotification }
  | { readonly kind: "lagged"; readonly lagged: LaggedNotification };

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

  private validateTarget(subscriptionId: string, sessionId: string): void {
    if (subscriptionId !== this.started.subscriptionId || sessionId !== this.started.sessionId) {
      throw new RuntimeProtocolError("session notification target does not match its subscription");
    }
  }
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
  if (challenge.instanceId !== locator.instanceId
    || challenge.expiresAtMs <= now
    || challenge.expiresAtMs > now + PUBLIC_LIMITS.challengeLifetimeMs
    || !/^nonce_[0-9a-f]{32}$/.test(challenge.nonceId)
    || Buffer.from(challenge.nonce, "base64url").byteLength !== 32) {
    throw new RuntimeProtocolError("Runtime challenge is stale, mismatched, or malformed");
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
    || JSON.stringify(initialized.grant ?? null) !== JSON.stringify(expectedGrant ?? null)) {
    throw new RuntimeProtocolError("Runtime initialization does not match the locator or credentials");
  }
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
    challenge,
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
    challenge,
    supportedRevisions,
    selectedRevision,
    client,
    clientCapabilities: capabilities,
    manifest,
  }));
}
