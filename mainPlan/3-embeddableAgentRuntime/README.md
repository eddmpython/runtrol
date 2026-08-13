# Embeddable Agent Runtime

Status: design complete and active. Automatic updates have graduated, so implementation starts with contract
falsification and the read-only public boundary.

Order 3: the public Runtime contract sits below every current and future product surface. Runtrol Studio is the
first-party client. Independent IDEs, desktop applications, local companions for hosted services, automation tools,
and later Runtrol Mission and PWA surfaces use the same provider-neutral Core without reimplementing CLI discovery,
session supervision, or approval transport.

## One sentence

**A product integrates Runtrol Runtime once and immediately gains a local, provider-neutral API over the coding-agent
CLIs already installed and authenticated on the user's computer.**

To the consumer, Runtrol behaves like one local agent provider. Internally it remains a thin supervisor over many
provider-native CLIs. It is not a model, model proxy, hosted API, prompt router, transcript database, or agent that
reasons on the consumer's behalf.

## Product family

| Product | Customer | Job | Session ownership |
|---|---|---|---|
| `Runtrol Runtime` | Product developers and local integrations | Discover and supervise installed agent CLIs through one stable contract | Core |
| `Runtrol Studio` | End users in VS Code | First-party session and workspace interface | None |
| Runtrol PWA | End users away from the PC | Bounded remote control after pairing | None |
| External consumer | IDE, desktop app, local service companion, automation tool | Build its own user experience over Runtime | None |

One daemon owns provider processes and session metadata. Every client is replaceable. Closing any client does not end
a provider session, transfer ownership, or remove the provider-native conversation.

## The product insight

Every service that wants local coding agents otherwise rebuilds the same unstable layer:

- locate installed CLIs across three operating systems
- discover versions, flags, models, modes, and structured surfaces
- understand provider-specific start, resume, input, interrupt, approval, and completion protocols
- map a provider-native session to an exact workspace
- contain child processes and recover from crashes
- keep many sessions cheap while rendering one selected stream
- survive provider updates without hardcoded model names or private transcript paths
- preserve the user's existing provider authentication without copying credentials

Runtrol already owns this layer for its first-party surface. Productizing the layer turns that work into reusable
infrastructure instead of keeping it trapped behind one VS Code extension.

## Product promise

An independent product can:

1. Locate or install one shared per-user Runtime.
2. Register as an integration and receive only locally approved scopes.
3. List installed providers without starting every provider process.
4. Ask for provider models and capabilities only when its UI needs them.
5. List Runtrol-managed sessions immediately.
6. Discover provider-native sessions only through an official enumerable provider surface.
7. Start or resume a session in an exact approved workspace.
8. Submit caller-owned input without Runtrol rewriting it.
9. Watch normalized events with bounded replay and explicit gaps.
10. Answer provider-native approvals under exact risk and subject binding.
11. Disconnect, update, or uninstall its own product without killing or owning the session.
12. gain support for a new provider without changing consumer code.

## Two public roles

Runtrol has two opposite extension directions. They remain separate contracts.

```text
Provider author                         Product author
implements a driver or ACP agent        embeds the Runtime client
          |                                      |
          v                                      v
Provider SPI -> Runtrol Core <- Runtime Consumer API
```

- The Provider SPI answers: "How does Core supervise this installed CLI?"
- The Runtime Consumer API answers: "How does another product use every CLI Core can supervise?"

A consumer never loads a provider driver, reads a provider manifest, or branches on provider names. A provider never
learns which external product is watching it unless its own official protocol requires client information.

## Accepted design

| Question | Decision |
|---|---|
| Library embedded into every product or one daemon | One shared daemon. SDKs are clients, never session owners |
| Publish the current internal wire | No. Keep its exact-version control contract private and add a negotiated public Runtime protocol |
| One endpoint or two | Separate internal control and public Runtime endpoints in one daemon |
| First-party client | Runtrol Studio remains the reference client and migrates to the public Runtime SDK before v1 stability |
| External identity | Locally enrolled integration instance with a generated signing key and exact scopes |
| Trust same-user processes as distinct principals | No strong claim. App identity prevents accidents and supports revocation, but same-user malware remains inside the operator trust boundary |
| Remote SaaS connects directly to Runtime | No. A service needs its own local companion. Runtime never binds a public listener for third parties |
| Session input | Consumer bytes pass through unchanged. Runtime does not create or amend a prompt |
| Native session discovery | Official capability only, paginated and explicit. No transcript-directory scan |
| Full session history | Not provided. Resume uses the provider's official surface and live events continue from that point |
| Multi-client writes | One renewable control lease per session, many read-only watchers |
| Retries | Every mutating request has an idempotency key. Ambiguous provider input is never resent automatically |
| Public protocol style | Length-framed JSON-RPC with revision and capability negotiation |
| SDK SSOT | Rust public protocol types generate checked schema and TypeScript bindings |
| Standards adapter | A later ACP facade may cover the ACP-shaped subset. It cannot replace the richer native Runtime contract |
| MCP as the primary API | Rejected. MCP does not own the required provider inventory, session lifecycle, cursor, control lease, or approval contract |

## Delivery slices

| Slice | User or integrator result | Exit condition |
|---|---|---|
| 0. Contract falsification | A repository-external client can use a read-only prototype without internal imports | Protocol, trust, lifecycle, and thin-boundary mutations fail correctly |
| 1. Runtime discovery | A client finds the shared Runtime, negotiates a revision, enrolls, and lists providers | Three operating systems, clean install, update, rollback, revoke, and stale locator gates pass |
| 2. Read-only sessions | A client lists managed sessions, models, capabilities, and bounded events | 30-session performance and independent TypeScript and Rust clients pass |
| 3. Session control | An approved client starts, resumes, submits, interrupts, and answers low-risk approvals | Control lease, idempotency, workspace, crash ambiguity, and exact-input gates pass |
| 4. Native session catalogue | Official provider session lists appear with honest coverage and resume capability | ACP `session/list` and at least one non-ACP official surface pass without storage scanning |
| 5. Consumer distribution | Product developers install versioned SDK packages and a standalone Runtime | Packed-artifact consumer journey passes offline on all release targets |
| 6. Compatibility facade | Existing ACP clients can use the expressible subset through one Runtrol agent facade | Standards conformance and feature-loss disclosure pass |

Each slice begins under `tests/_attempts/embeddableAgentRuntime/`. Production code receives no placeholder branch,
unused dependency, dormant protocol method, or uncalled gate.

## Documents

| Document | Authority inside this initiative |
|---|---|
| [Product contract](1-productContract.md) | Product category, thin boundary, user promise, non-goals, and public claims |
| [Consumer and ownership model](2-consumerModel.md) | Integration kinds, principals, sessions, leases, concurrency, and hosted-service boundary |
| [Public Runtime protocol](3-publicProtocol.md) | Endpoint, framing, initialization, methods, DTOs, errors, cursors, and idempotency |
| [Client SDK and distribution](4-clientSdkAndDistribution.md) | Rust and TypeScript SDKs, bootstrap, packaging, locator, examples, and release artifacts |
| [Provider and session discovery](5-providerAndSessionDiscovery.md) | Fast inventory, model discovery, official native session enumeration, coverage, and drift |
| [Security](6-security.md) | Enrollment, app scopes, workspace grants, secrets, same-user limit, revocation, and audit metadata |
| [Compatibility and lifecycle](7-compatibilityAndLifecycle.md) | Protocol support window, daemon updates, multi-client recovery, deprecation, and uninstall |
| [Gates](8-gates.md) | Failure mutations, independent consumer journeys, performance, security, and release evidence |
| [Rollout](9-rollout.md) | Exact files, symbols, tests, rollback, evaluation, and phase order |

## Completion

The initiative completes only when all conditions hold:

1. A consumer project outside the runtrol source tree uses only a published SDK artifact and public documentation.
2. The consumer finds the shared Runtime on Windows, macOS, and Linux without a configured executable path.
3. The user enrolls it once, sees exact requested scopes and workspace roots, and can revoke it immediately.
4. The consumer lists installed providers and Runtrol-managed sessions without starting every provider.
5. Official native session enumeration is capability-driven, bounded, paginated, and explicitly incomplete where
   unavailable.
6. The consumer starts and resumes two different real provider CLIs without provider-specific code.
7. Caller input reaches the provider byte-for-byte and no copy enters Runtrol storage or diagnostics.
8. Two independent consumers watch one session while exactly one holds write control, with deterministic transfer.
9. A consumer and Runtime can update or roll back independently inside the declared compatibility window.
10. Consumer exit, crash, uninstall, and credential revocation do not delete provider-owned sessions.
11. Adding an external ACP provider requires no consumer, Core, SDK, or public protocol source edit.
12. Existing provider isolation, security, memory, idle CPU, responsiveness, update, and uninstall gates remain green.
13. Stable contracts graduate to `docs/`, published packages are reproducible, and this folder is deleted.

## Kill criteria

Stop or shrink the public Runtime if any of these persist after its owning slice:

1. Consumers need provider-name branches for ordinary lifecycle or event handling.
2. Useful integration requires provider transcript scanning, credential forwarding, model API keys, or prompt rewriting.
3. A private daemon per consumer is required to avoid ownership conflicts.
4. Public compatibility forces Core to retain unbounded state or blocks safe provider drift handling.
5. Enrollment, locator repair, or update asks the end user to configure paths or repeat approval after ordinary app
   restarts.
6. Two consumers can submit to one session without an explicit control decision.
7. A stale or malicious local integration can exceed its approved roots or scopes through the public endpoint.
8. Independent consumers cannot survive Runtime update and rollback inside the stated support window.
9. The packed SDK and standalone Runtime add more integration work than implementing the relevant stabilized ACP
   surface directly for a typical product.
10. No independent product adopts the Runtime after a documented integration sample and one stable release cycle.

## Source basis

The official ACP protocol stabilized optional `session/list`, `session/resume`, session metadata updates, and session
configuration capabilities in 2026. Runtime uses those official surfaces when a provider exposes them and reports
absence honestly. The native Runtime protocol remains necessary for cross-provider inventory, app enrollment,
workspace grants, multi-consumer control leases, bounded cursor semantics, and provider-neutral error behavior that
one ACP agent connection does not fully express.
