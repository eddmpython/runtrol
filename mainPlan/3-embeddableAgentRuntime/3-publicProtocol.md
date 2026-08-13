# Public Runtime protocol

## Separation from internal control IPC

The current control wire is optimized for one release image: one-byte exact version agreement, private Rust request
types, and local-at-machine authority. It remains private for administration and bootstrap.

The public Runtime protocol has a separate endpoint, package, schema, compatibility policy, and caller identity. One
daemon serves both endpoints and one Core owns all sessions.

```text
private control endpoint -> CLI administration, repair, physical presence
public runtime endpoint  -> Studio and enrolled external consumers
```

No public client imports `runtrol-ipc`, `runtrol-daemon`, `runtrol-core`, or private TypeScript files from the VS Code
extension.

## Transport

| OS | Endpoint | Admission floor |
|---|---|---|
| Windows | Separate named pipe derived from canonical Runtrol home | Owner SID DACL, network deny, remote-client reject, peer process observation |
| Linux | Unix domain socket inside mode 0700 Runtime home | Socket mode 0600 and peer UID equality |
| macOS | Unix domain socket inside per-user Runtime home | Socket mode 0600 and peer UID equality |

There is no TCP, HTTP, WebSocket, CORS, cookie, or browser endpoint for local third-party integrations. The PWA uses
its separate paired encrypted transport and device scope wall.

The binary frame is:

```text
u32 big-endian payload length
payload length bytes of UTF-8 JSON
```

The initial maximum frame is the existing measured `16 MiB + 64 KiB` envelope ceiling. Runtime event admission keeps
the stricter existing 1 MiB live provider payload ceiling. Oversize input or response fails before allocation beyond
the declared frame bound. SDK readers cap the length before reserving memory.

## JSON-RPC envelope

Runtime uses JSON-RPC 2.0 request, response, notification, and error envelopes. IDs are strings or integers as allowed
by JSON-RPC, but SDKs generate monotonic connection-local integers. Product operation idempotency uses the separate
UUIDv7 `requestId` field.

Unknown methods return `methodNotFound`. Unknown fields are ignored only where the selected revision declares an open
extension object. Closed security, identity, scope, workspace, approval, and mutation objects reject unknown fields.

## Initialization

Initialization is the first request and the only unauthenticated method other than enrollment creation and panic stop.

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "runtime/initialize",
  "params": {
    "supportedRevisions": ["2026-08-13"],
    "client": {
      "integrationId": "int_01...",
      "instanceId": "cli_01...",
      "name": "Example Product",
      "version": "1.4.2"
    },
    "clientCapabilities": {
      "sessionEvents": {"opaqueExtensions": true},
      "approvalPresentation": {"bidiSafe": true}
    },
    "authentication": {
      "nonceId": "nonce_01...",
      "signature": "base64url..."
    }
  }
}
```

The signature covers a canonical transcript containing endpoint instance ID, server nonce, all supported revisions,
client identity, capability object, and an expiry. A signature cannot be replayed on another Runtime instance or with
wider capabilities.

Successful initialization returns:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "selectedRevision": "2026-08-13",
    "runtime": {
      "instanceId": "rtm_01...",
      "version": "0.2.0",
      "platform": "windows-x86_64"
    },
    "serverCapabilities": {
      "providerWatch": {},
      "managedSessionWatch": {},
      "nativeSessionCatalogue": {"pagination": "opaqueCursor"},
      "controlLease": {},
      "approvalResponse": {"riskClasses": ["low"]}
    },
    "grant": {
      "integrationId": "int_01...",
      "scopes": ["provider.read", "session.list"],
      "roots": ["prj_01..."]
    },
    "limits": {
      "maxFrameBytes": 16842752,
      "maxInputBytes": 1048576,
      "maxPageItems": 100,
      "maxSubscriptions": 32
    }
  }
}
```

The client sends `runtime/initialized` notification before other methods. No inventory is returned inside
initialization, so authentication and capability negotiation stay cheap and separately measurable.

## Enrollment methods

| Method | Authentication | Purpose |
|---|---|---|
| `integrations/requestEnrollment` | Owner-only socket and proof of private-key possession | Create pending enrollment, return opaque ID |
| `integrations/watchEnrollment` | Pending key proof | Wait for approval, denial, or expiry |
| `integrations/getGrant` | Enrolled integration | Read own current scopes, roots, and revocation generation |
| `integrations/rotateKey` | Current key plus local approval | Replace public key without inheriting silently |

Approval, denial, scope change, root change, listing other integrations, and revocation are private control operations
requiring physical presence. The public endpoint cannot grant itself.

## Inventory methods

| Method | Required scope | Result |
|---|---|---|
| `providers/list` | `provider.read` | Current manifest and install observations, no slow provider start |
| `providers/watch` | `provider.read` | Snapshot then changes, bounded queue |
| `providers/getCapabilities` | `provider.read` | Discovered structured lifecycle and event capabilities |
| `providers/listModels` | `model.read` | `known`, `partial`, `aliases`, `unknown`, or `unsupported` catalogue |
| `providers/listNativeSessions` | `session.native.discover` plus root | Official provider catalogue page with honest coverage |

`providers/list` returns unusable providers with a typed reason instead of omitting them. Slow discovery is explicit.

## Managed session methods

| Method | Required scope | Notes |
|---|---|---|
| `sessions/list` | `session.list` | Immediate Runtime-managed snapshot, filterable by approved project and provider |
| `sessions/watchIndex` | `session.list` | Snapshot plus list-visible changes, no conversation payload |
| `sessions/get` | `session.list` | One exact descriptor and current control state |
| `sessions/start` | `session.start` plus root | Exact provider, workspace, access, model, and discovered options |
| `sessions/adoptNative` | `session.resume` plus root | Resume one officially discovered native session into Runtime supervision |
| `sessions/resume` | `session.resume` plus root | Heat an existing Runtime-managed cold session |
| `sessions/acquireControl` | `session.input.write` | Atomic renewable lease |
| `sessions/renewControl` | Current lease | Extend before deadline |
| `sessions/releaseControl` | Current lease | Voluntary transfer point |
| `sessions/submitInput` | Current lease and `session.input.write` | Caller bytes, exact idempotency contract |
| `sessions/watchEvents` | `session.output.read` | Bounded replay plus exact cursor and gap |
| `sessions/interrupt` | Current lease and `session.stop` | Provider-native interrupt request |
| `sessions/cool` | Current lease and `session.stop` | Release hot process only when provider state permits |
| `sessions/forget` | `session.delete` and local confirmation policy | Remove Runtime pointer, never provider transcript |

`sessions/start`, `adoptNative`, `resume`, `submitInput`, `interrupt`, `cool`, and `forget` carry `requestId` and expected
generation fields. Provider fallback is never implicit.

## Approval methods

| Method | Required authority | Contract |
|---|---|---|
| `approvals/listPending` | Current control lease and output read | Pending structured requests for the controlled session |
| `approvals/respond` | Current lease plus low or high approval scope | Exact approval ID, option ID, subject digest, expiry, and risk evaluation |

Risk class is computed from the provider-native pending request held inside the driver. It is absent from caller input,
so a consumer cannot downgrade authority. Unknown or incomplete approval subject exposes reject only.

## Session descriptor

```json
{
  "sessionId": "ses_01...",
  "providerId": "provider-runtime-id",
  "nativeSessionId": "opaque-native-id",
  "source": "managed",
  "projectId": "prj_01...",
  "workspace": {
    "displayPath": "C:/work/project",
    "workingTreeId": "wkt_01...",
    "access": "exclusive"
  },
  "label": "operator-owned label",
  "providerTitle": null,
  "lifecycle": "hotIdle",
  "turnGeneration": 12,
  "controller": {
    "integrationId": "int_01...",
    "leaseGeneration": 7,
    "expiresAt": "2026-08-13T12:00:30Z"
  },
  "capabilities": {
    "resume": "available",
    "interrupt": "available",
    "approval": "available"
  }
}
```

`displayPath` is presentation data. Clients target `projectId` plus an approved path request and never treat a display
string as authority. `providerTitle` is provider-supplied metadata and may have been derived by that provider from its
conversation. Runtime passes it without generating or parsing it.

## Native session catalogue

```json
{
  "providerId": "provider-runtime-id",
  "coverage": {
    "kind": "complete",
    "source": "officialProtocol",
    "why": null
  },
  "sessions": [
    {
      "nativeSessionId": "opaque-native-id",
      "cwd": "C:/work/project",
      "additionalDirectories": [],
      "title": "provider-owned title",
      "updatedAt": "2026-08-13T11:30:00Z",
      "resume": "available",
      "alreadyManagedAs": null
    }
  ],
  "nextCursor": null
}
```

Provider cursors remain opaque, are bounded in size, and are not durable Runtime state. Unknown provider `_meta` stays
inside a bounded extension object and is never used for authorization, workspace identity, sorting truth, or resume.

## Event stream

`sessions/watchEvents` takes an optional `EventCursor { stream, epoch, seq }`. The response installs a subscription and
returns `startsAt`, `liveAt`, and an optional explicit gap. Notifications carry:

```json
{
  "jsonrpc": "2.0",
  "method": "sessions/event",
  "params": {
    "subscriptionId": "sub_01...",
    "sessionId": "ses_01...",
    "eventRevision": "2026-08-13",
    "event": {},
    "nextExpected": {"stream": "...", "epoch": 2, "seq": 81}
  }
}
```

The event is a versioned discriminated union for structural presentation. Unknown optional extensions remain opaque.
An unknown required event kind retires the subscription with `eventRevisionUnsupported`; it is never silently dropped.

Each subscription retains the existing frame and byte bounds. A slow consumer receives `sessions/lagged` with the
first missing cursor. Runtime does not turn the bounded replay ring into history for public clients.

## Error model

Every error has stable machine fields and safe operator text:

```json
{
  "code": "workspaceConflict",
  "message": "the requested working tree already has an exclusive writer",
  "retryable": false,
  "operatorAction": "chooseAnotherWorktree",
  "details": {"projectId": "prj_01..."},
  "correlationId": "err_01..."
}
```

Initial stable error kinds:

```text
runtimeNotInstalled
runtimeUnavailable
protocolIncompatible
notInitialized
unauthenticated
enrollmentPending
enrollmentDenied
integrationRevoked
scopeDenied
presenceRequired
rootDenied
providerUnavailable
capabilityUnavailable
modelUnavailable
nativeCatalogueUnsupported
sessionNotFound
sessionConflict
controlConflict
leaseExpired
workspaceConflict
approvalExpired
approvalOptionInvalid
idempotencyConflict
outcomeUnknown
resourceExhausted
rateLimited
gap
invalidRequest
internal
```

Clients branch on `code`, `retryable`, and `operatorAction`, never `message`. `details` is closed per error kind and
contains no provider raw output, prompt, reply, environment value, token, or private path outside an approved display
surface.

## Resource limits

The server advertises limits during initialization. Clients may request less, never more. Numeric SSOT values live in
the public protocol crate and are measured before graduation.

Required limit classes:

- frame and input bytes
- connections per integration and globally
- subscriptions per connection, integration, and session
- queued frames and bytes per subscription
- provider catalogue page items and cursor bytes
- managed session page items
- concurrent slow discovery calls
- control lease count and renewal rate
- idempotency entries and retention time
- enrollment attempts and pending lifetime
- safe error text and extension object bytes

Limit exhaustion is explicit and cannot evict a running turn merely to admit a new consumer request.

## Panic stop

The existing stop-everything capability remains reachable without a grant because its safe direction is to stop work.
The public protocol exposes it only after the transport proves same-user admission, and it accepts no target or
argument. Enrollment and initialization are not required. Every narrower destructive method remains authenticated.

