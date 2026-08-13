# Gates

## Gate philosophy

Runtime is a product boundary, not a successful demo. A happy-path client connected to one provider proves almost
nothing about compatibility, ownership, privacy, or independent adoption. Graduation requires repository-external
consumers, real provider evidence, failure mutation, update and rollback, all target packages, and measured resource
ceilings.

Every gate produces machine-readable evidence under an ignored evidence directory. Checked-in tests contain no real
conversation content, provider credential, private user path, or developer machine identity.

## Gate classes

| Class | Proves |
|---|---|
| Contract | Public schema, method, scope, error, event, and thin-boundary invariants |
| Consumer | A product outside the repository can adopt packed SDKs without private imports |
| Provider | Multiple real providers work through one consumer path and provider drift fails honestly |
| Security | Enrollment, scopes, roots, leases, approvals, endpoint separation, and redaction |
| Compatibility | Independent updates, revision negotiation, reconnect, deprecation, and rollback |
| Resource | Memory, idle CPU, latency, queues, frames, catalogues, and overload behavior |
| Release | Signed six-target Runtime and SDK packages install, update, roll back, and uninstall |
| Product | Developers understand the category and can complete the integration without provider branches |

## Contract gate

The generated schema and SDK bindings are reproducible. The gate fails when:

- a public DTO or method is defined in more than one source of truth
- Rust and TypeScript disagree on a required field, error, scope, or limit
- a closed security object accepts unknown fields
- a provider ID appears as a built-in enum or branching constant
- a mutation lacks request ID and generation requirements
- a public error requires parsing prose
- an event can be dropped because the client does not know its required kind
- a public package imports a private Core, daemon, IPC, store, driver, or Studio module
- a public method reaches the private dispatch table
- any public type promises transcript history or provider credentials

Schema snapshots are reviewed as public API. Regeneration with an uncommitted difference fails CI.

## Independent consumer gate

Two reference consumers are built in fresh temporary directories outside the runtrol repository:

1. a Rust CLI using the packed crate artifact
2. a TypeScript local application using the packed npm artifact

They have no workspace path dependency, source checkout, private fixture import, environment-provided Runtime path, or
provider-specific constant.

Each performs:

1. detect missing Runtime
2. install or connect through the documented user action
3. request enrollment and observe pending state
4. receive a narrowed grant for one project root
5. list providers and managed sessions
6. select opaque provider and model values returned at runtime
7. start one session and adopt or resume another
8. acquire and renew control
9. submit exact fixture input once
10. render content, tool, notice, usage, completion, and one safe approval event
11. reconnect from the last cursor
12. encounter and handle a gap
13. lose control to a second authorized consumer deterministically
14. receive revocation while connected
15. exit without stopping the provider session

The journey records elapsed setup time, number of user decisions, documentation lookups, errors, and lines of
provider-specific code. Provider-specific code must remain zero.

## First-party parity gate

Runtrol Studio uses the public TypeScript SDK for every method available to third parties. An import audit and
behavioral trace compare Studio with the independent TypeScript consumer.

The gate fails if Studio:

- imports public session DTOs from its old private protocol file
- receives an unadvertised capability or event
- bypasses app scopes, root grants, control leases, or idempotency
- sees a larger session catalogue through a private session method
- reconnects or retries input using an internal shortcut
- owns or kills a session when its window exits

Private administration remains explicitly allowlisted by method name.

## Provider neutrality gate

The same compiled consumer exercises at least two real provider CLIs with no rebuild and no provider-name branch. It
lists providers, obtains models or honest unknown state, starts, watches, interrupts, cools, and resumes according to
advertised capabilities.

A third external ACP provider is then added only through its provider extension path. The gate asserts no diff in:

- Core lifecycle source
- public Runtime protocol source
- Rust or TypeScript SDK operation source
- Studio session controller source
- independent consumer source

A static audit rejects provider ID comparisons outside manifests, drivers, and provider-specific tests.

## Native session gate

At least one ACP provider with official `session/list` and one non-ACP official enumerable surface are exercised when
available. The gate covers:

- complete, partial, unsupported, stale, and failed states
- opaque forward pagination and end cursor
- duplicate, reordered, missing, and oversize entries
- official title, working directory, and updated time without transcript content
- root filtering before disclosure
- deduplication with an existing managed session by native ID
- adopt and resume only when officially supported
- binary update between page requests
- provider list capability removal
- no access to provider state directories during the test

A filesystem monitor and path deny fixture fail the test if Runtime touches an undeclared provider transcript or
session-storage path.

## Exact-input and no-copy gate

Generated fixtures cover empty, Unicode, bidirectional controls, newlines, null-equivalent JSON escapes, maximum safe
size, and random binary-representable UTF-8 sequences allowed by the protocol.

The provider test boundary receives the exact negotiated byte sequence once. Tests then scan:

- Runtime database and migrations
- logs and structured traces
- metrics labels and values
- errors and diagnostics bundles
- update and crash artifacts
- locator and integration grant state

No input or reversible unkeyed digest may appear. Provider output fixtures receive the same no-copy scan. The bounded
in-memory replay test proves eviction at the declared frame and byte ceilings.

## Multi-consumer control gate

At least three client processes from two integration identities share one session:

- all authorized watchers receive ordered structural events
- only one lease generation can mutate
- same-integration windows do not gain implicit shared control
- disconnect does not transfer control early
- lease renewal has a bounded maximum and rate
- stale generation loses deterministically
- active-turn expiry creates orphaned control
- forced transfer requires private local authority
- Runtime crash and recovery never admit two controllers
- revocation wins against a concurrent parsed mutation before provider I/O

The race suite repeats under deterministic scheduling and high iteration count. Any duplicate input or double approval
is release-blocking.

## Idempotency and ambiguity gate

Each mutation is interrupted at every durable and provider-I/O boundary:

- before intent record
- after intent before dispatch
- during provider write
- after write before acknowledgement
- after acknowledgement before result record
- after result record before response
- during client response delivery

The same request ID returns the same known result, `idempotencyConflict`, or `outcomeUnknown` according to recorded
truth. It never silently executes twice. Low-entropy input cannot be recovered from stored authenticators in the
dictionary attack fixture.

## Enrollment and scope gate

Security mutations cover:

- another OS user and remote named-pipe caller
- unenrolled inventory and watch
- self-approval and pending-enrollment flooding
- forged, replayed, expired, and cross-instance signatures
- manifest identity substitution
- scope substitution between every public method class
- root escape through symlink, junction, case, worktree, submodule, and replacement
- scope and root narrowing during an in-flight request
- widening without physical approval
- key loss, rotation, theft simulation, and revocation
- approval subject, option, risk, expiry, session, and generation swaps
- public dispatch to every private method name

Every denial has stable code, safe details, and no information outside the caller's current grant.

## Protocol compatibility gate

The matrix contains current plus previous two finalized revision implementations on both sides. Tests cover:

- newest common revision selection independent of list order
- no common revision
- downgrade and revision stripping
- unknown optional capability
- unknown required event kind
- new machine error under an older client policy
- capability removed after initialization
- strict unknown-field rejection in closed security objects
- frame and extension limits before allocation
- upgrade Runtime with old clients connected
- roll back Runtime with new clients installed
- upgrade and roll back each SDK independently
- deprecation warning and final retirement
- emergency security retirement before inventory

The test uses released protocol fixtures or frozen compatibility binaries, not recompiled old source against new types.

## Locator and singleton gate

All three operating systems cover:

- clean install and missing Runtime
- stopped Runtime
- two simultaneous consumer starts
- stale locator after crash
- process ID reuse
- locator replacement and weakened permissions
- wrong endpoint type and oversize locator
- Runtime instance ID mismatch
- successful atomic locator replacement during update
- Runtime uninstall while consumers are stopped or running

No case asks the user for an executable, socket, port, provider home, or state path.

## Performance gate

Initial targets are hypotheses to be frozen or tightened by attempt evidence:

| Operation | Initial target |
|---|---|
| Locator, local connect, and initialization | p95 at or below 100 ms with running Runtime |
| Provider inventory snapshot | p95 at or below 50 ms without provider start |
| Managed session snapshot with 30 sessions | p95 at or below 50 ms |
| Switch event watch to an already hot session | p95 at or below 100 ms |
| Switch and resume a cold session | p95 at or below 1500 ms, provider capability permitting |
| Control lease renewal | p99 below one quarter of measured lease duration under supported load |
| Added idle CPU from public endpoint | No polling and no measurable sustained increase beyond noise threshold |

The existing Core memory ceiling remains a contract. Public connection, subscription, catalogue, idempotency, and
grant metadata receive separate byte budgets within it or an explicitly reviewed measured delta. The gate runs 30
managed sessions, multiple consumers, a slow reader, oversize frames, catalogue floods, and connection churn.

Percentiles are measured on Windows x86_64, macOS aarch64, and Linux x86_64 at minimum. The release target matrix
still performs functional gates on all six targets.

## Overload gate

Resource exhaustion returns `resourceExhausted` or `rateLimited` at the correct admission boundary. It cannot:

- allocate from an untrusted length before checking the ceiling
- grow subscriptions, idempotency, audit, or enrollment state without bound
- start all providers because one consumer lists inventory
- starve Core lifecycle work or approval expiry
- evict a running turn to admit a catalogue request
- convert a lagged subscription into an incomplete stream without a gap
- leak another integration's existence through limit details

Memory and handle counts return to their measured baseline after clients disconnect and cache lifetimes expire.

## Release and uninstall gate

For all six target artifacts, a clean per-user environment verifies:

- signature, checksum, target, and provenance
- installation without provider bundles or credentials
- locator publication only after readiness
- independent SDK connection and enrollment
- Runtime update with active managed sessions
- Runtime rollback inside store and protocol floor
- consumer uninstall without session loss
- Runtime uninstall removes owned endpoints, grants, caches, and metadata
- provider binaries, authentication, and provider-native conversations remain untouched
- reinstall creates a new Runtime instance identity and requires honest integration revalidation policy

The final artifact evidence records hashes and commands, not machine-specific secrets.

## Documentation and product gate

An external developer receives only public docs, packages, and an ordinary installed provider CLI. Product review
checks that the developer can correctly answer:

- what intelligence and credentials come from the provider CLI
- what Runtime owns and does not own
- why one daemon is shared
- why a hosted service needs a local companion
- the difference between managed and native session catalogues
- how control, input ambiguity, gaps, revocation, and unsupported capability appear
- what same-user app identity does and does not protect

Public copy is rejected if it says or implies unlimited history, every provider conversation, hosted inference,
provider API-key brokerage, prompt optimization, autonomous routing, or isolation from same-user malware.

## Completion evidence

The initiative graduates only when:

1. every gate above is green in one release candidate
2. existing security, provider isolation, lifecycle, memory, responsiveness, update, and uninstall gates remain green
3. packed SDK and standalone Runtime artifacts are reproducible
4. independent developer review has no undocumented private step
5. product review accepts category clarity, setup cost, failure UX, and support burden
6. stable protocol, SDK, operations, security, and integration documents move to `docs/`
7. provisional code and attempts are removed
8. this planning directory is deleted

