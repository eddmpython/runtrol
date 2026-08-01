# Provider discovery

## Boundary

runtrol discovers facts from the installed CLI at the moment a product action needs them. Daemon startup reads manifests and storage only. It does not start every provider process before showing the first session list.

Discovery owns executable resolution, binary identity, reported version, consumed flags, runtime capabilities, model choices, and surface drift. It never reads or stores a transcript. Installation, replacement, update smoke tests, session-boundary deferral, and rollback belong to the `launcherUpdate` initiative.

## Discovery ladder

The product uses the narrowest provider-owned surface available:

1. Resolve manifest candidate names through the operator's search path and unwrap package-manager launchers.
2. Stat the resolved program and ask its own version command.
3. Ask its argument parser about only the flags the selected driver consumes. Two invented controls must produce the same refusal before any answer is trusted.
4. Prefer structured live protocol responses for capabilities and model catalogues.
5. Read provider-owned configuration only when no enumerable protocol surface exists, and only through read-only file handles.
6. Use manifest aliases only for stable tokens that cannot be enumerated. Model identifiers and capability tables are rejected by the closed manifest schema.

The exact `Program` that was probed is handed to the driver. A second path resolution cannot select a different executable after its version and flags were inspected. A missing required flag refuses driver construction with the lost capability named. A missing optional flag is omitted when the feature was not requested; an explicit model or permission choice is refused by name instead of being silently dropped.

## Probe cache

The cache is keyed by provider identifier and the complete resolved program identity: executable path, size, modification time, launcher-resolved leading arguments, and the same file facts for any leading argument that names an absolute regular file. This makes an interpreted package entry point part of the identity instead of trusting the unchanged interpreter. The reported version and the exact driver-owned flag question are stored observations. A hit is valid only when both the program identity and question surface still match. There is no time-based TTL.

The cache is opened lazily from `RUNTROL_HOME/probe.json`, written through a flushed sibling file, and atomically replaced. Only driver discovery is serialized around that single file. Probes, model calls, and provider process opens run in each connection task before the request reaches the daemon's sole session owner. They therefore cannot stop event pumping or unrelated session operations. An unreadable or future-schema cache is treated as absent because every value can be asked again. A write failure is returned to the caller because repeated cold probes are user-visible latency.

A session open reserves one of the bounded process slots before any provider process starts. An idle victim is removed and stopped first, then the replacement opens, and only a matching request kind, provider identifier, session identifier, and reservation may attach it. Opening, closing, rejected, and cleanup processes keep their slot until the child has actually stopped. Cancellation, a vanished reply receiver, and a panicking cleanup release through owned guards. Connection and cleanup tasks belong to the serve loop's `JoinSet`: returned listener and store outcomes abort and reap them, while dropping the serve future aborts them and leaves final child teardown to process containment.

Probe processes are time-bounded and their stdout and stderr are drained concurrently. Each stream retains at most 256 KiB while excess bytes are discarded during the read, so a provider cannot turn the advertised output bound into an allocation after the fact or block on a full pipe.

## Model catalogues

Codex enumerates its current choices through `model/list` on every request. Those results are `ModelCatalog::Known` and carry the provider's identifiers, labels, descriptions, defaults, and reasoning choices.

Claude Code does not expose a complete account catalogue. runtrol returns `ModelCatalog::Partial`: stable manifest aliases first, followed by exact options from the provider-owned `~/.claude.json` `additionalModelOptionsCache`. The file is reopened read-only for every discovery request, so no watcher or copied cache is needed. Missing or damaged provider state keeps the stable aliases and exposes why the answer is partial.

Neither result claims that a credential-free hosted runner proves which models a particular account may use. Hosted CI proves runtime enumeration and honest partial fallback without credentials. Its isolated operator home contains a sentinel option in the provider-owned cache, and the product command must return that exact option. The operator's local preflight observes account-local state.

## Session paths

runtrol does not discover or calculate provider transcript paths. It carries the provider's native session identifier and uses the provider's official protocol or resume surface. This keeps session storage provider-owned and avoids turning a private directory layout into a runtime dependency.

## Drift and manifest validation

Provider manifests are validated when loaded. The schema is closed, unknown capability-shaped keys are refused, binary candidates cannot choose arbitrary paths, and model identifiers cannot hide as aliases. This loader-time validation is the manifest lint boundary rather than a second parser in a separate command.

`agentSurfaceDrift` installs current CLIs and checks only the schema methods and parser flags the built-in drivers consume. It does not claim the complete event surface, hidden control semantics, or account-backed model behavior. Fixed-version copies run on Windows, macOS, and Linux; the scheduled and manually dispatched job repeats the checks against current releases.

## Verification

- Core and daemon tests prove persisted cache reuse, interpreted-entry-point invalidation, question-surface invalidation, existing-cache replacement, bounded capture, exact program handoff, strict process-slot reservation, cancellation cleanup, prepared-request binding, required-flag refusal, optional degradation, and explicit-choice refusal. The production serve wiring keeps provider discovery, model calls, and process opens in connection tasks outside the session owner.
- `modelDetectionSmoke --require-all` drives every installed built-in through the product command, rejects missing provider coverage and missing hosted cache sentinels, and scans all production source for the exact runtime model identifiers it observed.
- `agentSurfaceDrift --require-all` refuses a run unless every built-in probe strategy executed against installed CLIs.
- `configReadOnly` prevents provider configuration writes, including the provider-owned model option source.
