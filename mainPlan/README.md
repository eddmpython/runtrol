# mainPlan 포트폴리오 인덱스

> **mainPlan 은 이니셔티브지 문서가 아니다.** 문서는 `docs/` 에 쓴다.
> 한 이니셔티브 = 폴더 하나. 그 안에 `README.md` (+ 설계가 크면 번호 파일들).
> **완료 = `_done` 보관이 아니라 `docs/` 로 승격 + 이니셔티브 폴더 삭제.** git 이력이 보관한다.
> 상태 범례: 활성 = 진행 중 · 설계 = 완료된 설계, 착수 대기 · 대기 = 선행 이니셔티브 미완

북극성은 정해졌다: **한 개의 VS Code 창에서 모든 프로젝트, 세션, 에이전트를 즉시 운영한다.**
그 결정의 근거와 접어야 할 조건은 [docs/positioning.md](../docs/positioning.md) 가 정본이다.

전문 에이전트 5 인 토론 (2026-07-30, r1) 의 결론을 카테고리화했다. 원본 토론은 `.claude/discussion/r1/` 에 있다 (L-local).

## 남은 이니셔티브 . 폴더 이름이 곧 순서다

**폴더 앞의 숫자가 짓는 순서다.** 카테고리로 묶어 두면 순서가 어디에도 안 적히고, 안 적힌 순서는 매번 다시 논쟁된다.
그래서 순서를 이름에 박았고, 각 줄에 **왜 그 자리인지 (앞의 무엇이 없으면 못 하는지)** 를 함께 적는다.

| 폴더 | 상태 | 한 줄 | 왜 이 자리인가 |
|---|---|---|---|
| [0-securityPosture](0-securityPosture/) | 활성 | default-deny 권한 모델, 소켓 표면 (Windows named pipe / Unix socket), 승인 전달, 감사 로그, 킬 스위치. **공개 저장소라 "모르는 사람이 기본값으로 켠다" 가 기준선이다.** | **순서상의 한 칸이 아니라 나머지 전부가 딛는 바닥이다.** 첫 커밋부터 함께 갔고, 남은 항목 (폰 표면 권한) 이 4 번에 걸려 있어 **마지막 뒤에 닫힌다.** 0 은 "먼저 하고 넘어간다" 가 아니라 "1~4 가 이것 위에서 돈다" 는 뜻이다 |
| [1-vscodeSurface](1-vscodeSurface/) | 활성 | VS Code 주력 표면, Core 자동 탐색, 단일 hot renderer, 세션별 workspace 및 worktree 결박, Marketplace VSIX | 새 북극성 자체다. 이 표면이 실물로 서지 않으면 뒤의 배포와 폰은 다른 제품을 배포하는 일이 된다. 첫 end-to-end slice, change-only session-index 구독, 실물 Extension Host 및 3,000fps Webview ratchet, 공유 이벤트 표현 SSOT, workspace 충돌 선택, 플랫폼 패키징은 구현됐고 Core worktree 예약, hosted 패키지 확증, 공개 배포가 남았다 |
| [2-autoUpdate](2-autoUpdate/) | 활성 | **정본은 GitHub Releases**, 서명 검증, 관리자 권한 불필요. 앱 갱신은 설치기를 다시 돌리지 않고 **살아 있는 이미지를 원자적으로 교체한다** (실측 근거). provider 는 **확증된 채널에서만** 갱신하고 롤백 대상은 버전 순서로 고른다. | VS Code 확장과 bundled Core의 버전 SSOT, 무중단 교체, 롤백을 세운다. Marketplace 패키징 완료 판정이 이 계약을 사용한다 |
| [3-landingSite](3-landingSite/) | 설계 | GitHub Pages 한 장. 로고·설명·다운로드 둘·우측 상단 SNS. **프론트 "astryx 방식" 이 운영자 확인 대기.** | 다운로드 링크를 릴리즈에서 파생하므로 2 번이 선행이고, 불변 origin은 뒤의 폰 전송 계층이 사용한다 |
| [4-pwaConnection](4-pwaConnection/) | 활성 | **origin 과 transport 를 분리한다.** 불변 HTTPS origin 하나 + 4 단 전송 사다리 + Noise E2E + 데몬 직접 Web Push. | 3 번이 확정한 불변 origin 위에서만 성립한다. 보안 기반은 0 번에서 이미 섰고, 남은 것은 릴레이, 원격 리스너, push, 클라이언트다 |
| [5-pwaSurface](5-pwaSurface/) | 대기 | PWA 자체. 연결 계층이 선 뒤에 짓는다. | 4 번의 프레임 스트림이 확정되기 전에 화면을 얹으면 전송이 바뀔 때 화면을 다시 짓는다 |

**1 과 2 를 3 앞에 두는 것이 이 순서의 유일한 비자명한 판단이다.** 폰이 가장 눈에 띄는 미완이지만,
설치할 수 없는 제품에는 폰으로 이을 PC 세션이 없고, 소유하지 않은 주소 위에는 불변 origin 이 없다.

## 마일스톤 (같은 순서를 사용자 관점으로 본 것)

| 마일스톤 | 내용 | 완료의 정의 |
|---|---|---|
| **M0 코어 슬라이스** (완료) | 공개 provider 경계 + Rust daemon + 세션 목록·시작·재개. 운영 정본은 [core runtime](../docs/coreRuntime.md) 이다 | 두 provider 의 세션이 한 명령에 뜨고 이어진다 |
| **M1 데스크톱 (dogfood)** (제품 구현 완료, dogfood 판정 대기) | Tauri v2 + 한 목록 + 대화 + 즉시 반응. 운영 정본은 [desktop GUI](../docs/desktopGui.md) 다 | **운영자가 GPT 앱 대신 이것을 매일 쓴다** |
| **M2 VS Code 배포** | 1-vscodeSurface -> 2-autoUpdate -> 3-landingSite | 낯선 사람이 Marketplace에서 설치해 한 창으로 실물 CLI를 운영한다 |
| **M3 폰** | 4-pwaConnection (릴레이 + E2E + push) -> 5-pwaSurface -> 폰 승인 | 밖에서 폰으로 내 세션을 잇고 승인한다 |

왜 이 순서인가.

1. **바닥 가치는 이미 섰다.** M1 은 여러 AI 세션을 한 목록에서 다루는 core와 desktop 바닥을 증명했다. 현재 최우선은 VS Code 주력 표면의 Webview 폭주 성능 ratchet, workspace overlap 처리, Marketplace 배포를 실물 게이트로 닫는 일이다.
2. **dogfood 가 설계를 현실에 부딪히게 한다.** kill criteria 5 번 (운영자가 안 쓰면 접는다) 을 가장 빨리 판정할 수 있는 순서다.
3. **폰은 구조적으로 뒤다.** 페어링 QR 표시와 물리 행동 승인이 PC 표면을 전제한다. PC 표면 없이 폰부터 지을 수 없다.
4. **가장 어려운 인프라 (연결·암호·push) 를 안정된 코어 위에 얹는다.** 반대로 하면 인프라를 두 번 짓는다.

북극성 증거 체계는 [공개 등록부](../docs/northStarEvidence.md) 와 [점수판 엔진](../tests/audit/northStar/) 에서 각 마일스톤과 동행한다. `어디서나 같은 방법` (macOS·Linux) 은 M2 이후다.

## 완료 판정

문서 작성이나 데모 영상으로 완료하지 않는다. 이니셔티브가 끝났다는 것은
(1) 그 능력이 실물 CLI 와 실물 기기로 동작하고
(2) 그것을 단언하는 게이트가 CI 에서 돌고
(3) 지식이 `docs/` 로 승격됐고
(4) 이니셔티브 폴더가 삭제됐다
는 뜻이다.

**완료 사례**: `positioningDecision` 은 2026-07-30 에 [docs/positioning.md](../docs/positioning.md) 로, `desktopGui` 는 2026-08-02 에 [docs/desktopGui.md](../docs/desktopGui.md) 로, `crossConsult` 는 2026-08-03 에 [docs/crossConsult.md](../docs/crossConsult.md) 로 승격되고 삭제됐다.

## 이 판을 지배하는 사실 (r1 조사)

- **벤더가 이미 냈다.** Anthropic Remote Control, OpenAI Codex Remote, GitHub Copilot CLI, Amp. 전부 무료 번들이거나 구독 포함. **그래서 "원격 조종" 은 우리 자리가 아니다. "여러 개를 한 곳에" 가 우리 자리다.**
- **상용 레이어는 무덤이다.** vibe-kanban (27.5k star, Rust) 폐업, terragon 폐업, crystal deprecated, omnara v1 deprecated.
- **성숙한 Rust 네이티브 세션 매니저 + 모바일 클라이언트는 존재하지 않는다.**
- **omnara v1 의 사인이 곧 최대 위험이다**: "CLI 를 감싸는 래퍼로 지었는데 CLI 가 끊임없이 바뀌어 유지가 불가능해졌다." -> [provider discovery](../docs/providerDiscovery.md) 의 발견 사다리와 drift 게이트가 직접 대응이다.
- **사용자는 릴레이 없는 모드를 원한다** (Happy 이슈 `local network only mode`).
- **claude-squad 는 Windows 를 못 한다** (creack/pty 미지원). tmux 기반 OSS 전반의 약점이다.
