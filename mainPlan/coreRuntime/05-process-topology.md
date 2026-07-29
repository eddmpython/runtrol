# 프로세스 위상 . 하나의 바이너리, 세 인격, 하나의 데몬

## 결정

**바이너리 하나가 세 인격을 갖는다.** CLI, 데몬, PWA 서버가 같은 실행파일이고 인자로 갈린다. 단일 정적 바이너리를 유지하는 것이 `어디서나 같은 방법` 축의 전제다 (Node 도 Python 도 안 깔아도 된다).

**데몬은 파이프 연결 시 지연 spawn 된다.** 사용자가 별도로 "서비스 시작" 을 할 필요가 없다.

## Windows Service 를 쓰지 않는다

**보안 모델이 깨지기 때문이다.**

- SCM 등록에 **관리자 권한**이 필요하다. 개인 도구가 설치할 때 관리자를 요구하는 것 자체가 신뢰 비용이다
- **session 0 에는 사용자 DPAPI 가 없다.** [pwaConnection](../pwaConnection/README.md) 이 기기 개인키를 Windows 에서 DPAPI 로 감싸기로 했는데, 서비스로 돌면 그 키를 사용자 컨텍스트에서 풀 수 없다
- 자식 CLI 는 사용자의 인증 (`~/.claude`, `~/.codex`) 을 읽어야 한다. session 0 은 그 사용자가 아니다

**대신 분리된 사용자 프로세스로 띄운다.**

## spawn 플래그와 job object 함정

```
DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
+ CREATE_BREAKAWAY_FROM_JOB   (조건부. 주변 job 을 먼저 probe 한 뒤에만)
```

**`CREATE_BREAKAWAY_FROM_JOB` 을 무조건 붙이면 안 된다.** 주변 job 이 breakaway 를 허용하지 않으면 `CreateProcess` 가 실패한다. GnuPG 가 T4333 에서 정확히 이 문제를 겪었고, 해법은 **먼저 현재 job 의 제한을 조회해서 필요하고 허용될 때만 붙이는 것**이다.

터미널을 닫아도 데몬이 살아남는 것과, 데몬이 죽으면 자식 CLI 가 같이 죽는 것은 **다른 층**이다. 후자는 자식용 job object 가 담당한다 (`orphanReaping` 게이트).

## 세션 계층

목록에 세션 1,000 개를 띄우는 데 자식 프로세스 1,000 개를 살려두지 않는다.

| 계층 | 무엇 | 비용 |
|---|---|---:|
| **cold** | provider 저장소에서 읽은 행. 프로세스 없음 | 256 B |
| **warm** | 파일 리더가 열려 있음. 프로세스 없음 | 8 KiB |
| **hot** | 자식 프로세스가 살아서 붙어 있음 | 128 KiB (+ 자식 자신의 RSS) |

전이: spawn / attach / detach / idle-evict / kill / resume-from-cold.

**cold 목록이 CLI 호출이 아니라 파일 읽기인 것이 결정적이다** ([04-memory-contract.md](04-memory-contract.md) 의 39.9 초 대 4.4 밀리초).

## codex Node shim 우회

`codex` 는 네이티브 Rust 바이너리인데 앞에 Node shim 이 붙어 있다. **shim 을 지나 실제 바이너리를 해소하면 세션당 약 50MB 를 아낀다.**

이것은 provider 해소 (`[bin].names`) 단계에서 처리한다. 하드코딩이 아니라 **발견**이다: shim 을 실행해서 무엇을 exec 하는지 알아내는 것이 아니라, 설치 레이아웃에서 네이티브 바이너리를 찾고 **버전 probe 로 동일성을 확인**한 뒤에만 우회한다. 확인 실패 시 shim 을 그대로 쓴다 (안전 쪽으로 실패).

## 게이트

- `orphanReaping`: 데몬을 강제 종료하면 자식 CLI 가 남지 않는다
- `daemonSurvivesTerminalClose`: 터미널을 닫아도 데몬이 산다
- `jobBreakawayProbe`: 주변 job 이 breakaway 를 금지한 환경에서도 spawn 이 성공한다
- `noAdminRequired`: 설치와 첫 실행에 관리자 권한이 필요하지 않다
- `shimBypassVerified`: 네이티브 바이너리 우회가 버전 probe 로 확인된 경우에만 일어난다
