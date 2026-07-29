# providerDiscovery

상태: 대기 (`providerAdapter` 선행).

## 한 문장 정의

**runtrol 은 provider 의 버전·모델·플래그·세션 경로를 하드코딩으로 알지 않고 런타임 발견으로 안다.** 이 이니셔티브는 그 발견 사다리, version-keyed 캐시, CLI 자동 업데이트, 그리고 표면 drift 감지를 하나로 묶는다. 넷은 같은 뿌리다.

## 왜 이게 생존 문제인가

**omnara v1 의 사인이 곧 runtrol 의 최대 위험이다.** 그들 자신의 말:

> Claude Code CLI 를 감싸는 래퍼로 지었는데, CLI 가 끊임없이 바뀌어서 유지가 불가능해졌다.

같은 패턴이 이 카테고리 전체에서 반복된다.

| 사례 | 무엇이 깨졌나 |
|---|---|
| agentapi #207 | Claude Code 2.1.83 패치 릴리즈가 터미널 캡처를 깼다. 첫 캡처만 되고 이후 갱신이 안 잡힌다. 2.1.78 로 롤백하니 동작 |
| claude-code-router #1601 | Claude Code 2.1.220 에서 keychain service 에 접미사가 붙어 로그인 임포트가 **조용히** 실패 |
| happy #1543 | `@anthropic-ai/claude-code` 2.1.x 가 `cli.js` 를 더는 안 실어서 "Claude Code 가 설치되지 않았다" 고 오탐 |
| async-code #33 | PyGithub 상위 API 제거 |

**핵심 관찰**: 화면 긁는 것은 **조용히 부분적으로** 실패하고, 프로토콜 클라이언트는 **시끄럽게** 실패한다. runtrol 이 PTY 를 v1 에서 빼는 이유가 이것이다.

## 발견 사다리 (위에서부터)

1. **CLI 가 스스로 뱉는 기계 판독 계약.** `codex app-server generate-json-schema` 가 프로토콜 스키마를 직접 출력한다. 이런 표면이 있으면 무조건 이것.
2. **구조화된 런타임 응답.** app-server initialize 응답, Claude `system/init.capabilities[]`. **버전 문자열로 feature 를 추론하지 않는다.**
3. **`--help` 파싱.** 플래그 존재 여부. 대화가 아니라 **도구 자기기술**이라 얇음 원칙을 안 깬다.
4. **CLI 자신의 설정 파일 (읽기 전용).** `~/.codex/config.toml`, `~/.claude.json`, `~/.codex/models_cache.json`. **절대 쓰지 않는다** (claude-code-router #1575 가 사용자 동의 없이 `~/.claude/settings.json` 을 덮어써서 커스텀 statusLine 을 매번 날린 사고가 있다).
5. **provider manifest.** 위 넷으로 정말 알 수 없는 것만. 항목마다 "왜 발견 불가인가" 를 남긴다. manifest 가 커지면 설계가 지고 있다는 신호다.

## 정직한 발견 가능성 판정 (실측)

| 항목 | Codex | Claude Code |
|---|---|---|
| 설치 여부 | 발견 가능 | 발견 가능 |
| 버전 | 발견 가능 | 발견 가능 |
| **모델 목록** | **완전 발견 가능** (`model/list` + `~/.codex/models_cache.json`) | **열거 불가.** 잘못된 모델을 주면 목록 없이 맨 404 |
| 현재 모델 | 발견 가능 | 부분 |
| 지원 플래그 | 스키마에서 | `capabilities[]` + `--help` |

Claude 모델의 정직한 대응: 별칭 토큰을 `~/.claude.json` 의 `additionalModelOptionsCache` 뒤에 병합하고, **UI 에 `Catalog::Unknown` 을 그대로 표시한다. 추측해서 채우지 않는다.**

## 캐시 규약

키 = `(providerId, 실행파일 경로, mtime, size, 버전 문자열)`. 하나라도 바뀌면 무효.
**시간 기반 TTL 만으로 캐시하지 않는다.** 업데이트 직후 낡은 모델 목록을 보여주는 것이 정확히 이 제품이 피해야 할 실패다.
무효화는 stat + 벤더 자신의 캐시 파일에 대한 `notify` 감시.

## 자동 업데이트

1. **채널을 감지한다, 가정하지 않는다** (npm global · `codex update` · 네이티브 설치기)
2. 업데이트 실행기를 돌리지 않고 가용성만 확인한다
3. **돌고 있는 세션을 깨지 않는다.** 업데이트는 세션 경계에서만
4. 업데이트 후 **스모크를 돌리고** 통과해야 확정, 실패면 **롤백**
5. 롤백 경로가 없으면 업데이트하지 않는다

## drift 게이트 (`agentSurfaceDrift`)

CI 가 주기적으로 최신 CLI 를 받아 생성 스키마와 저장 스키마를 대조한다.
**공급자가 표면을 바꾸면 사용자보다 먼저 red 가 된다.** 이것이 omnara 의 사인에 대한 유일한 구조적 방어다.

## 완료 판정

- `modelDetectionSmoke`: 실물 CLI 에서 모델 목록을 얻고, **소스에 모델 이름 리터럴이 없다**는 정적 검사 포함
- `cliUpdateRehearsal`: 구버전 설치 -> 업데이트 -> 세션 정상 -> 고의로 깨진 버전 -> 자동 롤백
- `agentSurfaceDrift` 가 CI 스케줄로 돈다
- manifest lint 가 probe 가능한 사실의 manifest 등재를 거부한다

## 원본

`.claude/discussion/r1/providerAdapter.md` 4 장 (capability discovery), 6 장 (auto-update), 7 장 (failure modes)
