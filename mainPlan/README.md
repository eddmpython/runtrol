# mainPlan 포트폴리오 인덱스

> **mainPlan 은 이니셔티브지 문서가 아니다.** 문서는 `docs/` 에 쓴다.
> 한 이니셔티브 = 폴더 하나. 그 안에 `README.md` (+ 설계가 크면 번호 파일들).
> **완료 = `_done` 보관이 아니라 `docs/` 로 승격 + 이니셔티브 폴더 삭제.** git 이력이 보관한다.
> 상태 범례: 활성 = 진행 중 · 설계 = 완료된 설계, 착수 대기 · 대기 = 선행 이니셔티브 미완

북극성은 정해졌다: **한 개의 VS Code 창에서 모든 프로젝트, 세션, 에이전트를 즉시 운영한다.**
그 결정의 근거와 접어야 할 조건은 [docs/positioning.md](../docs/positioning.md) 가 정본이다.

전문 에이전트 5 인 토론 (2026-07-30, r1) 의 결론을 카테고리화했고, 2026-08-13 에 공개 Runtime 제품화, 감독형 Mission, 검증된 프로젝트 능력 재사용 설계를 추가했다. 원본 토론은 `.claude/discussion/r1/` 에 있다 (L-local).

## 남은 이니셔티브 . 폴더 이름이 곧 순서다

**폴더 앞의 숫자가 짓는 순서다.** 카테고리로 묶어 두면 순서가 어디에도 안 적히고, 안 적힌 순서는 매번 다시 논쟁된다.
그래서 순서를 이름에 박았고, 각 줄에 **왜 그 자리인지 (앞의 무엇이 없으면 못 하는지)** 를 함께 적는다.

| 폴더 | 상태 | 한 줄 | 왜 이 자리인가 |
|---|---|---|---|
| [0-securityPosture](0-securityPosture/) | 활성 | default-deny 권한 모델, 소켓 표면 (Windows named pipe / Unix socket), 승인 전달, 감사 로그, 킬 스위치. **공개 저장소라 "모르는 사람이 기본값으로 켠다" 가 기준선이다.** | **순서상의 한 칸이 아니라 나머지 전부가 딛는 바닥이다.** 첫 커밋부터 함께 갔고, 남은 Runtime 앱 권한이 3 번, Mission 권한이 4 번, 폰 표면 권한이 6 번에 걸려 있어 **마지막 뒤에 닫힌다.** 0 은 "먼저 하고 넘어간다" 가 아니라 "1~6 이 이것 위에서 돈다" 는 뜻이다 |
| [3-embeddableAgentRuntime](3-embeddableAgentRuntime/) | 활성 | **Runtrol Runtime**을 공개 로컬 제품 경계로 만든다. 외부 IDE, 데스크톱 앱, 자동화 도구, 호스팅 서비스의 로컬 companion이 SDK 하나로 설치된 AI CLI, 모델, 관리 세션, 공식 native 세션, 입력, 이벤트, 승인을 provider 중립적으로 사용한다. | [automatic updates](../docs/automaticUpdates.md)가 독립 갱신과 롤백을 고정했으므로 이제 외부 제품에 안정된 Runtime을 약속할 수 있다. Mission과 PWA를 포함한 모든 표면이 같은 공개 계약을 먼저 써야 Core로 가는 사설 통로가 늘어나지 않는다 |
| [4-orchestrationGrowthOS](4-orchestrationGrowthOS/) | 설계 | 명시적으로 승인한 Mission DAG가 provider-native 세션을 정확한 workspace에서 감독하고, 결정적 증거로 완료를 판정한다. 검증된 프로젝트 절차의 재사용은 별도 후속 slice다. | 3 번의 공개 Runtime 계약을 첫 번째 상위 제품이 실제로 사용한다. 풍부한 원격 표면보다 로컬에서 여러 provider를 실제 작업 하나로 운영하는 차별점을 먼저 검증한다 |
| [5-pwaConnection](5-pwaConnection/) | 활성 | **origin 과 transport 를 분리한다.** 불변 HTTPS origin 하나 + 4 단 전송 사다리 + Noise E2E + 데몬 직접 Web Push. | 공개된 [site deployment](../docs/siteDeployment.md)의 불변 origin과 0 번 보안 기반 위에서 성립한다. 3 번 Runtime과 4 번 로컬 Mission 계약을 먼저 닫은 뒤 남은 릴레이, 원격 리스너, push, 클라이언트를 완성한다 |
| [6-pwaSurface](6-pwaSurface/) | 대기 | PWA 자체. 세션 제어가 본체이고, Mission은 조회·중단만 허용하는 보조 표면이다. | 5 번의 프레임 스트림이 확정되기 전에 화면을 얹으면 전송이 바뀔 때 화면을 다시 짓는다. 4 번의 로컬 전용 생성·통합·능력 승격 권한도 그대로 유지해야 한다 |

3 번은 DartLab을 만드는 계획이 아니다. DartLab 같은 외부 제품이 각자 AI 엔진 접착층을 다시 만들지 않고 Runtrol을 그 로컬 엔진으로 채택하게 만드는 계획이다. 소비자는 작은 SDK를 넣고, 하나의 사용자별 Runtime 데몬이 이미 로그인된 CLI를 발견하고 감독한다. 소비자는 세션을 소유하지 않고, Runtrol은 모델 API 키나 transcript를 소유하지 않는다.

VS Code 주력 표면, 공개 사이트, 자동 갱신은 [VS Code 운영 문서](../docs/vscodeSurface.md), [사이트 운영 문서](../docs/siteDeployment.md), [automatic updates](../docs/automaticUpdates.md)로 승격됐다. Marketplace에서 6개 네이티브 대상의 `Runtrol Studio 0.1.0`을 설치할 수 있다. 현재 순서는 Core를 외부 제품도 쓰는 공개 Runtime으로 고정하고, 그 위에서 로컬 감독형 Mission의 제품 가치를 실측한 다음 폰 연결과 표면을 닫는 것이다.

## 마일스톤 (같은 순서를 사용자 관점으로 본 것)

| 마일스톤 | 내용 | 완료의 정의 |
|---|---|---|
| **M0 코어 슬라이스** (완료) | 공개 provider 경계 + Rust daemon + 세션 목록·시작·재개. 운영 정본은 [core runtime](../docs/coreRuntime.md) 이다 | 두 provider 의 세션이 한 명령에 뜨고 이어진다 |
| **M1 PC 표면 통합** (완료) | 공개 PC 표면을 `Runtrol Studio` VS Code 확장 하나로 고정했다. 독립 데스크톱 GUI 코드와 실행 경로는 제거됐다 | **운영자가 별도 창 없이 VS Code에서 모든 세션을 다룬다** |
| **M2 VS Code 배포** (완료) | 공개 Marketplace 설치와 [automatic updates](../docs/automaticUpdates.md) | 낯선 사람이 Marketplace에서 설치해 한 창으로 실물 CLI를 운영하고 다음 버전으로 안전하게 갱신한다 |
| **M3 Embeddable Agent Runtime** | 3-embeddableAgentRuntime. 공개 protocol + 앱 등록·scope·root + Rust/TypeScript SDK + 관리 세션·공식 native 세션 + 독립 배포·롤백 | 저장소 밖 제품이 provider별 코드와 경로 설정 없이 설치된 두 CLI를 찾고, 세션을 시작·재개·감시·제어하며, Runtime과 독립적으로 갱신된다 |
| **M4 감독형 작업 운영** | 4-orchestrationGrowthOS. 명시적 Mission -> 증거 판정 -> bounded DAG -> 선택적 프로젝트 능력 재사용 | 한 목표를 두 provider가 격리된 worktree에서 수행하고, 대화 해석 없이 검증된 결과만 로컬에서 통합한다 |
| **M5 폰** | 5-pwaConnection (릴레이 + E2E + push) -> 6-pwaSurface -> 폰 승인 | 밖에서 폰으로 내 세션을 잇고 승인한다. Mission은 조회·중단 범위만 원격에 연다 |

왜 이 순서인가.

1. **바닥 가치는 이미 섰다.** M1 은 여러 AI 세션을 한 목록에서 다루는 Core를 증명했고 공개 PC 표면은 VS Code 하나로 통합됐다. Marketplace 배포와 안전한 갱신도 닫혔다.
2. **이미 있는 혁신을 제품 경계로 꺼낸다.** 빠른 CLI 발견, 런타임 capability 발견, 관리 세션 목록, provider-native 감독을 Runtrol Studio의 사설 구현으로 남겨 두지 않는다. M3에서 독립 소비자가 쓸 protocol과 SDK로 고정한다.
3. **첫 번째 소비자는 우리 자신이다.** Runtrol Studio가 일반 세션 경로를 공개 TypeScript SDK로 옮겨 외부 제품과 같은 계약을 쓴다. 관리와 물리 행동만 사설 control endpoint에 남긴다.
4. **여러 provider를 한 목록에 두는 것과 한 작업으로 운영하는 것은 다르다.** M4는 중앙 LLM이나 transcript 해석 없이 명시적 작업 그래프, worktree, 증거 Gate만으로 그 차이를 닫을 수 있는지 반증 가능하게 실측한다.
5. **폰은 구조적으로 뒤다.** 페어링 QR 표시와 물리 행동 승인이 PC 표면을 전제하고, 원격에서 Mission 권한을 넓히지 않으려면 로컬 계약이 먼저 고정돼야 한다.
6. **가장 어려운 인프라 (연결·암호·push) 를 안정된 코어 위에 얹는다.** 반대로 하면 인프라와 원격 권한 표면을 두 번 짓는다.

북극성 증거 체계는 [공개 등록부](../docs/northStarEvidence.md) 와 [점수판 엔진](../tests/audit/northStar/) 에서 각 마일스톤과 동행한다. M3와 M4는 코드나 산문만으로 점수나 새 축을 만들지 않는다. 저장소 밖 소비자 여정과 실물 Mission 여정이 각각 활성 게이트가 된 변경에서만 점수판을 바꾼다. `어디서나 같은 방법` (macOS·Linux) 은 M2 이후다.

## 완료 판정

문서 작성이나 데모 영상으로 완료하지 않는다. 이니셔티브가 끝났다는 것은
(1) 그 능력이 실물 CLI 와 실물 기기로 동작하고
(2) 그것을 단언하는 게이트가 CI 에서 돌고
(3) 지식이 `docs/` 로 승격됐고
(4) 이니셔티브 폴더가 삭제됐다
는 뜻이다.

**완료 사례**: `positioningDecision` 은 2026-07-30 에 [docs/positioning.md](../docs/positioning.md) 로, `crossConsult` 는 2026-08-03 에 [docs/crossConsult.md](../docs/crossConsult.md) 로 승격되고 삭제됐다.

## 이 판을 지배하는 사실 (r1 조사)

- **벤더가 이미 냈다.** Anthropic Remote Control, OpenAI Codex Remote, GitHub Copilot CLI, Amp. 전부 무료 번들이거나 구독 포함. **그래서 "원격 조종" 은 우리 자리가 아니다. "여러 개를 한 곳에" 가 우리 자리다.**
- **상용 레이어는 무덤이다.** vibe-kanban (27.5k star, Rust) 폐업, terragon 폐업, crystal deprecated, omnara v1 deprecated.
- **성숙한 Rust 네이티브 세션 매니저 + 모바일 클라이언트는 존재하지 않는다.**
- **omnara v1 의 사인이 곧 최대 위험이다**: "CLI 를 감싸는 래퍼로 지었는데 CLI 가 끊임없이 바뀌어 유지가 불가능해졌다." -> [provider discovery](../docs/providerDiscovery.md) 의 발견 사다리와 drift 게이트가 직접 대응이다.
- **사용자는 릴레이 없는 모드를 원한다** (Happy 이슈 `local network only mode`).
- **claude-squad 는 Windows 를 못 한다** (creack/pty 미지원). tmux 기반 OSS 전반의 약점이다.
