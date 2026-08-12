# docs

**운영문서 정본이다.** 무엇이 되는가, 무엇을 깨면 안 되는가, 정확한 표.

`mainPlan/` 은 문서가 아니라 **이니셔티브**다. 앞으로 지을 것이 거기 있고, 지어지면 그 지식을 **코드 실물에서 다시 캐서** 여기로 승격한 뒤 이니셔티브 폴더를 지운다. 설계서를 복사하지 않는다 (이니셔티브는 이미 낡았을 수 있다).

아무것도 `mainPlan/` 을 인용하지 않는다.

| 문서 | 내용 |
|---|---|
| [positioning.md](positioning.md) | 왜 runtrol 이 존재하는가. 경쟁 지형, 고른 자리와 그 이유, 접어야 할 조건 (kill criteria) |
| [providerArchitecture.md](providerArchitecture.md) | manifest, driver kind, provider-neutral lifecycle, approval wire, session ownership, drift and uninstall verification boundaries |
| [providerDiscovery.md](providerDiscovery.md) | lazy executable probes, binary-identity cache, required and optional flags, honest model catalogues, and drift boundaries |
| [coreRuntime.md](coreRuntime.md) | thin daemon boundary, runtime admission, memory and CPU budgets, bounded replay, cursor gaps, process containment, and metadata-only storage |
| [productSurfaces.md](productSurfaces.md) | public surface ownership, the VS Code-only PC decision, 30-session interaction contract, GitHub Pages distribution, and phone PWA boundary |
| [vscodeSurface.md](vscodeSurface.md) | the public VS Code runtime, module boundaries, 30-session and performance contracts, six-target distribution, and verification entry points |
| [siteDeployment.md](siteDeployment.md) | live GitHub Pages origin, dependency-free build, failure mutations, release-link truth, workflow permissions, and visual direction |
| [desktopGui.md](desktopGui.md) | Tauri desktop ownership, session lifecycle, bounded rendering, Korean IME, console policy, memory budgets, and evidence boundaries |
| [crossConsult.md](crossConsult.md) | the consult toggle: official-command wiring, control-name judgements, tools/list verification, the measured direction asymmetry, and the at-machine-only capability |
| [frontendStack.md](frontendStack.md) | surface-specific frontend choices and the shared brand, theme, performance, storage, and accessibility contracts |
| [northStarEvidence.md](northStarEvidence.md) | 게이트가 무엇을 단언하는가의 정본. 어느 축에 붙고 몇 점인지는 [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) 이 정본이고 `northStarBoard` 게이트가 계산한다 |

코드가 서면 여기가 는다.
