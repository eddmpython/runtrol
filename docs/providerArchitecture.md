# Provider architecture

## Boundary

runtrol supervises provider CLI processes and transports their live structured events. It does not own conversations, credentials, model catalogues, or provider session storage. The provider CLI remains the sole owner of its durable transcript and native resume surface. runtrol never discovers, derives, or reads a provider transcript path.

The public boundary has three parts:

1. A TOML manifest declares how to reach a CLI.
2. A driver kind implements a transport protocol.
3. The `Provider` and `Agent` traits expose one provider-neutral lifecycle to the daemon.

Adding a CLI that speaks an existing kind requires a manifest, not a core change.

## Manifest contract

The manifest schema is closed. Unknown keys, unsafe binary paths, invalid identifiers, model identifiers presented as aliases, and paths outside the operator home are rejected when the registry loads the file.

A manifest may declare only facts needed to reach the process:

- binary names to resolve through the operator's search path
- safe version and flag probes
- structured transport arguments
- stable model aliases only when the CLI cannot enumerate a catalogue
- provider-owned credential directories used by the workspace wall
- an update channel hint
- the CLI's own sign-in, self-diagnosis, and install commands

Capabilities, model catalogues, account state, and supported flags are discovered from the installed CLI at runtime. Probe policy, caching, model honesty, and loader-time manifest linting are defined in [provider discovery](providerDiscovery.md), not in the adapter boundary.

The help commands are the one entry whose declaration needs defending, since the hard rule is that a discoverable fact may not be declared. The install line is wanted exactly when no executable exists to ask. The other two exist inside an installed CLI, but the only way to learn their names is to read help text, and approximating a capability from help text is what the driver contract refuses: the shape differs per vendor and changes per release, and a wrong guess prints a command that silently does nothing. They remain a declaration of how to reach the CLI rather than a claim about what it can do, and nothing consults them to decide whether an operation is possible.

These strings are the only manifest text that reaches an operator's shell, and a manifest may come from the operator's own provider directory rather than from the product. So the loader refuses any character a shell could read as a separator, by whitelist rather than by blacklist. Runtime assembles the finished command line, because only Runtime knows which candidate executable resolved; a client that joined arguments to a name would be a second place deciding what runs, wrong on exactly the machine where the second candidate was the installed one. No surface may execute one: they are offered to a person, who reads the line and decides.

## Official service catalogue

The built-in service catalogue combines the measured handwritten manifests with a generated snapshot of the official
ACP Registry. `crates/runtrol-drivers/tooling/sync-acp-registry.mjs` is the maintainer-only synchronizer. It accepts a
bounded registry document, rejects redirects and malformed records, resolves exact npm package metadata only to learn
the installed executable names, and emits `generated_acp_registry.rs`. The snapshot records its source digest, total
agent count, safe adapter count, and skipped count. CI rejects a missing bound, a generated downloader, update
authority, or any registry URL in runtime source.

Runtime never contacts the ACP Registry or a package registry. It never invokes `npx`, `uvx`, an installer, or an
update command. Generated manifests name only local executable candidates. When one is already on the operator's
`PATH`, the ordinary provider-neutral resolver discovers it and the generic ACP driver serves it. When it is absent,
Studio can show the exact declared install line and place that line in a terminal only after the operator selects the
service. The command remains unexecuted.

This separates three concepts that must not be conflated:

- an ACP coding agent is a provider Runtrol can supervise directly;
- a model API such as DeepSeek or GLM is selected and authenticated inside a compatible installed coding agent;
- a local model runtime such as Ollama is likewise configured in that coding agent, not given to Runtrol as a model
  key or transcript source.

The current snapshot safely expresses 30 official registry agents as local adapters and skips six whose environment
or distribution semantics cannot be represented honestly. Three official entries use richer measured handwritten
manifests: two with the same identifiers, plus the official Grok launch whose executable and ACP arguments are already
represented by the measured Grok manifest. A generated test copies a real executable under the GLM adapter's declared name,
launches an isolated child with that directory on `PATH`, and proves the shipping resolver discovers it. Another test
requires every built-in sidebar display name to be distinct, including direct and ACP transports.

## Driver registry

Driver kinds live in one explicit table. Each entry either constructs a provider-neutral driver or gives a visible reason that the build cannot serve that kind. There is no distributed registration and no provider-name branch in the core.

The generic ACP v1 driver accepts an external manifest and supervises a separate stdio child process. The same path handles session creation, loading, prompts, streamed updates, and provider-declared turn completion. Cancellation is implemented by the driver but is not part of the hosted ACP journey yet.

## Session ownership

runtrol assigns its own session identifier for supervision and retains only provider identity, native session identity, workspace identity, labels, pins, and bounded event delivery state. It does not copy transcript content into its store. Session listings join this metadata with current supervised process state and never scan provider storage.

The public Runtime catalogue follows the same boundary. A provider exposes native sessions only through an official
registered command or protocol with honest `complete`, `partial`, `unsupported`, or `unavailable` coverage. Missing
catalogue support is a typed capability result. It is never replaced with a search through provider databases, JSONL
files, logs, caches, or guessed session directories.

Closing or removing a runtrol session removes the supervisor's pointer. It does not delete the provider-owned session. Removing `RUNTROL_HOME` therefore removes runtrol metadata, not the provider session. The deterministic ACP fixture proves direct native resume while runtrol is absent and proves that an optional reinstall can load the same native session again.

## Event and approval rules

Drivers map only the protocol surface runtrol consumes. Unknown notifications are carried as `Unmapped` frames with their original payload so a provider extension does not become data loss.

Approval responses are bound to the pending provider request, subject digest, offered choices, structural risk, and expiry. An incomplete subject cannot be approved. Provider-specific approval framing stays inside its driver.

The terminal command surface answers the provider-neutral boundary as `runtrol answer <session> <approval> <option> <subject-digest-hex>`. It never accepts a provider decision word. The stream-json built-in starts Claude Code with `--permission-prompt-tool stdio`; its `control_request/can_use_tool` and native `control_response` shapes remain private to that driver. A watch receives an explicit subscription acknowledgement before events, so a caller never sleeps and guesses whether an approval can still be missed.

## Remote wire boundary

The PWA wire uses a runtrol transport envelope, not raw ACP. A `WatchCursor` is the next expected boundary inside one bounded live stream and consists of stream incarnation, attachment epoch, and dense sequence. A reconnect receives the retained window exactly once or an explicit gap when that boundary is unavailable. The provider event's `src_end` is separate diagnostic ordering metadata for the current live source, not a transcript offset or reconnect token. The envelope also carries the session identifier, idempotent RPC identifier, and authorization scope. Event payload bytes pass through without interpretation or rewriting.

ACP remains one driver protocol behind that boundary. Exposing it as the remote wire would make non-ACP drivers imitate ACP, couple phone authentication and replay to one provider protocol, and move transport responsibilities into the adapter layer.

## Verification

The active gates establish separate claims:

- `providerContract` checks that an out-of-tree implementation can satisfy the public `Provider` and `Agent` traits, issue a native command, and preserve an unmapped event.
- `providerIsolation` checks that core session, transport, and API code contains no provider-specific branch.
- `approvalAuthorization` checks request, subject, option, risk, authority, and expiry binding after a driver creates a pending approval.
- `genericAcpSmoke` drives a deterministic external manifest fixture through the product daemon, CLI surface, child process, streamed turn, completion, and load path.
- `externalAcpSmoke` installs an independently distributed ACP implementation and drives two deterministic streamed turns around daemon restart and native load. The model endpoint is a local mock, while the ACP implementation is the real external executable.
- `claudeApprovalSmoke` runs an installed Claude Code process through the production stream-json driver against a loopback Messages endpoint. The real CLI emits its hidden `can_use_tool` request, consumes an explicit `rejectOnce` answer through the normal daemon boundary, makes the follow-up model request, declares `end_turn`, and leaves the denied target file absent. The model endpoint is deterministic and mock; the provider process and approval wire are real.
- `uninstallLeavesNoTrace` stores fixture state outside `RUNTROL_HOME`, removes the runtrol home, resumes directly through the provider executable while runtrol is absent, then loads the same native session after reinstallation.
- `agentSurfaceDrift` compares the schema-provider methods and stream-json provider flags that can be probed without an account. Scheduled hosted CI installs current CLIs and requires each built-in probe strategy to run.
- `acpRegistry` verifies the generated official snapshot, coverage arithmetic, local-only executable candidates,
  bounded maintainer fetch, absent runtime registry access, and absent update authority. Driver tests prove one
  generated adapter resolves from an isolated local `PATH` and all visible service names are distinct.
- The generic ACP, external ACP, Claude approval, and uninstall journeys run on Windows, macOS, and Linux.

Real account turns remain operator evidence because hosted CI receives no provider credential. The Claude approval gate proves the installed CLI wire against a mock model, not account-backed model behavior, and the North Star tier remains `mock`.
