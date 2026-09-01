# docs

This directory is the canonical index for public architecture and operations documentation. Each topic has one
owner. Executable values remain in code or a machine-readable catalogue, and these documents explain their meaning
and operating procedure.

These are current product contracts recovered from implemented code, not design proposals. Work that has not reached
the product remains outside this index.

| Document | Owns |
|---|---|
| [positioning.md](positioning.md) | why Runtrol exists, the competitive field, the chosen position, and explicit kill criteria |
| [providerArchitecture.md](providerArchitecture.md) | manifest, driver kind, provider-neutral lifecycle, approval wire, session ownership, drift and uninstall verification boundaries |
| [providerDiscovery.md](providerDiscovery.md) | lazy executable probes, binary-identity cache, required and optional flags, honest model catalogues, and drift boundaries |
| [coreRuntime.md](coreRuntime.md) | thin daemon boundary, runtime admission, memory and CPU budgets, bounded replay, cursor gaps, process containment, and metadata-only storage |
| [terminalSurface.md](terminalSurface.md) | one provider-owned TUI, live capture ladder, bounded multi-view transport, exact generation continuity, input authority, and latency evidence |
| [runtimeProtocol.md](runtimeProtocol.md) | public Runtime transport, identity, methods, scopes, mutations, streams, errors, and compatibility |
| [runtimeIntegration.md](runtimeIntegration.md) | Rust, TypeScript, and Python SDK adoption, enrollment, exact-generation terminal continuity, least privilege, recovery, and credentials |
| [runtimeSecurity.md](runtimeSecurity.md) | public endpoint trust layers, data ownership, authorization, hostile provider input, hosted companions, and incident response |
| [runtimeOperations.md](runtimeOperations.md) | standalone Runtime artifacts, install, locator repair, independent CLI administration, update, rollback, and uninstall |
| [automaticUpdates.md](automaticUpdates.md) | Studio Marketplace release operation, resilient artifact staging, client update ownership, provider update confirmation, and rollback |
| [productSurfaces.md](productSurfaces.md) | Runtime-first product identity, public client family, Studio GUI decision, unified sidebar, distribution, and phone boundary |
| [phonePwa.md](phonePwa.md) | phone pairing, relay transport, durable device authority, bodyless Web Push, browser storage, reconnect, and current release limits |
| [vscodeSurface.md](vscodeSurface.md) | the public VS Code runtime, module boundaries, release-load and performance contracts, target-catalogue distribution, and verification entry points |
| [siteDeployment.md](siteDeployment.md) | GitHub Pages operating manual: what deploys and when, the everyday change loop, local preview, build budget, contract test, page anatomy, brand regeneration, post-deploy checks, rollback, and troubleshooting |
| [frontendStack.md](frontendStack.md) | surface-specific frontend choices and the shared brand, theme, performance, storage, and accessibility contracts |
| [northStarEvidence.md](northStarEvidence.md) | what each evidence gate proves; [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) owns axis membership and scoring, which `northStarBoard` computes |
