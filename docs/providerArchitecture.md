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

Capabilities, model catalogues, account state, and supported flags are discovered from the installed CLI at runtime. Probe policy, caching, model honesty, and loader-time manifest linting are defined in [provider discovery](providerDiscovery.md), not in the adapter boundary.

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
- The generic ACP, external ACP, Claude approval, and uninstall journeys run on Windows, macOS, and Linux.

Real account turns remain operator evidence because hosted CI receives no provider credential. The Claude approval gate proves the installed CLI wire against a mock model, not account-backed model behavior, and the North Star tier remains `mock`.
