# Provider and session discovery

## Discovery promise

Runtime converts provider-specific discovery into structural, capability-driven catalogues without claiming knowledge
the provider has not exposed officially. Fast local inventory, slow provider negotiation, model discovery, managed
sessions, and provider-native sessions are separate operations with separate freshness and cost.

No consumer may cause Core to guess a binary, model, flag, permission mode, session path, transcript layout, or resume
syntax.

## Discovery layers

| Layer | Source | Process cost | Public result |
|---|---|---|---|
| Installation inventory | Existing provider registry and discovery ladder | No provider process required | Provider descriptor and observed installation state |
| Capability probe | Exact discovered binary and official protocol or help surface | Explicit, bounded, cached | Structured capability map with provenance and freshness |
| Model catalogue | Official runtime discovery surface | Explicit and potentially slow | Opaque model options with coverage |
| Managed session catalogue | Runtrol metadata plus live Core state | Immediate | Sessions Runtrol already supervises or can resume |
| Native session catalogue | Official provider enumerable session surface | Explicit, paginated, potentially slow | Provider-native sessions with honest coverage |

The public API never hides a slow probe behind `providers/list` or `sessions/list`.

## Provider descriptor

A provider descriptor contains structural observations only:

```json
{
  "providerId": "opaque-runtime-provider-id",
  "displayName": "Provider supplied or manifest label",
  "installation": {
    "state": "usable",
    "version": "opaque-observed-version",
    "binaryIdentity": "bin_01...",
    "source": "registeredDiscoveryLadder"
  },
  "capabilityFreshness": "stale",
  "capabilities": {
    "structuredEvents": "unknown",
    "resume": "unknown",
    "nativeSessionCatalogue": "unknown"
  }
}
```

`ProviderId` is assigned from registered discovery identity and remains opaque to consumers. It is not a public enum
and is never used as an authorization boundary by itself. A missing or unusable installation remains visible with a
safe typed reason so products can explain remediation without parsing stderr.

## Fast inventory

`providers/list` reads the existing registered provider manifests and cached installation observations. It does not
spawn every provider. The initial performance target is p95 at or below 50 ms for the measured supported-provider
set on a warm local Runtime, then frozen by the attempt.

Inventory invalidation follows platform filesystem notifications and bounded fallback checks. It does not poll while
idle. A provider list watch sends one snapshot and later structural changes, never raw path-scanner output.

## Capability probing

Capability probing is lazy and keyed by exact binary identity, version observation, driver version, and environment
facts that affect the official surface. It has:

- single-flight execution per provider identity
- explicit deadline and output byte limit
- sanitized structured result
- negative cache with a bounded shorter lifetime
- invalidation after binary drift, manifest change, or incompatible response
- no fallback from a structured surface to transcript storage inspection

A consumer can request fresh probing. Runtime may return the last safe result marked stale while one refresh runs,
but it cannot present a stale model or capability as current without the freshness marker.

## Model catalogue

Models are provider-owned opaque options. Public results distinguish:

| Coverage | Meaning |
|---|---|
| `known` | Official surface returned a bounded authoritative list for the current context |
| `partial` | Official surface returned a structurally limited list and Runtime can name that limit |
| `aliases` | Provider exposes selectable aliases but not a complete model inventory |
| `unknown` | Runtime has not probed or the answer cannot be interpreted structurally |
| `unsupported` | No registered official discovery capability exists |

Runtime does not scrape marketing pages, hardcode model IDs, infer entitlement, or promise that an option remains
available when a provider starts. Start and resume still accept only the returned opaque selection and report a typed
provider rejection if provider state changed.

## Managed sessions

The managed catalogue is the fast product surface. It joins Runtrol session metadata with current Core lifecycle and
workspace state. It contains only sessions previously started or adopted through Runtrol.

It returns:

- stable Runtrol `SessionId`
- opaque provider and provider-native IDs
- project and working-tree identity
- user-owned label and provider-owned title as distinct fields
- lifecycle, turn generation, controller, and resumability
- last operational activity time without conversation-derived summarization
- structural health and provider availability

It does not scan provider storage and does not claim to be the provider's full history.

## Provider-native sessions

Native discovery is an explicit provider operation. It is available only when the active provider driver has a
registered official enumerable surface.

The result contains:

- `coverage.kind` as `complete`, `partial`, or `unsupported`
- provenance such as `officialProtocol` or `officialCli`
- a structural explanation when partial
- a bounded page of opaque native session IDs
- provider-owned `cwd`, additional directories, title, and updated time when officially supplied
- discovered resume capability
- a merge pointer when the native session is already managed by Runtrol
- opaque next cursor or null

No entry contains transcript content, prompt preview, reply preview, derived topic, or Runtime-generated title.

## ACP session catalogue

For an ACP provider that advertises `sessionCapabilities.list`, the driver maps the stabilized `session/list` result
to the Runtime native catalogue. ACP pagination cursors remain opaque. Official `session/resume` capability controls
whether `sessions/adoptNative` is offered.

ACP is optional at the provider edge. Runtime does not pretend all providers implement it and does not make ACP agent
names part of the consumer API. Relevant protocol sources are:

- [ACP session/list stabilization](https://agentclientprotocol.com/announcements/session-list-stabilized)
- [ACP session/list specification](https://agentclientprotocol.com/rfds/session-list)
- [ACP session/resume specification](https://agentclientprotocol.com/rfds/session-resume)
- [ACP protocol updates](https://agentclientprotocol.com/updates)

The Runtime adapter ignores no required ACP field silently. Unsupported metadata stays in the bounded extension area
or causes an explicit capability limitation.

## Non-ACP official catalogues

A non-ACP provider may expose an official structured CLI command or protocol method. The provider driver declares:

- discovery command or handshake derived at runtime
- supported provider version evidence
- exact field mapping and cursor behavior
- completeness semantics
- resume capability and acknowledgement semantics
- bounded output and timeout
- drift fixtures and real-provider evidence

Human-formatted help, log text, terminal screen scraping, private databases, cache directories, and reverse-engineered
transcript paths are not official catalogues.

## Coverage semantics

Coverage is about the provider result, not Runtime confidence prose.

`complete` means the official capability claims the returned pagination spans every session visible to that provider
identity and current context. Runtime still does not claim access to another account, machine, deleted item, or an
unavailable provider scope.

`partial` requires a stable reason, for example:

- official endpoint exposes only the current workspace
- result is limited to a provider-defined recent window
- some returned sessions omit workspace and therefore cannot be adopted under a root grant
- provider exposes enumeration but not resume

`unsupported` means no safe registered official capability exists. It is a normal product state.

An execution failure is not converted to `unsupported`. It returns a typed provider or resource error and preserves
the last cache only with its stale marker.

## Pagination and bounds

Runtime applies its own page item and byte limits even when a provider allows more. Provider cursors are:

- opaque to Runtime except for bounded transport wrapping
- scoped to provider identity, discovery context, and integration root grant
- rejected after expiry or binary drift
- never logged or used as a durable session pointer
- capped before allocation and serialization

Runtime may fetch less than the provider maximum. It never drains every provider page in the background to build a
shadow catalogue.

## Workspace filtering

Native entries are filtered through approved canonical roots before disclosure. Runtime resolves provider-supplied
paths using the same project identity rules as session admission. An unresolvable path is omitted with an aggregate
partial reason or returned without adopt authority according to the measured privacy policy. It is never accepted as
authority because its string has an approved prefix.

The consumer cannot broaden discovery by sending a raw root not present in its grant. Symlink, junction, case,
worktree, submodule, and non-Git behavior matches Core.

## Merge and adoption

Runtime deduplicates a native entry against managed metadata by provider identity plus opaque native session ID. It
does not compare titles, timestamps, transcript text, or filesystem filenames.

`sessions/adoptNative` performs a fresh capability and workspace check, acquires ordinary workspace admission, asks
the provider to resume through its official surface, and only then creates or reconnects the Runtrol session pointer.
Listing a native session never mutates it.

If resume fails after provider-side work may have occurred, Runtime returns `outcomeUnknown` and does not create a
second provider-native session automatically.

## Provider drift

Every provider discovery implementation has mutation fixtures for:

- binary missing, moved, or replaced
- version output changed
- capability removed or renamed
- unknown enum and extension field
- malformed UTF-8 or structured payload
- output above the byte ceiling
- hung command or handshake
- pagination cursor repeated forever
- duplicate and reordered native session IDs
- missing or escaping workspace path
- resume advertised but rejected
- provider update during an active catalogue request

Drift can reduce a capability to unknown, partial, or unavailable. It cannot activate guessed fallback behavior.

## New-provider invariant

A provider addition passes only when all consumer-facing behavior works through existing structural contracts. The
allowed product changes are the provider manifest or driver, provider fixtures, and provider-specific gates.

The following files must show no provider-triggered change:

- public Runtime protocol methods and DTO categories
- Core session lifecycle and ownership rules
- Rust and TypeScript client method names
- Runtrol Studio provider and session UI branching
- independent consumer samples

The audit rejects string comparisons against provider IDs outside the provider registry and driver boundary.

## Privacy and retention

Runtime retains installation observations and managed session pointers needed for supervision. Native catalogue pages
are response data, not a background index. A bounded cache may retain structural entries only for a measured short
period and must be root-filtered before storage or keyed so one integration cannot reveal another integration's
catalogue.

No discovery cache contains prompt text, reply text, tool arguments, tool output, transcript paths, raw environment,
credentials, or provider command output.

