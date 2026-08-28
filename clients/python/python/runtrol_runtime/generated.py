"""Generated from the checked Rust Runtime schema. Do not edit by hand."""

from __future__ import annotations

from typing import ForwardRef, Literal, NotRequired, Required, TypeAlias, TypedDict

JsonValue: TypeAlias = None | bool | int | float | str | list["JsonValue"] | dict[str, "JsonValue"]
JsonObject: TypeAlias = dict[str, JsonValue]
SCHEMA_SHA256 = '66c8d0abbfbf751abff031caa4830be2dfc6b478359c1b2527b60663bcaa1f6a'

AcquireControlParams = TypedDict('AcquireControlParams', {
    'expectedLifecycle': Required[ForwardRef('LifecycleState')],
    'expectedSessionGeneration': Required[int],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
AdoptNativeSessionParams = TypedDict('AdoptNativeSessionParams', {
    'access': Required[ForwardRef('SessionWorkspaceAccess')],
    'adoptionToken': Required[str],
    'nativeSessionId': Required[str],
    'providerId': Required[ForwardRef('ProviderId')],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'workspace': Required[str],
})
AppScope: TypeAlias = Literal['provider.read'] | Literal['model.read'] | Literal['session.list'] | Literal['session.native.discover'] | Literal['session.output.read'] | Literal['session.start'] | Literal['session.resume'] | Literal['session.input.write'] | Literal['session.stop'] | Literal['approval.respond.low'] | Literal['approval.respond.high'] | Literal['session.delete']
ArchiveNativeSessionParams = TypedDict('ArchiveNativeSessionParams', {
    'nativeSessionId': Required[str],
    'providerId': Required[ForwardRef('ProviderId')],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'workspace': Required[str],
})
CapabilityFreshness: TypeAlias = Literal['current'] | Literal['stale']
CatalogueCoverage: TypeAlias = JsonObject | JsonObject | JsonObject
CatalogueSource: TypeAlias = Literal['officialProtocol'] | Literal['officialCli'] | Literal['providerStore']
ClientCapabilities = TypedDict('ClientCapabilities', {
    'opaqueEventExtensions': NotRequired[bool],
})
ClientInfo = TypedDict('ClientInfo', {
    'name': Required[str],
    'version': Required[str],
})
ControlLease = TypedDict('ControlLease', {
    'expiresAtMs': Required[int],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'sessionGeneration': Required[int],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
ControlLeaseParams = TypedDict('ControlLeaseParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
CoolSessionParams = TypedDict('CoolSessionParams', {
    'expectedSessionGeneration': Required[int],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
DeleteNativeSessionParams = TypedDict('DeleteNativeSessionParams', {
    'nativeSessionId': Required[str],
    'providerId': Required[ForwardRef('ProviderId')],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'workspace': Required[str],
})
EnrollmentDecision: TypeAlias = JsonObject | JsonObject | JsonObject | JsonObject
EnrollmentManifest = TypedDict('EnrollmentManifest', {
    'clientInstanceId': Required[str],
    'manifestDigest': Required[str],
    'publicKey': Required[str],
    'requestedRoots': Required[list[str]],
    'requestedScopes': Required[list[ForwardRef('AppScope')]],
})
EnrollmentReceipt = TypedDict('EnrollmentReceipt', {
    'expiresAtMs': Required[int],
    'pendingId': Required[ForwardRef('PendingEnrollmentId')],
})
ErrorResponse = TypedDict('ErrorResponse', {
    'error': Required[ForwardRef('RuntimeError')],
    'id': Required[ForwardRef('JsonRpcId')],
    'jsonrpc': Required[str],
})
EventCursor = TypedDict('EventCursor', {
    'epoch': Required[int],
    'seq': Required[int],
    'stream': Required[str],
})
EventGap = TypedDict('EventGap', {
    'liveAt': Required[ForwardRef('EventCursor')],
    'requested': Required[ForwardRef('EventCursor')],
})
ForgetSessionParams = TypedDict('ForgetSessionParams', {
    'expectedSessionGeneration': Required[int],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
GetProviderCapabilitiesParams = TypedDict('GetProviderCapabilitiesParams', {
    'providerId': Required[ForwardRef('ProviderId')],
})
GetSessionParams = TypedDict('GetSessionParams', {
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
InitializeParams = TypedDict('InitializeParams', {
    'authentication': NotRequired[ForwardRef('IntegrationAuthentication') | None],
    'client': Required[ForwardRef('ClientInfo')],
    'clientCapabilities': NotRequired[ForwardRef('ClientCapabilities')],
    'supportedRevisions': Required[list[ForwardRef('ProtocolRevision')]],
})
InitializeResult = TypedDict('InitializeResult', {
    'grant': NotRequired[ForwardRef('IntegrationGrant') | None],
    'limits': Required[ForwardRef('RuntimeLimits')],
    'runtime': Required[ForwardRef('RuntimeInstance')],
    'selectedRevision': Required[ForwardRef('ProtocolRevision')],
    'serverCapabilities': Required[ForwardRef('RuntimeCapabilities')],
})
InstallationObservation = TypedDict('InstallationObservation', {
    'state': Required[ForwardRef('InstallationState')],
    'version': NotRequired[str | None],
    'why': NotRequired[str | None],
})
InstallationState: TypeAlias = Literal['usable'] | Literal['missing'] | Literal['unavailable']
IntegrationAuthentication = TypedDict('IntegrationAuthentication', {
    'grantGeneration': Required[int],
    'integrationId': Required[ForwardRef('IntegrationId')],
    'keyGeneration': Required[int],
    'signature': Required[str],
})
IntegrationGrant = TypedDict('IntegrationGrant', {
    'grantGeneration': Required[int],
    'integrationId': Required[ForwardRef('IntegrationId')],
    'keyGeneration': Required[int],
    'roots': Required[list[str]],
    'scopes': Required[list[ForwardRef('AppScope')]],
})
IntegrationId: TypeAlias = str
JsonRpcId: TypeAlias = int | str
JsonRpcNotification = TypedDict('JsonRpcNotification', {
    'jsonrpc': Required[str],
    'method': Required[str],
    'params': NotRequired[JsonValue],
})
JsonRpcRequest = TypedDict('JsonRpcRequest', {
    'id': Required[ForwardRef('JsonRpcId')],
    'jsonrpc': Required[str],
    'method': Required[str],
    'params': NotRequired[JsonValue],
})
JsonRpcResponse: TypeAlias = ForwardRef('SuccessResponse') | ForwardRef('ErrorResponse')
LaggedNotification = TypedDict('LaggedNotification', {
    'nextExpected': Required[ForwardRef('EventCursor')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'subscriptionId': Required[str],
})
LifecycleState: TypeAlias = Literal['hotIdle'] | Literal['hotRunning'] | Literal['cold'] | Literal['failed']
ListModelsParams = TypedDict('ListModelsParams', {
    'providerId': Required[ForwardRef('ProviderId')],
})
ListNativeSessionsParams = TypedDict('ListNativeSessionsParams', {
    'cursor': NotRequired[str | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'root': NotRequired[str | None],
})
ListPendingApprovalsParams = TypedDict('ListPendingApprovalsParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
ListTerminalsParams: TypeAlias = JsonObject
ManagedSessionList = TypedDict('ManagedSessionList', {
    'sessions': Required[list[ForwardRef('SessionDescriptor')]],
    'warnings': Required[list[str]],
})
MutationRequestId: TypeAlias = str
NativeActivity = TypedDict('NativeActivity', {
    'active': Required[list[str]],
    'providerId': Required[ForwardRef('ProviderId')],
})
NativeActivityParams = TypedDict('NativeActivityParams', {
    'providerId': Required[ForwardRef('ProviderId')],
})
NativeResumeCapability: TypeAlias = Literal['available'] | Literal['unavailable'] | Literal['unknown']
NativeSessionCatalogue = TypedDict('NativeSessionCatalogue', {
    'coverage': Required[ForwardRef('CatalogueCoverage')],
    'nextCursor': NotRequired[str | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'sessions': Required[list[ForwardRef('NativeSessionDescriptor')]],
})
NativeSessionDescriptor = TypedDict('NativeSessionDescriptor', {
    'additionalDirectories': Required[list[str]],
    'adoptionToken': NotRequired[str | None],
    'alreadyManagedAs': NotRequired[ForwardRef('RuntimeSessionId') | None],
    'cwd': Required[str],
    'nativeSessionId': Required[str],
    'resume': Required[ForwardRef('NativeResumeCapability')],
    'title': NotRequired[str | None],
    'updatedAt': NotRequired[str | None],
})
PendingApproval = TypedDict('PendingApproval', {
    'approvalId': Required[str],
    'expiresAtMs': Required[int],
    'kind': Required[ForwardRef('RuntimeApprovalKind')],
    'options': Required[list[ForwardRef('RuntimeApprovalOption')]],
    'risk': Required[ForwardRef('RuntimeApprovalRisk')],
    'subject': Required[JsonValue],
    'subjectDigest': Required[list[int]],
    'subjectIncomplete': Required[bool],
})
PendingApprovalList = TypedDict('PendingApprovalList', {
    'approvals': Required[list[ForwardRef('PendingApproval')]],
})
PendingEnrollmentId: TypeAlias = str
ProtocolRevision: TypeAlias = str
ProviderAccount = TypedDict('ProviderAccount', {
    'checkedAtMs': Required[int],
    'limitsAbsent': NotRequired[ForwardRef('ProviderLimitsAbsent') | None],
    'method': NotRequired[str | None],
    'plan': NotRequired[str | None],
    'status': Required[ForwardRef('ProviderAccountStatus')],
    'why': NotRequired[str | None],
})
ProviderAccountStatus: TypeAlias = Literal['signedIn'] | Literal['signedOut'] | Literal['unpublished']
ProviderCapabilityAvailability: TypeAlias = Literal['available'] | Literal['unsupported'] | Literal['unknown']
ProviderCapabilityObservation = TypedDict('ProviderCapabilityObservation', {
    'availability': Required[ForwardRef('ProviderCapabilityAvailability')],
    'provenance': NotRequired[ForwardRef('ProviderCapabilityProvenance') | None],
    'why': NotRequired[str | None],
})
ProviderCapabilityProvenance: TypeAlias = Literal['officialProtocol'] | Literal['officialCli'] | Literal['driverContract']
ProviderDescriptor = TypedDict('ProviderDescriptor', {
    'account': NotRequired[ForwardRef('ProviderAccount') | None],
    'displayName': Required[str],
    'help': NotRequired[ForwardRef('ProviderHelp') | None],
    'icon': NotRequired[str | None],
    'installation': Required[ForwardRef('InstallationObservation')],
    'providerId': Required[ForwardRef('ProviderId')],
    'switchableModes': NotRequired[list[str]],
})
ProviderHelp = TypedDict('ProviderHelp', {
    'diagnose': NotRequired[str | None],
    'install': NotRequired[str | None],
    'signIn': NotRequired[str | None],
})
ProviderId: TypeAlias = str
ProviderLimitsAbsent = TypedDict('ProviderLimitsAbsent', {
    'kind': Required[ForwardRef('ProviderLimitsAbsentKind')],
    'why': Required[str],
})
ProviderLimitsAbsentKind: TypeAlias = Literal['unread'] | Literal['unmetered']
ProviderList = TypedDict('ProviderList', {
    'providers': Required[list[ForwardRef('ProviderDescriptor')]],
})
ProviderUsageCost = TypedDict('ProviderUsageCost', {
    'amount': Required[float],
    'currency': Required[str],
})
ProviderUsageGauge = TypedDict('ProviderUsageGauge', {
    'atMs': Required[int],
    'cost': NotRequired[ForwardRef('ProviderUsageCost') | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'reached': Required[bool],
    'tokensToday': NotRequired[int | None],
    'windows': NotRequired[list[ForwardRef('ProviderUsageWindow')]],
})
ProviderUsageList = TypedDict('ProviderUsageList', {
    'providers': Required[list[ForwardRef('ProviderUsageGauge')]],
})
ProviderUsageWindow = TypedDict('ProviderUsageWindow', {
    'governing': NotRequired[bool],
    'id': Required[str],
    'label': NotRequired[str | None],
    'resetsAtMs': NotRequired[int | None],
    'scope': NotRequired[str | None],
    'usedPercent': NotRequired[int | None],
    'windowMinutes': NotRequired[int | None],
})
ProviderWatchEndReason: TypeAlias = Literal['integrationRevoked'] | Literal['authorityChanged'] | Literal['runtimeUnavailable']
ProviderWatchEndedNotification = TypedDict('ProviderWatchEndedNotification', {
    'reason': Required[ForwardRef('ProviderWatchEndReason')],
    'subscriptionId': Required[str],
})
ProvidersChangedNotification = TypedDict('ProvidersChangedNotification', {
    'snapshot': Required[ForwardRef('ProviderList')],
    'subscriptionId': Required[str],
})
ProvidersUsageChangedNotification = TypedDict('ProvidersUsageChangedNotification', {
    'snapshot': Required[ForwardRef('ProviderUsageList')],
    'subscriptionId': Required[str],
})
PublicInputBlock: TypeAlias = JsonObject | JsonObject
RequestEnrollmentParams = TypedDict('RequestEnrollmentParams', {
    'manifest': Required[ForwardRef('EnrollmentManifest')],
    'signature': Required[str],
})
RespondApprovalParams = TypedDict('RespondApprovalParams', {
    'approvalId': Required[str],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'optionId': Required[int],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'subjectDigest': Required[list[int]],
})
ResumeSessionParams = TypedDict('ResumeSessionParams', {
    'access': Required[ForwardRef('SessionWorkspaceAccess')],
    'expectedLifecycle': Required[ForwardRef('LifecycleState')],
    'expectedSessionGeneration': Required[int],
    'model': NotRequired[str | None],
    'reasoningEffort': NotRequired[str | None],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'workspace': Required[str],
})
RotateIntegrationKeyParams = TypedDict('RotateIntegrationKeyParams', {
    'expectedKeyGeneration': Required[int],
    'newKeyProof': Required[str],
    'newPublicKey': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
})
RuntimeApprovalKind: TypeAlias = Literal['command'] | Literal['fileChange'] | Literal['permissions'] | Literal['elicitation'] | Literal['network'] | Literal['other']
RuntimeApprovalOption = TypedDict('RuntimeApprovalOption', {
    'kind': Required[ForwardRef('RuntimeApprovalOptionKind')],
    'label': Required[str],
    'optionId': Required[int],
    'unavailable': NotRequired[str | None],
})
RuntimeApprovalOptionKind: TypeAlias = Literal['allowOnce'] | Literal['allowAlways'] | Literal['rejectOnce'] | Literal['rejectAlways']
RuntimeApprovalRisk: TypeAlias = Literal['low'] | Literal['high']
RuntimeCapabilities = TypedDict('RuntimeCapabilities', {
    'integrationEnrollment': Required[bool],
    'managedSessionList': Required[bool],
    'modelDiscovery': Required[bool],
    'nativeSessionCatalogue': Required[bool],
    'providerInventory': Required[bool],
    'sessionControl': Required[bool],
    'sessionEvents': Required[bool],
    'terminalSurface': NotRequired[bool],
})
RuntimeEndpointKind: TypeAlias = Literal['namedPipe'] | Literal['unixSocket']
RuntimeError = TypedDict('RuntimeError', {
    'code': Required[ForwardRef('RuntimeErrorKind')],
    'correlationId': Required[str],
    'message': Required[str],
    'operatorAction': NotRequired[str | None],
    'retryable': Required[bool],
})
RuntimeErrorKind: TypeAlias = Literal['runtimeNotInstalled'] | Literal['runtimeUnavailable'] | Literal['protocolIncompatible'] | Literal['notInitialized'] | Literal['unauthenticated'] | Literal['enrollmentPending'] | Literal['enrollmentDenied'] | Literal['integrationRevoked'] | Literal['scopeDenied'] | Literal['presenceRequired'] | Literal['rootDenied'] | Literal['providerUnavailable'] | Literal['capabilityUnavailable'] | Literal['modelUnavailable'] | Literal['nativeCatalogueUnsupported'] | Literal['sessionNotFound'] | Literal['terminalNotFound'] | Literal['terminalGenerationUnavailable'] | Literal['terminalGone'] | Literal['terminalAlreadyLive'] | Literal['terminalWorkspaceConflict'] | Literal['nativeConversationBusy'] | Literal['legacyGenerationBusy'] | Literal['sessionConflict'] | Literal['controlConflict'] | Literal['leaseExpired'] | Literal['workspaceConflict'] | Literal['approvalExpired'] | Literal['approvalOptionInvalid'] | Literal['idempotencyConflict'] | Literal['outcomeUnknown'] | Literal['resourceExhausted'] | Literal['rateLimited'] | Literal['gap'] | Literal['invalidRequest'] | Literal['methodNotFound'] | Literal['internal']
RuntimeEventNotification = TypedDict('RuntimeEventNotification', {
    'event': Required[JsonValue],
    'eventRevision': Required[ForwardRef('ProtocolRevision')],
    'nextExpected': Required[ForwardRef('EventCursor')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'subscriptionId': Required[str],
})
RuntimeGeneration = TypedDict('RuntimeGeneration', {
    'controlEndpoint': Required[str],
    'digest': Required[str],
    'draining': Required[bool],
    'endpoint': Required[str],
    'endpointKind': Required[ForwardRef('RuntimeEndpointKind')],
    'liveSessions': Required[int],
    'processId': Required[int],
    'runtimeVersion': Required[str],
    'startedAtMs': Required[int],
})
RuntimeInstance = TypedDict('RuntimeInstance', {
    'buildDigest': NotRequired[str | None],
    'instanceId': Required[str],
    'platform': Required[str],
    'version': Required[str],
})
RuntimeLimits = TypedDict('RuntimeLimits', {
    'challengeLifetimeMs': Required[int],
    'controlLeaseLifetimeMs': Required[int],
    'enrollmentLifetimeMs': Required[int],
    'idempotencyWindowMs': Required[int],
    'maxAttachmentBase64Bytes': NotRequired[int],
    'maxFrameBytes': Required[int],
    'maxIdempotencyRecords': Required[int],
    'maxInputBlocks': NotRequired[int],
    'maxInputBytes': Required[int],
    'maxInputImages': NotRequired[int],
    'maxModelSelectionBytes': Required[int],
    'maxNativeAdoptionTokenBytes': Required[int],
    'maxNativePublicCursorBytes': Required[int],
    'maxPageItems': Required[int],
    'maxPendingEnrollments': Required[int],
    'maxReasoningSelectionBytes': Required[int],
    'maxRevisionOffers': Required[int],
    'maxSubscriptions': Required[int],
    'maxTerminalColumns': NotRequired[int],
    'maxTerminalIndexItems': NotRequired[int],
    'maxTerminalOutputBytes': NotRequired[int],
    'maxTerminalRows': NotRequired[int],
    'maxTerminalScreenBytes': NotRequired[int],
    'maxTerminalViewQueueChunks': NotRequired[int],
    'maxTerminalWriteBytes': NotRequired[int],
    'nativeCursorLifetimeMs': Required[int],
})
RuntimeLocatorRecord = TypedDict('RuntimeLocatorRecord', {
    'generations': Required[list[ForwardRef('RuntimeGeneration')]],
    'instanceId': Required[str],
    'schema': Required[int],
})
RuntimeMethod: TypeAlias = Literal['runtime/initialize'] | Literal['runtime/initialized'] | Literal['runtime/challenge'] | Literal['integrations/requestEnrollment'] | Literal['integrations/watchEnrollment'] | Literal['integrations/getGrant'] | Literal['integrations/rotateKey'] | Literal['providers/usage'] | Literal['providers/list'] | Literal['providers/watch'] | Literal['providers/getCapabilities'] | Literal['providers/listModels'] | Literal['providers/listNativeSessions'] | Literal['providers/nativeActivity'] | Literal['sessions/list'] | Literal['sessions/watchIndex'] | Literal['sessions/get'] | Literal['sessions/start'] | Literal['sessions/adoptNative'] | Literal['sessions/resume'] | Literal['sessions/acquireControl'] | Literal['sessions/renewControl'] | Literal['sessions/releaseControl'] | Literal['sessions/submitInput'] | Literal['sessions/submitBlocks'] | Literal['sessions/setModel'] | Literal['sessions/setMode'] | Literal['sessions/watchEvents'] | Literal['sessions/interrupt'] | Literal['sessions/cool'] | Literal['sessions/forget'] | Literal['sessions/deleteNative'] | Literal['sessions/archiveNative'] | Literal['terminals/list'] | Literal['terminals/watchIndex'] | Literal['terminals/open'] | Literal['terminals/attach'] | Literal['terminals/acquireControl'] | Literal['terminals/renewControl'] | Literal['terminals/releaseControl'] | Literal['terminals/write'] | Literal['terminals/resize'] | Literal['terminals/detach'] | Literal['terminals/stop'] | Literal['approvals/listPending'] | Literal['approvals/respond'] | Literal['sessions/indexChanged'] | Literal['sessions/indexEnded'] | Literal['providers/changed'] | Literal['providers/watchEnded'] | Literal['providers/usageChanged'] | Literal['sessions/event'] | Literal['sessions/lagged'] | Literal['terminals/indexChanged'] | Literal['terminals/indexEnded'] | Literal['terminals/output'] | Literal['terminals/lagged'] | Literal['terminals/exited'] | Literal['runtime/panicStop']
RuntimeModelCatalog: TypeAlias = JsonObject | JsonObject | JsonObject | JsonObject | JsonObject
RuntimeModelChoice = TypedDict('RuntimeModelChoice', {
    'description': Required[str],
    'displayName': Required[str],
    'id': Required[str],
    'isDefault': Required[bool],
    'reasoningEfforts': Required[list[ForwardRef('RuntimeReasoningChoice')]],
})
RuntimeProviderCapabilities = TypedDict('RuntimeProviderCapabilities', {
    'approvals': Required[ForwardRef('ProviderCapabilityObservation')],
    'cooling': Required[ForwardRef('ProviderCapabilityObservation')],
    'freshSession': Required[ForwardRef('ProviderCapabilityObservation')],
    'freshness': Required[ForwardRef('CapabilityFreshness')],
    'interrupt': Required[ForwardRef('ProviderCapabilityObservation')],
    'nativeSessionArchive': NotRequired[ForwardRef('ProviderCapabilityObservation') | None],
    'nativeSessionCatalogue': Required[ForwardRef('ProviderCapabilityObservation')],
    'nativeSessionDelete': NotRequired[ForwardRef('ProviderCapabilityObservation') | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'resume': Required[ForwardRef('ProviderCapabilityObservation')],
    'setModel': NotRequired[ForwardRef('ProviderCapabilityObservation') | None],
    'setReasoningEffort': NotRequired[ForwardRef('ProviderCapabilityObservation') | None],
    'structuredEvents': Required[ForwardRef('ProviderCapabilityObservation')],
})
RuntimeReasoningChoice = TypedDict('RuntimeReasoningChoice', {
    'description': Required[str],
    'id': Required[str],
})
RuntimeSessionId: TypeAlias = str
RuntimeTerminalId: TypeAlias = str
RuntimeTerminalViewId: TypeAlias = str
ServerChallenge = TypedDict('ServerChallenge', {
    'expiresAtMs': Required[int],
    'instanceId': Required[str],
    'nonce': Required[str],
    'nonceId': Required[str],
})
SessionDescriptor = TypedDict('SessionDescriptor', {
    'hot': Required[bool],
    'label': NotRequired[str | None],
    'lifecycle': Required[ForwardRef('LifecycleState')],
    'looksStuck': Required[bool],
    'memoryBytes': NotRequired[int | None],
    'nativeSessionId': NotRequired[str | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'sessionGeneration': Required[int],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'waitingOn': NotRequired[ForwardRef('WaitingOn') | None],
    'workspace': Required[str],
})
SessionIndexChangedNotification = TypedDict('SessionIndexChangedNotification', {
    'snapshot': Required[ForwardRef('ManagedSessionList')],
    'subscriptionId': Required[str],
})
SessionIndexEndReason: TypeAlias = Literal['integrationRevoked'] | Literal['authorityChanged'] | Literal['rootDenied'] | Literal['runtimeUnavailable']
SessionIndexEndedNotification = TypedDict('SessionIndexEndedNotification', {
    'reason': Required[ForwardRef('SessionIndexEndReason')],
    'subscriptionId': Required[str],
})
SessionOpenResult = TypedDict('SessionOpenResult', {
    'control': Required[ForwardRef('ControlLease')],
    'session': Required[ForwardRef('SessionDescriptor')],
})
SessionWorkspaceAccess: TypeAlias = Literal['exclusive'] | Literal['shared']
SetModeParams = TypedDict('SetModeParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'mode': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
SetModelParams = TypedDict('SetModelParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'model': Required[str],
    'reasoningEffort': NotRequired[str | None],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
StartSessionParams = TypedDict('StartSessionParams', {
    'access': Required[ForwardRef('SessionWorkspaceAccess')],
    'model': NotRequired[str | None],
    'permission': NotRequired[str | None],
    'providerId': Required[ForwardRef('ProviderId')],
    'reasoningEffort': NotRequired[str | None],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'workspace': Required[str],
})
SubmitBlocksParams = TypedDict('SubmitBlocksParams', {
    'blocks': Required[list[ForwardRef('PublicInputBlock')]],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
SubmitInputParams = TypedDict('SubmitInputParams', {
    'input': Required[str],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
SuccessResponse = TypedDict('SuccessResponse', {
    'id': Required[ForwardRef('JsonRpcId')],
    'jsonrpc': Required[str],
    'result': Required[JsonValue],
})
TerminalAcquireControlParams = TypedDict('TerminalAcquireControlParams', {
    'expectedTerminalGeneration': Required[int],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalAttachParams = TypedDict('TerminalAttachParams', {
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalControlLease = TypedDict('TerminalControlLease', {
    'expiresAtMs': Required[int],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'terminalGeneration': Required[int],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalControlParams = TypedDict('TerminalControlParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalDescriptor = TypedDict('TerminalDescriptor', {
    'geometry': Required[ForwardRef('TerminalGeometry')],
    'memoryBytes': NotRequired[int | None],
    'nativeSessionId': NotRequired[str | None],
    'openedAtMs': Required[int],
    'processState': Required[ForwardRef('TerminalProcessState')],
    'providerId': Required[ForwardRef('ProviderId')],
    'runtimeGeneration': Required[str],
    'terminalGeneration': Required[int],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
    'workspace': Required[str],
})
TerminalDetachParams = TypedDict('TerminalDetachParams', {
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
    'viewId': Required[ForwardRef('RuntimeTerminalViewId')],
})
TerminalExitedNotification = TypedDict('TerminalExitedNotification', {
    'exitCode': Required[int],
    'viewId': Required[ForwardRef('RuntimeTerminalViewId')],
})
TerminalGeometry = TypedDict('TerminalGeometry', {
    'columns': Required[int],
    'rows': Required[int],
})
TerminalIndexChangedNotification = TypedDict('TerminalIndexChangedNotification', {
    'snapshot': Required[ForwardRef('TerminalIndexSnapshot')],
    'subscriptionId': Required[str],
})
TerminalIndexEndReason: TypeAlias = Literal['integrationRevoked'] | Literal['authorityChanged'] | Literal['runtimeUnavailable']
TerminalIndexEndedNotification = TypedDict('TerminalIndexEndedNotification', {
    'reason': Required[ForwardRef('TerminalIndexEndReason')],
    'subscriptionId': Required[str],
})
TerminalIndexSnapshot = TypedDict('TerminalIndexSnapshot', {
    'terminals': Required[list[ForwardRef('TerminalDescriptor')]],
    'warnings': Required[list[str]],
})
TerminalLaggedNotification = TypedDict('TerminalLaggedNotification', {
    'lostChunks': Required[int],
    'nextSequence': Required[int],
    'screenBase64': Required[str],
    'viewId': Required[ForwardRef('RuntimeTerminalViewId')],
})
TerminalOpenParams = TypedDict('TerminalOpenParams', {
    'geometry': Required[ForwardRef('TerminalGeometry')],
    'providerId': Required[ForwardRef('ProviderId')],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'target': Required[ForwardRef('TerminalOpenTarget')],
    'workspace': Required[str],
})
TerminalOpenTarget: TypeAlias = JsonObject | JsonObject
TerminalOutputNotification = TypedDict('TerminalOutputNotification', {
    'bytesBase64': Required[str],
    'sequence': Required[int],
    'viewId': Required[ForwardRef('RuntimeTerminalViewId')],
})
TerminalProcessState: TypeAlias = Literal['running'] | Literal['stopping']
TerminalResizeParams = TypedDict('TerminalResizeParams', {
    'geometry': Required[ForwardRef('TerminalGeometry')],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalStopParams = TypedDict('TerminalStopParams', {
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
TerminalViewOpened = TypedDict('TerminalViewOpened', {
    'controlLease': NotRequired[ForwardRef('TerminalControlLease') | None],
    'screenBase64': Required[str],
    'terminal': Required[ForwardRef('TerminalDescriptor')],
    'viewId': Required[ForwardRef('RuntimeTerminalViewId')],
})
TerminalWriteParams = TypedDict('TerminalWriteParams', {
    'bytesBase64': Required[str],
    'leaseGeneration': Required[int],
    'leaseId': Required[str],
    'requestId': Required[ForwardRef('MutationRequestId')],
    'terminalId': Required[ForwardRef('RuntimeTerminalId')],
})
WaitingOn: TypeAlias = Literal['person'] | Literal['quota']
WatchEnrollmentParams = TypedDict('WatchEnrollmentParams', {
    'pendingId': Required[ForwardRef('PendingEnrollmentId')],
})
WatchEventsParams = TypedDict('WatchEventsParams', {
    'after': NotRequired[ForwardRef('EventCursor') | None],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
})
WatchEventsResult = TypedDict('WatchEventsResult', {
    'gap': NotRequired[ForwardRef('EventGap') | None],
    'liveAt': Required[ForwardRef('EventCursor')],
    'sessionId': Required[ForwardRef('RuntimeSessionId')],
    'startsAt': Required[ForwardRef('EventCursor')],
    'subscriptionId': Required[str],
})
WatchProvidersParams: TypeAlias = JsonObject
WatchProvidersResult = TypedDict('WatchProvidersResult', {
    'snapshot': Required[ForwardRef('ProviderList')],
    'subscriptionId': Required[str],
})
WatchSessionIndexParams: TypeAlias = JsonObject
WatchSessionIndexResult = TypedDict('WatchSessionIndexResult', {
    'snapshot': Required[ForwardRef('ManagedSessionList')],
    'subscriptionId': Required[str],
})
WatchTerminalIndexParams: TypeAlias = JsonObject
WatchTerminalIndexResult = TypedDict('WatchTerminalIndexResult', {
    'snapshot': Required[ForwardRef('TerminalIndexSnapshot')],
    'subscriptionId': Required[str],
})
