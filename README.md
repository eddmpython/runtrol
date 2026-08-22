# runtrol

**한 개의 VS Code 창에서 모든 프로젝트, 세션, 에이전트를 즉시 운영한다.**

한국어 | [English](README_EN.md) | [中文](README_ZH.md) | [日本語](README_JA.md)

> 상태: **코어와 주력 VS Code 확장을 구현했고 `Runtrol Studio 0.1.9`를 6개 네이티브 플랫폼 대상으로 공개했다.** 실시간 세션 인덱스, 실물 Extension Host와 초당 3,000 프레임 Webview 성능 ratchet, 설치된 실물 CLI의 전체 조작 여정, 깨끗한 Marketplace 설치, 활성 세션을 보존하는 VSIX 갱신과 롤백을 검증했다. 독립 데스크톱 GUI 코드와 실행 경로는 제거됐고 PC 표면은 VS Code 확장 하나다. 공개 Runtime 프로토콜, Rust와 TypeScript SDK, 외부 패키지 소비 게이트, 서명된 standalone Runtime 6개 대상 배포 파이프라인도 구현했다. 확증된 provider 채널의 자동 갱신, 배타 실행, 정확한 롤백도 구현했다. 로컬 Mission DAG, 결정적 Receipt, 수동 통합, 프로젝트 Capability의 명시적 재사용과 변조 롤백도 두 설치형 CLI의 헤드리스 여정으로 검증했다. 같은 검토 지시를 2개에서 4개 격리 worktree와 provider 세션에 한 번에 보내고, VS Code 그리드와 native diff로 비교한 뒤 통과 결과 하나만 선택해 최종 검증하는 Fleet Compare도 실물 두 CLI gate와 실제 Extension Host 눈검수로 검증했다. 일반 Mission은 `Continue Reviewed Mission` 한 동작으로 현재 안전한 파동을 시작하고, 완료 Task의 고정 Gate를 봉인하고, 다음 DAG 파동을 준비해 정확한 지시를 보낸다. 실물 CLI와 Extension Host의 2단계 여정이 이를 검증했다. [Marketplace 확장](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio)과 [GitHub Pages 사이트](https://eddmpython.github.io/runtrol/)가 공개되어 있다. 릴레이 기반 휴대폰 PWA, VS Code 페어링, 세부 권한 편집, 내용 없는 Web Push, Mission 조회와 중단 표면을 구현했다. 대화 헤더의 칩으로 답하는 모델과 permission mode 를 대화 중에 바꾸며, 선택은 각 CLI 자신의 전환 표면으로 중계되고 칩은 서비스가 답한 값만 보여 준다 (설치된 실물 Claude Code 여정 게이트가 전환과 복귀를 CLI 자체 공지로 검증). 대화 목록의 프로젝트 헤딩은 운영자가 직접 만들며, 폴더가 자동으로 헤딩이 되지 않는다. 페어링된 기기의 권한은 승인한 workspace root 로 한정된다: 세션 목록과 감시, 모든 세션 명령, Mission 조회가 같은 live root 검증을 통과해야 하고 root 회수는 즉시 효력을 갖는다. provider 준비는 provider 별 lane 으로 병렬화되어 cold 첫 만남 5 개가 직렬 18.1초 대신 8.7초에 끝나고, 새로 연 폴더의 기존 대화는 새로고침 없이 도착한다. 대화는 저마다 에디터 탭으로 열려 여러 대화를 동시에 띄우고 화면을 분할하며, 입력과 중단은 그 탭의 대화로 간다. 확장 갱신 뒤에도 돌고 있던 옛 코어가 유휴가 되는 즉시 새 빌드로 스스로 굴려져, 업데이트가 재시작 없이 실제로 도달한다. PWA 모듈이 production 데몬과 실물 CLI를 관통하는 세션, 승인, 원격 단절 복원 여정도 활성 게이트로 검증한다. 프로젝트 헤딩의 클릭 한 번으로 Agent Tools를 켜면 설치된 코딩 에이전트가 공개 Runtime의 루트 제한 7개 도구로 다른 작업을 위임할 수 있고, 끄면 provider 등록, Runtime 권한, 보호된 로컬 자격이 함께 회수된다. iOS 실기기 설치와 Web Push 운영 확인은 미검증 기여자 operator evidence로 남으며 현재 완료 범위와 점수에서 제외한다.
> 아래 점수 대부분이 0 인 것은 코드가 없어서가 아니라 그 축을 단언하는 게이트가 아직 없어서다.
>
> `Continue Ready Missions`는 최대 8개의 검토된 일반 Mission을 정확한 digest와 함께 한 번 확인하고,
> 여러 프로젝트의 현재 안전한 파동을 함께 진행한다. 실제 Extension Host에서 두 Git 프로젝트를 한 동작으로
> 동시에 시작하고 다음 한 동작으로 둘 다 `integrating`까지 보냈다.
>
> `Review and Apply Mission Landing`은 일반 Mission의 모든 통과 Receipt Artifact를 현재 프로젝트와 한 개의
> VS Code native multi-diff에서 검토한다. `Apply, run Gates and complete` 한 동작이 Mission, Receipt, 원본,
> 대상, 링크, 미저장 편집기를 다시 확인하고 검증된 임시 파일의 원자적 교체로 기존 파일과 새 파일에 정확한
> 바이트를 적용한다. Core는 고정 Gate 전후의 Artifact를 Receipt와 비교한다. 실제 Extension Host에서 공개
> 적용 버튼을 선택해 네 Artifact를 적용하고 봉인 후 원본 변조, 대상과 Receipt drift, 미저장 편집기, 링크
> 교체, Gate의 파일 변경을 각각 거부한 뒤 복구와 재시도 완료까지 확인했다.
>
> Fleet Compare는 이제 비교에서 멈추지 않는다. 통과 Task 하나를 선택하면 그 Task 이름과 Receipt만 담은 native
> winner multi-diff가 열리고, 공개 적용 버튼 한 번이 다른 후보를 섞지 않은 채 정확한 바이트를 적용하고 고정 Gate와
> Core 완료까지 잇는다. Core는 완료 뒤에도 선택한 Task와 Receipt를 영구 증거로 남겨 응답 유실 복구가 다른 후보를
> 성공으로 오인하지 못한다. 두 실제 CLI의 서로 다른 결과 중 `attempt-2`만 프로젝트에 적용해 `completed`가 되는
> 실제 Extension Host 여정과 화면을 검증했다.
>
> `Mission Auto Flight`는 검토된 일반 Mission을 PC에서 한 번 무장하면, 실제 provider 턴의 세대 변화를
> 증명한 뒤 고정 Gate와 Receipt를 봉인하고 다음 안전한 DAG 파동을 자동으로 시작한다. 사람과 quota 대기,
> 일시정지는 그대로 기다리고, 권한 변화, 모호한 전송, 복구 상태에서는 즉시 해제된다. 두 파동 실물 CLI
> 여정이 운영자 `Continue` 0회로 `integrating` 도착과 자동 권한 회수를 검증했고 세 화면을 직접 눈검수했다.
> 최종 Receipt Landing과 통합은 항상 명시적이다.
>
> `Schedule Reviewed Mission`은 검토된 Mission, 정책, Task 지시문, workspace mode, runtime에서 발견된
> provider 배정과 정확한 시작 시각을 Core에 한 번 고정한다. Studio를 닫아도 Core가 예약을 보존하고,
> 시각이 되면 기존 공개 Mission과 세션 경계만으로 첫 파동을 시작한다. 예약 교체와 취소는 현재 schedule ID를
> 다시 확인하며, 모호한 Send는 반복하지 않고 attention으로 멈춘다. 실제 VS Code 1.132.1의 첫 Host를 예약
> 시각 전에 닫고 UI가 없는 구간에 실제 Claude Code 세션이 시작된 뒤, 두 번째 Host가 `started` 상태와 실제
> 응답에 재접속하는 1456 x 908 여정을 검증했다.
>
> 폰 알림은 이제 대화 내용을 싣지 않은 채 실제로 운영자를 기다리는 첫 세션으로 직행한다. `Needs you`
> 수와 다음 대기 세션 이동은 사람 대기만 포함하고 계정 한도 대기는 구분한다. 실물 CLI 승인 게이트가
> 승인 중 진입과 답변 뒤 해제를 검증한다.
>
> Auto Flight의 사람 대기, 안전 중단, Receipt Landing도 같은 내용 없는 알림으로 도착한다. 폰은 인증 후
> Core의 최대 64개 구조 신호를 읽고 현재 root, Mission digest, 상태가 그대로인 정확한 세션 또는 Mission만
> 연다. 푸시에는 Mission ID, 지시, 경로, 출력이 없고 폰에는 opaque cursor만 남는다.

보안 경계와 기본 거부 설정은 [SECURITY.md](SECURITY.md)에 정리되어 있다.

## 북극성

**runtrol은 한 개의 VS Code 창을 모든 프로젝트, 지원되는 설치형 코딩 에이전트 CLI,
provider 소유 세션의 control plane으로 만든다. 각 에이전트는 결박된 저장소를 자율적으로 변경한다.
runtrol은 세션을 살려 두고 동시 작업을 격리하며, 대화 본문을 해석하지 않고 선택한 세션을 정확한
workspace 또는 worktree에 연결한다. 세션과 에이전트가 늘어도 renderer, 활성 subscription,
Code-hot workspace는 bounded 상태를 유지한다. streaming과 background 작업은 입력, 스크롤,
세션 전환, 파일 탐색을 버벅이게 해서는 안 된다. 설치된 CLI, 모델, capability는 런타임에 자동 발견한다.
대화는 사용자의 PC와 provider 사이에서만 오가며 runtrol은 그 사이에 끼어들지 않는다.**

### 변하지 않는 핵심

- **기능과 속도는 하나의 계약이다.** 기능이 많아져도 기다림과 버벅임은 허용하지 않는다. 보이는 지연, frame drop, 입력 지연은 출시를 막는 버그다.
- **멀티세션 비용은 세션 수에 비례하지 않는다.** 15개 세션은 일상 운용 기준이고 30개는 release gate 부하다. 논리 세션은 더 많이 존재할 수 있지만 hot process는 최대 8개, active renderer와 full stream은 정확히 하나다. 선택 세션 고정, 즉시 검색, 안정 정렬, workspace 전환은 30개에서도 같은 조작이어야 한다.
- **멀티에이전트는 provider-neutral이다.** 지원되는 설치형 CLI를 자동 발견하고 한 목록과 같은 조작법으로 운영한다. 새 provider는 core 수정 없이 manifest 또는 driver로 추가한다.
- **에이전트가 저장소를 자율적으로 변경한다.** provider CLI가 작업과 대화를 소유하고 runtrol은 session, workspace, worktree, process lifecycle, collision boundary만 감독한다.
- **대화 선택과 workspace 전환을 결박한다.** session 선택 즉시 대화와 파일 맥락을 전환하고, 실제 편집이 필요할 때만 정확한 workspace 또는 worktree를 Code-hot으로 승격한다. 대화 본문을 읽어 경로를 추측하지 않는다.
- **기기 연결과 세션 소유권을 분리한다.** VS Code와 폰은 같은 Core에 페어링된 표면이며 어느 쪽도 세션을 소유하지 않는다. 창, 기기, 네트워크 경로가 바뀌어도 Core가 세션을 살려 둔다. Tailscale 같은 기존 사설망은 발견되면 직결 경로로 활용할 수 있지만 페어링, push, 정합성은 그것에 의존하지 않는다.
- **사람이 항상 우선이다.** 긴 streaming, 여러 agent, build, test 중에도 사용자의 입력, 스크롤, 편집기와 파일 탐색이 먼저 반응한다.
- **얇은 경계는 바뀌지 않는다.** provider 계정 credential, transcript, 모델 API key, conversation copy를 소유하지 않는다.

현재 총점은 **74/140, 평균 5.3/10** 이다. 활성 CI 게이트가 선 축은 열셋이다.
10 점은 실제 환경에서 완결 여정이 반복 검증된 상태다.
**3 점을 넘는 점수의 근거는 CI 에서 실제로 도는 게이트다. 자동으로 실행되지 않는 경로는 구현돼 있어도 manual 층을 넘지 않는다.**

| 북극성 | 현재 점수 | 현 상태 | 도달할 상태 |
|---|---:|---|---|
| 하나의 세션 목록 | 5/10 | hosted CI가 실제 VS Code Extension Host에서 시작, 두 workspace 전환, 정확한 선택 복원, 재접속, 중단, 종료를 검증한다. 상대가 deterministic loopback model이라 mock 층이다. | 공급자가 Claude Code 든 Codex 든 그 다음 무엇이든, 지금 내 PC 에 살아 있는 세션이 한 목록에 뜨고 거기서 시작, 재개, 삭제가 끝난다. |
| 즉시 반응 | 5/10 | 실제 VS Code Extension Host가 production bundle을 측정한다. 30개 실제 세션 목록, 최대 8개 hot ACP 프로세스, provider-native cold 재개, 초당 3,000 원시 프레임, watch와 Webview paint가 끝난 세션 전환, workspace 변경 뒤 정확한 선택 복원까지 하나의 ratchet으로 막는다. 전송 상대가 mock이라 이 층에 머문다. | 목록이 기다림 없이 뜨고, 대화가 누르는 즉시 열리고, 긴 출력이 쏟아져도 스크롤과 입력이 끊기지 않는다. 사용자가 로딩을 인지하는 순간이 없다. |
| 폰에서 내 PC 세션 잇기 | 5/10 | hosted CI가 shipped PWA의 WebCrypto, Noise, CoreClient 모듈을 헤드리스 폰 프로세스에서 실행해 production 데몬과 설치된 실물 Claude Code 세션을 시작하고, 프롬프트와 watch 출력을 왕복한 뒤 종료한다. model 상대가 결정론 loopback fixture라 mock 층이다. | 폰을 PC 에 한 번 붙여 두면, 자리를 떠난 뒤에도 그 PC 에서 돌고 있는 세션에 폰에서 새 지시를 넣고 출력을 실시간으로 본다. 공급자 계정의 등급이나 인증 방식이 이 경험을 막지 않는다. |
| 공급자 확장성 | 5/10 | hosted CI 는 외부 드라이버 공개 계약, 3 개 OS 의 범용 ACP fixture, 독립 배포 ACP 구현의 두 턴과 native load, 실물 Claude Code 의 hidden 승인 거부 왕복을 검증한다. model endpoint 들은 로컬 mock 이며, 스케줄 CI 는 최신 CLI 로 parser probe 와 같은 승인 여정을 반복한다. 계정 기반 model 동작과 전체 event 표면은 주장하지 않는다. | 새 CLI 가 나오면 어댑터 하나만 추가되고 PC 화면과 폰 화면과 조작 방법은 그대로다. 사용자는 공급자가 늘어난 것을 목록이 길어진 것으로만 안다. |
| 대화 무통과 | 6/10 | 실물 루프백 소켓의 정확한 송신 허용 목록과 production Noise IK 및 IKpsk1 경계가 돈다 (`egressContract`). 프롬프트 표본은 릴레이 캡처와 진단 문자열에 평문으로 나타나지 않고, transport 는 디스크와 로그 API 를 갖지 않으며, 드라이버와 저장소는 공급자 transcript 경로를 모른다. 실물 폰과 릴레이를 잇는 live 게이트가 없어 천장이 6 이다. | 사용자의 프롬프트와 모델의 응답은 PC 와 공급자 사이, 그리고 사용자 자신의 기기 사이에서만 오간다. runtrol 은 본문을 저장하지 않고, 중간의 어떤 서버도 그것을 읽을 수 있는 형태로 받지 않는다. |
| 폰에서 승인 | 5/10 | 활성 게이트가 실물 Claude Code의 hidden Write 승인을 PWA watch 경로로 받고, 완전한 subject와 유일한 `rejectOnce`, 32 byte digest를 확인해 거부한 뒤 같은 provider 턴의 재개와 종료를 검증한다. model 상대가 결정론 loopback fixture라 mock 층이다. | 에이전트가 위험한 작업 앞에서 멈추면 폰에 뜨고, 폰에서 허용하거나 거부하면 PC 의 세션이 즉시 이어진다. |
| 끊겨도 살아남기 | 5/10 | 실제 PWA 모듈과 설치된 실물 CLI가 네트워크 절단 뒤 exact cursor로 재생하고, Core 재시작 뒤 명시적 gap과 native resume로 이어진다. model 상대는 mock이다. | 폰이 잠기거나 네트워크가 끊기거나 runtrol 을 재시작해도 PC 세션은 공식 resume surface 로 복구된다. bounded window 안은 exact cursor 로 이어지고, 밖은 조용히 건너뛰지 않고 명시적 gap 으로 보인다. |
| 상주 비용 | 6/10 | 세 hosted OS가 실제 debug daemon의 idle RSS와 10초 유휴 CPU를 하나의 ratchet으로 측정한다. 독립된 두 번째 증거 종류가 없어 천장은 6이다. | 하루 종일 켜 두어도 사용자가 존재를 눈치채지 못한다. 배터리, 팬, 작업 관리자 어디에서도 눈에 띄지 않는다. |
| 어디서나 같은 방법 | 8/10 | 활성 hosted CI가 정확한 네이티브 VSIX를 깨끗한 VS Code에 설치해, 구성된 Core 경로 없이 번들 Core를 발견하고 Runtrol을 열며 공개 새 대화 명령으로 `Runtrol: New chat` 작성 탭을 열었다 닫는다. 같은 게이트가 Windows, macOS, Linux에서 돌고 릴리스 행렬은 x64와 ARM64 6개 대상을 반복한다. 정적 계약과 실물 여정, multi-OS 증거만 주장한다. | Windows, macOS, Linux 에서 설치 방법과 조작이 같다. Windows 사용자가 WSL 이나 tmux 를 알 필요가 없다. |
| 알아서 최신 | 5/10 | `vscodeUpgradeRollback` 이 세 운영체제에서 VSIX와 Core 교체 중 세션 생존을 검증한다. `cliUpdateRehearsal` 은 확증된 provider 갱신의 실패, 정확한 원복, 진동 방지를 결정론 fixture로 검증한다. 실계정 provider 설치를 CI에서 바꾸지는 않으므로 mock 층이다. | 앱과 설치된 에이전트 CLI 가 알아서 최신이고, 업데이트가 세션을 깨면 사용자가 손대기 전에 되돌아가 있다. 사용자가 버전을 신경 쓰는 순간이 없다. |
| 모델 자동 인식 | 6/10 | hosted `modelDetectionSmoke --require-all` 은 자격증명 없이 최신 실물 CLI 를 설치해 Codex `model/list` 와 격리된 provider-owned option cache sentinel 을 포함한 Claude partial catalogue 를 검사하고, 관측한 identifier 가 production source 에 하드코딩되지 않았음을 확인한다. 특정 계정의 실제 사용 가능 여부는 주장하지 않아 live 한 종류의 천장 6 이다. | 지금 이 계정으로 쓸 수 있는 모델이 목록에 그대로 뜨고, 새 모델이 나와도 runtrol 을 고치지 않아도 뜬다. |
| 세션끼리 안 밟기 | 5/10 | 실제 Git metadata와 production Core admission이 같은 worktree의 하위 폴더를 한 writer로 묶고, opening, live, closing 예약의 겹침을 원자적으로 거부한다. linked worktree와 운영자가 명시한 공유 시작은 구분한다. provider가 fixture라 mock 층이다. | 어느 세션이 어느 폴더에서 무엇을 고치는지 항상 구분되고, 두 세션이 같은 폴더를 만지게 되면 시작 전에 경고받으며, 공급자가 격리 수단 (워크트리) 을 내주면 시작 화면에서 그대로 쓴다. |
| AI 끼리 서로 자문 | 3/10 | 토글이 실물 두 CLI 를 각자의 공식 명령으로 배선·검증·원상복구하고, 실제 턴 중 자문 수신까지 수기 실측했다 (2026-08-03). `crossConsultSmoke` 는 실물 구독 CLI 를 몰므로 운영자 기계에서 돌고, hosted CI 게이트가 없어 manual 층이다. | 토글 하나로 두 CLI 가 서로를 공식 표면 (MCP) 으로 등록해, 한 AI 가 턴 중에 다른 AI 의 의견을 직접 받아온다. 배선은 CLI 자신의 공식 명령으로만 만들고 (설정 파일을 직접 쓰지 않는다), 대화 본문은 여전히 runtrol 을 지나지 않는다. 사용자가 MCP 라는 개념을 몰라도 된다. |
| 떠날 자유 | 5/10 | `uninstallLeavesNoTrace` 가 공급자 상태를 runtrol 홈 밖에 둔 채 실제 데몬과 자식 프로세스로 턴을 끝내고, 홈 전체를 삭제한 뒤 새 데몬에서 같은 원생 세션을 불러와 두 번째 턴을 끝낸다. 상대가 ACP fixture 이므로 mock 층이다. | runtrol 을 지워도 세션과 기록은 각 CLI 의 것으로 그대로 남아 원래 방식으로 이어진다. runtrol 이 인질로 잡는 데이터가 없다. |

축마다 어떤 게이트가 그 점수를 떠받치는지는 [docs/northStarEvidence.md](docs/northStarEvidence.md) 가 정본이다.

### 채점 규칙

점수는 사람이 고르는 등급이 아니라 **기반 층 + 가산**으로 계산된다. 정본은
[tests/audit/northStar/board.toml](tests/audit/northStar/board.toml) 이고, `northStarBoard` 게이트가
계산하며 `readmeParity` 게이트가 4 개 언어 README 를 그 계산 결과와 대조한다.

**기반 층.** 축마다 하나만 성립하고, 이것이 천장이다.

| 기반 층 | 점수 | 성립 조건 |
|---|---:|---|
| `none` | 0 | 이 축을 단언하는 게이트가 없다 |
| `manual` | 3 | 사람이 손으로 한 번 봤다. 활성 hosted CI 게이트가 없다. 데모 영상, 스크린샷, "돌려봤더니 되더라" 가 전부 여기다 |
| `mock` | 5 | 등록된 게이트가 돌지만 상대가 가짜다. mock CLI, stub 공급자, 시뮬레이션된 폰 |
| `realOneKind` | 6 | 실물 상대로 돈다. 단 static (`contract`) 과 live (`smoke`, `bench`) 중 한 종류만 있다 |
| `realBothKinds` | 7 | 실물 상대로 static 과 live 를 둘 다 갖췄다 |

**가산.** `realBothKinds` 에서만 붙는다. 각 가산은 그에 맞는 종류의 게이트를 요구하고, 넷을 다 갖출 때 정확히 10 이 된다.

| 가산 | 점수 | 성립 조건 |
|---|---:|---|
| `multiProvider` | +1 | 같은 게이트가 공급자 둘 이상에서 green |
| `multiOs` | +1 | 같은 게이트가 OS 둘 이상에서 green. Windows 포함 |
| `faultInjection` | +0.5 | 장애 주입 (데몬 강제 종료, 네트워크 차단) 을 태우고도 green |
| `ratchet` | +0.5 | 회귀 ratchet 이 있어 숫자가 나빠지는 즉시 red |

점수가 부풀지 않게 하는 규칙:

1. **구현이 아무리 완성돼 보여도, 러너가 실제로 부르는 게이트가 없으면 `manual` (3 점) 이 최대다.** 예외 없다.
2. `operator` 종류 게이트 (실기기, 실계정이 필요한 것) 는 **총점 계산에서 뺀다.**
3. 점수를 올리는 PR 은 **게이트 이름과 CI 실행 링크**를 본문에 붙이고 `board.toml` 을 같이 고친다. 산문 근거는 점수가 아니다.
4. 점수는 0.5 단위로만 매긴다. 8.7 같은 숫자는 정밀함이 아니라 자기기만이다.
5. **한 축이 내려가는 것을 막지 않는다.** 공급자가 표면을 바꿔 게이트가 red 가 되면 점수는 내려간다. 이 표는 어제의 자랑이 아니라 오늘의 상태다.
6. **천장은 실행 횟수가 아니라 없는 게이트 종류가 정한다.** 한 종류만 가진 축은 아무리 green 이어도 6 을 못 넘는다. 지금 14 축 중 13 축이 그 상태이고, `northStarBoard` 가 축마다 그 천장을 인쇄한다.

### 점수가 되는 것과 안 되는 것

세 층은 섞지 않는다. 섞는 순간 사용자가 아무것도 받지 못했는데 총점이 오른다.

| 층 | 무엇이 들어가는가 | 어떻게 표시되는가 |
|---|---|---|
| **점수 축** | 사용자가 체감하는 결과 (위 표의 14 개) | 0 부터 10 까지, 합계 /140 |
| **바닥 게이트** | 모듈화, 클린코드, 보안, 위생, 예산 | **점수가 아니다.** green 아니면 red 뿐이고 red 면 머지되지 않는다 |
| **접어야 할 조건** | 혁신성, 포지셔닝 | **숫자가 없다.** [docs/positioning.md](docs/positioning.md) 의 kill criteria 로만 판정한다 |

- **모듈화와 클린코드에 부분점수를 주지 않는 이유.** 둘은 강행규칙이다. "클린코드 7/10" 은 "3 만큼 규칙을 어기는 중" 이라는 뜻이고, 그건 점수가 아니라 red 다. 대신 항목별 게이트로 쪼개서 각각 이름을 붙인다 (`dependencyDirection`, `providerIsolation`, `checkSilentFail`, `cargoClippy` 등). 전체 목록은 [docs/northStarEvidence.md](docs/northStarEvidence.md) 에 있다.
- **혁신성에 숫자를 주지 않는 이유.** 혁신은 위 14 축 그 자체다 ("여러 AI 를 한 곳에서 관리한다"). 따로 점수를 매기면 같은 것을 두 번 세는 것이고, 어떤 게이트도 그 숫자를 단언할 수 없어 규칙 3 에 걸린다. 혁신이 사라졌는지는 kill criteria 가 판정한다.

## 최상위 원칙 . 사용자 편의

모든 갈림길에서 사용자가 더 편한 쪽으로 간다. 판정 기준은 취향이 아니라 **사용자가 실제로 하는 동작의 수와 기다리는 시간**이다.

- 사용자가 **설정해야 알아서 되는 것**은 실패다
- 사용자가 **기다리는 것이 보이면** 실패다
- 사용자가 **개념을 배워야 하면** 실패다 (tmux, WSL, 터널, 포트포워딩, 인증서 설치)
- 사용자가 **같은 일을 두 번 하면** 실패다
- **버벅임은 최적화 대상이 아니라 버그다**

## 받기

| | |
|---|---|
| **PC (Windows, macOS, Linux)** | [VS Code Marketplace에서 `Runtrol Studio`](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio)를 설치한다. x64와 ARM64를 지원하며 별도 데스크톱 앱은 배포하지 않는다 |
| **모바일** | [영구 GitHub Pages 주소의 휴대폰 PWA](https://eddmpython.github.io/runtrol/app/). 먼저 VS Code에서 일회용 QR로 페어링한다 |

공개 릴리스 `0.1.9`과 6개 플랫폼별 VSIX는 [GitHub Releases](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.9)에서도 받을 수 있다.
Marketplace 설치는 VS Code가 자동 갱신한다. 예전 버전을 VSIX로 직접 설치했다면 VS Code가 그 확장의 자동 갱신을 끄므로 Marketplace에서 한 번 다시 설치한다.

## 에이전트에게 Runtrol 맡기기

프로젝트 헤딩의 반짝임 버튼에서 **Enable Agent Tools for This Project**를 누르면 된다. 설치된 코딩
에이전트는 provider와 모델을 발견하고, 그 프로젝트 안의 세션을 시작하고, 지시를 보내고, 이벤트를
읽고, 정확한 세션을 멈출 수 있다. 프로젝트 행에 `Agent Tools`가 나타나면 준비된 상태다.

권한은 그 canonical 프로젝트 root 하나에만 묶인다. 승인 응답, 대화 삭제, 몰래 공유 시작, API key,
transcript 사본, Runtrol 자체 agent loop는 없다. **Disable Agent Tools for This Project**를 누르면 Runtime
권한과 OS 보호 자격이 삭제되고, 마지막 프로젝트라면 provider 등록도 제거된다. 정확한 경계는
[Agent Tools 운영 문서](docs/agentTools.md)에 있다.

## runtrol 이 필요 없는 사람

**공급자 하나만 쓴다면 그 공급자 자신의 원격 제어가 더 낫다. 이걸 먼저 적는다.**

Claude Code 하나만 쓰는 사람에게 `claude --remote-control` 이 더 좋다. 만든 곳이 만들었고,
무료로 번들되고, 네이티브 푸시가 붙고, 앱 스토어에 있다.
Anthropic, OpenAI, GitHub, Amp 넷 모두 이미 자기 원격 제어를 냈다. 그것으로 충분하면 그것을 쓰면 된다.

**runtrol 은 그 목록이 넷으로 갈라지는 사람을 위한 것이다.**
Claude 앱에 Codex 세션은 영원히 안 뜬다. 그건 기능 차이가 아니라 구조이고, 공급자가 고칠 이유가 없다.

## runtrol 이 아닌 것

- **채팅 클라이언트가 아니다.** 대화의 렌더링은 각 CLI 가 이미 하는 일이다. runtrol 은 그 출력을 옮길 뿐 해석하지 않는다.
- **모델 프록시가 아니다.** 모델 API 를 부르지 않고, 토큰을 읽지 않고, 요청을 중계하지 않는다. 설계 취향이 아니라 생존 조건이다.
- **IDE 가 아니다.** diff 를 보여주는 것까지가 경계이고, 편집하는 것은 경계 밖이다.
- **자체 에이전트 프레임워크가 아니다.** Runtrol 소유 플래너나 자율 루프는 없다. 제한된 Runtime 도구를 provider 소유 agent loop에 제공하지만 그 loop가 되지는 않는다.
- **호스팅 서비스가 아니다.** 계정도, 로그인도, 요금제도 없다.
- **터미널 멀티플렉서가 아니다.** tmux 를 대체하려는 게 아니라 **요구하지 않으려는 것이다.**

## 왜 Rust 인가

정직하게, **Rust 자체는 차별점이 아니다.** 이 판의 경쟁자 열 개 이상이 이미 Rust 다.
Rust 는 목적이 아니라 위 표의 세 축을 위한 수단이다.

- **`어디서나 같은 방법`**: ConPTY 와 POSIX 를 같은 추상 뒤에서 직접 다뤄 tmux 없이 Windows 를 1급으로 만든다.
- **`상주 비용`**: 하루 종일 켜 두는 daemon 이다. 런타임 없는 단일 정적 바이너리라 Node 도 Python 도 깔 필요가 없다.
- **`즉시 반응`**: 목록과 대화가 기다림 없이 열리는 것은 GC 정지와 런타임 부팅이 없어야 가능하다.

그 축들을 게이트로 못박지 않으면 Rust 를 쓴 의미가 사라진다.

## 구조

| | | |
|---|---|---|
| `crates/` | 제품 코어 (Rust). daemon, provider 어댑터, 전송. 독립 GUI crate는 없다 | 구현됨 |
| [`clients/typescript/`](clients/typescript/) | 외부 제품용 공개 Runtime TypeScript SDK | packed 소비 검증 |
| [`extensions/runtrol-vscode/`](extensions/runtrol-vscode/) | 유일한 PC 표면 `Runtrol Studio` | 30개 세션 출시 부하 검증, 0.1.9 공개 |
| [`pwa/`](pwa/) | 모바일 PWA | 릴레이 연결, 세션 제어, 승인, `Needs you`와 Mission Flight Signals 직행 구현 |
| [`site/`](site/) | [무의존성 GitHub Pages 랜딩](https://eddmpython.github.io/runtrol/) | 공개됨 |
| [`assets/brand/`](assets/brand/) | 로고. SVG 가 정본, 파비콘·아이콘·소셜 카드는 파생 | |
| [`docs/`](docs/README.md) | 운영문서 정본 | |
| [`tests/audit/`](tests/audit/) | 계약 게이트 | |
| [`tests/audit/northStar/`](tests/audit/northStar/) | 점수판 엔진. 위 표의 숫자를 계산하고 4 개 언어를 대조한다 | |

## 개발

```bash
python -X utf8 tests/audit/preflight.py          # 로컬 CI 전체
python -X utf8 tests/audit/preflight.py lint     # lint 만
python -X utf8 tests/audit/preflight.py --list   # 무엇이 돌고 무엇이 건너뛰는지
git config core.hooksPath .githooks              # 클론마다 한 번
```

게이트는 **통과 도장이 아니라 결함 탐지기**다. 새 게이트를 세우면 통과를 보기 전에
잡아야 할 결함을 일부러 심어 red 가 나오는지부터 확인한다
(`python -X utf8 tests/audit/checkSilentFail.py --selftest` 가 그 형태다).

기여는 [CONTRIBUTING.md](CONTRIBUTING.md) 를 본다. 설계 단계의 기여도 진짜 기여다.

## 라이선스

제품 본체는 [AGPL-3.0-only](LICENSE). 공개 클라이언트 패키지 (`runtrol-runtime-protocol` ·
`runtrol-runtime-client` · `@runtrol/runtime-client`) 는 남이 링크하라고 내는 것이므로
[Apache-2.0](crates/runtrol-runtime-protocol/LICENSE).

runtrol 을 쓰는 것만으로는 당신 코드에 아무 의무도 생기지 않는다. runtrol 은 에이전트 CLI 를
별도 프로세스로 감독할 뿐 당신이 쓴 것에 링크되지 않는다.
