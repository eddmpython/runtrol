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
  "maxModelSelectionBytes": 4096,
  "maxNativeAdoptionTokenBytes": 2048,
  "maxNativePublicCursorBytes": 8192,
  "maxPageItems": 100,
  "maxPendingEnrollments": 64,
  "maxReasoningSelectionBytes": 4096,
  "maxRevisionOffers": 16,
  "maxSubscriptions": 32,
  "nativeCursorLifetimeMs": 300000
} as const;

/** Acquire control only if the caller still sees this exact live state. */
export interface AcquireControlParams { readonly expectedLifecycle: LifecycleState; readonly expectedSessionGeneration: number; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Adopt one exact native catalogue observation into Runtime supervision. */
export interface AdoptNativeSessionParams { readonly access: SessionWorkspaceAccess; readonly adoptionToken: string; readonly nativeSessionId: string; readonly providerId: ProviderId; readonly requestId: MutationRequestId; readonly workspace: string; }

/** Public integration authority, separate from remote device scopes. */
export type AppScope = "provider.read" | "model.read" | "session.list" | "session.native.discover" | "session.output.read" | "session.start" | "session.resume" | "session.input.write" | "session.stop" | "approval.respond.low" | "approval.respond.high" | "session.delete";

/** Freshness of a capability map relative to the installed binary identity. */
export type CapabilityFreshness = "current" | "stale";

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

/** Cool one exact idle session while retaining its provider-native pointer. */
export interface CoolSessionParams { readonly expectedSessionGeneration: number; readonly leaseGeneration: number; readonly leaseId: string; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

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

/** Forget one cold Runtime pointer after an exact local close confirmation. */
export interface ForgetSessionParams { readonly expectedSessionGeneration: number; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Select one provider for explicit capability discovery. */
export interface GetProviderCapabilitiesParams { readonly providerId: ProviderId; }

/** Select one exact Runtime-managed session. */
export interface GetSessionParams { readonly sessionId: RuntimeSessionId; }

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

/** Current control lease required to inspect pending approvals for one session. */
export interface ListPendingApprovalsParams { readonly leaseGeneration: number; readonly leaseId: string; readonly sessionId: RuntimeSessionId; }

/** A bounded Runtime-managed session snapshot. */
export interface ManagedSessionList { readonly sessions: ReadonlyArray<SessionDescriptor>; readonly warnings: ReadonlyArray<string>; }

/** A caller-minted UUIDv7 identifying one state-changing request. */
export type MutationRequestId = string;

/** Whether an officially listed session can be resumed through the same provider driver. */
export type NativeResumeCapability = "available" | "unavailable" | "unknown";

/** One bounded provider-native session page after authorization and filtering. */
export interface NativeSessionCatalogue { readonly coverage: CatalogueCoverage; readonly nextCursor?: string | null; readonly providerId: ProviderId; readonly sessions: ReadonlyArray<NativeSessionDescriptor>; }

/** One root-authorized official provider-native session. */
export interface NativeSessionDescriptor { readonly additionalDirectories: ReadonlyArray<string>; readonly adoptionToken?: string | null; readonly alreadyManagedAs?: RuntimeSessionId | null; readonly cwd: string; readonly nativeSessionId: string; readonly resume: NativeResumeCapability; readonly title?: string | null; readonly updatedAt?: string | null; }

/** One provider-neutral approval request retained by the live driver. */
export interface PendingApproval { readonly approvalId: string; readonly expiresAtMs: number; readonly kind: RuntimeApprovalKind; readonly options: ReadonlyArray<RuntimeApprovalOption>; readonly risk: RuntimeApprovalRisk; readonly subject: unknown; readonly subjectDigest: ReadonlyArray<number>; readonly subjectIncomplete: boolean; }

/** Every provider approval still pending for the exact controlled session. */
export interface PendingApprovalList { readonly approvals: ReadonlyArray<PendingApproval>; }

/** An opaque pending local enrollment decision. */
export type PendingEnrollmentId = string;

/** A finalized public Runtime contract date. */
export type ProtocolRevision = string;

/** Whether one structural provider operation is usable in the observed installation. */
export type ProviderCapabilityAvailability = "available" | "unsupported" | "unknown";

/** One sanitized structural capability observation. */
export interface ProviderCapabilityObservation { readonly availability: ProviderCapabilityAvailability; readonly provenance?: ProviderCapabilityProvenance | null; readonly why?: string | null; }

/** Provenance of an available structural capability. */
export type ProviderCapabilityProvenance = "officialProtocol" | "officialCli" | "driverContract";

/** One provider in the fast inventory. */
export interface ProviderDescriptor { readonly displayName: string; readonly help?: ProviderHelp | null; readonly icon?: string | null; readonly installation: InstallationObservation; readonly providerId: ProviderId; }

/** A coding service's own commands for making itself usable, ready to show a person.

# Why Runtime sends finished command lines

A declaration names arguments; only Runtime knows which executable actually resolved. A client that
joined the two would be a second place that decides what runs, and it would be wrong on exactly the
machine where a second candidate was the installed one.

# What a client may do with these

Offer them. Nothing else. Runtime does not run them and neither should a client: fetching and
executing on a person's behalf is the capability this product refused from the start, and an install
button that runs is that capability with a friendly label. The operator reads the line and decides.

Every string is validated at the declaration boundary to contain no character a shell could read as a
separator, so a client can present one without quoting it into something else. */
export interface ProviderHelp { readonly diagnose?: string | null; readonly install?: string | null; readonly signIn?: string | null; }

/** An opaque provider identity discovered by Runtime. */
export type ProviderId = string;

/** A bounded provider inventory snapshot. */
export interface ProviderList { readonly providers: ReadonlyArray<ProviderDescriptor>; }

/** One provider's most recent limit report. */
export interface ProviderUsageGauge { readonly atMs: number; readonly primary?: ProviderUsageWindow | null; readonly providerId: ProviderId; readonly reached: boolean; readonly secondary?: ProviderUsageWindow | null; }

/** Where each account stands against its limits, by each provider's own latest report.

Structured fields only, never the provider's verbatim payload: that payload rides the session event stream
under session-output authority, and this list answers under provider authority. A gauge absent from the list
means that provider has not reported since the Runtime started, which is different from a limit not existing,
and a surface says "no report yet" rather than inventing a green light. */
export interface ProviderUsageList { readonly providers: ReadonlyArray<ProviderUsageGauge>; }

/** One rate limit window, as far as the provider described it. */
export interface ProviderUsageWindow { readonly resetsAtMs?: number | null; readonly usedPercent?: number | null; readonly windowMinutes?: number | null; }

/** Why a provider inventory subscription ended. */
export type ProviderWatchEndReason = "integrationRevoked" | "authorityChanged" | "runtimeUnavailable";

/** Final typed reason for retiring a provider inventory subscription. */
export interface ProviderWatchEndedNotification { readonly reason: ProviderWatchEndReason; readonly subscriptionId: string; }

/** A changed complete provider inventory snapshot. */
export interface ProvidersChangedNotification { readonly snapshot: ProviderList; readonly subscriptionId: string; }

/** Prove possession of the key attached to a new enrollment. */
export interface RequestEnrollmentParams { readonly manifest: EnrollmentManifest; readonly signature: string; }

/** Answer one exact provider approval under the current control lease. */
export interface RespondApprovalParams { readonly approvalId: string; readonly leaseGeneration: number; readonly leaseId: string; readonly optionId: number; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; readonly subjectDigest: ReadonlyArray<number>; }

/** Heat one existing Runtime-managed cold session. */
export interface ResumeSessionParams { readonly access: SessionWorkspaceAccess; readonly expectedLifecycle: LifecycleState; readonly expectedSessionGeneration: number; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; readonly workspace: string; }

/** Replace one approved integration key after an exact local confirmation. */
export interface RotateIntegrationKeyParams { readonly expectedKeyGeneration: number; readonly newKeyProof: string; readonly newPublicKey: string; readonly requestId: MutationRequestId; }

/** Structural provider approval class. */
export type RuntimeApprovalKind = "command" | "fileChange" | "permissions" | "elicitation" | "network" | "other";

/** One provider-offered approval option and its current availability. */
export interface RuntimeApprovalOption { readonly kind: RuntimeApprovalOptionKind; readonly label: string; readonly optionId: number; readonly unavailable?: string | null; }

/** Structural effect of one provider-offered option. */
export type RuntimeApprovalOptionKind = "allowOnce" | "allowAlways" | "rejectOnce" | "rejectAlways";

/** Authority class derived from the pending request and selected option. */
export type RuntimeApprovalRisk = "low" | "high";

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
export interface RuntimeLimits { readonly challengeLifetimeMs: number; readonly controlLeaseLifetimeMs: number; readonly enrollmentLifetimeMs: number; readonly idempotencyWindowMs: number; readonly maxFrameBytes: number; readonly maxIdempotencyRecords: number; readonly maxInputBytes: number; readonly maxModelSelectionBytes: number; readonly maxNativeAdoptionTokenBytes: number; readonly maxNativePublicCursorBytes: number; readonly maxPageItems: number; readonly maxPendingEnrollments: number; readonly maxReasoningSelectionBytes: number; readonly maxRevisionOffers: number; readonly maxSubscriptions: number; readonly nativeCursorLifetimeMs: number; }

/** Operational bootstrap data published only after the public endpoint is ready. */
export interface RuntimeLocatorRecord { readonly endpoint: string; readonly endpointKind: RuntimeEndpointKind; readonly instanceId: string; readonly processId: number; readonly runtimeVersion: string; readonly schema: number; }

/** A public Runtime method implemented by the initial read-only boundary. */
export type RuntimeMethod = "runtime/initialize" | "runtime/initialized" | "runtime/challenge" | "integrations/requestEnrollment" | "integrations/watchEnrollment" | "integrations/getGrant" | "integrations/rotateKey" | "providers/usage" | "providers/list" | "providers/watch" | "providers/getCapabilities" | "providers/listModels" | "providers/listNativeSessions" | "sessions/list" | "sessions/watchIndex" | "sessions/get" | "sessions/start" | "sessions/adoptNative" | "sessions/resume" | "sessions/acquireControl" | "sessions/renewControl" | "sessions/releaseControl" | "sessions/submitInput" | "sessions/watchEvents" | "sessions/interrupt" | "sessions/cool" | "sessions/forget" | "approvals/listPending" | "approvals/respond" | "sessions/indexChanged" | "sessions/indexEnded" | "providers/changed" | "providers/watchEnded" | "sessions/event" | "sessions/lagged" | "runtime/panicStop";

/** The current model information Runtime can truthfully expose. */
export type RuntimeModelCatalog = { readonly coverage: "known"; readonly models: ReadonlyArray<RuntimeModelChoice>; } | { readonly aliases: ReadonlyArray<string>; readonly coverage: "aliases"; readonly reasoningEfforts: ReadonlyArray<RuntimeReasoningChoice>; readonly why: string; } | { readonly aliases: ReadonlyArray<string>; readonly coverage: "partial"; readonly models: ReadonlyArray<RuntimeModelChoice>; readonly reasoningEfforts: ReadonlyArray<RuntimeReasoningChoice>; readonly why: string; } | { readonly coverage: "unknown"; readonly why: string; } | { readonly coverage: "unsupported"; readonly why: string; };

/** One opaque model selection reported by a provider. */
export interface RuntimeModelChoice { readonly description: string; readonly displayName: string; readonly id: string; readonly isDefault: boolean; readonly reasoningEfforts: ReadonlyArray<RuntimeReasoningChoice>; }

/** Structural lifecycle and event capabilities for one exact provider installation. */
export interface RuntimeProviderCapabilities { readonly approvals: ProviderCapabilityObservation; readonly cooling: ProviderCapabilityObservation; readonly freshSession: ProviderCapabilityObservation; readonly freshness: CapabilityFreshness; readonly interrupt: ProviderCapabilityObservation; readonly nativeSessionCatalogue: ProviderCapabilityObservation; readonly providerId: ProviderId; readonly resume: ProviderCapabilityObservation; readonly structuredEvents: ProviderCapabilityObservation; }

/** One opaque reasoning-effort option reported by a provider. */
export interface RuntimeReasoningChoice { readonly description: string; readonly id: string; }

/** A stable Runtime-managed session identity. */
export type RuntimeSessionId = string;

/** One connection-bound challenge sent before initialization. */
export interface ServerChallenge { readonly expiresAtMs: number; readonly instanceId: string; readonly nonce: string; readonly nonceId: string; }

/** One Runtime-managed session in the immediate catalogue. */
export interface SessionDescriptor { readonly hot: boolean; readonly label?: string | null; readonly lifecycle: LifecycleState; readonly looksStuck: boolean; readonly nativeSessionId?: string | null; readonly providerId: ProviderId; readonly sessionGeneration: number; readonly sessionId: RuntimeSessionId; readonly waitingOn?: WaitingOn | null; readonly workspace: string; }

/** A changed authorized managed-session snapshot. */
export interface SessionIndexChangedNotification { readonly snapshot: ManagedSessionList; readonly subscriptionId: string; }

/** Why a managed-session index subscription ended. */
export type SessionIndexEndReason = "integrationRevoked" | "authorityChanged" | "rootDenied" | "runtimeUnavailable";

/** Final typed reason for retiring a managed-session index subscription. */
export interface SessionIndexEndedNotification { readonly reason: SessionIndexEndReason; readonly subscriptionId: string; }

/** One newly supervised or reheated session and its initial controller authority. */
export interface SessionOpenResult { readonly control: ControlLease; readonly session: SessionDescriptor; }

/** Whether a newly heated process must be the only writer for its working tree. */
export type SessionWorkspaceAccess = "exclusive" | "shared";

/** Start a new provider-native session in one exact authorized workspace. */
export interface StartSessionParams { readonly access: SessionWorkspaceAccess; readonly model?: string | null; readonly providerId: ProviderId; readonly reasoningEffort?: string | null; readonly requestId: MutationRequestId; readonly workspace: string; }

/** Submit caller-owned input under one exact control lease. */
export interface SubmitInputParams { readonly input: string; readonly leaseGeneration: number; readonly leaseId: string; readonly requestId: MutationRequestId; readonly sessionId: RuntimeSessionId; }

/** Successful JSON-RPC response. */
export interface SuccessResponse { readonly id: JsonRpcId; readonly jsonrpc: string; readonly result: unknown; }

/** What a running turn is waiting for, when it is waiting for anybody.

Structural, and deliberately only two values. A surface listing eight running sessions needs to answer one
question without opening any of them: which of these stopped for me? An approval identifier or the provider's
own wording would be conversation detail, which this protocol does not carry. */
export type WaitingOn = "person" | "quota";

/** Read one pending decision on the same proved connection. */
export interface WatchEnrollmentParams { readonly pendingId: PendingEnrollmentId; }

/** Install one bounded event subscription on a dedicated connection. */
export interface WatchEventsParams { readonly after?: EventCursor | null; readonly sessionId: RuntimeSessionId; }

/** Event subscription boundary returned before replay or live delivery. */
export interface WatchEventsResult { readonly gap?: EventGap | null; readonly liveAt: EventCursor; readonly sessionId: RuntimeSessionId; readonly startsAt: EventCursor; readonly subscriptionId: string; }

/** Install one dedicated provider inventory subscription. */
export type WatchProvidersParams = Readonly<Record<string, never>>;

/** Initial provider snapshot and connection-local subscription identity. */
export interface WatchProvidersResult { readonly snapshot: ProviderList; readonly subscriptionId: string; }

/** Install one dedicated managed-session index subscription. */
export type WatchSessionIndexParams = Readonly<Record<string, never>>;

/** Initial authorized snapshot and connection-local subscription identity. */
export interface WatchSessionIndexResult { readonly snapshot: ManagedSessionList; readonly subscriptionId: string; }
