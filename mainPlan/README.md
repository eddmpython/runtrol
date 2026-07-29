# mainPlan 포트폴리오 인덱스

> **mainPlan 은 이니셔티브지 문서가 아니다.** 문서는 `docs/` 에 쓴다.
> 한 이니셔티브 = 폴더 하나. 그 안에 `README.md` (+ 설계가 크면 번호 파일들).
> **완료 = `_done` 보관이 아니라 `docs/` 로 승격 + 이니셔티브 폴더 삭제.** git 이력이 보관한다.
> 상태 범례: 활성 = 진행 중 · 설계 = 완료된 설계, 착수 대기 · 대기 = 선행 이니셔티브 미완

북극성은 정해졌다: **여러 AI를 한 곳에서 관리한다.**
그 결정의 근거와 접어야 할 조건은 [docs/positioning.md](../docs/positioning.md) 가 정본이다.

전문 에이전트 5 인 토론 (2026-07-30, r1) 의 결론을 카테고리화했다. 원본 토론은 `.claude/discussion/r1/` 에 있다 (L-local).

## 1. 코어 . 얇은 연결

| 폴더 | 상태 | 한 줄 |
|---|---|---|
| [providerAdapter](providerAdapter/) | 설계 | ACP 를 내부 어휘로 채택하고 발명하지 않는다. 등록은 언제나 TOML manifest, 코어는 kind 별 범용 드라이버를 낸다. ACP 를 말하는 4 번째 CLI 는 TOML 10 줄, Rust 0 줄. |
| [coreRuntime](coreRuntime/) | 설계 | daemon 위상, 세션 상태 기계, **메모리 계약 숫자 확정** (유휴 6MB, 천장 48MB), backpressure, 고아 회수. 런타임·Windows I/O·저장소 결정이 전부 실측 근거다. |
| [providerDiscovery](providerDiscovery/) | 설계 | 모델·플래그·버전을 하드코딩 없이 아는 사다리와 version-keyed 캐시. 표면 drift 감지가 같은 뿌리다. |

## 2. 사용자 표면

| 폴더 | 상태 | 한 줄 |
|---|---|---|
| [desktopGui](desktopGui/) | 활성 설계 | PC 앞의 로컬 GUI. **GPT 앱의 편의를 가져오되 그 메모리는 안 가져온다.** `즉시 반응` 축이 여기 산다. GUI 스택은 프로토타입 실측 후 결정. |
| [pwaConnection](pwaConnection/) | 설계 | **origin 과 transport 를 분리한다.** 불변 HTTPS origin 하나 + 4 단 전송 사다리 + Noise E2E + 데몬 직접 Web Push. |
| [pwaSurface](pwaSurface/) | 대기 | PWA 자체. 연결 계층이 선 뒤에 짓는다. |
| [landingSite](landingSite/) | 설계 | GitHub Pages 한 장. 로고·설명·다운로드 둘·우측 상단 SNS (xlpod 방식). **프론트 "astryx 방식" 이 운영자 확인 대기.** |

## 3. 배포와 최신성

| 폴더 | 상태 | 한 줄 |
|---|---|---|
| [launcherUpdate](launcherUpdate/) | 설계 | exe 는 런처 방식. **정본은 GitHub Releases**, minisign 서명, 관리자 권한 불필요, 작업 중이면 미룸. runtrol 자신과 provider CLI **두 층**을 갱신한다 (clipscout 방식 승계). |

## 4. 안전

| 폴더 | 상태 | 한 줄 |
|---|---|---|
| [securityPosture](securityPosture/) | 설계 | default-deny 권한 모델, 소켓 표면 (Windows named pipe / Unix socket), 승인 전달, 감사 로그, 킬 스위치. **공개 저장소라 "모르는 사람이 기본값으로 켠다" 가 기준선이다.** |

## 5. 증거

| 폴더 | 상태 | 한 줄 |
|---|---|---|
| [northStarEvidence](northStarEvidence/) | 활성 | 북극성 13 축 각각을 떠받치는 게이트를 실물로 세운다. **게이트 없는 축은 3 점이 상한이다.** |

## 완료 판정

문서 작성이나 데모 영상으로 완료하지 않는다. 이니셔티브가 끝났다는 것은
(1) 그 능력이 실물 CLI 와 실물 기기로 동작하고
(2) 그것을 단언하는 게이트가 CI 에서 돌고
(3) 지식이 `docs/` 로 승격됐고
(4) 이니셔티브 폴더가 삭제됐다
는 뜻이다.

**첫 사례**: `positioningDecision` 이 2026-07-30 에 [docs/positioning.md](../docs/positioning.md) 로 승격되고 삭제됐다.

## 이 판을 지배하는 사실 (r1 조사)

- **벤더가 이미 냈다.** Anthropic Remote Control, OpenAI Codex Remote, GitHub Copilot CLI, Amp. 전부 무료 번들이거나 구독 포함. **그래서 "원격 조종" 은 우리 자리가 아니다. "여러 개를 한 곳에" 가 우리 자리다.**
- **상용 레이어는 무덤이다.** vibe-kanban (27.5k star, Rust) 폐업, terragon 폐업, crystal deprecated, omnara v1 deprecated.
- **성숙한 Rust 네이티브 세션 매니저 + 모바일 클라이언트는 존재하지 않는다.**
- **omnara v1 의 사인이 곧 최대 위험이다**: "CLI 를 감싸는 래퍼로 지었는데 CLI 가 끊임없이 바뀌어 유지가 불가능해졌다." -> `providerDiscovery` 의 발견 사다리와 drift 게이트가 직접 대응이다.
- **사용자는 릴레이 없는 모드를 원한다** (Happy 이슈 `local network only mode`).
- **claude-squad 는 Windows 를 못 한다** (creack/pty 미지원). tmux 기반 OSS 전반의 약점이다.
