# Rollout

Implementation uses falsification-first slices. A slice starts in `tests/_attempts/embeddableAgentRuntime/`, measures
its uncertain contract, and either freezes the evidence-backed design or deletes the attempt. Production modules are
added only after the owning mutation and rollback path pass.

## Phase order

| Phase | Scope | Production admission |
|---|---|---|
| 0 | Contract and category falsification | No production code |
| 1 | Public protocol, endpoint, locator, and read-only client | Read-only after negotiation and identity gates |
| 2 | Enrollment, app scopes, roots, grants, and revocation | Inventory for enrolled consumers |
| 3 | Managed sessions, events, control lease, mutations, and approvals | Session control after ambiguity and race gates |
| 4 | Official native session catalogues and adoption | Only registered provider capabilities |
| 5 | SDK packaging, first-party migration, standalone distribution | Public release candidate |
| 6 | Optional ACP compatibility facade | Only after native Runtime v1 evidence |

`2-autoUpdate` graduates before Runtime distribution depends on its signed activation contract. Read-only protocol
attempts may proceed earlier because they do not alter release behavior.

## Phase 0: contract falsification

Build a repository-external read-only client against a throwaway protocol facade. It must list structural provider and
managed-session fixtures without importing private types. Interview the contract through DartLab-shaped and unrelated
IDE-shaped consumers, but implement neither product.

Reject or revise the design if consumers require provider branches, transcript access, prompt rewriting, private
daemon ownership, configured paths, or provider API keys. Measure whether a direct ACP integration is materially
simpler for the ordinary product journey and retain Runtime only where its cross-provider supervision adds clear
value.

Exit artifacts:

- accepted product category and non-goals
- public and private endpoint split
- identity and same-user threat statement
- first revision draft and machine error taxonomy
- measured locator and initialization prototype
- deleted failed experiments

## Phase 1: read-only foundation

Create the public protocol crate, separate daemon listener, OS locator, revision negotiation, schema generator, and Rust
client. Admit only initialization, enrollment request state, provider inventory, managed session snapshot, and bounded
watch fixtures. Provider process start and every mutation remain unreachable.

Freeze frame, connection, subscription, and extension limits from hostile input attempts. Prove public dispatch cannot
reach the private request enum. Add TypeScript generation only after schema reproducibility passes.

## Phase 2: integration authorization

Add integration key proof, pending enrollment, local approval commands and Studio UI, grants, app scopes, project
roots, generation checks, revocation, and audit metadata. Keep provider and session output denied until enrollment is
approved.

Run path mutations on Windows junctions and case, Unix symlinks, Git worktrees, submodules, and replaced directories.
Run in-flight grant narrowing and revocation before any session mutation is introduced.

## Phase 3: supervised session control

Expose managed session list and event streams first, then start and resume, then the control lease, input, interrupt,
cool, and low-risk approval response. Every mutation lands with its idempotency and crash-boundary tests in the same
change.

Runtrol Studio migrates each ordinary operation to the public TypeScript SDK as it becomes available. The old public-
equivalent private route is removed in the same slice. Private administration remains separate and allowlisted.

Input is the last ordinary mutation enabled. It does not ship until byte equality, no-copy scans, ambiguous provider
write, client reconnect, daemon crash, lease race, and revocation race all pass.

## Phase 4: provider-native session catalogue

Implement the provider-driver capability for official session enumeration. Start with stabilized ACP `session/list`
and `session/resume`, then add a non-ACP official structured surface if a supported provider exposes one.

Expose complete, partial, and unsupported coverage. Paginate without background draining. Root-filter before
disclosure. Deduplicate only by provider and native session identity. Reject any implementation that opens a provider
transcript or private state directory.

## Phase 5: distribution and adoption

Pack the Rust and TypeScript clients, build all six standalone Runtime targets, sign release metadata, and execute the
consumer journey from clean temporary environments. Add public API reference, minimal samples, failure recipes,
compatibility matrix, installation, update, rollback, revocation, and uninstall documentation.

Migrate all ordinary Studio session behavior to the public SDK. Recruit one independent consumer outside the runtrol
source tree. A source-tree example alone does not satisfy adoption.

Freeze Runtime v1 only after one stable release cycle proves independent SDK and Runtime upgrades and the product
review accepts setup cost and category clarity.

## Phase 6: optional standards facade

Build an ACP facade as an ordinary Runtime consumer. It translates only the expressible subset and documents feature
loss. It cannot bypass integration enrollment, project roots, control leases, limits, or native machine errors on its
downstream Runtime connection.

Delete the facade if its compatibility value does not justify another protocol lifecycle or if it pressures the
native contract to expose provider-specific behavior.

## Files

These paths define intended ownership. Exact filenames may be refined during attempts, but any refinement must
preserve dependency direction and update this plan before production implementation.

### New public protocol and client crates

- `crates/runtrol-runtime-protocol/Cargo.toml`: dependency-minimal public schema crate.
- `crates/runtrol-runtime-protocol/src/lib.rs`: public export boundary only.
- `crates/runtrol-runtime-protocol/src/revision.rs`: revision negotiation and support inventory.
- `crates/runtrol-runtime-protocol/src/method.rs`: public method names and request classification.
- `crates/runtrol-runtime-protocol/src/types.rs`: identifiers, descriptors, capabilities, coverage, cursors, and limits.
- `crates/runtrol-runtime-protocol/src/error.rs`: stable machine error kinds and safe detail shapes.
- `crates/runtrol-runtime-protocol/src/event.rs`: revisioned structural event union.
- `crates/runtrol-runtime-protocol/schema/runtime.schema.json`: deterministically generated public schema.
- `crates/runtrol-runtime-client/Cargo.toml`: Rust public client package.
- `crates/runtrol-runtime-client/src/lib.rs`: exported locator and client API.
- `crates/runtrol-runtime-client/src/locator.rs`: per-platform locator validation.
- `crates/runtrol-runtime-client/src/connection.rs`: framing, initialization, authentication, and reconnect.
- `crates/runtrol-runtime-client/src/client.rs`: provider, session, approval, and integration operations.
- `crates/runtrol-runtime-client/src/subscription.rs`: bounded asynchronous watch surfaces.

### TypeScript package

- `clients/typescript/package.json`: publishable `@runtrol/runtime-client` package.
- `clients/typescript/src/generated.ts`: generated bindings, never hand-edited.
- `clients/typescript/src/locator.ts`: platform locator adapter.
- `clients/typescript/src/connection.ts`: runtime validation, framing, initialize, and reconnect.
- `clients/typescript/src/client.ts`: typed high-level operations.
- `clients/typescript/src/errors.ts`: `RuntimeError` mapping.
- `clients/typescript/src/subscription.ts`: cursor-aware async iterable.

The new `clients/` root is added to `.claude/hooks/workspaceHygiene.py` only when the package lands, with its public
distribution role documented. It is not allowlisted merely to make a failed attempt green.

### Daemon and security composition

- `crates/runtrol-daemon/src/runtime_serve.rs`: separate public listener and connection lifecycle.
- `crates/runtrol-daemon/src/runtime_dispatch.rs`: public method table and authorization order.
- `crates/runtrol-daemon/src/runtime_enrollment.rs`: pending enrollment and key proof composition.
- `crates/runtrol-daemon/src/lib.rs`: start and stop the public endpoint with the existing daemon lifecycle.
- `crates/runtrol-security/src/integration.rs`: `IntegrationId`, key identity, grant generation, and app scopes.
- `crates/runtrol-security/src/project_grant.rs`: canonical integration project authority.
- `crates/runtrol-store/src/integration.rs`: bounded grants, enrollment, idempotency, and audit metadata.
- `crates/runtrol-store/src/migrations/`: rollback-reviewed integration schema migration.
- `crates/runtrol-core/src/session/manager.rs`: Core-facing controller lease and public admission hooks only where
  existing lifecycle ownership requires them.
- `crates/runtrol-core/src/session/mod.rs`: structural exports without protocol dependency.

The dependency direction is `SDK -> public protocol` and `daemon composition -> public protocol, security, store,
Core`. Core, security, and store never depend on the SDK. Core never depends on JSON-RPC or public wire DTOs.

### Provider boundary

- `crates/runtrol-provider-api/src/`: add structural official native-session catalogue capability to the provider SPI.
- `crates/runtrol-drivers/src/`: map each official provider surface at the existing driver boundary.
- `crates/runtrol-drivers/manifests/`: declare discovery provenance and version evidence without model or session-path
  hardcoding.
- `docs/providerArchitecture.md`: graduate the official-catalogue boundary and restate the storage-scan prohibition.

### First-party and product surfaces

- `extensions/runtrol-vscode/package.json`: consume the packed or workspace public TypeScript package.
- `extensions/runtrol-vscode/src/controller.ts`: use public session client operations.
- `extensions/runtrol-vscode/src/protocol.ts`: remove public-equivalent duplicated DTOs, retain private administration
  types only.
- `extensions/runtrol-vscode/src/extension.ts`: Runtime locator, enrollment, reconnect, and failure UI.
- `mainPlan/4-orchestrationGrowthOS/`: consume and later extend the public Runtime contract, never add another client
  path to Core.
- `mainPlan/5-pwaConnection/`: remain on the paired remote transport and map into scoped Runtime behavior only through
  the daemon boundary.
- `mainPlan/6-pwaSurface/`: consume the same session semantics after the connection initiative graduates.

### Documentation and release

- `docs/runtimeProtocol.md`: finalized public wire and compatibility contract.
- `docs/runtimeIntegration.md`: consumer journey, SDK usage, enrollment, scopes, errors, and failure recovery.
- `docs/runtimeSecurity.md`: threat boundary, data ownership, and hosted companion guidance.
- `docs/runtimeOperations.md`: install, locator repair, update, rollback, revoke, and uninstall.
- `release/`: standalone target package assembly only if the root is accepted by the repository release plan and
  hygiene policy.

## Symbols

Symbols are named now to expose ownership and test seams. Production names may change only with the same semantics and
an updated cross-reference.

### Public protocol

- `ProtocolRevision`: finalized revision identifier with parse and ordered negotiation.
- `ClientCapabilities` and `ServerCapabilities`: closed negotiated feature maps.
- `RuntimeLimits`: numeric frame, subscription, catalogue, connection, and mutation bounds.
- `RuntimeRequest`, `RuntimeResponse`, and `RuntimeNotification`: JSON-RPC payload categories.
- `RuntimeErrorKind` and `RuntimeError`: stable machine failure and safe details.
- `IntegrationId`, `ClientInstanceId`, `SessionId`, `ProjectId`, and `SubscriptionId`: opaque public newtypes.
- `ProviderDescriptor`, `SessionDescriptor`, and `NativeSessionDescriptor`: structural catalogue DTOs.
- `CatalogueCoverage`: `complete`, `partial`, or `unsupported` with provenance.
- `EventCursor` and `RuntimeEvent`: bounded reconnect and revisioned structural event.
- `MutationRequestId`: UUIDv7 state-change identity.

### Client APIs

- `RuntimeLocator::system`: derive and validate the platform locator.
- `RuntimeConnector::connect`: establish the owner-only transport and obtain a server nonce.
- `RuntimeConnection::initialize`: negotiate revision and authenticate integration identity.
- `RuntimeClient::providers`, `sessions`, `approvals`, and `integration`: typed operation groups.
- `SessionClient::watch_events`: snapshot and cursor-aware asynchronous stream.
- `SessionClient::submit_input`: exact input with request and lease generations, never an automatic ambiguous retry.
- `RuntimeError`: language API failure with machine code, retryability, operator action, and correlation ID.

### Daemon and authorization

- `serve_runtime_endpoint`: own the public endpoint lifecycle separately from control IPC.
- `dispatch_runtime_request`: map only public methods after initialization.
- `authorize_runtime_request`: check peer, integration, revision capability, scope, root, lease, generation, and subject.
- `IntegrationGrant`: public key, scopes, roots, generation, and revocation state.
- `AppScope`: Runtime integration scope vocabulary, separate from `DeviceScope`.
- `ControlLease`: session, integration, generation, deadline, and orphan state.
- `IdempotencyRecord`: request identity, keyed parameter authenticator, result class, and expiry without input.
- `NativeSessionCatalogue`: provider SPI result with coverage, page, and cursor.

## Tests

Tests begin under the attempt directory and graduate beside their owning contract. Names below are required evidence
categories, not permission to add placeholder files before their phase.

### Attempts

- `tests/_attempts/embeddableAgentRuntime/consumerJourney.rs`: external Rust consumer with no private imports.
- `tests/_attempts/embeddableAgentRuntime/consumerJourney.ts`: external TypeScript consumer from packed npm artifact.
- `tests/_attempts/embeddableAgentRuntime/categoryInterview.md`: recorded product comprehension and integration cost.
- `tests/_attempts/embeddableAgentRuntime/locatorLatency.rs`: cross-platform locate, connect, and initialize measurement.
- `tests/_attempts/embeddableAgentRuntime/directAcpComparison.md`: measured comparison against direct ACP adoption.

### Unit and contract tests

- `crates/runtrol-runtime-protocol/tests/schema.rs`: deterministic schema and fixture round trips.
- `crates/runtrol-runtime-protocol/tests/negotiation.rs`: common revision, downgrade, and retirement.
- `crates/runtrol-runtime-client/tests/locator.rs`: malformed, stale, unsafe, and valid locator states.
- `crates/runtrol-runtime-client/tests/reconnect.rs`: cursor restoration without mutation retry.
- `clients/typescript/test/runtimeValidation.test.ts`: hostile server payload rejection.
- `tests/audit/runtimePublicBoundary.rs`: dependency direction, method separation, provider enum, and private imports.
- `tests/audit/runtimeNoConversationCopy.rs`: store, logs, diagnostics, metrics, and artifact content scan.

### Integration and race tests

- `tests/integration/runtimeEnrollment.rs`: pending, approve, narrow, widen, rotate, revoke, and flood.
- `tests/integration/runtimeScopes.rs`: every method against every insufficient scope and root.
- `tests/integration/runtimeControlLease.rs`: multi-process acquisition, expiry, orphan, transfer, and crash recovery.
- `tests/integration/runtimeIdempotency.rs`: all durable and provider-I/O ambiguity boundaries.
- `tests/integration/runtimeEvents.rs`: snapshots, cursor replay, gaps, lag, unknown kinds, and revocation.
- `tests/integration/runtimeNativeSessions.rs`: complete, partial, unsupported, pagination, root filter, merge, and adopt.
- `tests/integration/runtimeProviderNeutrality.rs`: same consumer over two real providers and one external ACP provider.
- `tests/integration/runtimeUpdate.rs`: live Runtime update, reconnect, compatibility, and rollback.

### Platform and release tests

- `tests/platform/runtimeLocatorWindows.rs`: SID, DACL, named pipe, remote reject, junction, and process replacement.
- `tests/platform/runtimeLocatorUnix.rs`: UID, modes, UDS, symlink, and endpoint replacement for Linux and macOS.
- `tests/release/runtimePackedConsumer.rs`: clean external install from signed packed artifacts.
- `tests/release/runtimeTargets.rs`: six-target contents, checksums, provenance, and public revision inventory.
- `tests/release/runtimeUninstall.rs`: owned-state removal and provider-state preservation.
- `tests/performance/runtimePublicSurface.rs`: latency, memory, CPU, handles, queues, and overload ratchets.

Existing project floors remain required: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, unit and
integration suites, workspace hygiene, dependency direction, provider isolation, memory, security, update, and
uninstall gates.

## Rollback

Rollback is designed per slice so a failed experiment cannot strand public clients, grants, or provider sessions.

### Attempt rollback

Phase 0 attempts own no production state. Delete `tests/_attempts/embeddableAgentRuntime/` and its ignored evidence.
Reject the initiative if its product or thin-boundary assumptions fail. Do not leave dormant crates, dependencies,
feature flags, protocol methods, or hygiene allowlist entries.

### Read-only rollback

Before public release, stop the public endpoint, remove its unpublished locator, client packages, protocol crate, and
daemon composition. The private control path and Core remain unchanged. No provider session or transcript migration is
required because read-only Runtime state owns neither.

### Enrollment rollback

Disable public admission before reverting grant schema. Export no private keys or conversation content. A schema
downgrade either reads the prior grant table safely or restores the verified pre-migration Runtime database. Revoked
or narrowed grants never become wider because of rollback.

### Session-control rollback

Drain new public mutations, preserve Core supervision, expire public control leases conservatively, and let local
private administration observe or stop active work. Never resend an uncertain input or approval. Removing the public
surface does not delete managed pointers or provider-native conversations.

### Released Runtime rollback

Activate only an artifact whose public revision range and store floor can serve installed supported clients. Preserve
the last verified Runtime artifact and rollback-safe state snapshot. SDKs reconnect and negotiate the older common
revision. If none exists, they receive `protocolIncompatible` before inventory or mutation.

### SDK rollback

Consumers may reinstall a previous SDK package independently. Package rollback changes language APIs but keeps its
declared protocol implementations. Runtime does not alter grants merely because the client product version changed.

### Standards facade rollback

The optional ACP facade is an ordinary client artifact. Stop and remove it without changing Runtime, Core, provider
drivers, app grants, or native SDKs.

## Evaluation

Evaluation separates engineering truth from product desirability. Passing tests cannot compensate for a confusing or
unnecessary product, and strong demand cannot waive the thin boundary.

### Developer review

An independent developer uses only published documentation and packed artifacts. Review records:

- time from package install to first provider list
- time and user actions through Runtime install and enrollment
- number and cause of integration errors
- provider-specific branches, which must be zero
- understanding of managed versus native sessions
- handling of unsupported capability, gap, control conflict, revocation, and `outcomeUnknown`
- ability to update and roll back the consumer without coordinating Runtime
- any need to inspect runtrol source, private paths, or provider storage, which fails the review

Security review independently verifies endpoint separation, same-user claims, app scopes, root canonicalization,
approval subject binding, redaction, and the exact-input no-copy contract. Maintainer review verifies dependency
direction, bounded memory, Rust idiom, and generated-schema ownership.

### Product review

Product review decides whether Runtime materially improves the product developer's outcome compared with integrating
providers or ACP directly. It evaluates:

- category clarity as a local provider-neutral runtime, not a model or hosted AI API
- installation and enrollment friction
- value of fast provider and managed-session inventory
- honest usefulness of partial native-session discovery
- multi-consumer and session continuity value
- support cost of protocol, SDK, target packages, and compatibility window
- independent adoption evidence after one stable release cycle
- fit with Runtrol Studio, later Mission orchestration, and PWA without coupling their roadmaps

The initiative is killed or reduced to an internal boundary if ordinary consumers still need provider knowledge, if
direct ACP is consistently simpler without losing required supervision, if a private daemon per product is needed, or
if no independent consumer adopts the documented stable release.

### Graduation decision

Graduation requires unanimous acceptance from protocol, security, Core lifecycle, release, first-party Studio, and
product owners. Accepted contracts move to `docs/runtimeProtocol.md`, `docs/runtimeIntegration.md`,
`docs/runtimeSecurity.md`, `docs/runtimeOperations.md`, provider architecture documentation, and package API docs.

After all provisional artifacts are deleted and every inherited project gate remains green,
`mainPlan/3-embeddableAgentRuntime/` is removed. Runtime then becomes a maintained product surface with its own support
window, not a permanent roadmap exception.

