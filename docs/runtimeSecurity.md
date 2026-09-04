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
widening, revocation, installation repair, and physical-presence confirmation remain owner-local administration
actions. The installed `runtrol` command provides this surface independently; Studio is an optional GUI for it.

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
| Runtime machine identity | Per-user OS protection. On macOS the login Keychain ACL follows the same OS-user boundary as the owner-only local endpoint, so a content-named Runtime update can restore the identity without a hidden approval dialog |
| Locator and install record | Owner-readable operational bootstrap metadata |
| Audit record | Bounded method, scope, target, generation, result class, time, and request identity without content |

Runtime errors, logs, metrics, crash reports, and diagnostics must not contain prompt text, replies, tool arguments,
approval text, provider raw output, credentials, environment values, signatures, nonces, or transcript paths.
Idempotency stores a keyed authenticator instead of a raw content hash so low-entropy input cannot be guessed from
durable state.

## Public audit boundary

Ordinary public control-plane requests reserve one bounded admission and pass through one bounded FIFO batch writer.
The writer acknowledges an `Attempted` row before dispatch and an `Allowed` or `Denied` row before returning the
answer. One server-minted UUIDv7 correlation joins those two rows even when equal methods complete out of order.
The admission state machine allows exactly one attempted row and one terminal row. While that generation owns redb,
each acknowledgement means the corresponding batch append completed. A full admission lane, closed writer, failed
append, or failed acknowledgement returns `auditUnavailable` rather than running without the required record. Normal
generation retirement closes admission, waits for every active pair and the writer, then waits for the relay receipt.

Bringing a window forward is a desktop action, not a data request: `windows/reveal` and `providers/focusNative`
are refused unless the calling connection has registered a VS Code window on this machine, so a paired device
that holds the same scope cannot raise or move anything the operator is looking at. `providers/focusNative` also
never tells the caller which window owns the conversation: the Runtime resolves the window itself and answers
only whether the request was delivered and what happened on the desktop.

Terminal attachment changes the connection into a bounded data plane. The public open or attach admission is audited,
but provider output notifications and view-bound input, resize, and lease frames are not appended to redb per frame or
per keystroke. An observed mirror is the same boundary in the opposite direction: `windows/mirrorOpen` and
`windows/mirrorEnd` are audited because they decide that a terminal is mirrored at all, while `windows/mirrorOutput`
carries the captured bytes and is not journaled, since a provider redrawing its screen would otherwise write two
durable rows per redraw and crowd out the events this journal exists for. They remain transient and are constrained by the authenticated view, current grant and root proof,
terminal generation, control lease, and transport bounds. This avoids turning conversation bytes into either an audit
payload or a synchronous storage operation.

A draining generation has released redb to its successor, so acknowledged control-plane rows enter an oldest-first
bounded in-memory relay under a process-unique UUIDv7 epoch and consecutive sequence. The successor commits rows,
bounded overflow marker, and its receipt watermark in one redb transaction. Only a later poll carries that receipt
back. Until then the old generation repeats the same batch and remains in the locator, so response loss, append
failure, successor crash, and a fast second upgrade do not lose or duplicate a row. Queue overflow is represented by
one stable content-free denial marker and a contiguous lost-through watermark. A receipt is bound to the executable
generation that produced it. It remains durable for as long as that generation appears in a successfully read current
locator, regardless of age or later activity, and is removed only after a later current locator proves that generation
absent. A restarted process of the same executable generation replaces its prior process epoch. The locator's fixed
generation ceiling therefore bounds the receipt table without an age-based eviction window. A damaged receipt or a
failed relay commit terminates the successor service rather than silently advancing authority without its audit rows.

This exactly-once contract begins when both generations implement receipt ACKs. A predecessor already running an older
destructive relay cannot be repaired retrospectively; its old-to-new handoff remains the explicit compatibility
exception. Authority relay state is periodic rather than durable, but missing or stale authority fails closed and
cannot widen the old generation's frozen grant ceiling.

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

Use `runtrol integrations revoke <integration-id>` or Studio's **Manage Runtime Integrations** command to revoke a
compromised or removed consumer. Revocation does not delete a provider conversation or hide whether a turn is still
active. Interrupting work is a separate explicit operation.

If locator ownership or permissions are unsafe, do not delete it through an SDK. Stop the verified Runtime, inspect
the state directory owner, remove only the verified stale locator, and start the signed installed executable. If the
Runtime binary or release provenance cannot be verified, reinstall from an attested artifact before approving any
integration.
