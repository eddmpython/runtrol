# docs

**운영문서 정본이다.** 무엇이 되는가, 무엇을 깨면 안 되는가, 정확한 표.

`mainPlan/` 은 문서가 아니라 **이니셔티브**다. 앞으로 지을 것이 거기 있고, 지어지면 그 지식을 **코드 실물에서 다시 캐서** 여기로 승격한 뒤 이니셔티브 폴더를 지운다. 설계서를 복사하지 않는다 (이니셔티브는 이미 낡았을 수 있다).

아무것도 `mainPlan/` 을 인용하지 않는다.

| 문서 | 내용 |
|---|---|
| [positioning.md](positioning.md) | 왜 runtrol 이 존재하는가. 경쟁 지형, 고른 자리와 그 이유, 접어야 할 조건 (kill criteria) |
| [providerArchitecture.md](providerArchitecture.md) | manifest, driver kind, provider-neutral lifecycle, session ownership, drift and uninstall verification boundaries |
| [frontendStack.md](frontendStack.md) | Astryx + StyleX. 랜딩·PWA·데스크톱 세 표면이 공유하는 컴포넌트 층과 테마 계약 |
| [northStarEvidence.md](northStarEvidence.md) | 게이트가 무엇을 단언하는가의 정본. 어느 축에 붙고 몇 점인지는 [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) 이 정본이고 `northStarBoard` 게이트가 계산한다 |

코드가 서면 여기가 는다.
