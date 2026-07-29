# 북극성 증거 등록부

`README.md` 북극성 표의 각 축이 **어떤 실행 가능한 게이트에 기대어 점수를 주장하는지**의 정본이다.

산문 증거는 썩는다. 이름 붙인 스모크는 이름이 바뀌고, 지워지고, 러너에 등록되지 않은 채로 남는데 축은 계속 그것을 근거로 점수를 주장한다. 그래서 축과 게이트의 대응은 여기 한 곳에만 있고, 게이트 러너가 이 표를 대조한다.

## 게이트 종류

| 종류 | 뜻 | 점수에 세는가 |
|---|---|---|
| `contract` | 순수 계약·정적 검사. 외부 프로세스 없이 돈다 | 예 |
| `smoke` | 실물 CLI 바이너리 또는 실물 브라우저를 태운다 | 예 |
| `bench` | 예산 ratchet. 넘으면 red | 예 |
| `operator` | 실계정·실기기가 필요해 기계로 못 돌린다 | **아니오. 총점에서 뺀다** |

## 축과 증거

현재 전 축이 미구현이므로 아래는 **지어야 할 게이트의 명세**다. 게이트가 실재하고 CI 에서 돌기 전에는 그 축의 점수 상한이 3 이다.

| 축 | 게이트 | 종류 | 무엇을 단언하는가 |
|---|---|---|---|
| 하나의 세션 목록 | `sessionLifecycleSmoke` | smoke | 시작 -> 목록 등장 -> 재개 -> 삭제 -> 목록에서 사라짐이 두 provider 에서 동일하게 성립 |
| 즉시 반응 | `interactionLatencyBudget` | bench | 목록 첫 페인트, 대화 열기, 첫 토큰 도달, 입력 반응의 p95 상한. **내려가기만 하는 ratchet** |
| 즉시 반응 | `scrollUnderLoadSmoke` | smoke | 초당 수천 줄이 쏟아지는 동안 스크롤과 입력이 프레임 예산 안에 머문다 |
| 폰에서 내 PC 세션 잇기 | `phoneDrivesPcSmoke` | smoke | headless 브라우저의 실물 PWA 가 실물 데몬을 통해 실물 `claude`/`codex` 세션에 프롬프트를 넣고 출력을 받는다 |
| 공급자 확장성 | `providerContract` | contract | 모든 어댑터가 같은 trait 계약을 통과. **코어에 provider 고유명사 분기가 없다**는 정적 검사 포함 |
| 공급자 확장성 | `agentSurfaceDrift` | smoke | 최신 CLI 를 받아 생성 스키마와 저장 스키마를 대조. 공급자가 표면을 바꾸면 사용자보다 먼저 red |
| 대화 무통과 | `egressContract` | contract | allowlist 밖 목적지로 소켓이 안 열린다. 프롬프트·응답 본문이 runtrol 의 디스크나 로그에 안 남는다. **벤더 세션 파일을 여는 코드가 없다**는 정적 검사 포함 |
| 폰에서 승인 | `approvalRoundtripSmoke` | smoke | 실제 permission prompt 가 폰 표면에 도달하고, 폰의 응답이 세션을 재개시킨다 |
| 끊겨도 살아남기 | `resilienceFaultInjection` | smoke | 네트워크 차단, 데몬 강제 종료, 폰 재연결 각각에서 세션이 살아남고 **출력 손실 0** |
| 상주 비용 | `idleFootprintRatchet` | bench | idle RSS 와 CPU 상한. **내려가기만 하는 ratchet** |
| 어디서나 같은 방법 | `crossPlatformMatrix` | smoke | 같은 종단 스모크가 Windows·macOS·Linux 러너에서 전부 green. **Windows 잡은 WSL 없이 돈다** |
| 알아서 최신 | `cliUpdateRehearsal` | smoke | 구버전 -> 업데이트 -> 세션 정상 -> 고의로 깨진 버전 -> 자동 롤백 |
| 알아서 최신 | `appUpdateRehearsal` | smoke | 런처가 GitHub Releases 에서 서명된 업데이트를 받아 설치하고, 서명이 안 맞으면 거부한다 |
| 모델 자동 인식 | `modelDetectionSmoke` | smoke | 실물 CLI 에서 모델 목록을 얻는다. **소스에 모델 이름 리터럴이 없다**는 정적 검사 포함 |
| 세션끼리 안 밟기 | `concurrentSessionIsolation` | smoke | 같은 레포에서 N 개 세션이 동시에 파일을 고쳐도 서로의 변경을 잃지 않는다 |
| 떠날 자유 | `uninstallLeavesNoTrace` | smoke | runtrol 제거 후 `claude --resume` 과 `codex resume` 이 그 세션들을 그대로 연다 |
| 폰에서 내 PC 세션 잇기 | `iosInstallAndPush` | **operator** | iOS 홈화면 설치 + Web Push 수신. 실기기 필요. **점수에서 뺀다** |

## 안전 게이트 (축에 안 붙지만 바닥 조건)

| 게이트 | 무엇을 단언하는가 |
|---|---|
| `scopeGrantability` | 부여 불가 스코프 (`device.pair` · `config.write` · `approval.auto`) 를 원격에서 부여하려는 코드가 **컴파일되지 않는다** |
| `rebindingDefenses` | Host allowlist, Origin 기본 거부, 쿠키 인증 부재, CORS wildcard 부재를 실제 요청으로 확인 |
| `argumentEscaping` | Windows `.cmd` 실행 인자 이스케이프 (BatBadBut CVE-2024-24576) |
| `configReadOnly` | provider 설정 파일에 **쓰는** 코드가 없다 |
| `orphanReaping` | 데몬을 죽이면 자식 CLI 프로세스가 남지 않는다 |

## 등록 규약

1. 축은 **최소 하나의 `contract` 와 최소 하나의 `smoke`** 를 가져야 한다. 하나뿐이면 그 축은 8 점을 넘지 못한다.
2. 여기 적힌 게이트 파일이 **실재하지 않으면** 등록부 검사가 red 다.
3. 게이트가 **러너에 등록되지 않으면** red 다. 저장소에 있는 것과 도는 것은 다른 말이다.
4. `operator` 종류는 총점 계산에서 빠지고, 그 사실이 `README.md` 에 보인다.
5. 새 게이트는 **통과를 보기 전에 실패할 수 있는지부터 확인한다.** 잡아야 할 결함을 일부러 심어 red 를 본다.
