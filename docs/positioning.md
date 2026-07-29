# 포지셔닝 . 왜 runtrol 이 존재하는가

운영자 결정 (2026-07-30): **"여러 AI를 한 곳에서 관리한다" 가 혁신이자 한 줄 북극성이다.**

## 결정 전에 알아야 했던 것

전문 에이전트 5 인 토론의 가장 중요한 산출은 설계가 아니라 정직한 진단이었다. **"PC 의 코딩 CLI 를 원격에서 다룬다" 만으로는 중복이다.**

| 이미 존재하는 것 | 무엇을 하는가 |
|---|---|
| Anthropic Remote Control (2026-02-24) | 전 요금제 무료. QR 페어링, 세션 목록, 재개, push, `--spawn worktree`, `--capacity 32` |
| OpenAI Codex Remote (Windows 호스트 2026-05-29, GA 06-25) | 같은 것을 Codex 에서 |
| GitHub Copilot CLI · Amp | Amp 는 2026-07-08 부터 원격 새 세션 생성까지 |
| Happy (slopus/happy) 22.9k star | MIT, iOS·Android·web, Claude Code + Codex, E2E, 자체 호스팅 가능 |
| agent-of-empires 2.9k star | 이미 Rust, MIT, PWA, QR 페어링, 10+ CLI 자동 감지 |
| ACP (Agent Client Protocol) v1 | `session/new|list|resume|delete`, Rust SDK, provider manifest 레지스트리 |

**Rust 는 차별점이 아니다.** 이 판의 경쟁자 열 개 이상이 이미 Rust 다.
**"똑똑한 비하드코딩 provider 관리" 도 이미 표준으로 존재한다.** 그래서 ACP 를 채택하고 발명하지 않는다.

상용 레이어는 무덤이다: vibe-kanban (27.5k star, Rust) 2026-04-10 폐업, terragon 폐업, crystal deprecated, omnara v1 deprecated, cui 는 "Anthropic 이 냈다" 며 자진 archive.

## 그래서 고른 자리

**여러 provider 를 하나의 목록에서.**

이것이 유일하게 **구조적인** 갭이다. 나머지 셋 (벤더가 배제한 구성, headless supervisor, tmux 없는 Windows) 은 이 축을 떠받치는 부수 효과이지 첫 문장이 아니다.

왜 구조적인가:

- **Claude 앱에 Codex 세션은 영원히 안 뜬다.** 벤더가 남의 provider 를 자기 앱에 넣을 이유가 없다
- **목록이 넷으로 갈라지는 것은 기능 차이가 아니다.** 각 벤더가 각자 완벽하게 만들수록 사용자의 파편화는 더 심해진다
- **벤더가 이 문제를 고치면 그건 자기 제품을 남에게 여는 것**이므로, 경쟁 구도상 일어나지 않는다

경쟁자 중에 여러 provider 를 다루는 것 (Happy, agent-of-empires) 은 있다. 그들과의 차이는 **한 곳에서** 라는 말의 범위다: PC 앞의 앱과 폰의 PWA 가 같은 세션 모델을 공유하고, 그 사이에 누구의 서버도 없다.

## 첫 문장을 떠받치는 부수 갭 셋

첫 문장은 아니지만 같은 방향으로 힘을 보태는 것들이다. 별도 축으로 승격하지 않고 북극성 표의 기존 축 안에서 처리한다.

| 갭 | 내용 | 어느 축이 흡수하는가 |
|---|---|---|
| **벤더가 배제한 구성** | Bedrock, Vertex, Foundry, LLM gateway, 사내 프록시, API key 인증, ZDR 조직, `DO_NOT_TRACK`. Anthropic Remote Control 은 claude.ai OAuth 를 요구하므로 이 전부에서 켜지지 않는다. Dispatch 는 Pro/Max 전용, Copilot CLI 원격 제어는 org 정책에 걸린다 | `폰에서 내 PC 세션 잇기` 의 "계정 등급이나 인증 방식이 이 경험을 막지 않는다" |
| **진짜 supervisor 를 가진 headless** | Codex 는 데스크톱 앱이 사실상 필수 브리지이고 headless Linux 는 미해결. Claude 는 프로세스가 죽으면 세션이 끝나서 사용자가 tmux 를 직접 써야 한다 | `끊겨도 살아남기` |
| **tmux 없는 네이티브 Windows** | claude-squad 는 Windows 에서 아예 안 뜬다 (`creack/pty` 미지원). tmux 기반 OSS 전반의 공통 약점. 단 벤더 것들은 Windows 를 지원하므로 OSS 경쟁자에게만 유효하다 | `어디서나 같은 방법` |

## 구조적으로 유리한 사실

Anthropic 이 2026-01 과 04 에 **OAuth 토큰 프록시를 금지**했다. 그런데 **공식 바이너리를 spawn 하는 것은 여전히 허용**된다.

즉 "채팅을 가로채지 않는다" 는 취향이 아니라 **유일하게 살아남는 아키텍처**다. 이 제약을 지키는 제품만 계속 존재할 수 있고, 지키지 않은 것들 (OpenCode 계열) 은 하루아침에 끊겼다.

또: **성숙한 Rust 네이티브 세션 매니저 + 모바일 클라이언트는 존재하지 않는다.** 유일한 진지한 시도 vibe-kanban 은 폐업했고 사유는 기술이 아니라 사업모델이었다 ("압도적 다수가 무료 사용자였고 신날 만한 수익 모델을 못 찾았다"). **오픈소스로 수익을 좇지 않는다면 그 사인은 runtrol 에 해당하지 않는다.**

## 접어야 할 조건 (kill criteria)

미리 적어두지 않으면 접어야 할 때 못 접는다.

1. **벤더 중 하나가 남의 provider 세션을 자기 앱 목록에 넣는다.** 구조적 갭이 사라진다
2. **ACP 가 공용 클라이언트를 낳고 그것이 멀티 provider 목록과 모바일을 둘 다 한다.** 우리가 지을 이유가 없다
3. **자식 바이너리 spawn 이 ToS 로 금지된다.** 아키텍처 자체가 불법이 된다
4. **북극성 `즉시 반응` 과 `상주 비용` 이 6 개월 안에 8 점에 못 간다.** 편의가 없으면 존재 이유가 없다
5. **운영자 본인이 3 개월 이상 안 쓴다.** 스스로 안 쓰는 제품은 완성되지 않는다

## 인정하고 시작하는 것

**공급자 하나만 쓰는 사람에게 runtrol 은 필요 없다.** README 에 그렇게 적는다. 그렇게 적을 수 있는 제품만이 나머지 사용자에게 믿음을 얻는다.
