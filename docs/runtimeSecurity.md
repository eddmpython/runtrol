# Runtime security

## Trust boundary

The public Runtime endpoint admits only the same OS user that owns Runtime. It is local transport, not a network service. Endpoint
ownership, permissions, peer identity, instance proof, one-use server challenge, integration signature, grant
generation, method scope, project root, session visibility, control lease, and request idempotency are checked as
separate layers.

The locator and process ID are not credentials. A process that can replace a locator still has to pass endpoint owner
checks and prove the named Runtime instance. Suspicious ownership, permissions, endpoint shape, or instance mismatch
fails closed and requires local repair.

The public dispatcher cannot invoke private administration methods. Integration approval, grant widening, root
widening, revocation, installation repair, and physical-presence confirmation remain local administration actions in
Runtrol Studio.

## Data ownership

| Data | Owner and handling |
|---|---|
| Provider credential | Provider CLI only. Runtime never holds or forwards a model API key |
| Provider conversation | Provider CLI only. Runtime never scans, parses for meaning, rewrites, or stores a copy |
| Consumer input | Consumer until Runtime transports the exact bytes to the provider. No durable copy or automatic retry |
| Live provider event | Bounded transient transport and replay only. It is not a transcript store |
| Native session pointer | Provider ID, opaque native ID, workspace identity, and bounded metadata in Runtime |
| Integration private key | Consumer secure storage only |
| Integration public key and grant | Runtime integrity-protected metadata with generations |
| Locator and install record | Owner-readable operational bootstrap metadata |
| Audit record | Bounded method, scope, target, generation, result class, time, and request identity without content |

Runtime errors, logs, metrics, crash reports, and diagnostics must not contain prompt text, replies, tool arguments,
approval text, provider raw output, credentials, environment values, signatures, nonces, or transcript paths.
Idempotency stores a keyed authenticator instead of a raw content hash so low-entropy input cannot be guessed from
durable state.

## Authorization

Enrollment proves possession of an Ed25519 key and creates a bounded pending request. A third-party integration is
decided by the operator locally, who approves a narrowed scope and root set. The public caller cannot approve, widen,
or choose a different root after review.

Runtrol Studio's own enrollment is settled differently, because the ceremony it would otherwise perform proves
nothing. Studio materializes and spawns the Core it enrolls with, and the local approval challenge returns its own
phrase inside the response, so any program that can reach the owner-only private endpoint could already read that
phrase back and complete the approval unaided. Studio instead signs the pending identity with the enrolling key over
a domain-separated payload. That establishes which enrollment the caller is, which the phrase never established.
Self-approval grants the enrollment exactly as requested and can neither widen nor narrow it: narrowing remains a
reviewed local decision, the root deny list still refuses any root overlapping a provider credential directory, and
the request is a local administration type that no remote caller can reach.

Grant changes are generation-based. Narrowing and revocation apply before the next parsed request is dispatched and
retire active subscriptions. Widening requires a new local decision. Key rotation proves the old and replacement keys
and also requires exact local confirmation.

Project authorization uses canonical filesystem and worktree identity. It rejects traversal, links, junction escapes,
case aliases, root replacement, and display-path authority. Exclusive workspace admission prevents two incompatible
writers even when callers use different subdirectories of one worktree.

One control lease protects session mutation. Lease generation prevents stale renewal, transfer, input, interruption,
cooling, and approval response. Disconnect does not give another integration control during an active turn.

## Provider and approval input

Provider output is hostile structured input. Drivers and the public serializer enforce payload, string, extension,
queue, and page bounds. Unknown optional provider extensions may remain opaque only inside declared bounds. Unknown
required event structure ends the subscription instead of silently losing data.

Runtime answers only approvals discovered through the provider's registered structured protocol. Risk and available
options come from the pending request held by the driver. A caller cannot send free-form approval text, downgrade
risk, change the subject, answer another session, or answer an expired request.

## Hosted companions

A hosted product must ship a local companion. The companion enrolls as the Runtime integration, owns its cloud login,
associates devices, authenticates remote intent, and requests minimum Runtime scopes and roots. Runtime sees only the
local companion identity.

Runtime never accepts cloud bearer tokens, cookies, webhook secrets, model credentials, arbitrary callbacks, or a
request to bind a public listener. A companion cannot turn the owner-only Runtime endpoint into remote authority.

## Incident response

Use `Runtrol: Manage Runtime Integrations` to revoke a compromised or removed consumer. Revocation does not delete a
provider conversation or hide whether a turn is still active. Interrupting work is a separate explicit operation.

If locator ownership or permissions are unsafe, do not delete it through an SDK. Stop the verified Runtime, inspect
the state directory owner, remove only the verified stale locator, and start the signed installed executable. If the
Runtime binary or release provenance cannot be verified, reinstall from an attested artifact before approving any
integration.
