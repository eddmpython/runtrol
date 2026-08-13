# pwaSurface

상태: 진행 중, 착수 순서 6 번. 세션과 Mission 감독 표면 및 실물 CLI 자동 게이트가 구현됐다. iOS 홈화면 설치와 Web Push 수신의 실기기 운영 확인만 남았다.

## 한 문장 정의

**폰에서 내 PC 의 세션 목록을 보고, 새 세션을 열고, 지시를 넣고, 출력을 실시간으로 보고, 승인 요청에 답하는 설치형 웹앱.**

## Current implementation

The installable static PWA now pairs from the VS Code QR, lists sessions, starts and resumes only within current exact authority, sends prompts, watches bounded output with reconnect cursors, interrupts and removes sessions, renders approval choices, exposes panic stop, and forgets the local phone identity. A separate scope-gated Mission tab reads bounded Mission and Task metadata and exposes only pause, safe resume, and cancel actions valid for the current state and exact device scopes. It does not create, start, integrate, archive, or promote capabilities remotely. It does not queue offline commands or store conversation content.

## 화면 (설계 시 채운다)

| 화면 | 무엇을 하나 | 어떤 북극성 축 |
|---|---|---|
| 세션 목록 | provider 무관하게 살아있는 세션 하나의 목록. 시작·재개·삭제 | 하나의 세션 목록 |
| 세션 뷰 | 실시간 출력. 커서 기반이라 재연결해도 빠짐 없음 | 폰에서 내 PC 세션 잇기 · 끊겨도 살아남기 |
| 승인 카드 | 전체 명령·대상 경로·세션 라벨·workspace 를 보여준다. ANSI/bidi 스푸핑 방어 | 폰에서 승인 |
| 페어링 | QR 스캔 + PC 승인. 양쪽에서 키 지문 표시 | (보안) |
| 기기 관리 | 페어링된 기기 목록, 개별 폐기 | (보안) |
| 패닉 | 전 세션 kill. 권한 없이 항상 가능 | (보안) |

## Component layer

The source of truth is [docs/frontendStack.md](../../docs/frontendStack.md). The PWA uses dependency-free HTML, CSS, and browser JavaScript, and reuses the canonical brand assets and interaction contracts without creating a second desktop application stack.

## 하지 않는 것

- 대화를 예쁘게 렌더링하려 들지 않는다. 각 CLI 의 출력 그대로가 정본이다
- diff 를 **편집**하지 않는다 (보여주는 것까지가 경계)
- 오프라인에서 명령을 큐잉하지 않는다 (PC 가 꺼져 있으면 "오프라인" 을 정직하게 보여준다)

## 완료 판정

- `phoneDrivesPcSmoke`: shipped PWA의 WebCrypto, Noise, record, CoreClient 모듈이 헤드리스 폰 프로세스에서 production 데몬과 설치된 실물 Claude Code 세션을 시작하고 프롬프트와 출력을 왕복한다. 활성 hosted CI 게이트다
- `approvalRoundtripSmoke`: 실제 permission prompt가 폰 watch 경로에 도달하고, 정확한 거부 응답이 같은 실물 CLI 세션을 재개시킨다. 활성 hosted CI 게이트다
- iOS 홈화면 설치 + Web Push 수신이 실기기로 확인됨 (operator 게이트. 점수로 세지 않는다)
