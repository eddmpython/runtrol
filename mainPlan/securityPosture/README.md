# securityPosture

Status: in progress. Scope grantability, default-deny dispatch, argument escaping, read-only provider configuration, browser rebinding defenses, exact egress allowlisting, the end-to-end Noise boundary, exact PC-bound pairing approval, risk-bound remote approval authorization, and both provider-native approval mappings are implemented and verified. The paid Claude live approval smoke and the phone surface remain.

## 한 문장 정의

**runtrol 의 존재 목적은 전화기가 인터넷 너머에서 개인 PC 의 코드를 읽고 쓰고 실행하게 하는 것이다.** 취미 개발자가 설치할 수 있는 것 중 가장 위험한 로컬 데몬 축에 든다. 그래서 권한 모델은 기능이 아니라 **첫 커밋의 골격**이다.

이 저장소는 공개된다. 기준선은 **"모르는 사람이 clone 해서 기본값으로 켠다"** 이다.

## 이 카테고리에서 이미 터진 것들 (상상한 위협이 아니다)

| 사고 | 무엇이 일어났나 | runtrol 이 배우는 것 |
|---|---|---|
| Ollama CVE-2024-28224, MCP SDK CVE-2025-66414 / 66416, Playwright MCP CVE-2025-53034 | 웹 페이지가 DNS rebinding 으로 로컬 데몬을 탈취 | Host 검사가 rebinding 을 막는 유일한 수단이다 |
| CVE-2025-52882 (WebSocket 변종) | same-origin policy 가 **아예 적용되지 않는다** | WS 는 101 이전에 인증해야 한다 |
| **`claude-code-ui` CVE-2026-31975** | `.env.example` 에 없는 하드코딩 fallback JWT 시크릿 + 사용자 존재를 검사 안 하는 WS 인증 + bash 문자열 보간. **기본 설치 전부가 미인증 RCE** | 공개 저장소의 기본값이 곧 사용자의 보안이다 |
| **Happy #1503** (미해결) | 페어링 공개키를 검증 없이 수용. 악의적 릴레이가 모든 세션 DEK 를 얻고, 유효한 AEAD 로 프롬프트를 위조해 **개발 머신에서 RCE**. E2E 주장을 무너뜨린다 | 페어링은 **양쪽에서 키를 표시하고 사람이 확인**해야 한다 |
| **Happy #1514** | 원격 제어 도구인데 `--dangerously-skip-permissions` 가 **기본 권한 모드** | 위험 모드는 원격에서 구조적으로 켤 수 없어야 한다 |
| **claude-code-router #1575 / #1577 / #1602** | 사용자 동의 없이 `~/.claude/settings.json` 을 덮어씀. 커스텀 statusLine 이 매 세션 시작마다 지워짐 | **남의 설정 파일을 읽되 절대 쓰지 않는다** |
| **container-use #337** (미해결, 사실상 유지보수 중단) | `environment_file_write` 에 `../../../.bashrc` 를 주면 worktree 밖 호스트 파일시스템에 쓴다. 샌드박스 탈출 | 경로 경계는 테스트여야지 주석이면 안 된다 |
| **backlog.md #810** | 웹 UI 가 인증 없이 `0.0.0.0` 바인딩. LAN 전체에 데이터 노출 | 기본 바인딩은 절대 `0.0.0.0` 이 아니다 |
| s1ngularity | npm 침해 후 **로컬에 설치된 코딩 CLI 자체를 무기화** | provider CLI 는 신뢰 경계 밖이다 |
| **BatBadBut CVE-2024-24576 (CVSS 10.0)** | Rust 표준 라이브러리의 `.cmd` 실행 인자 이스케이프. **npm 이 Windows 에 `claude.cmd` · `codex.cmd` 를 깐다** | runtrol 이 Rust 이고 Windows 우선이라 **정확히 해당한다.** 놓치기 쉬운데 바로 걸린다 |

## 최상위 결정

### 1. 소켓 표면 . OS 별로 다르게

| OS | 결정 | 이유 |
|---|---|---|
| **Windows** | **named pipe** | DACL 명시 (소유자 SID 만, `S-1-5-2` NETWORK DENY), `PIPE_REJECT_REMOTE_CLIENTS`, peer 신원은 `GetNamedPipeClientProcessId` + 토큰 SID. **Windows 의 AF_UNIX 는 기각** (peer credential API 없음, `SCM_*` 없음, ACL 의미가 미명세. Envoy #11354 가 아직 열려 있다) |
| **Linux / macOS** | **Unix domain socket** | `$XDG_RUNTIME_DIR/runtrol/`, 디렉토리 0700, `bind` 주위 umask 로 소켓 0600, peer uid 는 `SO_PEERCRED` / `LOCAL_PEERCRED` |
| **폰 대면 평면만** | loopback TCP | 그리고 그때는 전부 켠다 (아래) |

Windows 의 loopback TCP 에는 **OS ACL 이 없다.** 모든 사용자 세션의 모든 프로세스가 붙을 수 있다. 이것이 OS 별로 갈라야 하는 이유다.

폰 대면 평면의 필수 스택: Host allowlist · Origin 기본 거부 · `Sec-Fetch-Site` · `Authorization` 헤더 bearer · **쿠키 절대 금지** · CORS wildcard 금지 · WS 는 101 이전 인증.

**`Sec-Fetch-Site` 는 CSRF 를 막지만 rebinding 은 못 막는다. rebinding 을 막는 것은 Host 검사다.** Chrome 142 LNA 는 도움이 되지만 WebSocket 을 게이트하지 않으므로 여기에 하중을 싣지 않는다.

### 2. 전화기는 권한을 줄일 수는 있어도 늘릴 수 없다

**구조적으로 부여 불가능한 스코프 셋** (런타임 검사가 아니라 **타입 시스템**으로 막는다):

- `device.pair` . 새 기기 페어링
- `config.write` . 설정 쓰기
- `approval.auto` . 자동 승인

**PC 앞 물리 행동을 요구하는 것** (runtrol 이 소유한 창에서 무작위 단어를 타이핑, 60 초 타임아웃):

- workspace 루트 추가
- provider 추가
- `approval.respond.high` (고위험 승인 권한)
- `session.delete`
- `mode.dangerous` (workspace 단위, 최대 8 시간 TTL, **git 트리 청결 필수**)

**승인 만료 = 거부.** 적대적 릴레이가 할 수 있는 최악은 "거부" 여야 한다.

### 3. 모바일 자동 승인은 영원히 없다

승인을 폰으로 전달하는 순간 **6 인치 화면이 보안 경계가 된다.** 그래서:

- 프롬프트에 무엇이 보여야 동의가 유의미한지 규정한다 (전체 명령, 대상 경로, 세션 라벨, workspace)
- **ANSI / bidi override 스푸핑**을 막는다 (표시되는 명령이 실행될 명령과 달라지는 공격)
- 두 개가 동시에 도착했을 때 **엉뚱한 프롬프트를 승인하는 혼동 공격**을 막는다 (요청 id 결박, 응답에 요청 해시 포함)
- 타임아웃은 거부다

### 3.1. 내용을 모르면 거부만 가능하다 (실측에서 나온 규칙)

**Codex 의 `item/fileChange/requestApproval` 은 diff 를 싣지 않는다.** 무엇을 바꾸는지 별도 조회로 이어붙여야 한다.

**그 조회가 비면 그 승인은 거부만 가능하게 만든다.** 허용 버튼을 아예 내지 않는다.

**이름 없는 행위에 대한 동의는 동의가 아니다.** "무언가를 수정합니다" 에 허용을 누르게 하는 것은 승인 UI 가 아니라 승인 연극이고, 그건 보안을 낮추면서 보안을 낮췄다는 사실까지 숨긴다. 사용자 편의 최상위 원칙과도 충돌하지 않는다: 알 수 없는 것을 승인하게 만드는 것은 편의가 아니다.

### 3.2. Claude 승인 경로는 문서에 없는 플래그다

`claude --permission-prompt-tool <tool>` 가 `control_request{subtype:"can_use_tool"}` 을 stdio 로 보낸다. 이것으로 Codex 와 동등한 승인이 가능하다 (근거와 대조 실험은 [providerAdapter](../providerAdapter/README.md) 참조).

**보안 관점의 함의**: 승인 경로가 **문서화되지 않은 표면**에 걸려 있다. 벤더가 조용히 없애면 승인 전달이 죽는데, 그때 **조용히 권한 모드로 강등되면 안 된다.** 강등은 반드시 사용자에게 보이고 (`Notice{TierDowngraded}`), 강등된 상태에서 원격이 위험 작업을 시작할 수 없다. 즉 **기능이 사라질 때 열리는 방향이 아니라 닫히는 방향으로 실패한다.**

### 4. 비밀

**runtrol 은 모델 API 키를 필요로 하지 않는다.** 자식 CLI 가 자기 인증을 소유한다. 이것을 불변식으로 못박고 기계로 강제한다.

정직한 한계: **CLI 출력 자체가 비밀을 담을 수 있다** (에이전트가 `.env` 를 읽으면 그 내용이 폰으로, 그리고 릴레이를 지난다). 완전한 편집(redaction) 은 달성 불가능하다. 그래서 **정직한 보장은 "runtrol 은 릴레이가 읽을 수 없게 한다" 이지 "비밀이 흐르지 않는다" 가 아니다.** README 에 그렇게 적는다.

### 5. 킬 스위치

폰에서의 패닉 (전 세션 kill) 은 **권한 없이 항상 가능**하다. 해제는 PC 에서만.
에이전트가 이미 쓴 파일은 되돌릴 수 없다. 제품이 그것을 정직하게 말하고, 그래서 위험 모드의 전제로 git 청결을 요구한다.

## 완료 판정

- 스코프 셋이 타입으로 표현되고, 부여 불가 스코프가 **컴파일 에러**임을 테스트가 단언
- `egressContract` 게이트: allowlist 밖 목적지로 소켓이 안 열리고, relay 에는 Noise ciphertext 만 보임
- rebinding / CSRF 방어가 테스트다 (주석이 아니라)
- BatBadBut 인자 이스케이프 게이트
- 기본값 표가 `SECURITY.md` 로 승격됨

## 운영자 결정 대기 3 건

원본 문서 말미 참조. 핵심은 **침해된 폰의 blast radius 를 어디까지 허용하는가**이다.

## 원본

`.claude/discussion/r1/securityModel.md` (약 12,500 단어. 위협 행위자 9 종별 영향·가능성·완화, 스코프 셋 전문, 승인 UI 요건, 감사 로그 설계, 비협상 기본값 표, CVE 인용)
