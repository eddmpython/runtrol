# Consumer and ownership model

## Principals

| Principal | Identity | May own |
|---|---|---|
| Operator | Physical local presence plus OS user | Integration approvals, roots, high-risk actions, Runtime lifecycle |
| Runtime | One per-user daemon instance | Provider processes, Runtrol session IDs, event cursors, workspace claims |
| Integration | Enrolled public key plus `IntegrationId` | Its scopes, control leases, subscriptions, idempotency records |
| Client instance | One process connection under an integration | Connection-local requests and subscriptions |
| Provider | Runtime-discovered `ProviderId` and binary identity | Provider-native session and transcript |
| Remote service | Authenticated by its own local companion | Nothing inside Runtime directly |

Display names, executable paths, OS publisher observations, package identifiers, and integration manifests are
diagnostic metadata. The integration public key is the authentication identity. Runtime does not treat a reverse DNS
name or a claimed publisher string as proof.

## Integration kinds

### Native desktop application

The product embeds a Runtime SDK, generates one integration key in its local secure storage, and connects directly to
the per-user Runtime endpoint.

### IDE extension

The extension host embeds the TypeScript SDK. If the IDE extension sandbox cannot protect a long-lived private key,
its native host or first-party bridge owns the key. The browser or webview never receives it.

### Local automation tool

A command or daemon uses the Rust SDK and requests narrow project and lifecycle scopes. Headless enrollment still
requires local operator approval through the Runtrol administration CLI. A configuration file alone cannot grant
authority.

### Hosted service with local companion

The hosted service cannot connect to Runtime over the internet. Its separately installed local companion authenticates
to the service, authenticates independently to Runtime, and translates the service's product action into an explicit
Runtime request.

```text
Hosted service <-> service-owned secure channel <-> local companion <-> Runtime endpoint <-> provider CLI
```

The companion owns cloud credentials and cloud data policy. Runtime owns neither. Runtime authorizes the companion as
one local integration and cannot attest which remote human or service action originated a request. The enrollment UI
therefore states when an integration declares remote initiation.

### Browser-only product

Unsupported. A page does not receive named-pipe or Unix-socket access, an app credential, or a loopback CORS escape.
It requires a native companion or the later paired Runtrol PWA path.

## Integration manifest

An optional, non-authoritative manifest improves enrollment presentation:

```toml
schema = "runtrol.dev/integration/v1alpha1"
id = "dev.example.product"
name = "Example Product"
homepage = "https://example.dev"
privacy_policy = "https://example.dev/privacy"
remote_initiation = false

requested_scopes = [
  "provider.read",
  "model.read",
  "session.list",
  "session.output.read",
  "session.input.write",
]

requested_roots = ["userChoosesAtEnrollment"]
```

Runtime validates the closed schema and shows the manifest beside observed executable, package, and signature facts.
It never trusts the manifest for authorization. The operator selects the final scope and roots.

## Integration identity lifecycle

1. The SDK generates an Ed25519 key pair in the consumer's local secure storage.
2. The consumer sends the public key, manifest digest, and diagnostic process observations over the owner-only Runtime
   endpoint.
3. Runtime returns an opaque pending enrollment ID and exposes no provider or session data.
4. Runtrol Studio or `runtrol integrations approve` shows exact requested scopes and roots.
5. The operator completes a physical-presence challenge.
6. Runtime stores `IntegrationId`, public key, approved scopes, approved roots, timestamps, and safe diagnostic labels.
7. Each connection signs a server nonce plus protocol initialization transcript.
8. Runtime verifies the signature before returning inventory.

Private keys never enter Runtime. A lost private key creates a new pending integration and does not inherit the old
grant. Revocation takes effect before the next request and closes current connections after a final typed revocation
event.

## Honest same-user boundary

The key distinguishes cooperating integrations and enables exact revocation. It does not prove that the same OS user
has not injected into, debugged, or read another application's process or storage. Platform code signatures and
package identities are useful enrollment observations, not a universal sandbox.

Security claims are therefore:

- another OS user cannot connect through the owner-only endpoint
- an unenrolled integration receives no inventory
- an enrolled integration cannot exceed its Runtime scopes and roots
- accidental cross-product control is prevented and attributable
- a stolen integration private key acts with that integration's grant until revocation
- malware already executing as the operator remains inside the local trust boundary

## Session ownership

The provider owns conversation state. Runtime owns supervision. No integration owns a session.

A session has:

- stable Runtrol `SessionId`
- opaque provider and provider-native identifiers
- canonical project and working-tree identity
- lifecycle and turn state
- zero or one hot provider process
- zero or one control lease
- zero or more bounded read subscriptions
- provider-native approval state

An integration that starts a session becomes its initial controller but not its owner. If it disappears, the provider
process and session continue according to Core lifecycle policy.

## Control lease

Read operations are multi-consumer. State-changing operations require a control lease.

| Property | Contract |
|---|---|
| Scope | One `SessionId`, one `IntegrationId`, one lease generation |
| Acquisition | Atomic compare against no current controller and current turn state |
| Renewal | SDK renews before deadline on an independent lightweight request |
| Duration | Numeric value frozen by attempt measurement, never an unbounded lock |
| Disconnect | Lease remains for its short deadline so a network or host restart does not immediately transfer control |
| Active turn | Expired controller becomes `orphanedController`; another integration may observe but not submit until the provider is idle or operator transfers control |
| Transfer | Current controller releases, or operator performs a local forced transfer with exact session and app identities |
| Interrupt | Requires current control lease or separately granted emergency stop scope |
| Approval response | Requires current lease plus sufficient approval scope and exact pending subject binding |
| Close or forget | Separate scope and local confirmation when metadata removal is irreversible |

The lease gates Runtime requests. It does not claim the provider itself understands multiple clients.

## Multi-window and multi-process integrations

One integration may have several client instances. They share its grant but not implicit selection. The integration
chooses whether one instance owns the control lease or coordinates through its own process.

Runtime does not infer a primary window. Every request carries exact session, lease generation, request ID, and when
relevant workspace and cursor. Two instances using stale lease generations receive `controlConflict` rather than
having input land on whichever window wrote last.

## State-changing request discipline

Every mutation carries:

- `requestId`, a UUIDv7 generated once by the consumer
- exact target ID
- expected lifecycle or lease generation when applicable
- declared operation parameters
- negotiated protocol revision

Runtime retains a bounded per-integration idempotency record. It stores the request ID, a keyed message authenticator
over sensitive parameters, operation kind, and result classification. It never stores caller input text.

The same request ID and same authenticator returns the prior result. The same request ID with different parameters is
`idempotencyConflict`. Expired records produce `outcomeUnknown`, never an automatic re-execution.

## Input ambiguity

Provider input is the most dangerous retry boundary. Runtime durably records an input intent before writing to the
provider pipe and records a structured provider acknowledgement when the discovered surface has one.

If Runtime or the provider dies between write and acknowledgement:

- Runtime does not resend automatically
- the session exposes `inputOutcome = unknown`
- the control lease holder sees the exact request ID and safe timing metadata
- the user or consumer may resume and decide, but a new input uses a new request ID
- Runtime never reads a transcript to decide whether the previous input arrived

## Workspace grants

An integration grant contains canonical approved project roots, not arbitrary caller path prefixes. A session start or
resume resolves the requested path through the existing project identity boundary and proves it falls under one
approved root.

An approved root does not bypass writer collision admission. Shared writer admission remains local-only unless a
future scope explicitly and safely models it. Symlink, junction, case, Git worktree, submodule, and non-Git cases use
the same canonical Core identity as Runtrol Studio.

## Data ownership matrix

| Data | Owner | Runtime retention |
|---|---|---|
| Provider credential | Provider CLI | None |
| Conversation transcript | Provider CLI, and possibly consumer under its own policy | None |
| Live normalized event | Transport path | Bounded in-memory replay only |
| Runtrol session pointer | Runtime | Durable metadata |
| Consumer cloud account | Consumer | None |
| Integration private key | Consumer | None |
| Integration public key and grant | Runtime | Durable metadata |
| Control lease | Runtime | Durable or recoverable bounded metadata as required by crash gate |
| Caller input idempotency authenticator | Runtime | Bounded metadata, no input text |
| Provider-native title | Provider | Passed as metadata only, never generated by Runtime |

