# providerAdapter

상태: 대기 (`positioningDecision` 선행). 설계는 완료, 착수는 운영자 go.

## 한 문장 정의

**runtrol 은 provider 어휘를 발명하지 않는다. ACP (Agent Client Protocol) 를 내부 이벤트 어휘이자 PWA 와이어 모양으로 채택하고, provider 등록은 언제나 TOML manifest 하나로 끝나며, 코어는 `kind` 별 범용 드라이버를 낸다.**

## 최상위 결정

### 1. ACP 를 채택한다. 발명하지 않는다

ACP 는 이미 runtrol 어휘의 약 90% 를 표준화했다: `session/new|load|resume|list|delete|close|prompt|cancel|set_mode|set_config_option`, `session/update`, `session/request_permission`, `fs/*`, `terminal/*`, `elicitation/*`. Rust SDK 가 crates.io 에 있다.

| 대상 | 결정 |
|---|---|
| ACP 메서드 이름과 `SessionUpdate` 모양 | **채택.** 내부 + PWA 어휘 |
| Codex 진입 전송으로서의 ACP | 건너뜀. `app-server` 가 상위집합이다 |
| Claude Code 진입 전송으로서의 ACP | 1급으로는 건너뜀. manifest 전용 대안으로 허용 |
| `agent-client-protocol` Rust crate | **채택.** `kind = "acp"` 범용 드라이버용 |
| Codex `app-server` JSON Schema 생성기 | **채택.** 빌드 시점 + probe 시점 산출물 |
| Claude `system/init.capabilities` 배열 | **채택.** feature detection 채널 |
| vibe-kanban `CodingAgent` enum | **기각. 안티 요구사항이다** |
| vibe-kanban `normalize_logs` | **기각.** 너무 두껍다 (transcript 를 소유한다) |
| `@zed-industries/claude-code-acp` 의존 | **기각.** 세션당 Node 프로세스 하나 |

### 2. 등록은 언제나 manifest. 코어는 절대 안 고친다

> provider 는 TOML manifest 로 등록된다. manifest 는 `kind` 를 지목한다. 코어는 `kind` 별 범용 드라이버를 낸다. `kind` 는 out-of-tree Rust `Provider` 구현을 지목할 수도 있다. **provider 를 추가하려고 코어 코드를 고치는 일은 없다.**

| `kind` | 드라이버 | 추가에 Rust 필요? |
|---|---|---|
| `acp` | stdio 위 범용 ACP 클라이언트 | **아니오. TOML 10 줄** |
| `codex-app-server` | Codex JSON-RPC 드라이버 | 아니오 |
| `claude-stream-json` | Claude ndjson 드라이버 | 아니오 |
| `exec-oneshot` | 턴당 프로세스 하나, ndjson 출력 | 아니오 |
| `pty` | 원시 바이트 (feature gate, 기본 off) | 아니오 |
| `native:<id>` | 형제 crate 의 out-of-tree `Provider` 구현 | 예. 단 **본인 crate 에서** |

ACP 를 말하는 4 번째 CLI 의 등록 파일 전체:

```toml
schema = 1
id = "opencode"
display_name = "OpenCode"
kind = "acp"
[bin]
names = ["opencode"]
[transport]
argv = ["acp"]
```

**순수 선언형만으로 안 되는 이유**: TOML DSL 은 stream framing, session-id 추출, interrupt 의미, 승인 왕복을 표현하려면 결국 프로그래밍 언어가 된다 (vibe-kanban `profile.rs` 가 간 길).
**순수 trait 만으로 안 되는 이유**: 이미 ACP 를 말하는 CLI 하나 붙이려고 crate 와 재컴파일을 요구하는 것은 2026 년에 말이 안 된다.

### 3. manifest 는 작고 정직하다 (lint 강제)

**런타임에 probe 가능한 사실은 manifest 에 있으면 안 된다.** `runtrol provider lint <file.toml>` 이 probe 를 돌려서 manifest 키가 probe 결과를 되풀이하면 실패시킨다.

허용 키는 `schema` · `id` · `display_name` · `kind` · `[bin]` · `[probe]` · `[transport]` · `[models].aliases` (**토큰만. 모델 id 절대 금지**) · `[update].hint` · `[fallback]` 뿐이다. capability 목록, 모델 id, 플래그 표, 이벤트 매핑은 **없다.** 그건 발견하거나 드라이버에 컴파일된다.

### 4. PTY 는 v1 에 없다

VT 에뮬레이터 비용이 들고, 화면 긁기를 강제하며, **Codex 는 `process/spawn` 과 `process/resizePty` 를 자기가 이미 내준다.** feature gate 로 꺼둔다.

### 4.5. 완료 신호의 출처는 provider 마다 다르다 (실측 확인)

정규화 모델이 **1 급으로** 다뤄야 하는 비대칭이다. 이벤트 어휘가 아니라 **턴이 끝났다는 것을 어디서 아는가**가 다르다.

| | Codex | Claude |
|---|---|---|
| 전송 | 데몬 하나가 모든 thread 다중화 | 세션당 프로세스 하나 |
| 턴 시작 | 요청 -> **2 ms ack** (`status: inProgress`) | stdin 한 줄 |
| 턴 이벤트 | 데몬 알림 스트림 (thread id 로 구분) | 그 프로세스의 stdout |
| **턴 완료** | **`turn/completed` 알림** | **`result` 이벤트** |
| 세션 파일 경로 | **응답이 직접 준다** | **uuid 로 검색해야 한다** |

`turn/start` 가 fire-and-forget 이라는 것을 모르고 probe 를 짰다가 **8 초짜리 턴을 0.01 초에 "끝났다" 고 읽었다.** 드라이버가 같은 실수를 하면 세션이 영원히 진행 중이거나 즉시 완료로 보인다. `Agent` trait 은 완료를 **provider 가 선언**하게 하고 코어가 추론하지 않는다.

부수 소득: **`account/rateLimits/updated` 가 턴마다 공짜로 온다.** `desktopGui` 의 "사용량과 한도가 보인다" 편의가 추가 호출 0 으로 성립한다.
주의: `mcpServer/startupStatus/updated` 가 6 회 오는 등 **알림 전부가 사용자 대면이 아니다.** 소음과 신호를 가리는 것이 바인딩 목록 규율의 일부다.

### 5. 스키마 drift 에 대한 구조적 답

정규화 모델은 ACP 모양이되 `payload: Box<RawValue>` 를 통과시키고, **매핑되지 않은 것을 버리지 않는 `Unmapped` variant** 를 둔다. `AgentCommand::Raw` 도 같은 이유다: PWA 가 runtrol 이 들어본 적 없는 provider 고유 기능을 몰아도 runtrol 은 파이프로 남는다.

**이것이 omnara v1 의 사인에 대한 직접 대응이다.** 그들의 말: "Claude Code CLI 를 감싸는 래퍼로 지었는데, CLI 가 끊임없이 바뀌어서 유지가 불가능해졌다."

### 6. 세션 식별자 . runtrol 이 발급하되 소유하지 않는다

Claude 는 `--session-id` 로 runtrol 이 UUID 를 **발급**한다. 그래서 `native_id == runtrol_id` 다. 저장하는 것은 **세션당 약 200 바이트** (라벨, 핀, 커서) 이고 **transcript 는 0** 이다.

**수용 테스트**: `rm -rf $RUNTROL_HOME` 을 해도 잃는 것은 라벨과 핀뿐이어야 한다. 세션 자체는 `claude --resume` 과 `codex resume` 으로 그대로 열려야 한다. 이것이 북극성 `떠날 자유` 축이다.

Claude 세션 삭제는 unlink 가 아니라 **7 일 휴지통으로 rename** 한다.

### 7. 코어와 provider 의 소유 경계

| 코어가 소유 | provider 가 소유 |
|---|---|
| runtrol 세션 id (UUIDv7), 라벨, 핀 | native 세션 id |
| manifest 로더와 `kind` -> 드라이버 표 | 자기 seam 에 닿는 방법 |
| probe 캐시, TTL, 무효화, 파일 감시 | 무엇을 probe 하고 어떻게 파싱하나 |
| 프로세스 감독: job object, process group, reaper | 자기 자식을 올바로 spawn |
| `AgentEvent` enum 과 시퀀스 번호 | 자기 와이어 메시지를 거기로 매핑 |
| 업데이트 정책: 언제, 락, 롤백 판단 | 자기 채널의 업데이트 기법 |
| PWA 전송, 인증, fan-out, 재연결 링버퍼 | PWA 에 대해 아무것도 |
| backpressure 와 세션당 예산 | 없음 |

### 8. 소비 표면 최소화 . 드리프트 위험은 소비한 표면에 비례한다

이 카테고리의 1 번 사인은 CLI 드리프트다 (omnara, agentapi #207). 드리프트에 노출되는 면적은 **우리가 바인딩한 메서드 수**이지 벤더가 내주는 메서드 수가 아니다. Codex app-server 는 요청 126 개를 내주지만 runtrol 이 묶이는 것은 10 여 개다.

- **바인딩 목록은 한 파일에 명시한다.** Codex (실측 확인분): `initialize` · `thread/list` · `thread/start` · `thread/resume` · `thread/archive` · `thread/delete` · `turn/start` · `turn/interrupt` · `model/list` · `account/read` · `experimentalFeature/list` + 승인 서버요청 3 종 (`item/commandExecution/requestApproval` · `item/fileChange/requestApproval` · `item/permissions/requestApproval`) + 소비하는 알림 (delta · completed · tokenUsage · `rate_limit` · error 계열)
- 그 밖의 모든 메시지는 `Unmapped` / `Raw` 로 통과시킨다. 새 기능이 필요해질 때만 목록에 한 줄 늘린다
- **`agentSurfaceDrift` 게이트도 이 목록만 경보한다.** 벤더가 우리가 안 쓰는 메서드를 늘리는 것은 정보이지 red 가 아니다. 안 그러면 벤더 릴리즈마다 소음 red 가 나고, 소음 red 는 게이트를 죽인다

## 실물 검증 (2026-07-30, `tests/_attempts/providerProbe/`)

설계를 문서로만 두지 않고 두 CLI 에 **직접 붙여서** 확인했다. 미해결이던 급소가 풀렸다.

### 급소 해소 . Claude 는 API key 없이 돈다

| 질문 | 결과 |
|---|---|
| ACP 의 Claude 어댑터가 Agent SDK 를 경유해 API key 를 요구하는데, "이미 인증된 구독 세션" 과 충돌하는가 | **충돌하지 않는다.** 어댑터를 안 쓰면 된다 |
| `claude -p --output-format stream-json` 이 구독 인증만으로 도는가 | **된다.** `system/init` 이 `apiKeySource: none` 을 보고했다 |

`ANTHROPIC_API_KEY` 를 제거한 환경에서 성공했다. **공식 바이너리 spawn 경로는 살아있다.**

### 확정된 계약

| 설계 주장 | 실측 |
|---|---|
| runtrol 이 세션 id 를 발급한다 (`native_id == runtrol_id`) | **확인.** `--session-id` 가 그대로 파일명이자 `result.session_id` |
| `system/init.capabilities` 가 feature detection 채널이다 | **확인.** `tools` · `skills` · `agents` · `mcp_servers` · `plugins` · `capabilities` · `claude_code_version` 전부 온다. 버전 문자열 추론 불필요 |
| Codex 세션 목록을 CLI 호출 없이 얻는다 | **확인.** `thread/list` **607 ms** 에 jsonl 경로 · cwd · git branch · cliVersion · preview · name 까지 |
| Codex 모델은 완전 발견 가능하다 | **확인.** `model/list` **16 ms**, 7 개, `displayName`·`description`·`supportedReasoningEfforts` 동반 |
| 구독 인증을 알 수 있다 | **확인.** `account/read` 54 ms -> `{"type":"chatgpt","planType":"pro"}` |
| 프로토콜 규모 | **확인.** ClientRequest 126 · ServerNotification 70 · ServerRequest 11 |

### 발견 사다리가 옳았음이 실증됐다

Claude 세션 파일 경로 규칙을 **첫 시도에 틀리게 추측했다.**

```
내 예상: C-Users-MSI-...-tests-_attempts-providerProbe
실제:    C--Users-MSI-...-tests--attempts-providerProbe
```

실제 규칙은 `:` `\` `/` `_` `.` 를 **전부** `-` 로 치환이다. 설계가 "경로를 계산하지 말고 uuid 로 검색하라" 고 한 것이 맞았다. **하드코딩했으면 첫날부터 틀렸다.**

같은 계열: Codex 응답 배열 키가 `items` 가 아니라 **`data`** 다. 이것도 추측이 틀렸고 스키마를 읽어 고쳤다.

## 실측 근거 (2026-07-29~30, 이 기계)

- `codex app-server generate-json-schema --experimental` 실행 확인: `ClientRequest` 126 개, `ServerNotification` 70 개, `ServerRequest` (server to client) 11 개
- Claude stream-json 실측 확인. `system/init.capabilities[]` 존재 확인
- **Claude 모델 목록은 CLI 에서 열거 불가**로 확인됐다 (잘못된 모델을 주면 목록 없이 맨 404). 정직한 대응은 별칭 토큰 넷을 `~/.claude.json` 의 `additionalModelOptionsCache` 뒤에 병합하는 것이고, 이 한계를 UI 에 `Catalog::Unknown` 으로 그대로 표시한다. **추측하지 않는다.**
- Codex 모델은 완전 발견 가능 (`model/list` + `~/.codex/models_cache.json`)

## 완료 판정

- `providerContract` 게이트: 모든 어댑터가 같은 trait 계약을 통과하고, **코어에 provider 고유명사 분기가 없다**는 정적 검사 포함
- `agentSurfaceDrift` 게이트: 최신 CLI 를 받아 생성 스키마와 저장 스키마를 대조. 공급자가 표면을 바꾸면 사용자보다 먼저 red
- ACP 를 말하는 3 번째 provider 를 TOML 하나로 붙이는 것이 실측으로 증명됨
- `rm -rf $RUNTROL_HOME` 수용 테스트 통과

## 운영자 결정 대기 3 건

1. Claude 승인 (permission prompt) 을 Codex 와 동등하게 다룰 것인가, 아니면 Claude 는 승인 전달을 v2 로 미룰 것인가
2. PWA 와이어를 raw ACP 로 낼 것인가, runtrol 봉투로 감쌀 것인가
3. 세션 삭제의 의미. 휴지통 7 일이 맞는가, 아니면 CLI 에 위임하고 runtrol 은 목록에서만 숨길 것인가

## 원본

`.claude/discussion/r1/providerAdapter.md` (1014 줄. trait 전문, 이벤트 매핑표, probe 캐시 설계, 자동 업데이트 롤백 절차, 실패 모드 7 종, 검증된 명령 인벤토리)
