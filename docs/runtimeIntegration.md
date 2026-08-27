# Runtime integration

## What a consumer embeds

A product embeds one public client:

| Language | Package |
|---|---|
| Rust | `runtrol-runtime-client` |
| TypeScript | `@runtrol/runtime-client` |
| Python | distribution `runtrol-runtime-client`, import `runtrol_runtime` |

The client locates and authenticates one shared per-user Runtime. It does not bundle Runtime, start a private daemon,
inspect provider transcript files, hold provider credentials, call a model API, or own a provider conversation.

Runtime releases contain the Rust protocol and client crates, one TypeScript package, and six CPython 3.11 stable-ABI
Python wheels for Windows, macOS, and Linux on x64 and ARM64. Python publishes no source distribution, so installation
cannot silently become a local Rust build. Release jobs build from a clean checkout, attest exact artifacts, and run
an isolated consumer outside the repository.

Package examples are provider-neutral. They select opaque provider, model, session, and terminal identifiers only
from Runtime responses.

## Adoption flow

1. Create an integration identity and store its PKCS#8 private bytes in consumer-owned secure storage.
2. Derive the system `RuntimeLocator` and inspect it. Missing Runtime is `runtimeNotInstalled`, not permission for a
   silent download.
3. Connect with the client name and version. Do not accept a caller-supplied endpoint in production.
4. Request minimum scopes and canonical project roots. Persist the pending ID with the same identity.
5. Ask the operator to run `runtrol integrations review <pending-id>` in a local interactive terminal. Studio's
   **Runtrol: Review Integration Requests** command is an equivalent optional GUI.
6. Watch the enrollment decision. On approval, persist the grant with the identity.
7. Reconnect with `IntegrationCredentials`. Validate returned key and grant generations.
8. List providers and capabilities before selecting an opaque provider ID or optional operation.
9. Start, adopt, or resume under an approved root. Keep each mutation request ID until the result is certain.
10. Acquire control before input, interruption, cooling, approval response, or terminal write.
11. Consume structured events or terminal output with a bounded cursor or sequence. Accept a cursor only after the
    application has handled its event.
12. Close the SDK connection when the product exits. Do not stop Runtime or the provider session implicitly.

Runtrol Studio follows this public SDK flow for provider inventory, sessions, approvals, and terminal tabs. Its private
local connection is limited to bundled-Core bootstrap and optional owner administration. Studio contains no private
terminal protocol variants.

## Python shape

`AsyncRuntimeClient` is the native asynchronous API. `RuntimeClient` provides the synchronous facade with the same
method families. Both expose provider, session, and terminal operations, typed schema objects, exact-generation
terminal attach, and public exception classes derived from Runtime error codes.

The wheel contains one native client module, the generated public protocol types, the exact Runtime schema, license,
notice, README, and change log. It contains no Runtime executable. The CPython 3.11 stable ABI makes the same wheel
usable by supported later CPython 3 releases on that platform.

## Locator trust

On Windows, a packaged Node.js consumer that selected an exact absolute Runtime executable may pass it to
`RuntimeLocator.system({ runtimeExecutable })`. The executable's read-only `runtrol runtime-locator` command reuses
the Rust client's owner and DACL validation. The TypeScript SDK still opens the standard locator, validates its closed
record and endpoint, and requires every security field to equal that native observation.

Rust and Python use the same native owner validation. Unix requires the platform-standard owner and restrictive mode.
No client installs or starts Runtime during locator validation.

## Scope selection

Start read-only integrations with `provider.read` and `session.list`. Add `model.read` only when the product displays
Runtime-discovered models. Add `session.output.read` only when it renders structured events or terminal output.
Mutation scopes are requested only when matching controls exist.

Session and approval scopes require at least one approved root. Runtime canonicalizes roots at approval and checks
filesystem identity again at dispatch. A display path is never authority.

`approval.respond.high` is not a general confirmation bypass. Runtime derives risk from the pending provider request
and may require a new local action. A consumer echoes only the approval ID, offered option ID, and subject digest.

## Terminal continuity

Use `terminals/list` to discover `runtimeGeneration`, `terminalGeneration`, and `terminalId`. An attach after transport
loss must target that exact Runtime generation. The TypeScript and Python helpers re-read the locator and refuse a
different generation with `terminalGenerationUnavailable`; they never redirect to the latest Runtime.

`terminalAlreadyLive` returns the live owner when an open would duplicate a native conversation. A consumer lists
generations, finds that exact descriptor, and attaches. It never sends the input again. Terminal writes, resize,
control changes, stop, and detach follow the same no-blind-retry mutation rule.

## Connection and retry

`connect_with_retry` in Rust and `connectSystemWithRetry` in TypeScript re-read the validated locator and retry only
transient connection establishment with capped exponential delay, jitter, and a total deadline. Authentication,
protocol, enrollment, revocation, unsafe locator, and authorization failures return immediately. Python exposes the
same explicit connection boundary without hiding Runtime installation or enrollment.

Read-only watcher helpers reconnect provider, session-index, event, and terminal-index subscriptions. Event recovery
uses only the last cursor the consumer explicitly accepted. They never retry input, approval, interrupt, lifecycle,
lease, or terminal mutations and never reacquire control silently.

Each watcher or terminal view owns a dedicated streaming transport. Closing one cancels its pending receive and
cannot strand local pipe capacity during rapid view switching. Ordinary request-response clients retain graceful
close behavior.

The TypeScript and Python packages ship the complete JSON Schema. Generated validators and types come from the same
Rust definitions, preserving closed objects, unions, formats, numeric bounds, and method names.

## Failure recipes

| Failure | Consumer action |
|---|---|
| `runtimeNotInstalled` | Present verified installation instructions; never download silently |
| `runtimeUnavailable` | Show repair guidance and retry only within the caller's deadline |
| `protocolIncompatible` | Show both revision inventories, then require a signed update or rollback |
| `enrollmentPending` | Keep the same identity and pending ID, then wait for local review |
| `enrollmentDenied` | Stop and require new operator intent for another request |
| `integrationRevoked` | Delete obsolete credentials and enroll a new identity only with user intent |
| `scopeDenied`, `rootDenied` | Disable the operation and direct the operator to grant review |
| `controlConflict`, `leaseExpired` | Refresh state and require an explicit control decision |
| `terminalAlreadyLive` | List exact generations and attach to the returned owner without resubmitting input |
| `terminalGenerationUnavailable`, `terminalGone` | Mark that terminal unavailable; do not redirect or reopen implicitly |
| `nativeConversationBusy`, `legacyGenerationBusy` | Preserve the existing owner and require an explicit later retry |
| `gap` | Replace the local view from the next complete snapshot and disclose the gap |
| `outcomeUnknown` | Retain the request ID and inspect current state, never submit new input |
| `presenceRequired` | Ask the operator to run `runtrol requests review <pending-id>`, then retry unchanged params |
| `idempotencyConflict` | Treat request-ID reuse as a consumer defect and stop |

## Credential lifecycle

Consumer private keys stay outside Runtime. During key rotation, retain replacement identity, mutation request ID,
and previous key generation until the operator confirms the exact integration and fingerprint. Retry only the
unchanged rotation request. The old key then stops authenticating.

On uninstall, revoke with `runtrol integrations revoke <integration-id>` or Studio's equivalent command, remove the
consumer key, and remove only consumer-owned files. Revocation closes Runtime access but does not kill or delete
provider sessions.
