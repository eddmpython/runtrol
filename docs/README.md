# docs

**운영문서 정본이다.** 무엇이 되는가, 무엇을 깨면 안 되는가, 정확한 표.

**여기 있는 것은 설계서가 아니라 코드 실물에서 다시 캔 지식이다.** 능력이 서면 그 능력을 읽어서 여기 적는다. 계획 단계의 스케치를 승격시키지 않는다 (스케치는 지어지는 동안 이미 낡는다).

| 문서 | 내용 |
|---|---|
| [positioning.md](positioning.md) | 왜 runtrol 이 존재하는가. 경쟁 지형, 고른 자리와 그 이유, 접어야 할 조건 (kill criteria) |
| [providerArchitecture.md](providerArchitecture.md) | manifest, driver kind, provider-neutral lifecycle, approval wire, session ownership, drift and uninstall verification boundaries |
| [providerDiscovery.md](providerDiscovery.md) | lazy executable probes, binary-identity cache, required and optional flags, honest model catalogues, and drift boundaries |
| [coreRuntime.md](coreRuntime.md) | thin daemon boundary, runtime admission, memory and CPU budgets, bounded replay, cursor gaps, process containment, and metadata-only storage |
| [runtimeProtocol.md](runtimeProtocol.md) | public Runtime transport, identity, methods, scopes, mutations, streams, errors, and compatibility |
| [runtimeIntegration.md](runtimeIntegration.md) | Rust and TypeScript SDK adoption, enrollment, least privilege, reconnect, failure recovery, and credential lifecycle |
| [runtimeSecurity.md](runtimeSecurity.md) | public endpoint trust layers, data ownership, authorization, hostile provider input, hosted companions, and incident response |
| [runtimeOperations.md](runtimeOperations.md) | standalone Runtime artifacts, install, locator repair, administration, update, rollback, and uninstall |
| [agentTools.md](agentTools.md) | one-click project enablement, the fixed seven-tool MCP catalogue, root-bound Runtime authority, provider registration, revocation, recovery, and real CLI verification |
| [missionOperations.md](missionOperations.md) | reviewed Mission schema, local authority, Task scheduling, worktrees, evidence Receipts, integration, recovery, bounds, and verification |
| [capabilityTrust.md](capabilityTrust.md) | project capability schemas, provenance, independent verification, exact approval, explicit reuse, tamper detection, and rollback |
| [productSurfaces.md](productSurfaces.md) | public surface ownership, the VS Code-only PC decision, 30-session interaction contract, GitHub Pages distribution, and phone PWA boundary |
| [phonePwa.md](phonePwa.md) | phone pairing, relay transport, durable device authority, bodyless Web Push, browser storage, reconnect, and current release limits |
| [vscodeSurface.md](vscodeSurface.md) | the public VS Code runtime, module boundaries, 30-session and performance contracts, six-target distribution, and verification entry points |
| [siteDeployment.md](siteDeployment.md) | GitHub Pages operating manual: what deploys and when, the everyday change loop, local preview, build budget, contract test, page anatomy, brand regeneration, post-deploy checks, rollback, and troubleshooting |
| [crossConsult.md](crossConsult.md) | the consult toggle: official-command wiring, control-name judgements, tools/list verification, the measured direction asymmetry, and the at-machine-only capability |
| [frontendStack.md](frontendStack.md) | surface-specific frontend choices and the shared brand, theme, performance, storage, and accessibility contracts |
| [northStarEvidence.md](northStarEvidence.md) | 게이트가 무엇을 단언하는가의 정본. 어느 축에 붙고 몇 점인지는 [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) 이 정본이고 `northStarBoard` 게이트가 계산한다 |

코드가 서면 여기가 는다.
