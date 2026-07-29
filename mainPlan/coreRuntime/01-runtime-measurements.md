# 런타임 선택 . 이 기계에서 실측한 숫자

측정 환경: Windows 11 Home build 26200, 논리 CPU 16, rustc 1.97.0, `x86_64-pc-windows-msvc`, tokio 1.53.1.
릴리즈 프로파일 전부 동일: `opt-level=3, lto=true, codegen-units=1, panic="abort", strip=true`.

**추정이 아니라 빌드하고 돌려서 잰 값이다.** 근거 없이 "Rust 라서 가볍다" 고 말하지 않기 위한 바닥이다.

## 결론 먼저

**tokio + `features = ["rt","process","io-util","macros","time","sync","signal"]` + `new_current_thread()`.**
단, 파이프를 연 자식이 약 200 개를 넘길 전망이면 compio 를 진지하게 재평가한다.

## 가장 중요한 발견 (문서에 없다)

**Windows 에서 `tokio::process` 는 자식의 stdout 을 `spawn_blocking` 으로 읽는다. 진행 중인 파이프 읽기 하나당 OS 스레드 하나이고, `max_blocking_threads` 기본값 512 가 하드 상한이다.**

근거 (소스 확인): `tokio/src/process/windows.rs` 의 `ChildStdio` 가 `Blocking<ArcFile>` 이고, `tokio/src/io/mod.rs` 가 그것을 `crate::blocking::spawn_blocking` 으로 돌린다.

실측. 자식마다 30 초간 파이프 출력이 없는 명령을 돌리고 리더 태스크를 하나씩 붙였다.

| 자식 수 | 감독자 OS 스레드 | 증가분 | WorkingSet | Private |
|---:|---:|---:|---:|---:|
| 0 | 19 | . | 10,256 KB | 2,860 KB |
| 32 | 51 | **+32** | 11,552 KB | 4,824 KB |
| 128 | 145 | **+126** | 14,332 KB | 10,232 KB |

거의 정확히 1 대 1 이다.

**runtrol 에 대한 함의:**

- **조용한 파이프 자식의 천장이 약 512 개다.** 넘으면 읽기가 큐에 쌓이고 **조용히** 멈춘다. 조용한 실패는 이 제품이 제일 피해야 할 것이다
- `tokio::fs` 나 자체 `spawn_blocking` 을 쓰면 **같은 512 풀을 두고 경쟁**한다
- stdout 과 stderr 를 둘 다 캡처하면 자식당 스레드가 **두 배**다
- 완화책: `max_blocking_threads` 를 명시하고, **실시간으로 파싱할 필요 없는 출력은 파이프가 아니라 파일로 리다이렉트**한다 (얇음 원칙과 같은 방향이다. 안 읽을 것은 안 읽는다)

## Windows 스레드 메모리 . 예약과 커밋은 다르다

블록된 `spawn_blocking` 스레드 512 개로 측정.

| 항목 | 스레드당 |
|---|---:|
| Virtual (예약) | **2.014 MB** (교과서적인 2 MiB reserve) |
| Private (커밋) | **55.3 KB** |
| WorkingSet (상주) | **26.7 KB** |

**512 스레드가 주소공간 약 1GB 를 예약하지만 실제로는 RSS 약 13.7MB, 커밋 약 28MB 다.**

결론: **64 비트 Windows 데몬에서 RSS 를 줄이려고 `thread_stack_size` 를 손대지 마라. 얻는 게 거의 없다.** 실제로 비용인 것은 스택 크기가 아니라 **스레드 개수** (스케줄러 압력, 컨텍스트 스위치, 512 천장) 다.

## 런타임 유휴 비용

| 모드 | OS 스레드 | WorkingSet | Private |
|---|---:|---:|---:|
| 런타임 없음 (기준선) | 4 | 9,952 KB | 2,024 KB |
| `new_current_thread()` 유휴 | 4 | 9,624 KB | 1,980 KB |
| `new_multi_thread()` 기본 | **20** | 10,156 KB | 2,904 KB |
| `new_multi_thread()` 2 worker / 4 blocking / 256 KiB 스택 | 6 | 9,728 KB | 2,076 KB |

`worker_threads` 기본값이 CPU 수라 4 에서 20 으로 뛴다. **감독자에는 CPU 바운드 일이 없다.** `rt-multi-thread` 는 코드 5,120 바이트와 OS 스레드 16 개를 사서 100% I/O 대기인 일을 스케줄한다. `rt` 로 충분하다.

## 태스크당 비용

current_thread 런타임, 대기 중인 태스크 N 개.

| 태스크 | Private 증가 | 태스크당 |
|---:|---:|---:|
| 10,000 | 4,356 KB | **446 B** |
| 100,000 | 43,128 KB | **441.6 B** |
| 1,000,000 | 420,784 KB | **430.9 B** |

두 자릿수 규모에 걸쳐 선형이다. **감독자에게는 무의미한 수치다** (자식 1,000 개면 태스크 메모리 약 430KB). 스케줄러 선택은 메모리가 아니라 **스레드 모델**로 한다.

## 바이너리 크기와 crate 수 (같은 소스, 같은 프로파일)

| 스택 | 바이트 | KiB | tokio 최소 대비 | crate 수 |
|---|---:|---:|---:|---:|
| `std::process` (async 없음) | 209,408 | 204.5 | -60,928 | 1 |
| **tokio `rt,process,io-util`** | **270,336** | **264.0** | . | **8** |
| compio 0.19.1 `runtime,process,io` | 281,600 | 275.0 | +11,264 | 60 |
| tokio `full` | 301,056 | 294.0 | +30,720 | 22 |
| **`async-process` + `futures-lite` (smol)** | **314,880** | **307.5** | **+44,544** | **31** |

**"smol 이 가벼운 쪽" 은 이 워크로드에서 사실이 아니다.** smol 스택이 최소 tokio 보다 44.5KB 크고 crate 를 약 4 배 (31 대 8) 끌어온다. tokio `full` 보다도 크다. smol 의 미니멀함은 executor 층에서는 진짜지만 `async-process` + `futures-lite` 를 붙이는 순간 사라진다. tokio 의 `process` feature 가 사람들 생각보다 훨씬 외과적이기 때문이다.

std/CRT 바닥 204.5 KiB 를 빼면 **런타임 귀속 코드는 tokio 최소 약 59.5 KiB, compio 약 70.5 KiB, tokio full 약 89.5 KiB, smol 약 103 KiB** 다.

## feature 함정 두 개

1. **`features = ["process"]` 만으로 컴파일도 되고 `spawn()` 도 성공하지만, 자식 stdout 의 첫 `poll_read` 에서 런타임 패닉이 난다** (``requires the `rt` Tokio feature flag``). 컴파일 타임에 안 잡히고 런타임에 터진다. **최소 조합은 `["rt","process","io-util"]` 이다.**
2. Windows 에서 `process` 는 `net` **feature** 를 요구하지 않는다. 다만 같은 mio 하위 feature 를 독립적으로 켠다. 그리고 Windows 프로세스 구현은 **mio 를 실제로 쓰지 않는다** (종료 감지는 `RegisterWaitForSingleObject`, 파이프는 `spawn_blocking`). unix 에서는 `process` 만으로 SIGCHLD 시그널 기구와 I/O 드라이버가 켜지므로 **Linux 쪽이 더 무겁다.**

## 기각한 것들

| 후보 | 기각 사유 |
|---|---|
| **async-std** | **RUSTSEC-2025-0052 로 discontinued.** 전 버전 영향, 패치 없음. crates.io 설명이 "Deprecated in favor of smol" |
| **tokio-uring** | 사실상 방치. 최근 2 년간 실질 커밋 1 개 (2025-07-07 clippy 정리) 에 열린 PR 41 개. **Linux 전용이라 애초에 무관** |
| **monoio** | 0.2.4 (2024-08) 에서 정체. Linux 전용 |
| **Windows IoRing** | opcode 9 개 전부 파일 I/O 지향. **프로세스·파이프·소켓 연산이 아예 없다.** Win11 22000+ 필요 (Windows 10, Server 2019/2022 배제). Rust 래퍼 없음. **runtrol 에 아무것도 안 준다** |
| **smol / async-process** | 바이너리가 더 크고 crate 가 4 배. **Windows 에서 tokio 와 똑같이 blocking 스레드 풀을 쓴다** (500 개 상한, 게다가 `BLOCKING_MAX_THREADS` 환경변수로만 조절 가능해서 데몬에 부적합) |

## compio . 언제 다시 볼 것인가

`compio 0.19.1` (2026-06-14, 릴리즈 활발) 은 **Windows 에서 진짜 IOCP** 를 쓴다. 소스 확인: 프로세스 종료는 `GetExitCodeProcess` 를 감싼 `WaitProcess` op 을 `OpType::Event` 로 IOCP 에 등록하고, 자식 stdio 는 진짜 overlapped `Read`/`Write` 다. **파이프당 blocking 스레드가 없다.** 자식 1 개 실측에서 스레드 5 개 고정.

**runtrol 의 워크로드에 아키텍처적으로 우월하다.** 비용은 crate 60 개, pre-1.0 (약 2 개월마다 breaking), 얇은 생태계, completion I/O 의 버퍼 소유권 API.

**판단**: 512 자식 천장이 실제 제약이 되면 그때 간다. 그전에는 churn 이 값어치를 못 한다.

**미검증 항목**: compio 에 대해 128 자식 스레드 스케일링 테스트는 돌리지 않았다 (1 자식만 돌렸다). IOCP 주장은 소스 독해 + 1 자식 실행 근거다. **이 선택이 갈림길이 되면 128 자식 테스트를 먼저 돌린다.**

## 이 숫자들이 게이트가 되는 방식

`idleFootprintRatchet` 의 초기 상한은 위 기준선에서 잡는다.

- 유휴 daemon: current_thread 기준 RSS 약 10MB 근방
- 자식당 증분: 파이프 스트림당 OS 스레드 1 개 + 상주 약 27KB
- **상한은 내려가기만 한다.** 올리려면 운영자 승인
