# launcherUpdate

상태: 설계. `desktopGui` 와 함께 배포된다.

## 한 문장 정의

**exe 는 런처 방식이다.** 사용자가 한 번 설치하면 그 뒤로 버전을 신경 쓰는 순간이 없다. **정본은 GitHub Releases 다.**

북극성 `알아서 최신` 축이 이 이니셔티브다. 최상위 원칙 (사용자 편의) 의 직접 적용이기도 하다: **"사용자가 설정해야 알아서 되는 것은 실패다."**

## 선행 의존 . GUI 셸 결정

본 문서는 clipscout 의 **Tauri v2 updater** 승계를 가정한다. `desktopGui` 의 셸이 Tauri 가 아니게 결정되면 업데이트 계층은 셸 무관 방식 (별도 소형 런처 exe 가 본체 바이너리를 검증·교체) 으로 재설계한다. **계약은 어느 쪽이든 동일하다**: Releases 정본 · minisign 서명 · 작업 중 미룸 · 롤백 경로 없으면 업데이트 안 함.

## 형제 프로젝트 clipscout 에서 가져오는 방식

clipscout 이 Tauri v2 updater 로 이미 실전 운영 중이다. 그 구조를 그대로 승계한다.

### 설정 (`tauri.conf.json` 형태)

```
bundle.createUpdaterArtifacts = true
bundle.targets = ["nsis"]
bundle.windows.nsis.installMode = "currentUser"     # 관리자 권한 요구 안 함
plugins.updater.pubkey = <minisign 공개키>
plugins.updater.endpoints = [<latest.json URL>]
plugins.updater.windows.installMode = "passive"     # 설치 중 사용자를 막지 않음
```

`installMode = "currentUser"` 가 중요하다. **설치에 관리자 권한을 요구하지 않는다** ([core runtime](../../docs/coreRuntime.md#process-containment-and-restart-recovery) 이 Windows Service 를 쓰지 않는 것과 같은 이유).

### 업데이트 루프 (clipscout `updates.rs` 승계)

배경 태스크가 돌면서 상태에 따라 재시도 간격을 달리한다.

| 상태 | 다음 확인까지 |
|---|---|
| 최신 | 6 시간 |
| 작업 중이라 미룸 | 1 분 |
| 아직 준비 안 됨 | 5 분 |
| 실패 | 15 분 |

핵심 설계 셋:

1. **작업 중이면 설치하지 않는다.** 세션이 돌고 있으면 미룬다. 사용자의 일을 끊는 업데이트는 편의가 아니라 방해다
2. **상태를 항상 복원 가능하게 든다.** 화면이 언제 열려도 마지막 업데이트 상태를 보여준다
3. **즉시 재확인 경로가 있다.** 사용자가 실패 상태에서 직접 누를 수 있고, 로그인 등 조건이 바뀌면 예약 시각을 안 기다리고 깨운다

### 서명

**minisign 공개키를 바이너리에 박고 Tauri updater 가 서명을 검증한다.** 서명이 안 맞으면 설치하지 않는다. GitHub Releases 가 침해돼도 서명 없는 아티팩트는 설치되지 않는다.

이 저장소는 공개되므로 **개인키는 절대 저장소에 없다.** GitHub Actions secret 으로만 존재한다.

## runtrol 고유 요구 . 두 층을 갱신한다

clipscout 과 다른 점: **runtrol 은 자기 자신과 자식 CLI 를 둘 다 최신으로 유지한다.**

| 층 | 채널 | 실패 시 |
|---|---|---|
| **runtrol 앱** | GitHub Releases + minisign | 설치 안 함, 다음 주기 재시도 |
| **provider CLI** (`claude`·`codex`) | 채널을 **감지한다** (npm global · `codex update` · 네이티브 설치기). 가정하지 않는다 | **자동 롤백**. 읽기 전용 발견 계약은 [provider discovery](../../docs/providerDiscovery.md) |

**provider CLI 쪽이 더 위험하다.** 벤더가 패치 하나로 표면을 바꾸는 것이 이 카테고리의 1 번 사인이기 때문이다 (agentapi #207, claude-code-router #1601, happy #1543). 그래서:

- 업데이트 후 **스모크를 돌리고** 통과해야 확정
- 실패하면 **이전 버전으로 되돌린다**
- **롤백 경로가 없으면 업데이트하지 않는다**
- 돌고 있는 세션은 안 깬다. 업데이트는 세션 경계에서만

## provider 신규 설치 (설치까지가 이 이니셔티브다)

갱신만이 아니라 **처음 설치도 runtrol 이 대행한다.** 사용자가 목록에서 아직 없는 provider 를 켜면:

1. **채널을 감지해 공식 경로로 설치한다** (npm global · 네이티브 설치기 · winget 류). 갱신과 같은 사다리, 같은 원칙: 채널을 가정하지 않는다
2. **관리자 권한 불필요 채널을 우선한다.** 관리자 권한 없이는 불가능한 provider 는 그 사실을 설치 전에 보여준다
3. **설치 직후 스모크를 돌리고** 실패하면 제거해 원상복구한다 (반쯤 설치된 CLI 를 남기지 않는다)
4. **원격 (폰) 에서 설치 트리거는 기본 거부.** 새 실행 파일을 시스템에 들이는 것은 PC 앞 행동이다
5. 인증 (로그인) 은 대행하지 않는다. CLI 자신의 인증 흐름을 열어줄 뿐이다 (모델 API 키 미보유 불변식)

## 릴리즈 파이프라인

GitHub Actions 로 태그에서 자동 생성한다.

1. 태그 push (`v0.1.0`) -> 빌드 매트릭스 (Windows 우선, 이후 macOS·Linux)
2. `createUpdaterArtifacts` 로 업데이트 번들 + `.sig` 생성
3. minisign 개인키 (Actions secret) 로 서명
4. GitHub Release 에 아티팩트와 `latest.json` 첨부
5. 랜딩 페이지가 **최신 릴리즈를 GitHub API 로 읽어** 다운로드 버튼을 만든다 (버전을 손으로 안 적는다)

**손으로 적은 버전 번호는 반드시 썩는다.** 랜딩의 다운로드 링크는 릴리즈에서 파생된다.

## 완료 판정

- `appUpdateRehearsal`: 런처가 서명된 업데이트를 받아 설치하고, **서명이 안 맞으면 거부한다**
- `cliUpdateRehearsal`: 구버전 -> 업데이트 -> 세션 정상 -> 고의로 깨진 버전 -> **자동 롤백**
- 설치에 관리자 권한이 필요하지 않다 (`noAdminRequired`)
- 세션이 도는 중에는 설치가 미뤄진다
- 랜딩 다운로드 링크가 릴리즈에서 파생되고 손으로 적힌 버전이 없다
