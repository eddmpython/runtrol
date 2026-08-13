# Compatibility and lifecycle

## Compatibility principle

The internal control wire and public Runtime protocol serve different release contracts.

The internal wire remains exact-version and lockstep because its callers ship in one Runtrol release image. The public
protocol negotiates a revision because independent consumer products and Runtime installations update on different
schedules.

No public SDK simulates compatibility by checking only the Runtime product version.

## Revision negotiation

The client sends an ordered set of protocol revisions it implements. Runtime selects the newest common finalized
revision and returns the capability map and numeric limits for that revision.

```text
client:  [2026-12-01, 2026-08-13]
server:  [2026-08-13, 2026-05-01]
chosen:   2026-08-13
```

No intersection returns `protocolIncompatible` with safe product versions and supported revision ranges. It does not
enroll, mutate state, start a provider, download an update, or fall back to the private wire.

Revision identifiers are dates of finalized public contracts, not build dates. Draft revisions are available only in
explicit prerelease SDK and Runtime channels and never enter the stable support promise.

## Support window

After v1, a stable Runtime release supports its current finalized public revision and the previous two finalized
revisions unless a published security retirement applies. A stable SDK supports the same window from the client side.

Before freezing this number, the compatibility attempt measures maintenance cost across real additive and breaking
changes. A shorter window requires product review and independent consumer migration evidence. A longer window cannot
retain unsafe behavior, unbounded memory, provider hardcoding, or transcript storage.

Each release publishes:

- product version
- supported finalized revisions
- default selected revision
- deprecated methods and capabilities by revision
- earliest security retirement date, if any
- minimum store schema safe for rollback
- tested SDK package version matrix

## Additive and breaking change policy

An additive change inside a revision may add only:

- an optional advertised capability
- an optional method reachable only when that capability is present
- a bounded opaque extension field in an already open extension object
- a new optional event extension that old clients may preserve or ignore by contract

A new revision is required for:

- changing method meaning, authorization, ordering, or idempotency
- removing or renaming a method, field, scope, error, or event kind
- making an optional field required
- changing identifier or cursor semantics
- widening data disclosure
- changing control lease, approval, workspace, or input ambiguity behavior
- changing a closed security object
- changing an existing limit from a safe failure into silent truncation

Capability presence never weakens the selected revision's security rules.

## Event compatibility

Every event carries an event revision compatible with the selected protocol revision. Structural event variants are a
discriminated union.

Unknown optional provider extensions remain opaque within advertised bounds. An unknown required event kind or a
known kind missing a required field retires the subscription with `eventRevisionUnsupported`. The server never drops
it and continues as if the client had a complete stream.

Clients persist only the last accepted bounded cursor needed for reconnect. They do not persist events as protocol
migration state.

## Capability lifecycle

Capabilities have `unavailable`, `available`, and where necessary `degraded` structural states. A provider capability
may disappear after binary drift. A Runtime product capability may be unavailable in an older selected revision.

The SDK checks both negotiated server capability and current provider capability before sending an optional request.
Runtime repeats the authoritative check during dispatch. A stale client receives `capabilityUnavailable`, not an
undocumented fallback.

## SDK SemVer

SDK SemVer describes the language API. Protocol revision negotiation describes the wire. They are related but not
substitutes.

- Patch releases fix implementation defects without changing documented public behavior.
- Minor releases may add optional APIs and support a new finalized revision while retaining the declared window.
- Major releases may remove language APIs or old revision implementations after the public retirement process.

A Runtime product major release does not force an SDK major release unless the protocol support window is also
retired through policy.

## Runtime update

Runtime update follows the existing signed, target-specific update plan. Before activation, the updater verifies:

- artifact signature and checksum
- operating system and architecture
- public revision inventory
- store migration and rollback compatibility
- locator schema compatibility
- provider process and session handoff policy

The update drains public request admission, emits a bounded restart notice, preserves supervised session truth, and
activates atomically. SDKs reconnect through the locator, reinitialize, revalidate grants, and restore read
subscriptions from cursors.

No update automatically resubmits input, reacquires write control, answers an approval, or restarts an ambiguous
provider mutation.

## Independent rollback

The compatibility matrix includes four directions:

| Change | Required result |
|---|---|
| Upgrade Runtime, keep consumer | Common revision continues or typed incompatibility occurs before mutation |
| Roll back Runtime, keep consumer | Previous revision selected and grant remains valid if schema is rollback-safe |
| Upgrade SDK or consumer, keep Runtime | Client selects common older revision without hidden feature emulation |
| Roll back SDK or consumer, keep Runtime | Runtime serves declared older revision and rejects unavailable capabilities |

Rollback never reads provider transcripts to rebuild state. Store migrations follow the product update rollback floor.
If a migration is not backward-readable, activation must preserve a verified backup and expose a clear rollback limit
before proceeding.

## Locator lifecycle

Runtime writes the locator only after the public endpoint is ready. It replaces the record atomically and removes it
only when it can prove ownership of the matching instance.

The SDK validates:

- locator schema and byte limit
- owner and permissions
- platform endpoint form
- Runtime process or peer proof where reliable
- initialization instance ID match
- freshness without trusting process ID reuse

A crashed Runtime may leave a stale locator. The next verified Runtime start repairs it. A consumer cannot overwrite
or delete a suspicious locator through an unrestricted SDK call.

## Daemon singleton and restart

There is one Runtime daemon per OS user and canonical Runtrol home. Singleton admission uses the existing safe owner
lock and endpoint ownership rules. Two consumers starting simultaneously either connect to the winner or receive a
typed temporary state. They never each create a private Runtime.

On restart, Core restores managed session metadata and reconciles provider processes according to existing lifecycle
truth. Public app grants survive. Client instances, sockets, subscriptions, and server nonces do not. Control leases
are restored only if the crash-safety attempt proves no double-controller window; otherwise they expire
conservatively.

## Consumer crash and uninstall

A consumer crash closes its connections and subscriptions. It does not stop Runtime, kill a provider, forget a
session, release workspace admission, or delete the provider-native conversation.

The short control lease remains until expiry. During an active turn it becomes orphaned rather than transferring
automatically.

Consumer uninstall should revoke its Runtime integration through a user-visible local action, then remove its private
key and files. If the product is already gone, the operator can revoke it in Studio or the local CLI. Runtime never
executes a consumer-provided uninstall callback.

## Runtime uninstall

Runtime uninstall follows the repository uninstall contract:

1. show active sessions, enrolled integrations, and retained metadata classes
2. stop or detach provider supervision through explicit product policy
3. remove binaries, locator, endpoints, grants, caches, and Runtime-owned metadata
4. leave provider installations, provider authentication, and provider-owned conversations untouched
5. emit a machine-verifiable uninstall result

An external consumer later sees `runtimeNotInstalled`. It does not silently install or recreate Runtime.

## Integration grant lifecycle

Grant changes are generation-based:

- ordinary reconnect with the same key reads the current grant
- root or scope narrowing applies immediately
- widening requires local approval
- key rotation creates a new key generation
- revocation closes connections and prevents renewal
- deleted integration identifiers are never reassigned

Protocol revision upgrades do not imply new scopes. A new method remains unavailable until both capability and grant
permit it.

## Deprecation

A stable public method, field, scope, error, or behavior is deprecated only with:

- replacement and migration guide
- telemetry-free local compatibility evidence or opt-in aggregate product metrics
- first and last Runtime and SDK versions supporting it
- earliest removal revision and date
- repository-external consumer fixture proving the migration
- product review of user-visible breakage

Deprecation warnings are structural SDK diagnostics. Runtime does not inject warning text into provider conversations.

## Security retirement

A vulnerable revision may be retired before the normal window only when continuing it creates a concrete security
risk. The release publishes the affected contract, safe replacement, minimum versions, and operator action.

Retirement fails initialization before inventory or mutation. It does not silently select an older unsafe revision.
Emergency update remains user-visible and signed.

## Standards facade lifecycle

An optional ACP facade is implemented above the native Runtime client after native v1 behavior passes. It maps only
the expressible provider-neutral subset and publishes feature loss for integration grants, root selection, control
leases, Runtime inventory, and Runtime-specific errors.

The facade follows ACP's own version and capability negotiation. The native Runtime revision is still negotiated on
its downstream connection. Neither side is allowed to infer the other's capabilities.

MCP lifecycle negotiation is useful precedent for explicit version and capability exchange, but MCP is not the
primary Runtime session contract. Relevant lifecycle guidance is the official
[MCP lifecycle specification](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle).

## Compatibility evidence

Every release candidate runs a matrix containing:

- current Runtime against current and previous two SDK protocol implementations
- previous two stable Runtime releases against current SDK
- independent Rust and TypeScript consumers
- Runtime upgrade and rollback with live managed sessions
- SDK reconnect through Runtime restart
- grant narrow, widen, rotate, and revoke across supported revisions
- unknown optional capability and event extension
- unknown required event kind
- provider capability loss without Runtime revision change
- stale, replaced, malformed, and permission-weakened locator
- store schema forward and rollback floor

The matrix uses packed or published artifacts, not workspace dependencies.

