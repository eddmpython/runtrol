# Runtime integration

## What a consumer embeds

A product embeds either `runtrol-runtime-client` or `@runtrol/runtime-client`. The package locates and authenticates
one shared per-user Runtime. It does not bundle Runtime, start a private daemon, inspect provider files, hold provider
credentials, call a model API, or own a provider conversation.

Release candidates contain two `.crate` archives and one `.tgz` package. The release workflow builds each from a clean
checkout, attests it, and compiles a temporary consumer outside this repository. TypeScript consumers can install the
`.tgz` directly. Rust release consumers can unpack the two `.crate` files and use the client package through a vendored
source or approved registry mirror. Registry publication must preserve the same attested bytes and dependency graph.

The package READMEs contain compilable provider-neutral examples. They select provider and model identifiers only from
Runtime responses.

## Adoption flow

1. Create an integration identity and store its PKCS#8 private bytes in consumer-owned secure storage.
2. Derive the system `RuntimeLocator` and inspect it. Missing Runtime is `runtimeNotInstalled`, not permission for a
   silent download.
3. Connect with client name and version. Do not accept a caller-supplied endpoint in production.
4. Request the minimum scopes and canonical project roots. Persist the returned pending ID with the same identity.
5. Ask the operator to run `Runtrol: Review Integration Requests` in VS Code and review the exact name, instance,
   scopes, and roots.
6. Watch the enrollment decision. On approval, persist the grant together with the identity.
7. Reconnect with `IntegrationCredentials`. Validate the returned key and grant generations.
8. List providers and capabilities before selecting an opaque provider ID or optional operation.
9. Start, adopt, or resume under an approved root. Keep the mutation request ID until the result is certain.
10. Acquire control before input, interruption, cooling, or approval response.
11. Consume events with a bounded cursor. Accept each cursor only after the application has handled its event.
12. Close the SDK connection when the product exits. Do not stop Runtime or the provider session.

Runtrol Studio follows this same public SDK flow for ordinary provider and session operations. Private IPC remains
only for local administration such as enrollment review, grant management, revocation, update administration, and
physical-presence confirmation.

On Windows, a packaged Node.js consumer that already selected an exact absolute Runtime executable may pass it to
`RuntimeLocator.system({ runtimeExecutable })`. The executable's read-only `runtrol runtime-locator` bootstrap command
uses the Rust client's native owner and DACL validation. The TypeScript SDK still opens the platform-standard file,
validates its closed record and endpoint, and requires its security-relevant fields to equal the native observation.
The command never installs or starts Runtime. Consumers without an exact executable use the SDK's direct Windows
validation path. An older selected Runtime without the bootstrap command also falls back to that direct validation.

## Scope selection

Start read-only integrations with `provider.read` and `session.list`. Add `model.read` only when the product displays
Runtime-discovered choices. Add `session.output.read` only when it renders live events. Mutation scopes should be
requested separately and only when their controls exist in the product.

Session and approval scopes require at least one approved root. Runtime canonicalizes roots at approval and checks
the current filesystem identity again at dispatch. A display path is never authorization.

`approval.respond.high` is not a general confirmation bypass. Runtime still derives risk from the pending provider
request and may require a fresh local action. Consumers echo only the approval ID, offered option ID, and subject
digest returned by Runtime.

## Connection and retry

`connect_with_retry` in Rust and `connectSystemWithRetry` in TypeScript re-read the validated locator and retry only
transient connection establishment with capped exponential delay, jitter, and a total deadline. Authentication,
protocol, enrollment, revocation, unsafe locator, and authorization failures return immediately.

Read-only watcher helpers reconnect provider, session-index, and event subscriptions. Event recovery uses only the
last cursor the consumer explicitly accepted. They never retry input, approval, interrupt, lifecycle, or lease
mutations and never reacquire control silently.

Each watcher owns a dedicated streaming transport. Closing one drains bytes already handed to the operating system,
delivers an orderly end to Runtime, and destroys the unread client side without waiting for another server frame.
Cancellation therefore wakes a pending receive immediately and cannot strand local pipe capacity during rapid view
switching. Ordinary request-response clients keep their graceful close path.

The TypeScript package ships the complete JSON Schema as its public documentation artifact. Its runtime validator is
generated from the same definitions with documentation-only fields removed and is capped at 40 KiB by tests. The
projection retains every definition, property name, reference, closed-object rule, union, format, and numeric bound
that the validator executes.

## Failure recipes

| Failure | Consumer action |
|---|---|
| `runtimeNotInstalled` | Present an explicit verified Runtime install action or installation instructions |
| `runtimeUnavailable` | Show repair guidance and retry connection only within the caller's deadline |
| `protocolIncompatible` | Show both product versions and revision lists, then require a signed update or rollback |
| `enrollmentPending` | Keep the same identity and pending ID, then wait for the local decision |
| `enrollmentDenied` | Stop and let the operator initiate a new request if intent changes |
| `integrationRevoked` | Delete obsolete local credentials and create a new identity only with user intent |
| `scopeDenied`, `rootDenied` | Disable the operation and direct the operator to grant review, never self-widen |
| `controlConflict`, `leaseExpired` | Refresh session state and require an explicit control decision |
| `gap` | Replace the local view from the next complete snapshot and disclose the missing interval |
| `outcomeUnknown` | Keep the original request ID and ask Runtime for current state, never submit new input automatically |
| `presenceRequired` | Ask the operator to confirm the exact queued request in VS Code, then retry unchanged parameters (session forget, integration key rotation, and any session open with shared working-tree access) |
| `idempotencyConflict` | Treat request-ID reuse as a consumer defect and do not retry |

## Credential lifecycle

Consumer private keys stay outside Runtime. On key rotation, retain the replacement identity, mutation request ID, and
previous key generation until VS Code confirms the exact integration and replacement fingerprint. Retry the unchanged
rotation request to receive new credentials. The old key then stops authenticating.

On consumer uninstall, revoke the integration through `Runtrol: Manage Runtime Integrations`, remove the consumer key,
and remove only consumer-owned files. Revocation closes Runtime access but does not kill or delete provider sessions.
