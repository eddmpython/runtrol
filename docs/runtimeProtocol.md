# Runtime protocol

## Authority

The public Runtime protocol is a provider-neutral JSON-RPC 2.0 contract for local products. The Rust DTOs in
`runtrol-runtime-protocol` are the source of truth. They deterministically produce
`schema/runtime.schema.json`, which is copied byte for byte into the TypeScript package and standalone Runtime
archives.

The private control wire remains a separate exact-version administration surface. Public consumers cannot import or
reach private Core, daemon, IPC, store, driver, or VS Code extension modules.

The only finalized revision in release 0.1.1 is `2026-08-13`. Client and Runtime exchange their implemented revision
lists and select the newest common value. No common value returns `protocolIncompatible` before enrollment,
inventory, or mutation. Until three finalized revisions exist, the support matrix contains only revisions that have
actually shipped.

### The initialization exchange is permanently compatible

`runtime/initialize` is the one exchange every shipped version must be able to complete with every other, because
version skew is only discoverable by talking. Its types therefore never reject unknown fields, and every field added
after a revision was finalized carries a default, so absence means the feature does not exist on that side rather
than that the message is invalid. Adding a required field to this exchange inside a finalized revision is a breaking
change to every installed Runtime, which is exactly the defect that shipped in 0.1.8 and reached users.

Two gates hold this. `hello_corpus/` keeps one fixture per shipped shape of the initialization result and requires
both the Rust types and the TypeScript validator to accept all of them, and requires the current shape to be present
so a new field cannot be added without recording it. `shippedRuntimeInterop` downloads the Runtime binaries out of
the last published packages, runs each one, and requires this build's client to complete a real initialization
against it before the release pipeline will publish.

Everything after the initialization exchange may assume both ends are the same build, because a manager that
installed the Runtime rolls an older daemon forward before using it. See `docs/coreRuntime.md`.

## Local transport and locator

Runtime publishes an owner-readable `runtime.locator.json` only after its public endpoint is ready. The locator names
an opaque endpoint, Runtime instance ID, product version, start time, and process ID. It is bootstrap data, not
authority.

The Rust and TypeScript SDKs derive the platform state directory, cap the locator at 8 KiB, reject links and malformed
records, verify owner-only permissions, and validate the endpoint kind. Windows uses an owner-only named pipe. macOS
and Linux use an owner-only Unix domain socket. There is no public TCP or HTTP listener.

Each frame is a 32-bit big-endian byte length followed by UTF-8 JSON. The frame ceiling is 16 MiB plus 64 KiB. Input
has a separate 1 MiB ceiling. SDK readers validate lengths before allocation.

## Initialization and identity

Runtime sends `runtime/challenge` first. The client signs a canonical payload containing the Runtime instance, nonce,
revision offer, client information, capabilities, and current integration generations. Runtime accepts each nonce
once and rejects expired, replayed, or connection-mismatched proofs. Runtime still expires the challenge after 60
seconds. Both SDKs allow at most five additional seconds only when checking the challenge's future wall-clock bound,
so small local clock-resolution or adjustment differences cannot reject a fresh challenge. An already expired
challenge receives no tolerance.

The client then sends `runtime/initialize`. Runtime returns the selected revision, instance information, public
capabilities, numeric limits, and the current grant when authentication succeeds. The client must send
`runtime/initialized` before any ordinary request.

An unenrolled identity may call `integrations/requestEnrollment` and `integrations/watchEnrollment`. Approval,
denial, grant changes, and revocation happen through the local administration surface in Runtrol Studio. A public
connection cannot approve itself.

## Methods and scopes

| Methods | Required public authority |
|---|---|
| `providers/list`, `providers/watch`, `providers/getCapabilities`, `providers/usage` | `provider.read` |
| `providers/listModels` | `model.read` |
| `providers/listNativeSessions` | `session.native.discover`, plus an approved root when one is named |
| `sessions/list`, `sessions/watchIndex`, `sessions/get` | `session.list` |
| `sessions/start` | `session.start` plus an approved root |
| `sessions/adoptNative`, `sessions/resume` | `session.resume` plus an approved root |
| `sessions/acquireControl`, `sessions/submitInput`, `sessions/submitBlocks`, `sessions/setModel`, `sessions/setMode` | `session.input.write` and the current lease where applicable |
| `sessions/watchEvents`, `approvals/listPending` | `session.output.read` and authorized session visibility |
| `sessions/interrupt`, `sessions/cool` | `session.stop` and the current lease |
| `sessions/forget` | `session.delete`, a cold session, and exact local confirmation |
| `approvals/respond` | `approval.respond.low` or `approval.respond.high`, plus the current lease |
| `sessions/renewControl`, `sessions/releaseControl` | The current lease generation |
| `integrations/getGrant`, `integrations/rotateKey` | The authenticated integration, with local confirmation for rotation |
| `runtime/panicStop` | Same-user transport admission, no request arguments |

Notifications are `providers/changed`, `providers/watchEnded`, `sessions/indexChanged`, `sessions/indexEnded`,
`sessions/event`, and `sessions/lagged`. Notification names cannot be invoked as requests.

### Native discovery scope

`providers/listNativeSessions` takes an optional `root`. Naming one keeps the old behaviour exactly:
the folder must be in the caller's grant, and rows outside it are dropped. Omitting it asks the
provider for every conversation it will name, which four of the five measured CLIs answer directly
because their own listing treats the working directory as a filter rather than a required argument.
Each returned row carries its own folder, so grouping stays a fact the provider reported.

A folderless request is answered on the owner-only local endpoint, where a caller already holds
machine-wide authority through the private administration wire, and where the managed session index
made the same move for the same reason. It is refused by name for a provider whose own surface
cannot enumerate without a folder, so the caller knows to ask per folder rather than receiving one
folder's worth that reads as everything. Catalogue cursors bind the scope they were issued for, so a
machine-wide cursor cannot be replayed into a folder listing or the other way round.

## Session and mutation rules

Runtime stores only supervision metadata and provider-native pointers. It never stores a conversation copy. Native
session discovery uses an official provider command or protocol registered by the provider extension. Runtime never
scans provider storage for conversation files.

A session in the index carries `waitingOn` when Core observed its running turn stop for something. `person` means a
pending approval or a request for free-form input, and `quota` means an account limit. Both are derived from the
provider's own structural turn frames and carry no approval identifier or provider wording, so a client can rank a
list of running sessions without reading any conversation. The field is omitted when the turn is not waiting and is
always cleared when the turn ends, so it cannot outlive what it describes. A client written before the field existed
reads the same index it always did.

`sessions/start` accepts optional opaque `model` and `reasoningEffort` values previously returned by
`providers/listModels`. Runtime bounds both values and refreshes the selected provider catalogue immediately before
launch. A missing or stale explicit value returns `modelUnavailable`; absent values leave the provider's own defaults
in control. Runtime does not accept arbitrary CLI flags through this public boundary.

One renewable control lease authorizes writes to a hot session. Lease ID and generation bind input, interrupt, cool,
and approval operations. Disconnect does not transfer control. A stale lease returns a typed conflict or expiry.

Every state-changing method carries a UUIDv7 mutation request ID. Runtime binds the ID to the integration, method,
target, and a keyed authenticator over sensitive parameters. An exact duplicate receives the recorded result. Reuse
with different parameters returns `idempotencyConflict`. If delivery may have happened after the retention boundary,
Runtime returns `outcomeUnknown` and never repeats the provider mutation automatically.

## Streams

Provider and managed-session watchers begin with a complete snapshot and then emit changed complete snapshots. Event
watchers use `EventCursor { stream, epoch, seq }`. The start result states `startsAt`, `liveAt`, and any explicit gap.
The consumer accepts a cursor only after it has consumed the event. Reconnect resumes from that accepted cursor.

Queues and replay are bounded. Lag ends or marks the subscription with the first unavailable cursor. The SDK never
accumulates an unbounded transcript and never treats provider source offsets as reconnect cursors.

## Errors and compatibility

Public failures contain a stable `code`, `retryable`, optional `operatorAction`, bounded safe `details`, and a
correlation ID. Consumers branch on these fields, never message text. The current machine error inventory includes
installation, availability, protocol, enrollment, authorization, root, provider, capability, model, native catalogue,
session, control, lease, workspace, approval, idempotency, ambiguity, limit, rate, gap, request, method, and internal
failures.

```text
runtimeNotInstalled runtimeUnavailable protocolIncompatible notInitialized unauthenticated
enrollmentPending enrollmentDenied integrationRevoked scopeDenied presenceRequired rootDenied
providerUnavailable capabilityUnavailable modelUnavailable nativeCatalogueUnsupported sessionNotFound
sessionConflict controlConflict leaseExpired workspaceConflict approvalExpired approvalOptionInvalid
idempotencyConflict outcomeUnknown resourceExhausted rateLimited gap invalidRequest methodNotFound internal
```

Package SemVer describes a language API. Protocol revision negotiation describes the wire. An SDK update does not
replace Runtime, and a Runtime update does not rewrite consumer dependencies. Additive capability is advertised
structurally. Changes to meaning, authority, ordering, identifiers, cursor behavior, or data disclosure require a new
finalized revision.

The exact schema and revision inventory ship in every Runtime archive. Release metadata records the minimum
rollback-safe store schema. Current release compatibility is:

| Runtime | Rust SDK | TypeScript SDK | Finalized revision | Store rollback floor |
|---|---|---|---|---|
| 0.1.1 | 0.1.1 | 0.1.1 | `2026-08-13` | 1 |
