# Client SDK and distribution

## Distribution contract

An integration installs a small client package. The client package locates one shared per-user Runtime, negotiates a
public protocol revision, enrolls the integration, and exposes typed operations. It does not bundle Core, start a
private daemon, inspect provider installations, or read Runtime storage.

The distribution has three independently versioned artifact families:

| Artifact | Audience | Version contract |
|---|---|---|
| `runtrol-runtime-client` Rust crate | Native applications, local companions, and conformance tools | Package SemVer plus negotiated protocol revisions |
| `@runtrol/runtime-client` TypeScript package | IDE extensions, Electron applications, and Node.js companions | Package SemVer plus negotiated protocol revisions |
| Standalone Runtrol Runtime | Users and managed desktop fleets | Product SemVer, signed target packages, and public protocol support window |

An SDK update does not update Runtime. A Runtime update does not rewrite consumer dependencies. Compatibility comes
from negotiation and the support policy, not coordinated installation.

## Public package boundary

The language-neutral protocol schema is the source of truth for all public DTOs, method names, event revisions,
limits, capabilities, scopes, and machine errors. Rust types produce the checked schema and generated TypeScript
bindings through one deterministic direction. No hand-maintained mirror may become a second authority.

The public Rust crate may depend on the public protocol crate and ordinary transport libraries. It may not depend on
`runtrol-core`, `runtrol-daemon`, `runtrol-ipc`, `runtrol-store`, provider drivers, or private security types.

The TypeScript package is built from generated bindings plus a small maintained client layer. It may not import from
`extensions/runtrol-vscode`, copy that extension's protocol declarations, or require the runtrol repository at build
or runtime.

The package export surface has four layers:

1. `locator`: find and validate an installed Runtime without starting it.
2. `connection`: connect, initialize, authenticate, reconnect, and close.
3. `client`: typed provider, session, approval, and integration operations.
4. `testing`: public fake transport and contract fixtures that never expose daemon internals.

## Locator

Runtime writes one atomic, owner-readable locator record into a platform-standard per-user state location. The SDK
derives that location from the OS. A consumer never supplies `runtrol.exe`, a socket name, a port, or a provider home.

The locator contains only operational bootstrap data:

```json
{
  "schema": 1,
  "instanceId": "rtm_01...",
  "endpointKind": "namedPipe",
  "endpoint": "opaque-platform-endpoint",
  "runtimeVersion": "0.2.0",
  "startedAt": "2026-08-13T09:00:00Z",
  "processId": 1234
}
```

The endpoint is not authority. The SDK validates file ownership and permissions, connects with platform peer checks,
and proves the server instance during initialization. A stale locator results in a typed repairable state.

Locator states are:

| State | Meaning | SDK behavior |
|---|---|---|
| `notInstalled` | No verified Runtime installation and no locator | Return `runtimeNotInstalled` |
| `installedStopped` | Verified installation exists, Runtime is not running | Offer explicit user-initiated start |
| `running` | Locator and peer validate | Connect and initialize |
| `stale` | Locator points to no matching Runtime | Remove only through the owned repair path, then start or reinstall with user intent |
| `incompatible` | Runtime is real but shares no public revision | Return versions and supported revisions without mutating either product |
| `unsafe` | Ownership, permissions, peer, or instance proof fails | Refuse connection and require local repair |

The SDK does not search the whole filesystem, PATH-spawn an arbitrary binary, guess a well-known TCP port, or trust an
environment variable in production. Test-only locator injection is compiled or exported under an explicit testing
surface.

## Runtime installation journey

`runtimeNotInstalled` is a product state, not permission to download software silently.

1. The host application presents an `Install Runtrol Runtime` action initiated by the user.
2. The host resolves the official release manifest through its own update or installer UI.
3. The manifest signature, target, checksum, and minimum bootstrap version are verified before execution.
4. The installer writes a per-user installation without administrator rights where the platform permits it.
5. Runtime starts, publishes its locator atomically, and enrollment begins.
6. Failure leaves the previous verified installation and consumer app intact.

Products may direct users to a separately installed Runtime instead. The SDK does not force one vendor-specific
installation channel.

## Rust client shape

The Rust API is asynchronous, cancellation-safe, and transport-agnostic at the public test seam.

```rust
let runtime = RuntimeLocator::system()
    .connect(ClientOptions::new(identity))
    .await?;

let client = runtime.initialize(SUPPORTED_REVISIONS).await?;
let providers = client.providers().list(Default::default()).await?;
let sessions = client.sessions().list(Default::default()).await?;
```

Public identifiers are newtypes. Open provider values and extension objects remain bounded opaque values. Scope,
error, coverage, lifecycle, and capability states are exhaustive only inside one negotiated revision. A future unknown
value maps to an explicit unknown representation where the revision permits it.

Dropping a request future does not cancel a provider mutation unless the protocol method itself has a cancellation
operation. Dropping `RuntimeClient` closes only that connection. It never stops Runtime or its sessions.

## TypeScript client shape

The TypeScript package ships ESM and declaration files with one documented runtime baseline. It validates every
server message at runtime before exposing generated types.

```ts
const connection = await locateRuntime().connect({ identity });
const runtime = await connection.initialize({ supportedRevisions });

const providers = await runtime.providers.list();
const subscription = await runtime.sessions.watchEvents({
  sessionId,
  cursor,
  signal: abortController.signal,
});
```

The client converts protocol failures into `RuntimeError` with `code`, `retryable`, `operatorAction`, `details`, and
`correlationId`. It never asks callers to parse error messages. Abort signals stop local waiting and subscriptions;
they do not imply a provider interrupt.

The SDK owns connection heartbeat and subscription recovery. It never automatically retries `submitInput` after an
ambiguous disconnect. It returns `outcomeUnknown` with the original request ID.

## Enrollment API

The SDK provides identity helpers, not an authorization shortcut:

- generate an Ed25519 integration key in consumer-owned secure storage
- construct and validate the closed integration manifest
- prove possession during enrollment and initialization
- watch pending enrollment without polling
- expose exact approved scopes, roots, and revocation generation
- rotate a key only through the protocol's local approval flow

Consumers may implement their own secure key storage. SDK examples use Windows Credential Manager, macOS Keychain,
or a freedesktop secret service where available. A plaintext development identity is explicitly test-only and cannot
be used by conformance examples.

## Connection and reconnect

The SDK state machine is observable:

```text
unlocated -> connecting -> authenticating -> ready
     |           |               |           |
     v           v               v           v
notInstalled   unavailable   enrollment    reconnecting
                              or revoked
```

Reconnect uses capped exponential backoff with jitter and a total caller-configurable deadline. A successful
reconnect reinitializes, revalidates the grant generation, and recreates read subscriptions from their last accepted
cursor. A gap is surfaced immediately. The SDK does not reacquire a control lease silently if ownership may have
changed.

## Subscription ergonomics

Both SDKs expose an asynchronous stream abstraction with:

- snapshot boundary and live boundary
- exact last accepted cursor
- explicit lag and gap values
- cancellation separate from session interruption
- bounded client queue and selectable overflow policy limited to fail or disconnect
- final typed reason on revocation, revision retirement, Runtime exit, or provider loss

There is no convenience API that accumulates all events into an unbounded array or reconstructs a transcript.

## Package contents

Every SDK release contains:

- package license and provenance
- public API reference generated from the release source
- supported protocol revision list
- compatibility matrix against maintained Runtime releases
- minimal provider-neutral start, resume, and watch examples
- enrollment and scope selection example
- `outcomeUnknown`, gap, revocation, and control conflict examples
- deterministic protocol fixtures without real conversation content
- changelog separating package API, wire, and behavior changes

Examples use opaque provider IDs returned at runtime. An example that selects `codex`, `claude`, a model name, or a
provider filesystem path as a constant fails review.

## Standalone Runtime artifacts

Runtime publishes the same six target classes as the main product release policy:

| OS | Architecture |
|---|---|
| Windows | x86_64, aarch64 |
| macOS | x86_64, aarch64 |
| Linux | x86_64, aarch64 |

Each artifact includes Runtime, the local administration CLI, notices, schema revision inventory, and an install and
uninstall manifest. It does not bundle provider CLIs, provider credentials, models, or a consumer application.

Release metadata is signed and contains checksums, minimum rollback-safe store schema, public revision range, and
artifact provenance. Package managers may wrap the artifact but cannot replace its verification or per-user state
contract.

## First-party migration

Runtrol Studio moves ordinary session operations to `@runtrol/runtime-client` in stages:

1. provider and managed-session read paths
2. event subscriptions and cursor recovery
3. model and capability discovery
4. start, resume, control lease, input, interrupt, and approvals
5. removal of duplicated public DTOs from the extension

The extension keeps private administration operations on the internal endpoint. A test asserts that public Studio
session modules import the package and never import private IPC declarations.

## Repository shape

The implementation may refine names during attempts, but graduation requires equivalent boundaries:

```text
crates/runtrol-runtime-protocol/   public DTOs, schema, limits, errors
crates/runtrol-runtime-client/     Rust locator, connection, and client
clients/typescript/                TypeScript package and generated bindings
examples/runtime-consumer-rust/    repository example only
examples/runtime-consumer-ts/      repository example only
```

Any new root is registered in the workspace hygiene allowlist with its public distribution reason. Packed-artifact
tests run from a temporary directory outside the repository so accidental private imports cannot pass.

## Developer experience exit

A new product developer who has not read runtrol internals must be able to complete this journey from public docs:

1. install one package
2. detect `runtimeNotInstalled`
3. install or connect to Runtime
4. enroll with minimal scopes and one project root
5. list providers and managed sessions
6. select returned opaque IDs
7. acquire control and start or resume
8. submit one input with an idempotency key
9. render events and one approval
10. recover from disconnect without duplicate input
11. revoke and uninstall cleanly

Median time, errors, required user choices, and provider-specific code are measured in the independent consumer gate.

