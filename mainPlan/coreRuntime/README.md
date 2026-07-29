# coreRuntime

상태: 대기 (`positioningDecision` 선행). **설계 완료. 전부 실측 근거다.**

| 문서 | 내용 |
|---|---|
| [01-runtime-measurements.md](01-runtime-measurements.md) | 이 기계에서 실측한 런타임 비교. tokio 대 smol 대 compio, 스레드·메모리·바이너리 숫자, feature 함정 |
| [02-windows-io-decision.md](02-windows-io-decision.md) | PTY 를 쓰지 않는 결정적 근거와, 512 스레드 천장을 없애는 named pipe 경로 |
| [03-storage-decision.md](03-storage-decision.md) | redb 채택. sled·fjall·LMDB·canopydb·native_db 를 기각한 근거 |
| [04-memory-contract.md](04-memory-contract.md) | **확정 숫자.** 실측이 바로잡은 전제 셋, 버퍼링 입장, backpressure, `pwaConnection` 과의 충돌 조정 |
| [05-process-topology.md](05-process-topology.md) | 하나의 바이너리 세 인격, Windows Service 를 안 쓰는 이유, job object 함정, 세션 계층 |

## 한 문장 정의

**runtrol 코어는 코딩 CLI 자식 프로세스를 감독하고 그 이벤트를 순서 보장하며 흘려보내는 daemon 이다.** transcript 를 소유하지 않고, 메모리 예산을 숫자 계약으로 지킨다.

## 왜 메모리 예산이 형용사가 아니라 계약인가

운영자가 첫 문장에 "아주 메모리 효율적이여야한다" 를 넣었다. 그런데 이 카테고리에서 메모리는 **실제로 측정된 실패 지점**이다.

| 실측 | 무엇 |
|---|---|
| **happy-cli #164** (미해결) | `--resume` 때마다 이전 `claude` 프로세스가 PPID 1 로 떨어져 **죽지 않고** MCP 자식들을 살려둔다. 측정치: 고아 하나당 약 **285MB**, 고아 3 개 + MCP 10 개로 **858MB** 가 유휴 상태로 낭비 |
| **claude_code_agent_farm** | 스스로 문서에 에이전트당 약 500MB, 기본 20 에이전트 = 약 10GB 라고 적어 놓았다 |
| **claude-code-router #1534** | 요청·응답 본문 전체를 SQLite 에 넣고 `SELECT` 로 통째로 꺼내다 게이트웨이가 OOM 루프 |
| **happy #1453** | 배경 히스토리 prefetch 가 세션 전체를 메모리에 올려 긴 세션에서 웹이 느려진다 |
| **container-use** | Dagger 엔진 + 환경당 컨테이너 하나. 구조상 가장 무겁다 |

**패턴이 뚜렷하다: transcript 를 복제하는 순간 메모리를 먹고, 자식을 회수 안 하면 메모리를 먹는다.** 둘 다 얇음 원칙과 같은 결론에 도달한다.

## 확정된 원칙

### 0. 런타임과 자식 I/O (실측으로 확정)

- **tokio + `["rt","process","io-util","macros","time","sync","signal"]` + `new_current_thread()`.** 감독자에는 CPU 바운드 일이 없다. `rt-multi-thread` 는 코드 5,120 바이트와 OS 스레드 16 개를 사서 100% I/O 대기인 일을 스케줄한다
- **"smol 이 가볍다" 는 이 워크로드에서 거짓이다.** 실측: smol 스택 31 crate / 307.5 KiB 대 tokio 최소 8 crate / 264.0 KiB. tokio `full` 보다도 크고, **Windows 에서 tokio 와 똑같이 blocking 스레드 풀을 쓴다**
- **PTY 를 쓰지 않는다.** ConPTY 는 `cols` 에서 줄을 하드 랩해서 **긴 JSON 한 줄을 쪼갠다.** 비용이 아니라 정확성 문제다 (02 번 문서)
- **512 자식 천장의 해법은 `FILE_FLAG_OVERLAPPED` 자체 named pipe** + `tokio::net::windows::named_pipe`. 진짜 IOCP, blocking 스레드 0
- 기각: async-std (RUSTSEC-2025-0052 discontinued), tokio-uring (2 년간 실질 커밋 1 개, Linux 전용), Windows IoRing (파일 전용 opcode 9 개, 프로세스 연산 없음)
- **저장소는 redb 4.1.0 이상.** 유휴 RSS 약 1~4MB (실측 PR), 필수 의존성 0, **C 컴파일러 불필요** (Windows 설치 동등성), **배경 스레드 없음** (03 번 문서). sled 는 절대 안 쓴다 (fsync 안 함, 11GB 보고, 생태계가 떠났다)

### 1. transcript 사본을 만들지 않는다

runtrol 이 세션당 저장하는 것은 **약 200 바이트** (runtrol id, native id, 라벨, 핀, 이벤트 커서) 이고 **본문은 0** 이다. 본문의 정본은 provider CLI 자신의 저장소다.

**수용 테스트**: `rm -rf $RUNTROL_HOME` 을 해도 잃는 것은 라벨과 핀뿐. 세션은 `claude --resume` / `codex resume` 으로 그대로 열린다.

### 2. 무제한 버퍼 금지

scrollback 을 `Vec<u8>` 에 누적하거나 전체 로그를 `String` 으로 들지 않는다. 출력은 **bounded ring 또는 pass-through** 다. 재연결용 링버퍼는 세션당 바이트 예산이 있고, 그 너머는 provider 의 자체 로그가 정본이다.

### 3. 자식 회수는 기본값이다

- Windows: **job object**. 데몬이 죽으면 자식도 죽는다
- Unix: process group
- 재시작 시 고아 탐지와 회수

`orphanReaping` 게이트가 이것을 단언한다. **happy-cli #164 가 정확히 이 게이트가 없어서 생긴 사고다.**

### 4. 세션 계층 (cold / warm / hot)

세션 1,000 개를 목록에 띄우는 데 자식 프로세스 1,000 개를 살려두지 않는다. 목록은 provider 의 세션 저장소를 **읽어서** 만들고, 실제 자식은 attach 된 것만 산다. 유휴 세션은 evict 되고 cold 에서 resume 된다.

**목록을 CLI 에 물어보지 않는 이유가 실측으로 확정됐다: `claude agents --json` 39.9 초 대 파일 직독 4.4 밀리초.** 얇음 원칙과 성능이 같은 답을 가리킨다. 계층별 비용은 [05](05-process-topology.md), 숫자는 [04](04-memory-contract.md).

### 5. backpressure . 위치를 버리되 데이터는 안 버린다

큐는 64 프레임 / 256KiB 로 유한하다. 넘치면 **그 구독자의 위치를 버리고 데이터는 절대 안 버린다.** 클라이언트는 provider 파일에서 범위 복구한다.

**데이터의 정본이 우리 것이 아니라서 가능한 설계다.** 얇음이 정확성을 사 준 자리다.

### 6. 확정된 메모리 상한

| 상태 | 상한 |
|---|---:|
| 유휴 | **6 MB** |
| 세션 1,000 개 색인 | **9 MB** |
| hot 8 + 구독자 4 | **18 MB** |
| 하드 천장 | **48 MB** |

바닥 실측 1.43MB. 단위당: cold 행 256B, warm 리더 8KiB, hot 세션 128KiB, 구독자 20KiB. **내려가기만 하는 ratchet 이다.**

### 6. Windows 가 1 급이다

`claude-squad` 는 Windows 에서 아예 안 뜬다 (`creack/pty` 미지원). tmux 기반 OSS 전반의 공통 약점이다. runtrol 은 ConPTY 와 POSIX 를 같은 추상 뒤에서 직접 다뤄 **tmux 없이** Windows 를 지원한다. 이것이 `어디서나 같은 방법` 축이고 Rust 를 고른 이유 중 하나다.

**BatBadBut (CVE-2024-24576)**: npm 이 Windows 에 `claude.cmd` · `codex.cmd` 를 깐다. Rust 의 `.cmd` 실행 인자 이스케이프에 정확히 해당하므로 게이트로 잠근다.

## 완료 판정

- `idleFootprintRatchet`: idle RSS 와 CPU 상한. **내려가기만 하는 ratchet**
- `orphanReaping`: 데몬을 죽이면 자식이 안 남는다
- `resilienceFaultInjection`: 네트워크 차단·데몬 강제 종료·재연결에서 **출력 손실 0**
- `rm -rf $RUNTROL_HOME` 수용 테스트
- `crossPlatformMatrix` 의 Windows 잡이 WSL 없이 green

## 원본

`.claude/discussion/r1/coreRuntime.md` (작성 중). 실측 근거는 `.claude/discussion/r1/` 의 선행기술 조사에도 있다.
