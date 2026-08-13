# Product contract

## Category

Runtrol Runtime is a local-first, embeddable agent runtime. It converts the set of coding-agent CLIs already installed
and authenticated on one computer into one supervised, provider-neutral service for local products.

"Embeddable" means a product embeds a small client SDK and speaks to the shared daemon. It never means statically
linking Core, copying a daemon into each product's private state, or transferring session ownership into the product.

## Customer contract

The product developer writes one integration against Runtime and receives provider additions, provider updates,
session persistence, process containment, model discovery, approval transport, and workspace collision protection
through that contract.

The end user installs or already has one Runtime, approves each integration once, and reuses their existing provider
authentication. They never paste a model API key into Runtrol or into every consumer merely to reach a CLI that is
already authenticated.

## Thin boundary

| Runtime does | Runtime never does |
|---|---|
| Locates installed provider programs through the registered discovery ladder | Calls a model API directly |
| Observes exact versions, flags, capabilities, models, and official session surfaces | Hardcodes model identifiers or guesses unsupported flags |
| Starts, resumes, contains, interrupts, and cools provider processes | Implements a reasoning loop, planner, or hidden subagent |
| Carries caller input to the chosen provider without rewriting | Creates, summarizes, injects, or repairs a prompt |
| Normalizes structural lifecycle, content, tool, approval, notice, and usage event envelopes | Parses content for meaning, routing, learning, title generation, or policy |
| Retains bounded live frames and exact reconnect cursors | Stores a prompt, reply, event history, transcript copy, or raw command output |
| Stores provider-native session pointers and operational metadata | Discovers or derives a private transcript path |
| Uses provider-owned authentication in the child CLI | Reads, forwards, copies, or brokers provider credentials |
| Exposes honest partial and unsupported capability states | Silently substitutes another provider, model, workspace, or permission mode |

Runtime sees caller input and provider output transiently because transport is its job. "No conversation copy" means
the bytes are not interpreted for meaning, duplicated into durable storage, indexed, logged, or retained beyond the
existing bounded live-delivery window.

## Consumer contract

A conforming consumer:

1. Uses only negotiated Runtime methods and advertised capabilities.
2. Treats provider, model, native session, option, cursor, and extension values as opaque.
3. Displays partial, unknown, unsupported, gap, and operator-required states rather than inventing a fallback.
4. Sends only user or product-owned input and does not claim Runtrol authored it.
5. Requests the narrowest app scopes and project roots required by its product.
6. Does not persist a Runtime event stream as a shadow transcript unless its own product explicitly owns and discloses
   that behavior. Such storage is outside Runtrol's guarantee and branding.
7. Preserves request IDs across retry and never retries ambiguous input without the Runtime outcome.
8. Releases or renews its control lease and handles another consumer already controlling a session.
9. Does not branch on undocumented provider names, wire fields, error prose, or filesystem paths.
10. Keeps its own cloud authentication, account, billing, and remote transport outside Runtime.

Runtrol cannot prevent a third-party product from storing data it legitimately receives. The SDK and integration
documentation make the ownership boundary visible, and the conformance badge is withheld from products that claim
Runtrol itself stores or owns the transcript.

## End-user experience

### Existing Runtime

1. The consumer detects the Runtime in the platform-standard per-user locator.
2. The consumer asks to register and opens Runtrol Studio or the local approval command.
3. The user sees consumer name, executable and publisher observations when available, requested scopes, and project
   roots.
4. The user approves once.
5. Providers and managed sessions appear immediately.
6. The consumer asks for slower model or native-session discovery only when needed.

### Missing Runtime

1. The SDK returns typed `runtimeNotInstalled` without starting a download.
2. The consumer presents one user-initiated `Install Runtrol Runtime` action.
3. The action opens or invokes the verified per-user installer for the correct target.
4. The installer verifies release provenance, installs without administrator rights, starts Runtime, and returns to
   enrollment.
5. The user is never asked to locate a binary, socket, port, provider home, or transcript directory.

### Runtime already used by another product

The second product connects to the same daemon and receives its own identity and scopes. It does not start a second
daemon, open the database, take ownership of existing sessions, or duplicate provider processes.

## Public surfaces

The product ships four public artifacts:

| Artifact | Purpose | Stability |
|---|---|---|
| Runtime protocol specification and generated schema | Language-neutral contract | Revisioned compatibility policy |
| Rust client crate | Native products and tests | SemVer package API plus negotiated wire |
| TypeScript client package | IDE extensions, desktop hosts, and local companions | SemVer package API plus negotiated wire |
| Standalone Runtime package | Shared per-user daemon and administration CLI | Product SemVer plus signed target artifact |

An ACP facade is an optional fifth artifact after the native contract is proven. It is a compatibility adapter, not
the Runtime protocol SSOT.

## First-party dogfood

Runtrol Studio migrates its ordinary provider list, session list, model, start, resume, input, watch, interrupt, and
approval paths to the public TypeScript SDK before Runtime v1 is declared stable.

Studio may retain a separate internal control path only for operations unavailable to any consumer, such as Runtime
installation repair, provider update administration, integration approval, and physical-presence actions. Public
session behavior must not have a first-party shortcut.

## Provider neutrality

The public protocol contains no built-in provider enum. A `ProviderId` is an opaque runtime value accompanied by a
descriptor and capability map. A model selection is an opaque option returned by that provider's discovery result.

Adding a provider may change only:

- an external manifest or provider driver
- provider-specific discovery and event adapter code
- provider-specific real and drift gates

It may not change Core session logic, Runtime protocol methods, SDK method names, consumer sample code, Runtrol Studio
session logic, or external consumer code.

## Session discovery claim

Runtime has two distinct catalogues:

- `managed`: sessions already known to Runtrol metadata, returned from the store and live supervisor immediately
- `native`: sessions enumerated from an official provider surface, requested explicitly and reported with coverage

Runtime does not scan provider storage. A provider without an official list returns `unsupported`. A provider with a
limited official list returns `partial` and states the structural limitation. Runtime never presents its managed list
as every conversation that exists in the provider.

## Non-goals

- hosted Runtrol accounts or a cloud Runtime endpoint
- model API proxying, usage billing, or provider credential brokerage
- direct browser access to the local socket
- public loopback HTTP for third-party integrations
- loading Runtime as an in-process shared library
- one private Runtime home per consumer in production
- transcript search, history reconstruction, or provider storage crawling
- semantic provider selection or automatic prompt routing
- automatic prompt generation, rewriting, enrichment, or hidden instruction injection
- arbitrary terminal or shell exposure
- filesystem editing API
- native provider session deletion before an official provider surface and separate high-risk contract exist
- organization-wide deployment and policy in v1
- a promise that same-user malware is isolated from the operator's local account

## Public claim gate

No public page may call Runtime an "AI engine" without the clarifying phrase "local provider-neutral runtime for
installed coding-agent CLIs" in the same section. The product must not imply that Runtrol supplies a model,
intelligence, hosted inference, or provider subscription.

Claims such as "all conversations", "every model available to your account", "secure from every local app",
"lossless history", and "works with any agent" are forbidden. Public copy names the measured providers, official
capability coverage, operating systems, reconnect window, and security boundary.

