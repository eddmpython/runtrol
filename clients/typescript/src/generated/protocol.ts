// Generated from crates/runtrol-runtime-protocol/schema/runtime.schema.json. Do not edit.

export const FINALIZED_REVISIONS = ["2026-08-13"] as const;
export const PUBLIC_LIMITS = {
  "challengeLifetimeMs": 60000,
  "controlLeaseLifetimeMs": 30000,
  "enrollmentLifetimeMs": 600000,
  "idempotencyWindowMs": 86400000,
  "maxFrameBytes": 16842752,
  "maxIdempotencyRecords": 2048,
  "maxInputBytes": 1048576,
  "maxPageItems": 100,
  "maxPendingEnrollments": 64,
  "maxRevisionOffers": 16,
  "maxSubscriptions": 32
} as const;

/** Acquire control only if the caller still sees this exact live state. */
export interface AcquireControlParams { readonly expectedLifecycle: LifecycleState; readonly expectedSessionGeneration: number; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Public integration authority, separate from remote device scopes. */
export type AppScope = "provider.read" | "model.read" | "session.list" | "session.native.discover" | "session.output.read" | "session.start" | "session.resume" | "session.input.write" | "session.stop" | "approval.respond.low" | "approval.respond.high" | "session.delete";

/** Honest native catalogue coverage for the current provider and root context. */
export type CatalogueCoverage = { readonly kind: "complete"; readonly source: CatalogueSource; } | { readonly kind: "partial"; readonly source: CatalogueSource; readonly why: string; } | { readonly kind: "unsupported"; readonly why: string; };

/** The official provider surface used for discovery. */
export type CatalogueSource = "officialProtocol" | "officialCli";

/** Client features understood by the initial read-only revision. */
export interface ClientCapabilities { readonly opaqueEventExtensions?: boolean; }

/** Safe client presentation metadata. */
export interface ClientInfo { readonly name: string; readonly version: string; }

/** One opaque renewable control authority for one live session incarnation. */
export interface ControlLease { readonly expiresAtMs: number; readonly leaseGeneration: number; readonly leaseId: string; readonly sessionGeneration: number; readonly sessionId: RuntimeSessionId; }

/** Renew or release one exact lease generation. */
export interface ControlLeaseParams { readonly leaseGeneration: number; readonly leaseId: string; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Current enrollment decision. */
export type EnrollmentDecision = { readonly state: "pending"; } | { readonly grant: IntegrationGrant; readonly state: "approved"; } | { readonly state: "denied"; } | { readonly state: "expired"; };

/** Closed self-description and exact authority requested for first enrollment. */
export interface EnrollmentManifest { readonly clientInstanceId: string; readonly manifestDigest: string; readonly publicKey: string; readonly requestedRoots: ReadonlyArray<string>; readonly requestedScopes: ReadonlyArray<AppScope>; }

/** Enrollment was recorded without revealing Runtime inventory. */
export interface EnrollmentReceipt { readonly expiresAtMs: number; readonly pendingId: PendingEnrollmentId; }

/** Failed JSON-RPC response. */
export interface ErrorResponse { readonly error: RuntimeError; readonly id: JsonRpcId; readonly jsonrpc: string; }

/** Public reconnect cursor over the existing bounded Runtime replay ring. */
export interface EventCursor { readonly epoch: number; readonly seq: number; readonly stream: string; }

/** Explicit replay gap when the requested cursor fell outside the bounded ring. */
export interface EventGap { readonly liveAt: EventCursor; readonly requested: EventCursor; }

/** Initialization is negotiation only. Inventory is a separate authorized request. */
export interface InitializeParams { readonly authentication?: IntegrationAuthentication | null; readonly client: ClientInfo; readonly clientCapabilities?: ClientCapabilities; readonly supportedRevisions: ReadonlyArray<ProtocolRevision>; }

/** Successful initialization before integration authorization. */
export interface InitializeResult { readonly grant?: IntegrationGrant | null; readonly limits: RuntimeLimits; readonly runtime: RuntimeInstance; readonly selectedRevision: ProtocolRevision; readonly serverCapabilities: RuntimeCapabilities; }

/** Structural provider installation evidence with no credentials or raw output. */
export interface InstallationObservation { readonly state: InstallationState; readonly version?: string | null; readonly why?: string | null; }

/** Whether an installed provider can currently be used. */
export type InstallationState = "usable" | "missing" | "unavailable";

/** Authentication for an approved integration during initialization. */
export interface IntegrationAuthentication { readonly grantGeneration: number; readonly integrationId: IntegrationId; readonly keyGeneration: number; readonly signature: string; }

/** The caller's current approved authority. */
export interface IntegrationGrant { readonly grantGeneration: number; readonly integrationId: IntegrationId; readonly keyGeneration: number; readonly roots: ReadonlyArray<string>; readonly scopes: ReadonlyArray<AppScope>; }

/** An operator-approved local integration instance. */
export type IntegrationId = string;

/** A JSON-RPC request identifier. */
export type JsonRpcId = number | string;

/** One JSON-RPC notification with no response ID. */
export interface JsonRpcNotification { readonly jsonrpc: string; readonly method: string; readonly params?: unknown; }

/** One JSON-RPC request. Method parameter schemas remain closed at their typed decode boundary. */
export interface JsonRpcRequest { readonly id: JsonRpcId; readonly jsonrpc: string; readonly method: string; readonly params?: unknown; }

/** A response is exactly one success or one failure. */
export type JsonRpcResponse = SuccessResponse | ErrorResponse;

/** A slow subscriber was retired at an exact missing boundary. */
export interface LaggedNotification { readonly nextExpected: EventCursor; readonly sessionId: RuntimeSessionId; readonly subscriptionId: string; }

/** Structural Runtime supervision state without conversation meaning. */
export type LifecycleState = "hotIdle" | "hotRunning" | "cold" | "failed";

/** Select one provider for explicit, potentially slow model discovery. */
export interface ListModelsParams { readonly providerId: ProviderId; }

/** Select one provider and one exact approved root for explicit native discovery. */
export interface ListNativeSessionsParams { readonly cursor?: string | null; readonly providerId: ProviderId; readonly root: string; }

/** A bounded Runtime-managed session snapshot. */
export interface ManagedSessionList { readonly sessions: ReadonlyArray<SessionDescriptor>; readonly warnings: ReadonlyArray<string>; }

/** A caller-minted UUIDv7 identifying one state-changing request. */
export type MutationRequestId = string;

/** Whether an officially listed session can be resumed through the same provider driver. */
export type NativeResumeCapability = "available" | "unavailable" | "unknown";

/** One bounded provider-native session page after authorization and filtering. */
export interface NativeSessionCatalogue { readonly coverage: CatalogueCoverage; readonly nextCursor?: string | null; readonly providerId: ProviderId; readonly sessions: ReadonlyArray<NativeSessionDescriptor>; }

/** One root-authorized official provider-native session. */
export interface NativeSessionDescriptor { readonly additionalDirectories: ReadonlyArray<string>; readonly alreadyManagedAs?: RuntimeSessionId | null; readonly cwd: string; readonly nativeSessionId: string; readonly resume: NativeResumeCapability; readonly title?: string | null; readonly updatedAt?: string | null; }

/** An opaque pending local enrollment decision. */
export type PendingEnrollmentId = string;

/** A finalized public Runtime contract date. */
export type ProtocolRevision = string;

/** One provider in the fast inventory. */
export interface ProviderDescriptor { readonly displayName: string; readonly installation: InstallationObservation; readonly providerId: ProviderId; }

/** An opaque provider identity discovered by Runtime. */
export type ProviderId = string;

/** A bounded provider inventory snapshot. */
export interface ProviderList { readonly providers: ReadonlyArray<ProviderDescriptor>; }

/** Prove possession of the key attached to a new enrollment. */
export interface RequestEnrollmentParams { readonly manifest: EnrollmentManifest; readonly signature: string; }

/** Public product capabilities for the selected revision. */
export interface RuntimeCapabilities { readonly integrationEnrollment: boolean; readonly managedSessionList: boolean; readonly modelDiscovery: boolean; readonly nativeSessionCatalogue: boolean; readonly providerInventory: boolean; readonly sessionControl: boolean; readonly sessionEvents: boolean; }

/** Local transport kind named by the platform locator. */
export type RuntimeEndpointKind = "namedPipe" | "unixSocket";

/** A public failure with stable machine fields and bounded safe text. */
export interface RuntimeError { readonly code: RuntimeErrorKind; readonly correlationId: string; readonly message: string; readonly operatorAction?: string | null; readonly retryable: boolean; }

/** A stable public failure category. Clients never branch on prose. */
export type RuntimeErrorKind = "runtimeNotInstalled" | "runtimeUnavailable" | "protocolIncompatible" | "notInitialized" | "unauthenticated" | "enrollmentPending" | "enrollmentDenied" | "integrationRevoked" | "scopeDenied" | "presenceRequired" | "rootDenied" | "providerUnavailable" | "capabilityUnavailable" | "modelUnavailable" | "nativeCatalogueUnsupported" | "sessionNotFound" | "sessionConflict" | "controlConflict" | "leaseExpired" | "workspaceConflict" | "approvalExpired" | "approvalOptionInvalid" | "idempotencyConflict" | "outcomeUnknown" | "resourceExhausted" | "rateLimited" | "gap" | "invalidRequest" | "methodNotFound" | "internal";

/** One provider-neutral normalized event notification. */
export interface RuntimeEventNotification { readonly event: unknown; readonly eventRevision: ProtocolRevision; readonly nextExpected: EventCursor; readonly sessionId: RuntimeSessionId; readonly subscriptionId: string; }

/** Public Runtime instance facts used to reject a stale or replaced locator. */
export interface RuntimeInstance { readonly instanceId: string; readonly platform: string; readonly version: string; }

/** Numeric public bounds advertised during initialization. */
export interface RuntimeLimits { readonly challengeLifetimeMs: number; readonly controlLeaseLifetimeMs: number; readonly enrollmentLifetimeMs: number; readonly idempotencyWindowMs: number; readonly maxFrameBytes: number; readonly maxIdempotencyRecords: number; readonly maxInputBytes: number; readonly maxPageItems: number; readonly maxPendingEnrollments: number; readonly maxRevisionOffers: number; readonly maxSubscriptions: number; }

/** Operational bootstrap data published only after the public endpoint is ready. */
export interface RuntimeLocatorRecord { readonly endpoint: string; readonly endpointKind: RuntimeEndpointKind; readonly instanceId: string; readonly processId: number; readonly runtimeVersion: string; readonly schema: number; }

/** A public Runtime method implemented by the initial read-only boundary. */
export type RuntimeMethod = "runtime/initialize" | "runtime/initialized" | "runtime/challenge" | "integrations/requestEnrollment" | "integrations/watchEnrollment" | "integrations/getGrant" | "providers/list" | "providers/listModels" | "providers/listNativeSessions" | "sessions/list" | "sessions/acquireControl" | "sessions/renewControl" | "sessions/releaseControl" | "sessions/submitInput" | "sessions/watchEvents" | "sessions/interrupt" | "sessions/event" | "sessions/lagged" | "runtime/panicStop";

/** The current model information Runtime can truthfully expose. */
export type RuntimeModelCatalog = { readonly coverage: "known"; readonly models: ReadonlyArray<RuntimeModelChoice>; } | { readonly aliases: ReadonlyArray<string>; readonly coverage: "aliases"; readonly why: string; } | { readonly aliases: ReadonlyArray<string>; readonly coverage: "partial"; readonly models: ReadonlyArray<RuntimeModelChoice>; readonly why: string; } | { readonly coverage: "unknown"; readonly why: string; } | { readonly coverage: "unsupported"; readonly why: string; };

/** One opaque model selection reported by a provider. */
export interface RuntimeModelChoice { readonly description: string; readonly displayName: string; readonly id: string; readonly isDefault: boolean; readonly reasoningEfforts: ReadonlyArray<RuntimeReasoningChoice>; }

/** One opaque reasoning-effort option reported by a provider. */
export interface RuntimeReasoningChoice { readonly description: string; readonly id: string; }

/** A stable Runtime-managed session identity. */
export type RuntimeSessionId = string;

/** One connection-bound challenge sent before initialization. */
export interface ServerChallenge { readonly expiresAtMs: number; readonly instanceId: string; readonly nonce: string; readonly nonceId: string; }

/** One Runtime-managed session in the immediate catalogue. */
export interface SessionDescriptor { readonly label?: string | null; readonly lifecycle: LifecycleState; readonly providerId: ProviderId; readonly sessionGeneration: number; readonly sessionId: RuntimeSessionId; }

/** Submit caller-owned input under one exact control lease. */
export interface SubmitInputParams { readonly input: string; readonly leaseGeneration: number; readonly leaseId: string; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Successful JSON-RPC response. */
export interface SuccessResponse { readonly id: JsonRpcId; readonly jsonrpc: string; readonly result: unknown; }

/** Read one pending decision on the same proved connection. */
export interface WatchEnrollmentParams { readonly pendingId: PendingEnrollmentId; }

/** Install one bounded event subscription on a dedicated connection. */
export interface WatchEventsParams { readonly after?: EventCursor | null; readonly sessionId: RuntimeSessionId; }

/** Event subscription boundary returned before replay or live delivery. */
export interface WatchEventsResult { readonly gap?: EventGap | null; readonly liveAt: EventCursor; readonly sessionId: RuntimeSessionId; readonly startsAt: EventCursor; readonly subscriptionId: string; }
