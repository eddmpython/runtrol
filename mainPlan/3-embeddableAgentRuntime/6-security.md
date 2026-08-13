# Security

## Security position

Runtime opens a reusable local product surface, so owner-only transport is necessary but no longer sufficient. Every
public request is admitted through five independent checks:

```text
OS peer -> enrolled integration -> negotiated capability -> app scope -> project root and live subject
```

A success at one layer never implies success at the next. Provider credentials remain in the provider CLI. Runtime
does not become a credential broker, model proxy, remote gateway, or same-user sandbox.

## Threat boundary

| Actor or failure | Inside the claim | Required defense |
|---|---|---|
| Different OS user | Yes | Owner-only locator, endpoint ACL or mode, peer identity check |
| Unenrolled same-user program | Yes | No inventory before enrollment, nonce proof, default deny |
| Enrolled integration making an accidental broad request | Yes | Exact scopes, roots, subject binding, limits, audit metadata |
| Revoked integration with a live connection | Yes | Grant generation check and immediate connection retirement |
| Stolen integration key | Partly | Narrow grant, revocation, rotation, expiry where measured |
| Same-user malware able to read another process or secret store | No isolation claim | Clear operator boundary, platform hardening where available |
| Remote network caller | Yes | No public network listener, remote named-pipe rejection |
| Compromised hosted consumer backend | Outside Runtime directly | Local companion owns cloud authentication and mapping |
| Malicious provider output | Yes at transport boundary | Size, schema, terminal safety, and structural normalization gates |
| Malicious prompt or reply meaning | Outside Runtime interpretation | Runtime does not parse for meaning or execute content |

No document or UI may say that integration signing proves an application is benign. It proves possession of a key
that the operator associated with a grant.

## Endpoint separation

The daemon serves two distinct local endpoints:

| Endpoint | Callers | Authority |
|---|---|---|
| Private control | Runtrol CLI, installer, Studio administration | Repair, integration approval, provider update, physical-presence actions |
| Public Runtime | Studio session client and enrolled external integrations | Negotiated, scope-bound provider and session API |

The endpoints have different names, request enums, protocol crates, and dispatch tables. Public input cannot select a
private method through a string alias, extension map, or forwarded envelope. Dependency and method-table audits prove
the separation.

## Integration identity

Each installed integration instance generates an Ed25519 key pair. Runtime stores only:

- `IntegrationId`
- public key and key generation
- approved scopes and canonical project roots
- operator-approved display label
- observed executable, package, publisher, and signature facts when available
- manifest digest
- grant generation, enrollment, last-use, rotation, and revocation timestamps

The manifest is diagnostic and closed-schema. Self-declared name, publisher, callback path, or scope rationale grants
nothing.

Authentication signs a canonical initialization transcript containing the Runtime instance nonce, endpoint identity,
revision offer, client instance, requested capabilities, key generation, and short expiry. Field reordering, replay to
another daemon, revision stripping, and capability widening fail verification.

## Enrollment

Enrollment is default deny and requires physical local approval.

1. The unenrolled client connects through the owner-only public endpoint.
2. It proves possession of a generated private key and submits the closed manifest.
3. Runtime records one bounded pending request and returns an opaque pending ID.
4. Studio or `runtrol integrations approve` presents observed identity, requested scopes, roots, and risk descriptions.
5. The operator may narrow but never widen the request.
6. A local physical-presence challenge binds the approval to the pending ID and public key.
7. Runtime commits the grant atomically and increments its generation.
8. The pending channel receives approval, denial, or expiry only.

Pending enrollment cannot list providers, models, projects, sessions, integration grants, or Runtime diagnostics. It
has attempt, byte, concurrency, and lifetime limits. Repeated requests cannot create an approval flood.

Ordinary app restarts do not require reapproval. A new key, broadened scope, new root, changed trusted package identity,
or revoked grant requires a new local decision according to the exact mutation policy.

## App scopes

Runtime integration scopes are separate from remote `DeviceScope`. They share default-deny semantics but are not
interchangeable tokens or enums.

Initial public scope vocabulary:

| Scope | Allows | Does not allow |
|---|---|---|
| `provider.read` | Provider descriptors and capabilities | Models, sessions, updates |
| `model.read` | Official model catalogue | Model selection without start or resume authority |
| `session.list` | Managed session metadata under roots | Event content or native catalogue |
| `session.native.discover` | Official native session pages under roots | Resume or transcript access |
| `session.output.read` | Live bounded normalized events | Historical transcript |
| `session.start` | Start under an approved root | Input after losing control |
| `session.resume` | Resume or adopt under an approved root | Scan or import arbitrary provider state |
| `session.input.write` | Acquire control and submit caller input | Prompt rewriting by Runtime |
| `session.stop` | Interrupt or cool an exact controlled session | Global process termination |
| `approval.respond.low` | Answer exact low-risk provider approvals | Shell, credential, or unknown approval |
| `approval.respond.high` | Answer supported high-risk classes after additional policy | Local administration or scope changes |
| `session.delete` | Request metadata forget under confirmation policy | Delete provider transcript |

High-risk approval automation remains denied until its own attempts prove a safe structural classifier and operator
contract. Unknown approvals expose reject only. The initial v1 may deliberately publish only low-risk response.

Scopes authorize method classes. They do not bypass project roots, control leases, workspace collision, lifecycle,
provider capability, request freshness, or approval subject binding.

## Root grants

The operator approves canonical project identities or roots through the existing Core path authority. Runtime stores
the canonical grant representation, not a caller-chosen string prefix.

Every session list, native catalogue, start, resume, event, approval, and mutation checks current root authority. The
check covers:

- Windows drive and case behavior
- symlinks and junctions
- Git worktrees and their shared identity
- submodules
- non-Git directories
- path replacement after approval
- provider-supplied working directories

Renaming, replacing, or escaping an approved directory cannot silently preserve authority. Ambiguous identity fails
closed and asks for a new local approval.

## Live subject binding

Authorization decisions bind to current state, not merely a session ID.

State-changing requests include expected session generation, control lease generation, integration grant generation,
and the exact subject. Approval responses additionally include provider-native approval ID, option ID, subject digest,
risk class from the held pending request, and expiry.

Runtime recomputes authority immediately before provider I/O. A scope revoke, root revoke, controller transfer,
provider restart, session generation change, or expired approval between request parsing and dispatch causes a typed
denial.

## Control lease safety

Exactly one integration controls one session generation. Many integrations may watch if separately authorized.

The lease is renewable, short, and generation-bound. Runtime never transfers it because a socket disconnected. An
expired controller during an active turn creates an orphaned control state. Another integration may observe but cannot
send input until the provider becomes idle or the operator performs a local forced transfer.

A consumer cannot hold a lease forever through request flooding. Renewal has rate limits and maximum horizons. Runtime
shutdown and recovery preserve or conservatively invalidate lease state so two controllers are never admitted.

## Mutation and replay protection

Every state-changing public method carries UUIDv7 `requestId`. Runtime retains a bounded per-integration record with
operation kind, target, result class, expiry, and a keyed authenticator over sensitive parameters.

The authenticator key belongs to Runtime state. A raw hash of low-entropy input is not stored because it could disclose
the input through guessing. Raw input, tool arguments, approval text, and provider output never enter the idempotency
table.

Duplicate same-authenticator requests return the recorded result. A different request under the same ID fails. Once a
record expires, uncertain mutations return `outcomeUnknown` instead of running again.

## Input confidentiality and integrity

Runtime necessarily carries caller input to the provider process. It guarantees:

- byte-for-byte transport for the negotiated input field
- no prompt prefix, suffix, system instruction, repair, title, or summarization
- no input in durable store, logs, traces, panic text, metrics, crash reports, or diagnostic bundles
- bounded transient buffers cleared by ordinary ownership drop and never reused as public response storage
- no automatic retry after an ambiguous provider write

The exact-input gate uses generated non-secret fixtures and byte comparison at the provider test boundary. Production
telemetry proves lengths and result classes only, never content-derived hashes visible outside the Runtime secret
boundary.

## Provider output handling

Provider output is hostile structured input. Drivers enforce the existing live payload ceiling before normalization.
Public serialization applies frame and extension limits again. Unknown structural fields remain bounded and opaque only
where allowed.

User-visible strings receive terminal control, bidirectional text, length, and invalid Unicode handling at the UI
boundary. Runtime does not interpret their semantic truth. Raw provider stderr is never inserted into a public error.

## Approval safety

Runtime forwards only approvals discovered through the provider's registered structured surface. A driver holds the
pending native request and derives structural risk from the registered operation shape.

The public response names one offered option. A consumer cannot submit a free-form replacement, alter risk, change the
subject, answer an expired request, or answer a request from another session generation. Incomplete or unknown
structure offers reject only.

High-risk approvals may require a fresh local physical action even when an integration has `approval.respond.high`.
The policy is explicit per risk class and cannot be relaxed by provider metadata.

## Hosted-service companion

A hosted service cannot connect directly to Runtime. It ships or integrates with a local companion that:

- enrolls as the Runtime integration
- owns its own cloud login and device association
- maps cloud user actions to local product intent
- requests only approved Runtime scopes and roots
- applies its own remote authentication, authorization, replay, and transport policy

Runtime sees only the local companion identity. It never accepts the service's model API key, cloud bearer token,
cookie, webhook secret, or arbitrary internet callback. The companion cannot ask Runtime to bind a network listener.

## Secrets

Runtime never stores or forwards provider model credentials. The provider CLI inherits only the environment and
authentication behavior already permitted by its registered driver and current security posture.

Consumer integration private keys remain in consumer-owned secure storage. Runtime public keys and grants are not
secrets but are integrity-protected state. Runtime's idempotency authenticator key and local administration secrets use
the existing restricted state boundary and rotation policy.

Logs and diagnostics redact environment values, endpoint nonces, signatures, cursors, provider raw output, input,
events, native titles where policy marks them sensitive, and unapproved paths.

## Revocation and changes

Revocation is a private local operation. It increments the grant generation, closes current public connections, drops
subscriptions, invalidates pending lease renewals, and prevents new requests before returning success.

Revocation does not kill a provider session or delete provider state. An active turn remains supervised by Core. The
operator may separately interrupt it through a local control action.

Scope or root narrowing has the same immediate generation behavior. Widening always requires a new explicit approval.
Key rotation proves the current key and requires local approval; recovery without the key creates a new integration.

## Audit metadata

Runtime keeps bounded operational audit metadata sufficient to answer:

- which integration and key generation attempted an operation
- which stable method, scope, project, session, and request ID were involved
- whether it was allowed or denied and the machine reason
- when a control lease changed
- when enrollment, scope, root, key, and revocation changed

It does not retain prompts, replies, event payloads, approval text, tool arguments, raw provider output, credentials, or
transcript locations. Retention and byte ceilings are frozen through security attempts and exposed to local purge and
uninstall policy.

## Abuse limits

Admission has limits per connection, integration, session, provider, and daemon for:

- connection and enrollment rate
- frame and request bytes
- concurrent provider probes
- catalogue pages and cursor size
- subscriptions, replay bytes, and pending notifications
- control acquisition and renewal
- mutations and idempotency records
- pending approvals and responses
- safe diagnostic and extension bytes

Limit errors are stable and do not disclose another integration's identity unless the operator has authorized that
diagnostic surface. Resource pressure cannot evict a live provider turn solely to serve public inventory.

## Required security mutations

The initiative cannot graduate while any mutation succeeds:

- connect as another OS user
- use a stale locator or endpoint replacement
- initialize with a replayed nonce or stripped revision offer
- enumerate before enrollment
- self-approve, self-widen, or choose an unapproved root
- call one scope through another method or extension name
- use a revoked grant on an already parsed request
- escape a root through symlink, junction, case, worktree, or path replacement
- steal control with disconnect timing or stale generation
- answer another session's approval or downgrade its risk
- trigger duplicate input through reconnect or request ID expiry
- obtain prompt, reply, credential, or raw provider bytes from logs, errors, store, or diagnostics
- reach a private control method through the public dispatcher
- create a TCP, HTTP, browser, or remote named-pipe path to Runtime

