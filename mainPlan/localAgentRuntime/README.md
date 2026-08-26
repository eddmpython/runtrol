# Local Agent Runtime

Status: planned initiative

Owner decision date: 2026-08-26

## Decision

Runtrol is the provider-neutral local runtime that makes supported agent CLIs installed on the machine available to
approved applications. Each provider CLI keeps ownership of its model access, credentials, agent loop, native
session, and transcript. Runtrol owns discovery, process supervision, bounded transport, live-process continuity,
reconnect state, authorization, and a stable application contract. This initiative makes that existing core
independently usable by applications outside Studio.

The product family has three parts:

1. **Runtrol Runtime** is the shared per-user process and the product core.
2. **Runtrol SDKs** let approved applications use that Runtime without provider-specific code.
3. **Runtrol Studio** is the flagship human interface and the local graphical administrator. It is a first-party
   Runtime client, not the definition of the Runtime itself.

The Python package is this initiative's external-consumer proof. It is an official Runtime client, not a second
engine or the product identity itself. It does not move the Rust engine into a Python process, wrap provider SDKs,
call model APIs, or create a second transcript owner.

The one-sentence identity is:

> Runtrol turns supported agent CLIs installed on the machine into one user-owned local runtime that approved
> applications can discover, control, and observe without taking ownership of model credentials or conversations.

## Current state, target state, and unchanged invariants

| Boundary | Current state | Target state in this initiative |
|---|---|---|
| Runtime | A standalone Rust Runtime ships for six native targets | The same Runtime remains the only process owner |
| Structured API | Rust and TypeScript clients use the public provider-neutral protocol | Python gains the same public contract through the Rust client |
| Terminal | Core owns the provider TUI, but Studio reaches it through private IPC | The provider-faithful terminal becomes a scoped public Runtime surface and Studio adopts it |
| Administration | Studio approves integrations and presence-required mutations | The non-GUI `runtrol` executable can perform the same authority-changing operations in an interactive terminal |
| Distribution | Runtime, Rust client, and TypeScript client are release artifacts | Attested Python wheels and their provenance join the release inventory |

The following invariants do not change:

- one shared per-user Runtime owns supervised process lifetime
- provider CLIs own credentials, agent behavior, native identities, and durable transcripts
- Runtrol transports content without interpreting, rewriting, indexing, or storing a conversation copy
- remote authority is default-deny and Runtime never holds a provider model API key
- provider facts are measured at runtime and public clients contain no provider-specific branches
- adding a provider is a reviewed product decision and does not require editing Runtime core or SDKs

## Why this initiative exists

### The current product statement is narrower than the code

The current public positioning says the PC product is one VS Code window. The current product contract narrows the
visible product further to Projects, Conversations, Agent Usage, and provider-owned terminal conversations. This
flagship surface is not the whole reusable capability already implemented.

The repository already contains:

- a standalone Runtime release for six native targets
- a provider-neutral public JSON-RPC protocol
- a public Rust client
- a public TypeScript client
- integration enrollment, scoped roots, control leases, mutation identities, bounded streams, and reconnect cursors
- provider discovery, native session discovery, model discovery, usage observation, start, resume, input, interrupt,
  approval, deletion, and archival contracts
- a Core-owned pseudo terminal that runs the provider's own TUI and fans exact bytes out to viewers

The Runtime is therefore not a speculative extraction from Studio. It already exists. The missing work is to make
its public contract include the provider-faithful terminal surface and make it independently usable by applications
that do not install VS Code.

### The current reusable boundary has three concrete gaps

1. **No Python client.** Python applications must write the local framing, locator validation, identity proof,
   revision negotiation, authorization, reconnect, and cursor rules themselves.
2. **The faithful terminal surface is private.** Studio uses the private Core wire for `terminalOpen`, input, resize,
   and output. The public Runtime exposes normalized session events but not the provider's own live terminal surface.
3. **Administration depends on Studio.** An external integration can request enrollment, but approval and grant
   management are described as Studio actions. A headless Runtime is not independently adoptable while VS Code is a
   required administrator.

These are product gaps, not documentation gaps. Documentation changes only after the executable behavior is built.

### Provider SDKs do not remove the need

Provider SDKs are the right answer when an application chooses one provider and wants to own that provider's agent
configuration, prompt policy, tools, and result objects. They are not a provider-neutral shared supervisor.

As of 2026-08-26:

- the official Codex Python SDK starts and resumes Codex threads, streams progress, and reuses Codex authentication
- the Claude Agent SDK lets a Python process configure models, system prompts, tools, permissions, and session state
- Codex App Server and Agent Client Protocol expose structured client-to-agent protocols
- Claude Code Remote Control connects a running local Claude session to Anthropic's own web and mobile surfaces

Runtrol must not compete by offering a weaker `run(prompt) -> final_answer` wrapper. Its distinct job is the shared
local control plane across provider-owned CLIs and native sessions.

Use a provider SDK when all of these are true:

- the application chooses one provider
- the application owns the agent configuration and conversation lifecycle
- provider-specific result types are desirable
- using that provider's package and authentication contract is acceptable

Use Runtrol when any of these are true:

- the user chooses among installed providers at runtime
- the application must reuse the user's existing CLI authentication and native sessions
- sessions must survive the client application
- several applications must observe the same Runtime-owned live processes
- a faithful provider TUI is required
- provider changes must be isolated behind one measured driver instead of repeated in every application

## Product identity

### What Runtrol provides

Runtrol provides six capabilities as one local service.

| Capability | Contract |
|---|---|
| Availability | Discover installed CLIs, resolved executable identity, version, account state, models, and measured capabilities at runtime |
| Setup assistance | Return the provider's declared install, sign-in, and diagnosis actions without silently downloading, installing, or executing them |
| Session control | List native and managed sessions, start, resume, acquire control, submit exact input, interrupt, cool a structured process while retaining its known Runtime pointer, archive, and delete through provider-owned surfaces |
| Faithful presentation | Carry the provider's own terminal bytes and terminal geometry without parsing the terminal display for conversation meaning |
| Continuity | Keep process ownership in Runtime, retain bounded structured-event history, expose structured reconnect cursors, disclose gaps, and reconnect clients without moving transcript ownership |
| Policy | Admit the same OS user, enroll integrations, restrict scopes and roots, bind mutations, and keep risky administration local |

The common denominator is supervision and transport. It is not a common model, prompt, message, tool, or final answer.

### What Runtrol does not provide

- no model API client
- no API key vault for providers
- no generic LLM abstraction
- no application-owned agent loop
- no system prompt injection or prompt enhancement
- no generic `ChatMessage` transcript
- no semantic parser over TUI bytes
- no result summarizer, classifier, search index, memory, or context compactor
- no bundled provider CLI
- no public TCP or HTTP control endpoint
- no hosted account or cloud session owner

An application may compose the exact user input it intentionally sends. Runtrol transports those bytes or typed
blocks unchanged. Runtrol itself never adds instructions.

### Provider admission policy

Provider-neutral does not mean that every installed executable is automatically supported. It means Runtime core,
SDKs, and consuming applications do not branch on a provider name. The shipped set remains Claude, Codex, and Grok
for this initiative. A later provider enters only after its CLI surface is measured, an existing or new driver kind
expresses that surface, its manifest passes the provider gates, and the operator explicitly chooses to ship it.

This initiative does not add a provider, import a registry wholesale, or turn an arbitrary command into an agent.

### Ownership table

| Concern | Owner |
|---|---|
| Domain data and application workflow | Consuming application |
| User intent before submission | Consuming application and user |
| Provider selection | User or consuming application from Runtime-discovered choices |
| Provider credential and subscription | Provider CLI |
| Model catalogue truth | Provider CLI, observed by Runtime |
| Agent loop and tool semantics | Provider CLI |
| Durable transcript and native session identity | Provider CLI |
| Process and terminal lifetime | Runtrol Runtime |
| Runtime session pointer, supervised process state, and transport cursor | Runtrol Runtime |
| Integration private key | Consuming application |
| Authoritative integration grant and public key | Runtrol Runtime; the application stores only the returned public grant snapshot needed to detect stale authority |
| Rendering | Consuming application, using either terminal bytes or supported structured events |

### One Runtime, not one engine per application

The core remains a standalone Rust process. A Python import must not embed `runtrol-daemon`, create a private Runtime
home, or tie provider process lifetime to Python garbage collection. All clients connect to one shared per-user
Runtime through the local endpoint restricted to the same OS user.

This preserves:

- one Runtime process owning supervised process lifetime across Studio, Python applications, TypeScript applications,
  and the phone surface
- provider work after a client exits
- content-named Runtime generations and drain behavior
- native process containment on Windows, macOS, and Linux
- the AGPL Runtime and Apache client-package boundary
- one implementation of locator validation, authentication, leases, cursors, and error semantics

## Target public product contract

### Two explicit execution surfaces

Installed CLIs do not all expose the same rich structured protocol. Runtrol therefore exposes two explicit surfaces
instead of pretending every CLI is one generic chat API.

### Session and terminal state model

The two surfaces can refer to the same provider-owned conversation, but they are not the same Runtime object.

| Identity | Meaning | Lifetime and owner |
|---|---|---|
| `RuntimeSessionId` | One structured Runtime-managed session pointer and, while live, its supervised structured process | Runtrol may persist the pointer; the provider owns the underlying conversation |
| `RuntimeTerminalId` | One hosted provider TUI process in one Runtime generation | Memory-only in the owning Runtime generation; removed after the process exits |
| `RuntimeTerminalViewId` | One connection-bound terminal output subscription | Ends on detach, disconnect, revocation, or terminal exit |
| `NativeSessionId` | The provider's durable conversation identity | Provider-owned and opaque to Runtrol |

`TerminalDescriptor` contains only terminal ID, Runtime generation digest, provider ID, canonical workspace,
optional known native ID, process state, open time, and bounded geometry. It contains no screen snapshot, title
inferred from content, prompt, reply, or transcript preview.

The state rules are:

1. Opening a fresh terminal creates a new `RuntimeTerminalId`. Runtrol does not infer the native identity later by
   parsing output or searching provider storage.
2. Opening a listed native conversation records its `(provider_id, native_session_id)` on the terminal descriptor.
   A second open for the same native conversation and canonical workspace joins the existing terminal. A different
   workspace is a conflict, not a second process.
3. At most one live structured process or terminal process may own the same known native conversation. An inactive
   `RuntimeSessionId` pointer may coexist with a terminal, but Runtime refuses to start its structured process until
   that terminal exits.
4. `terminals/list` exposes only live terminal descriptors visible through the caller's approved roots. It is how a
   second approved application discovers a terminal before attaching.
5. Detaching the final viewer does not stop the CLI. Process exit removes the terminal, its views, and its leases
   after pending output and the exit notification have drained. It does not remove provider state.
6. A client reconnect targets the terminal's recorded Runtime generation and attaches from a fresh screen snapshot.
   It never claims replay of missed terminal bytes. A graceful Runtime update keeps that generation listed and
   draining while the terminal lives. A Runtime crash ends the terminal. If the provider later exposes a native
   identity, recovery requires a new provider-native resume.
7. `RuntimeTerminalId` never becomes a durable transcript key. It is valid only with its Runtime instance and
   generation identity.

Generation routing is part of every language client, not application code. The SDK re-reads the current OS user's
local Runtime locator through `RuntimeLocator.inspect_all()`, validates every listed generation, and may connect to
the descriptor's exact digest even when that generation is draining. `TerminalClient.list_all_generations()` queries
each compatible listed generation and returns descriptors keyed by `(runtime_generation, terminal_id)` plus an
explicit outcome for each generation it could not authenticate or query. A missing or non-answering recorded
generation returns typed `terminalGenerationUnavailable`; the SDK never redirects that terminal ID to the successor.

Every generation advertising public terminal capability must also advertise `GenerationAuthorityRelay` in its
private `GenerationHandoffCapabilities`. A terminal-capable generation without that relay is protocol-incompatible
and excluded from public terminal routing. Draining generations with the relay do not retain independent authority.
During handoff, the successor becomes the grant authority for that Runtime home and maintains one private
`GenerationAuthorityRelay` with each capable draining generation:

- the draining generation freezes its admission ceiling to the grants it already knew at handoff
- the successor propagates revocation, key rotation, grant-generation change, and scope or root reduction; a key
  rotation invalidates the integration's old key in the draining generation but does not admit the rotated key there
- the draining generation intersects updates with its frozen ceiling and never accepts a new integration or wider
  authority approved after drain began
- loss of the relay causes Runtime to retire terminal views and leases and refuse reconnects; the provider process
  continues until exit or an explicit local panic action

The same handoff also transfers every live claim before the successor accepts new work. The successor's
`NativeLiveClaimRegistry` is the atomic admission owner across that Runtime home for structured and terminal
processes in all listed generations. The registry records an exact `(provider_id, native_session_id)` claim for a
known native conversation. A fresh process whose native ID is unknown holds an unresolved
`(provider_id, canonical_workspace)` claim;
that claim permits other fresh work but blocks native resume for the same provider and workspace until it is replaced
with a native ID reported explicitly through the running CLI's declared control surface or released on exit. Runtrol
never resolves it by reading terminal output, inferring identities from catalogue data, or searching provider storage.

Exact-claim admission depends on the current owner:

- a terminal open joins a terminal in the same generation and canonical workspace and returns a new view
- opening against a terminal in another generation returns `terminalAlreadyLive` with its generation and terminal ID
- opening the same native ID in another workspace returns `terminalWorkspaceConflict`
- opening against a structured owner returns `nativeConversationBusy` with its generation and `RuntimeSessionId`
- resuming a structured session against a terminal owner returns `terminalAlreadyLive` rather than opening another
  process

The draining generation releases its claims on process exit. If it crashes, the successor releases them only after
the generation process and its contained provider child have both exited. Client-side listing is never used as the
admission lock.

#### Structured session surface

The structured surface is for automation and custom application interfaces. It uses a provider's official protocol
or structured command and exposes only capabilities that the measured provider surface supplies.

It includes the existing provider, model, native catalogue, managed session, input, event, approval, usage, and
lifecycle methods. A missing capability is `unsupported`, `unavailable`, or `unknown`. It is never emulated by parsing
terminal text.

#### Terminal session surface

The terminal surface is the provider-faithful interface available across supported CLIs. Runtime starts or resumes
the provider's own TUI on the existing Core-owned pseudo terminal and exposes:

- open a fresh terminal conversation
- open a listed native conversation
- attach another viewer to a live terminal
- receive the current bounded screen snapshot followed by exact output bytes
- send exact keyboard bytes under the current control lease
- resize the shared PTY from one viewer
- receive an explicit lag boundary and replacement screen snapshot
- detach a viewer without stopping the CLI
- observe provider process exit

Terminal output is not converted into messages or final answers. Applications render it with a terminal renderer
such as xterm.js or otherwise present it only as terminal output.

The new protocol revision adds these request methods:

| Method | Contract |
|---|---|
| `terminals/list` | Return the caller-visible live terminal descriptors in this Runtime generation |
| `terminals/watchIndex` | Start a bounded, root-filtered terminal index subscription |
| `terminals/open` | Open a fresh terminal or resume one listed native conversation, attach one view, and return descriptor plus initial screen snapshot |
| `terminals/attach` | Attach one view to a visible live terminal at its current shared geometry and return descriptor plus current screen snapshot |
| `terminals/acquireControl` | Acquire the one renewable write lease from an observed terminal generation |
| `terminals/renewControl` | Renew the specified lease generation |
| `terminals/releaseControl` | Release the specified lease generation |
| `terminals/write` | Submit exact caller-owned bytes once under the current lease and mutation identity |
| `terminals/resize` | Set shared PTY geometry within Runtime limits under the current control lease |
| `terminals/detach` | End only the caller's connection-bound view |
| `terminals/stop` | Stop the specified hosted CLI process under the current lease and mutation identity; this does not promise that an unknown native conversation can be resumed |

The revision adds these server notifications:

| Notification | Contract |
|---|---|
| `terminals/indexChanged` | Replace the caller-visible terminal index snapshot |
| `terminals/indexEnded` | State why an index subscription ended |
| `terminals/output` | Carry a bounded chunk of exact output bytes for a view |
| `terminals/lagged` | Declare lost terminal bytes and carry one bounded replacement screen snapshot atomically |
| `terminals/exited` | Deliver the provider process exit code after preceding output has drained |

`open` and `attach` are view-producing operations on dedicated streaming transports. `open` also returns an initial
control lease when the caller holds `session.input.write`; `attach` never grants control implicitly. Terminal output
has no event cursor. Reattach means a new screen snapshot boundary followed by live bytes.

The method set reuses the existing `runtrol-core::terminal` and daemon `terminal_surface::Terminals`; a second
terminal host is forbidden.

### Terminal authorization matrix

This initiative reuses the closed `AppScope` vocabulary. It does not create a broad `terminal.*` grant that bypasses
existing session authority.

`TerminalControlLease` reuses the existing control-lease lifetime, generation, renewal, revocation, and stale-write
rules, but it is keyed by `RuntimeTerminalId` and cannot authorize a structured session mutation. A structured
`ControlLease` cannot authorize terminal input.

| Operation | Required scope | Root and provider check | Lease or presence |
|---|---|---|---|
| list and watch index | `session.list` | Filter every descriptor to approved canonical roots | None |
| open fresh | `session.start` and `session.output.read` | Requested provider must be observed and workspace must canonicalize inside an approved root | Optional initial lease only with `session.input.write` |
| open native | `session.resume` and `session.output.read` | Same root check plus the exact native identity from Runtime's listing | Optional initial lease only with `session.input.write` |
| attach and receive output | `session.output.read` | The descriptor's provider and canonical workspace must remain within the caller's grant | None |
| detach | `session.output.read` | Bound to the caller's visible connection-bound view | None |
| acquire | `session.input.write` | The descriptor's provider and canonical workspace must remain within the caller's grant | No existing lease; observed terminal generation and single-winner acquisition rules apply |
| renew, release, write, and resize | `session.input.write` | The descriptor's provider and canonical workspace must remain within the caller's grant | Exact current `TerminalControlLease`; write is never retried automatically |
| stop | `session.stop` and `session.input.write` | The descriptor's provider and canonical workspace must remain within the caller's grant | Exact current lease and mutation identity |

Every operation is checked again after grant generation changes. Revocation retires views and leases. Terminal byte
content, screen snapshot content, and provider transcript content never enter an authorization record or diagnostic.

### Capability-first selection

Applications select only from Runtime observations. They do not branch on `provider_id` or version strings.

The provider capability result must distinguish at least:

- structured fresh session
- structured resume
- terminal fresh session
- terminal native resume
- model catalogue coverage
- native session catalogue coverage
- structured approval
- native delete and archive
- account usage coverage

A consuming product chooses the required surface from these observations. Runtrol never silently substitutes a
terminal session for a structured session or the reverse.

### Independent local administration

Studio remains the easiest graphical administrator, but it cannot be required for Runtime adoption. The `runtrol`
executable gains these local administration commands:

| Command | Behavior |
|---|---|
| `runtrol integrations review <pending-id>` | Show identity, key fingerprint, manifest digest, requested scopes, and canonical roots; interactively narrow and approve or deny |
| `runtrol integrations list` | Show approved identities, scopes, roots, key generation, and grant generation without secret material |
| `runtrol integrations revoke <integration-id>` | Revoke after the owner types the full integration ID |
| `runtrol requests review <pending-id>` | Review a specific presence-required mutation with its bounded consequences |
| `runtrol providers help <provider-id>` | Print measured state and manifest-declared install, sign-in, and diagnosis commands for the user to run explicitly |

The first-use journey is fixed:

1. The application generates and durably stores its own signing identity.
2. It submits an exact enrollment manifest and receives a bounded pending ID without receiving Runtime inventory.
3. The application shows that pending ID and its public-key fingerprint to the user.
4. The user runs `runtrol integrations review <pending-id>` in an interactive terminal owned by the same OS user,
   compares the identity and fingerprint, narrows scopes and roots, then approves or denies.
5. The connection that proved possession of the pending identity observes the decision. On approval the application
   stores its private identity plus the returned public `IntegrationGrant` snapshot as `IntegrationCredentials`, then
   reconnects; on denial or expiry it receives no Runtime authority.

`IntegrationGrant` is not a bearer token. Runtime remains the authority source and checks its current grant and key
generations on every authenticated connection. The consumer's copy lets it name the integration, sign with its own
private key, and reject an unexpected or stale Runtime answer.

Approval requires a real interactive owner terminal and retyping the full pending ID shown in the review. There is
no `--yes`, piped stdin, environment variable, public Runtime method, or remote route that can approve or widen
authority.
Machine-readable inventory may be printed, but authority-changing commands stay interactive. Provider help commands
are displayed, never run by Runtime. These commands use the executable generation's private administration endpoint,
which admits only the same OS user. They do not use an integration-callable public Runtime method.

### Installation experience

The Runtime binary and language clients remain separate artifacts.

| Artifact | Contents | License |
|---|---|---|
| Standalone Runtime archive | Rust executable, schema, manifest, checksums, per-user installer and uninstaller | AGPL-3.0-only |
| Rust client crate | Public protocol client only | Apache-2.0 |
| TypeScript package | Public protocol client only | Apache-2.0 |
| Python package | Binding over the public Rust client and generated Python types | Apache-2.0 |

The Python distribution is named `runtrol-runtime-client` and imports as `runtrol_runtime`. It does not bundle a
provider CLI or Runtime executable. A consuming product may ship the exact signed Runtime archive in its own
installer and invoke the verified per-user installer after an explicit user action. Importing the Python package or
calling `connect()` never downloads or installs software.

The SDK returns typed `runtimeNotInstalled`, `runtimeUnavailable`, `protocolIncompatible`, `legacyGenerationBusy`,
`nativeConversationBusy`, `terminalAlreadyLive`, `terminalGone`, `terminalGenerationUnavailable`, and
`terminalWorkspaceConflict` failures with the same meaning in Rust, TypeScript, and Python. It never starts a private
daemon when the shared Runtime is unavailable.

### Python distribution contract

| Decision | Contract |
|---|---|
| Registry and names | Publish `runtrol-runtime-client` to PyPI; import it as `runtrol_runtime` |
| Interpreter floor | Support GIL-enabled CPython 3.11 and later through the `abi3-py311` stable ABI; PyPy, GraalPy, and free-threaded CPython are unsupported in the first release |
| Build owner | Build the native extension with PyO3 and maturin from the repository Rust workspace |
| Wheels | Publish one `abi3` wheel for each supported Windows, macOS, and Linux x64 or ARM64 target |
| OS floor | Each wheel uses the same Rust target triple, minimum Windows or macOS policy, and Linux libc policy as its matching Runtime archive; the release gate rejects a mismatched wheel tag or linked-system floor |
| Source distribution | Do not publish an sdist in the first release; unsupported platforms fail at package resolution instead of compiling an unverified local variant |
| Publication | Use PyPI Trusted Publishing from the protected Runtime release workflow and attach provenance for every immutable wheel |
| Versioning | Python SemVer describes the Python API; compatible Runtime protocol revisions remain a separate negotiated set |

The exact PyO3, maturin, and build-action versions are release dependencies selected and locked by the repository at
implementation time. The product contract above does not depend on one remembered tool version.

### Python client shape

The Python package wraps the existing Apache-licensed `runtrol-runtime-client` Rust crate through a native extension.
It does not reimplement Windows DACL inspection, Unix owner checks, Ed25519 proofs, framing, revision negotiation,
cursor recovery, or mutation rules in Python.

Python-facing models and type declarations are generated from the checked JSON Schema. Generated files are derived
artifacts; Rust DTOs remain the protocol source of truth.

The public package provides:

- `RuntimeLocator` and safe system inspection
- `IntegrationIdentity` for the consumer-owned private signing key
- `IntegrationCredentials` for that identity plus the public `IntegrationGrant` snapshot used during reconnect
- `AsyncRuntimeClient` as the canonical implementation
- `RuntimeClient` as a synchronous facade with behavioral parity
- typed provider, session, approval, terminal, cursor, and error values
- context-manager close semantics that close only the client connection
- reconnecting provider, session-index, terminal-index, structured-event, and terminal-view subscriptions
- explicit cursor acceptance after the application consumes a structured event
- terminal reattach from a replacement screen snapshot, never a claim of byte replay
- no automatic retry for input, approvals, interrupts, lifecycle changes, or lease mutations

Target usage:

```python
from runtrol_runtime import AsyncRuntimeClient

runtime = await AsyncRuntimeClient.connect_system(
    credentials=load_integration_credentials_from_secure_storage(),
)

providers = await runtime.providers.list()
provider = next(item for item in providers if item.installation.state == "usable")
capabilities = await runtime.providers.capabilities(provider.provider_id)
if not capabilities.terminal_fresh:
    raise RuntimeError("the selected provider has no measured terminal surface")

view = await runtime.terminals.open_fresh(
    provider_id=provider.provider_id,
    workspace=approved_workspace,
    columns=120,
    rows=36,
)

render_terminal_screen(view.initial_screen)
async with view.control() as control:
    await control.write(b"Explain this repository\r")
async for update in view.updates():
    apply_terminal_update(update)
```

This example is a target contract, not a claim about the current package.

### Hosted applications

A hosted application cannot reach a local endpoint restricted to the user's OS account directly. It must ship a
local companion.
The companion is the enrolled Runtime integration, owns the hosted product login, authenticates remote intent, and
requests minimum scopes and roots. Runtime never accepts cloud cookies, bearer tokens, webhooks, or arbitrary public
listeners.

The Python package can implement the companion, but installing the package on a cloud server does not make a user's
local CLI remotely available.

## Architecture decisions

### Reuse the public Rust client

The Python binding depends only on `runtrol-runtime-client` and `runtrol-runtime-protocol`. It must not link private
Core, daemon, driver, IPC, store, vault, Studio, or phone modules. A boundary gate rejects those dependencies.

This choice keeps the security-critical client behavior in one implementation and avoids a second locator,
authenticator, frame reader, or reconnect state machine.

### Promote terminal transport into the public protocol

The current terminal implementation is already provider-neutral and bounded. The work is to place an authenticated,
scoped public adapter in front of it.

The public adapter must:

- use the same terminal table as the private wire
- bind open and attach to approved provider and canonical root authority
- enforce one live process for each known provider-native conversation across structured and terminal surfaces
- bind write, resize, and stop to one renewable terminal control lease
- bound columns, rows, output chunks, screen snapshots, and queues
- keep terminal bytes out of logs, errors, audit records, and durable stores
- disclose lag and process exit explicitly
- keep a terminal alive when the last viewer disconnects
- count a terminal as live work during Runtime generation drain
- let a reconnect target its still-listed owning generation and reattach from a replacement screen snapshot
- deduplicate opening the same provider-native conversation and reject a conflicting workspace

Studio switches every new terminal and every terminal from a generation advertising public terminal capability to
the public client. The current generation removes its private `terminalOpen`, `terminalAttach`, `terminalInput`, and
`terminalResize` server path. A public terminal failure never falls back to private IPC.

### Bridge the first public-terminal update without losing live work

A Runtime generation shipped before this revision cannot gain a public method while it is running. Studio therefore
retains a client-only `LegacyTerminalAttachment` for each terminal tab that was opened before the update and whose
recorded generation advertises no public terminal capability. Before switching generations, Studio persists only the
terminal ID and generation digest needed to recover that tab, never terminal bytes or a screen snapshot. The bridge:

- resolves only the tab's recorded generation through Studio's existing private resolver scoped to that OS user
- may attach to that known terminal ID, receive output, send exact input, resize its PTY, and detach
- cannot list, discover, or open a private terminal
- cannot target the current generation or be used after the old terminal exits
- is never selected after a public terminal error

The new Runtime contains no legacy server implementation; the already running old generation serves the historical
wire it shipped with. The bridge remains only while the supported upgrade floor includes the last pre-public terminal
revision and is deleted when that floor advances. This is a bounded in-flight upgrade adapter, not a second current
terminal contract. A pre-public generation is never placed in public terminal integration routing and does not need
to implement `GenerationAuthorityRelay`; only the Studio bridge restricted to the same OS user can reach its already
known private terminal.

That pre-public generation cannot export native live claims. While any such draining generation remains listed, the
successor conservatively refuses every structured or terminal resume of a known native conversation with typed
`legacyGenerationBusy`; fresh work remains available. Runtime permits native resumes again only after every
pre-public generation exits. Client-supplied legacy tab data is never accepted as an admission claim.

### Make Runtime independently administrable

The existing private administration authority remains private, but the `runtrol` executable becomes its supported
interactive client. Studio and the CLI call the same administration service. Neither duplicates grant validation or
presence checks.

The standalone archive already contains the administration executable. This initiative adds interactive commands for
the complete enrollment, grant, revocation, and presence-review lifecycle alongside its existing install, status,
endpoint, and uninstall operations.

### Keep provider extensions out of clients

Python, TypeScript, Rust, Studio, and the administration CLI consume the same provider inventory and capability data.
None contains provider installation commands, model names, resume flags, session paths, or provider-name branches.

Provider support remains:

```text
manifest -> measured driver kind -> provider-neutral Runtime capability
```

Agent Client Protocol remains one driver protocol. It is not the Runtrol public wire because non-ACP providers,
local authorization, terminal viewers, bounded delivery, and shared Runtime generations are Runtrol responsibilities.

### Protocol and package compatibility

Public terminal support and independent administration require a new finalized Runtime protocol revision. The
semantic shape is fixed by this document. The implementation assigns the next unused numeric revision before code
lands and adds it beside every already shipped initialization shape; changing a method shape requires updating this
initiative before implementation continues.

Compatibility rules:

- every shipped client still completes `runtime/initialize`
- old clients keep their current methods and behavior
- terminal capability is advertised through `RuntimeCapabilities` and absent on old Runtime versions
- Python package SemVer describes the Python API
- protocol revision describes the wire
- Runtime update does not rewrite an application's Python dependency
- Python package update does not silently replace Runtime
- an already published revision is never removed during rollback

## Delivery sequence

The sequence follows dependency direction. No public release claims this initiative until the final external
consumer journey passes.

1. **Protocol contract**
   - add the terminal state model, DTOs, finalized methods, notifications, limits, scope mapping, and capability fields
   - export a new checked JSON Schema
   - add old-initialization corpus compatibility
2. **Runtime terminal adapter**
   - route authenticated public connections to the existing terminal table
   - enforce root, provider, cross-surface native identity, lease, queue, byte, and drain boundaries
   - add the successor-owned grant relay and native live-claim registry for the Runtime home before accepting new work
   - prove no terminal content enters storage or diagnostics
3. **Independent administration**
   - add interactive integration and presence review commands to `runtrol`
   - route Studio and CLI administration through one daemon-owned implementation
   - prove a clean machine can enroll and revoke a client without VS Code
4. **Existing clients and Studio migration**
   - add Rust and TypeScript terminal clients with explicit per-generation fleet outcomes
   - switch Studio terminal tabs from private IPC to the public client
   - retain only the bounded client-side attachment bridge for already live pre-public terminal generations
   - remove the private terminal server path and every private path for new work
5. **Python package**
   - create the Python native binding over the public Rust client
   - generate Python types from the checked schema
   - ship sync and async parity, typed errors, subscriptions, examples, and a packed external-consumer test
6. **Distribution**
   - build `abi3-py311` Python wheels for the same six native target classes as the Runtime
   - publish through PyPI Trusted Publishing without an sdist
   - include the Python artifact inventory in the Runtime release manifest and provenance
   - verify installation from the built wheel outside the repository
7. **Product proof**
   - run a standalone Python consumer against an installed release Runtime and real installed CLI
   - enroll through the interactive `runtrol` administration command without Studio
   - discover, start, type, receive terminal bytes, disconnect, reconnect, resume, and uninstall without touching
     provider credentials or transcript ownership
8. **Knowledge promotion**
   - recut `docs/` from the implemented code
   - update the public product identity and package instructions in all README variants
   - delete this initiative folder after every retained fact has a code or `docs/` owner

## Implementation plan

### 1. Impact files

| Area | Expected files |
|---|---|
| Workspace | root `Cargo.toml` and `Cargo.lock` for the binding and release dependencies |
| Protocol SSOT | `crates/runtrol-runtime-protocol/src/{method,integration,rpc,terminal}.rs`, `crates/runtrol-runtime-protocol/schema/runtime.schema.json`, `crates/runtrol-runtime-protocol/hello_corpus/**` |
| Runtime dispatch | `crates/runtrol-daemon/src/{runtime_serve,runtime_auth,runtime_control,integration_admin,terminal_surface,scope,generations,compose}.rs` |
| Generation handoff and legacy wire | `crates/runtrol-ipc/src/wire.rs` plus focused generation-control compatibility tests |
| Shared terminal core | `crates/runtrol-core/src/terminal/**` only where the public adapter reveals a missing reusable contract |
| Rust client | `crates/runtrol-runtime-client/src/{client,connection,lib}.rs` |
| TypeScript client | `clients/typescript/src/**`, generated schema and protocol, tests and packed consumer |
| Python client | new `clients/python/` package with `Cargo.toml`, `pyproject.toml`, `README.md`, `CHANGELOG.md`, `LICENSE`, native binding, generated models and type declarations, tests, examples, and packed consumer |
| Administration | `crates/runtrol-cli/src/**`, `crates/runtrol/src/main.rs`, daemon administration modules |
| Studio | `extensions/runtrol-vscode/src/{runtimeClient,terminalTabs,protocol,extension}.ts`, bounded legacy tab identity state, and focused tests |
| Release | `.github/scripts/release/runtimePackage.py`, `.github/workflows/runtime-release.yml`, PyPI publishing configuration, release manifest checks |
| Gates | license and dependency policy plus focused Runtime boundary, Python SDK, terminal, distribution, compatibility, and external-consumer gates under `tests/audit/` |
| Final docs | `README*.md`, `docs/{coreRuntime,runtimeProtocol,runtimeIntegration,runtimeSecurity,runtimeOperations,productSurfaces,terminalSurface}.md` |

### 2. Impact functions and symbols

The implementation starts by tracing and changing these existing owners rather than adding parallel helpers:

- `RuntimeMethod`
- `RuntimeErrorKind`
- `AppScope`
- `RuntimeCapabilities`
- `RuntimeLimits`
- `RuntimeTerminalId` and `RuntimeTerminalViewId`
- `TerminalDescriptor` and `TerminalIndexSnapshot`
- `TerminalOpenParams`, `TerminalAttachParams`, and `TerminalViewOpened`
- `TerminalControlLease`, `TerminalControlParams`, `TerminalWriteParams`, and `TerminalStopParams`
- `TerminalOutputNotification`, `TerminalLaggedNotification`, and `TerminalExitedNotification`
- `dispatch_public`
- existing `required_scope` and its composite `required_scopes` replacement
- `terminal_surface::Terminals`
- `runtrol_core::terminal::Terminal`
- the new daemon `TerminalRuntimeAdapter`
- the new `GenerationHandoffCapabilities`, `GenerationAuthorityRelay`, and `NativeLiveClaimRegistry`
- `RuntimeClient`, `SessionClient`, the new `TerminalClient`, and `TerminalSubscription`
- `RuntimeLocator`
- `StudioRuntimeClient`
- `CoreTerminal`
- the Studio-only `LegacyTerminalAttachment`
- the new CLI `AdminCommand` and its daemon-owned administration handlers
- Python `_native`, `AsyncRuntimeClient`, `RuntimeClient`, and generated public models
- Runtime release `sdkArtifacts`

New Python exports are projections of these public owners. They do not become another protocol source of truth.

### 3. Tests

The initiative is complete only when all of these behaviors are executable evidence:

1. Every terminal method has a positive and negative scope test, and every descriptor or action is filtered or
   refused against its stored canonical root.
2. A public terminal cannot open without an observed provider, approved root, start or resume scope, and output-read
   scope.
3. Two concurrent open requests for one known native conversation join one process in the same workspace and return
   a conflict across workspaces or against an already live structured process.
4. A fresh terminal remains distinct from a structured `RuntimeSessionId`, and Runtime never invents a native ID by
   reading terminal output or provider storage.
5. Two viewers racing for control produce one lease holder. Expiry, stale generation, failed renewal, and revocation
   all make later writes fail.
6. Terminal input and stop are rejected without the specified current lease and mutation identity. An ambiguous
   transport failure never causes an SDK to resubmit bytes or reacquire control silently.
7. Attach delivers the initial screen snapshot before subsequent bytes. A slow viewer receives an atomic lag boundary
   and bounded replacement screen snapshot, while another viewer continues without duplication.
8. A viewer without the current control lease cannot resize the PTY. The lease holder's latest accepted bounded
   resize sets shared PTY geometry for every viewer.
9. Detaching or closing Python, TypeScript, Studio, or Rust clients does not stop the provider CLI.
10. A reconnecting client reaches the descriptor's recorded generation and receives a new screen snapshot boundary
    before live bytes. If that generation vanished, it receives `terminalGenerationUnavailable`; if the generation
    answers but the terminal ended, it receives `terminalGone`.
11. `TerminalClient.list_all_generations()` validates and queries current and draining generations, merges identities
    without collision, and reports an explicit unsupported outcome for an old generation without terminal capability.
12. Runtime generation drain keeps live terminals, refuses new ones, and lets a client reconnect to the still-listed
    draining generation while the successor accepts new work.
13. A revocation or grant reduction in the successor retires affected draining-generation views and leases. An
    integration approved or widened after drain cannot use that new authority against the old generation, and key
    rotation invalidates the old key there.
14. Two generations racing to open the same known native conversation produce one admitted claim in
    `NativeLiveClaimRegistry`. A same-generation, same-workspace terminal open joins the existing terminal. A
    cross-generation request returns `terminalAlreadyLive`, a request against a structured owner returns
    `nativeConversationBusy`, and a workspace mismatch returns `terminalWorkspaceConflict`. A crashed generation's
    claim is released only after the generation process and its contained provider child have both exited. An
    unresolved fresh claim blocks native resume for the same provider and workspace without blocking other fresh work.
15. Updating Runtime and Studio with a live terminal from the final pre-public revision reattaches that recorded tab
    through `LegacyTerminalAttachment`. The bridge cannot discover or open private terminals and is never used after a
    public-terminal error. The successor returns `legacyGenerationBusy` for every known-native resume until the old
    generation exits, but still accepts fresh work. The same barrier holds when the old terminal has no viewer and no
    saved Studio tab.
16. Loss of the relay causes Runtime to retire views and leases and refuse reconnects without stopping the provider
    process or exposing terminal content.
17. `terminals/stop` ends only the hosted process. A known native ID remains eligible for a later explicit resume; an
    unknown fresh terminal returns no resume promise or invented identity.
18. Terminal exit drains preceding output, retires views and leases, removes the live descriptor, and leaves provider
    conversation state untouched.
19. Terminal bytes and screen snapshots never appear in Runtime storage, logs, errors, audit records, crash
    diagnostics, or enrollment records.
20. Denying an enrollment yields no inventory. Revoking an approved integration retires its existing views and makes
    authenticated reconnect fail.
21. Approval is unavailable through non-TTY input, piped stdin, a `--yes` flag, or an environment variable, and fails
    when the full pending ID challenge differs. No public Runtime method or remote route can approve or widen a grant.
22. Old Rust and TypeScript clients initialize and retain current behavior against the new Runtime.
23. Python sync and async clients expose the same operations, cancellation behavior, close behavior, and typed
    failures.
24. Importing Python and calling `connect()` never download, install, or start Runtime. Runtime absence returns
    `runtimeNotInstalled` and creates no private daemon or Runtime home.
25. Python bindings contain no provider identifier, model identifier, provider flag, transcript path, or install
    command.
26. The Python package links only the two Apache public Rust crates and approved binding dependencies. License gates
    inspect the built wheel, not only source manifests.
27. Every wheel installs into a clean external CPython environment and imports without repository paths. Unsupported
    interpreters and platforms fail without building from an sdist.
28. Every wheel tag, target triple, linked-system floor, and architecture matches its paired Runtime archive in the
    release manifest.
29. A standalone consumer requests enrollment, the owner approves it through interactive CLI administration, and the
    consumer reconnects without Studio installation. Denial, expiry, narrowing, and revocation are separate cases.
30. A real provider journey starts or resumes one terminal, sends exact input, receives exact terminal bytes, exits
    the consumer, reconnects, and later resumes through a provider-native identity when one is known.
31. Removing Runtime-owned state leaves the provider CLI installation, authentication, and conversation usable by
    the provider's own command.
32. Windows, macOS, and Linux run the same public journey; x64 and ARM64 Runtime and Python artifacts are produced and
    inspected.
33. Existing `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, TypeScript, protocol compatibility,
    memory, and idle CPU contracts remain green and within their current ceilings.

The first new gate must prove its own red cases before it is counted. Gate count is not initiative progress.
Completion is determined by the packed external consumer and real-provider journey.

### 4. Rollback

| Checkpoint | Recoverable action |
|---|---|
| Before protocol publication | Remove the unshipped revision and adapter together; no compatibility fact exists |
| After protocol publication, before Studio switch | Stop advertising faulty terminal capability and ship a compatible Runtime fix; keep the published revision readable |
| After Studio switch, before private-path removal | Disable the public capability and use the last release-tagged, gate-proven private Studio path only long enough to ship the fix |
| After private-path removal | Roll forward the public adapter; do not restore a permanent dual path |
| Before Python publication | Remove unshipped wheels and release wiring |
| After Python publication | Keep immutable wheels available and publish a corrected SemVer release |

After publication, a finalized protocol revision and published package version are permanent compatibility facts.
Every rollback path preserves these invariants:

- keep initialization and every already published method shape readable
- keep each Python package version available and publish corrected bytes under a new version
- never delete provider conversations, credentials, or native CLI installations

The initiative does not authorize force push, release deletion, package replacement, or provider-state cleanup.

### 5. Evaluation

The initiative succeeds when all five statements are true:

1. **Identity:** a contributor can describe Runtrol without mentioning VS Code first: one shared local runtime for
   supported agent CLIs installed on the machine, with Studio as its flagship client.
2. **Independence:** a Python application can install the client, request enrollment from an explicitly installed
   Runtime, receive owner approval through the non-GUI administration CLI, and complete a real CLI conversation
   without installing Studio.
3. **Faithfulness:** the application can choose structured provider events when supported or the exact provider TUI
   when it needs provider-faithful presentation. No terminal parser presents terminal output as a generic chat model.
4. **Ownership:** closing or uninstalling the application and removing Runtrol metadata leaves provider credentials
   and native conversations under provider ownership and usable through the provider CLI.
5. **Neutrality:** adding a measured provider that speaks an existing driver kind changes a manifest and shipped
   provider decision, not Runtime core, SDKs, Studio, or the consuming application.

The initiative fails if any of these become necessary:

- one private Runtime per application
- a provider-specific branch in a public client
- a copied transcript or generic message store
- parsing terminal bytes to find a final answer
- requiring a model API key in Runtime
- silent provider installation or sign-in
- Studio as a mandatory dependency for headless enrollment
- a private terminal path for new work or a fallback after a public terminal failure
- claiming completion without a packed external Python consumer and a real provider journey

## Rejected alternatives

### Rewrite the engine in Python

Rejected. Process containment, pseudo terminals, same-user transport, bounded fan-out, generations, and low idle
cost are already implemented in Rust. Rewriting them creates a second engine and binds session lifetime to the host
application.

### Embed the daemon through PyO3

Rejected. The Python native extension wraps only the public Rust client. Embedding the daemon would create one engine
per interpreter, mix AGPL core code into the consuming process, and break the single shared per-user Runtime model.

### Aggregate provider SDKs

Rejected. This would make Runtrol own provider package versions, provider agent configuration, result types, and
authentication differences. It would be a permanently lagging facade over products that already have official SDKs.

### Expose only normalized chat messages

Rejected. Some providers expose structured events and others expose only a complete TUI. A universal message model
would either overstate capability or parse provider content. The explicit structured and terminal surfaces preserve
faithful behavior across both surfaces.

### Keep terminal access private to Studio

Rejected. A Runtime that gives its provider-faithful terminal surface only to one private client is not an application
runtime. Studio must consume the same public terminal contract that other approved applications receive.

### Let SDKs install and approve automatically

Rejected. Import-time or connect-time installation changes the machine without an explicit user action, and
self-approval destroys the integration grant boundary. Installation and authority remain separate, visible local
actions.

## Completion and promotion

This folder is an initiative, not permanent documentation. Nothing in product code, public docs, package metadata,
or tests may cite it.

When the implementation and external journey are complete:

1. recut durable behavior from code into the relevant `docs/` owners
2. update all public README variants from those owners
3. verify no retained artifact points into this folder
4. delete this folder in the same completion change

## Research sources

Repository evidence:

- [`docs/runtimeProtocol.md`](../../docs/runtimeProtocol.md)
- [`docs/runtimeIntegration.md`](../../docs/runtimeIntegration.md)
- [`docs/runtimeSecurity.md`](../../docs/runtimeSecurity.md)
- [`docs/terminalSurface.md`](../../docs/terminalSurface.md)
- [`docs/productSurfaces.md`](../../docs/productSurfaces.md)
- [`crates/runtrol-runtime-protocol/src/method.rs`](../../crates/runtrol-runtime-protocol/src/method.rs)
- [`crates/runtrol-runtime-client/src/lib.rs`](../../crates/runtrol-runtime-client/src/lib.rs)
- [`crates/runtrol-daemon/src/terminal_surface.rs`](../../crates/runtrol-daemon/src/terminal_surface.rs)
- [`crates/runtrol-drivers/src/shipped.rs`](../../crates/runtrol-drivers/src/shipped.rs)
- [`clients/typescript/`](../../clients/typescript/)

Official external evidence, checked 2026-08-26:

- [Claude Code Remote Control](https://code.claude.com/docs/en/remote-control)
- [Claude Agent SDK to Managed Agents comparison](https://platform.claude.com/docs/en/managed-agents/migration)
- [Codex Python SDK](https://github.com/openai/codex/tree/main/sdk/python)
- [Codex App Server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [Agent Client Protocol overview](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/v1/overview.mdx)
- [PyO3 building and distribution](https://pyo3.rs/main/building-and-distribution.html)
- [maturin distribution](https://www.maturin.rs/distribution.html)
- [PyPI attestations](https://docs.pypi.org/attestations/producing-attestations/)
