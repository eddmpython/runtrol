# Windows 자식 I/O 결정 . PTY 를 쓰지 않는 이유와 512 천장을 없애는 길

01 번 문서의 실측이 "파이프당 blocking 스레드 1 개, 상한 512" 를 밝혔다. 이 문서는 그 천장을 없애는 길과, PTY 를 v1 에서 빼는 결정의 근거를 확정한다.

## 결정 셋

1. **기본은 평범한 파이프다. PTY 를 쓰지 않는다.**
2. **확장 경로는 `FILE_FLAG_OVERLAPPED` 자체 named pipe + `tokio::net::windows::named_pipe` 다.** 이것이 512 천장을 구조적으로 없앤다.
3. **PTY 는 자식이 정말 TTY 밖에서 동작을 거부할 때만**, feature gate 뒤에서.

## 1. ConPTY 는 line/JSON 프로토콜의 전송로가 될 수 없다

**이것이 PTY 를 빼는 결정적 이유다. 비용 문제가 아니라 정확성 문제다.**

ConPTY 는 전송로가 아니라 **렌더러**다. 콘솔 텍스트 버퍼를 유지하고 그것을 VT 로 **다시 직렬화**한다. 바이트가 그대로 통과하지 않는다.

runtrol 에 직접 해당하는 손상:

- **`cols` 에서 하드 랩.** `cols` 보다 긴 JSON 한 줄에 `\r\n` 이 주입되어 쪼개진다. runtrol 이 다루는 것이 정확히 ndjson 과 JSON-RPC 다
- 커서 위치 재지정 시퀀스 주입
- SGR 정규화 (`[49m` 이 `[m` 으로 접힘. microsoft/terminal#362)
- **OSC 제목 변경 시퀀스 합성** (portable-pty 자체 주석이 인정한다)
- 스크롤 영역 재그리기 시 전체 화면 재도색 (microsoft/terminal#7019, #10462)

**얇음 원칙과도 정면 충돌한다.** ConPTY 를 쓰면 runtrol 은 provider 가 보낸 것이 아니라 **ConPTY 가 다시 그린 것**을 전달하게 된다. 그것을 되돌리려면 VT 파서를 들여야 하고, 그 순간 runtrol 은 얇지 않다.

## 2. ConPTY 는 세션당 진짜 프로세스를 띄운다

소스 확인 (`src/winconpty/winconpty.cpp`): `CreatePseudoConsole` 은 `conhost.exe --headless ... --signal 0x... --server 0x...` 를 **CreateProcess 로 띄운다.** 스레드가 아니라 프로세스다.

| 항목 | 평범한 파이프 | ConPTY |
|---|---|---|
| 프로세스 | **0** | **1** (conhost/OpenConsole, 작업집합 약 4~10MB) |
| 핸들 | 2 | 약 5 개 보유 + 파이프 3 쌍 |
| 커널 버퍼 | 약 4KiB | + 콘솔 텍스트 버퍼 (`rows x cols`) |
| 종료 코드 | `GetExitCodeProcess` 로 정확 | **주지 않는다.** 직접 자식 핸들을 들고 있어야 한다 |
| 종료 시 교착 | 없음 | **알려진 버그 계열** |

**자식 100 개면 conhost 프로세스 100 개, 작업집합 약 0.5~1GB.** 감독자에게는 이것 하나만으로도 실격이다.

교착 계열도 실재한다: microsoft/terminal #1810, #4050, #17489, #17688, #19922. Windows 11 24H2 (build 26100) 부터 `ClosePseudoConsole` 이 즉시 반환하지만 **Windows 10 과 Server 2019 는 무한 대기**한다. runtrol 이 그 OS 들을 버릴 이유가 없다.

## 3. 512 천장을 없애는 길

01 번 문서의 천장은 **익명 파이프**에서 온다. `CreatePipe` 로 만든 익명 파이프는 `FILE_FLAG_OVERLAPPED` 로 열 수 없어서 IOCP 에 등록할 수 없고, 그래서 tokio 가 `spawn_blocking` 으로 읽는다.

**우회 경로가 있다:**

1. 고유한 이름으로 named pipe 를 직접 만든다
2. **부모 쪽 끝은 `FILE_FLAG_OVERLAPPED`**, 자식 쪽 끝은 overlapped 없이 상속 가능하게
3. 자식 쪽 끝을 `CreateProcess` 에 `hStdOutput` 으로 넘긴다
4. 부모 쪽은 `tokio::net::windows::named_pipe` 로 읽는다. **진짜 IOCP 다. blocking 스레드 0**

선행 사례가 둘 있다. `tokio-anon-pipe` 크레이트가 정확히 이것을 하고 ("고유할 것 같은 이름을 만들고 overlapped 가 켜진 named pipe 를 만든다"), .NET 도 같은 길을 갔다 (dotnet/runtime #125643 "Process: use overlapped I/O for parent end of stdout/stderr pipes on Windows").

**이 경로는 평범한 파이프에만 된다. ConPTY 는 동기 파이프를 요구하므로 불가능하다.** PTY 를 버리는 것이 성능 상한도 같이 푸는 셈이다.

주의: `NamedPipeServer::from_raw_handle` 은 `unsafe` 이고 핸들이 `FILE_FLAG_OVERLAPPED` 로 생성됐어야 한다. 아니면 **에러가 아니라 오동작**한다. `ServerOptions::max_instances` 는 254 초과 시 패닉한다.

## 4. 자식 종료 감지는 이미 진짜 async 다

`tokio::process` 의 Windows 종료 감지는 `RegisterWaitForSingleObject` (libuv 와 같은 전략) 를 쓴다. OS 스레드풀의 wait 스레드 하나가 핸들 63 개를 다중화하므로 **`Child::wait()` 는 잘 확장된다.** 알려진 Windows 행 버그도 없다.

즉 문제는 종료 감지가 아니라 **stdout 읽기 하나뿐**이고, 3 절이 그것을 푼다.

## 5. TTY 가 아닐 때 CLI 가 바꾸는 것과 그 대응

PTY 없이도 필요한 것을 얻는다.

| 동작 | TTY | 파이프 | runtrol 의 레버 |
|---|---|---|---|
| ANSI 색 | 켜짐 | 꺼짐 | `FORCE_COLOR` / `CLICOLOR_FORCE` |
| 진행바·스피너 | 켜짐 | 꺼짐 | 대개 도구별 플래그 |
| **stdio 버퍼링** | 줄 단위 | **4KiB 블록** | 아래 |
| 대화형 프롬프트 | 표시 | 억제 | `--non-interactive` 류 |
| 화면 폭 | ioctl | 기본 80 | `COLUMNS` |

**진짜 함정은 색이 아니라 버퍼링이다.** libc stdio 를 쓰는 C·Python 자식은 파이프에서 완전 블록 버퍼로 바뀌어 4KiB 가 쌓이거나 프로세스가 끝날 때까지 **아무것도 안 나온다.** 사람들이 PTY 로 도망가는 1 번 이유다.

그런데 runtrol 이 감싸는 대상은 괜찮다.

- **Rust 자식**: `std::io::Stdout` 이 항상 `LineWriter` 라 문제 없음 (Codex CLI 가 Rust 다)
- **Node 자식**: 쓰기마다 flush. 문제 없음 (Claude Code 가 Node 다)
- **Go 자식**: `os.Stdout` 무버퍼. 문제 없음
- **Python 자식**: `PYTHONUNBUFFERED=1` 또는 `-u` 로 해결
- **플래그 없는 임의 C 바이너리**: 이것만이 진짜 PTY 용례다

**즉 지금 붙일 두 provider 모두 PTY 가 필요 없다.**

자식 환경 설정 (색은 얻고 PTY 는 안 쓴다):

```
FORCE_COLOR=3
CLICOLOR_FORCE=1
TERM=xterm-256color
COLORTERM=truecolor
PYTHONUNBUFFERED=1
(NO_COLOR 는 설정하지 않는다)
```

그리고 **SGR 만 파싱한다.** 커서 이동도, 리플로우도, conhost 도 없다.

TTY 판정은 `std::io::IsTerminal` (Rust 1.70+ stable).

## 6. PTY 가 정말 필요해질 때의 규약

feature gate 뒤에 두고, 켜질 때는 이 규약을 지킨다.

- `portable-pty` 를 쓰되 **crates.io 가 아니라 git 핀**. 0.9.0 (2025-02-11) 이후 릴리즈가 없고 git main 이 약 17 개월 앞서 있다
- **`WinChild` 를 절대 `.await` 하지 않는다.** `poll()` 이 `Pending` 을 반환할 때마다 **새 `std::thread` 를 띄우는데 dedup 도 `JoinHandle` 보관도 없다.** `select!` 루프나 `FuturesUnordered` 재폴링에서 poll 마다 스레드가 샌다. `try_wait()` 를 타이머로 돌리거나 `as_raw_handle()` + 자체 `RegisterWaitForSingleObject` 를 쓴다
- pty 당 리더 스레드 **하나**, 버퍼 32~64KiB, `stack_size` 명시 (wezterm 은 pane 당 스레드 2 개 + 버퍼 약 3MiB 인데 그건 탭 몇 개짜리 터미널 에뮬레이터에 맞춘 값이다)
- **ConPTY 종료 순서** (여기서 모두가 교착한다): 출력 파이프를 **다른 스레드**에서 계속 읽는다 -> `ConptyReleasePseudoConsole` (24H2+) 또는 출력 핸들 닫기 -> `ERROR_BROKEN_PIPE`/EOF 대기 -> **그 다음에** `ClosePseudoConsole`. **26100 이전 Windows 에서 리더 스레드가 `ClosePseudoConsole` 을 부르면 안 된다**
- ConPTY 쌍 (`conpty.dll` + 맞는 `OpenConsole.exe`) 을 exe 옆에 사이드로드하면 portable-pty 의 `load_conpty()` 가 자동으로 집는다. **둘을 반드시 짝으로 갱신한다** (불일치가 wezterm#7774 의 PowerShell `FailFast` 원인)

## 7. 생태계 현황 (2026-07)

**Windows 에서 진짜 async PTY I/O 를 주는 성숙한 크레이트는 없다. OS 가 금지한다** (ConPTY 가 동기 파이프를 요구한다).

| 크레이트 | 버전 | Windows | async | 판정 |
|---|---|---|---|---|
| portable-pty | 0.9.0 (2025-02) | ConPTY | **없음** (블로킹 `Read` 만) | 유일한 성숙한 크로스플랫폼 선택지. pty 당 스레드 |
| pty-process | 0.5.3 (2025-07) | **없음** | 있음 (진짜 tokio `AsyncRead`) | 생태계 최고의 async API. **Windows 를 안 한다** |
| expectrl | 0.9.0 (2026-05) | 있음 | smol 계열 | tokio 와 섞으려면 `async-compat` |
| conpty | 0.7.0 (2024-09) | Windows 전용 | 없음 | 얇은 래퍼. 사이드로드 없음 |
| xpty | 0.3.6 | 있음 | "async-ready" (미검증) | 다운로드 647. 의존하기엔 미성숙 |
| pseudoterminal | 0.2.1 | 있음 | **없음** (문서가 미구현이라 밝힘) | 회피 |

## 게이트로 옮길 것

- `orphanReaping` 에 conhost 프로세스 잔존 검사를 넣지 않는다. **conhost 를 애초에 안 띄우기 때문이다.** 대신 `noPtyByDefault` 정적 검사: 기본 feature 집합에 PTY 백엔드가 링크되지 않음을 단언
- `lineIntegritySmoke`: `cols` 보다 긴 ndjson 한 줄이 손상 없이 왕복하는지. **ConPTY 를 실수로 켜면 red 가 되는 게이트다**
- `idleFootprintRatchet` 에 프로세스 수를 포함한다 (자식 CLI 외의 보조 프로세스 0)
