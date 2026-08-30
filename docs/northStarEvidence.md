# 북극성 증거 등록부

**게이트가 무엇을 단언하는가의 정본이다.** 어느 게이트가 어느 축에 붙는지, 그 축이 지금 몇 점인지는 여기가 아니라 [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) 이 정본이고, `northStarBoard` 게이트가 계산한다. 두 곳의 게이트 이름 집합이 어긋나면 red 다 (양방향). manual 층을 넘는 점수에는 활성 hosted CI 작업이 실제로 호출하는 게이트만 들어간다.

산문 증거는 썩는다. 이름 붙인 스모크는 이름이 바뀌고, 지워지고, 러너에 등록되지 않은 채로 남는데 축은 계속 그것을 근거로 점수를 주장한다. 그래서 **기계가 읽는 것 (축 대응·종류·점수) 과 사람이 읽는 것 (무엇을 단언하는가) 을 갈라 두고**, 둘의 대응을 게이트가 강제한다.

## 게이트 종류

| 종류 | 뜻 | 채점에서의 위치 |
|---|---|---|
| `contract` | 순수 계약·정적 검사. 외부 프로세스 없이 돈다 | **static.** 기반 층 `realBothKinds` 가 요구하는 두 종류 중 하나 |
| `smoke` | 실물 CLI 바이너리 또는 실물 브라우저를 태운다 | **live.** 나머지 하나. `faultInjection` 가산의 전제 |
| `bench` | 예산 ratchet. 넘으면 red | **live.** `ratchet` 가산의 전제 |
| `operator` | 실계정·실기기가 필요해 기계로 못 돌린다 | **점수에 세지 않는다.** 총점에서 뺀다 |

한 종류만 가진 축은 아무리 green 이어도 천장이 6 이다. 천장은 실행 횟수가 아니라 **없는 게이트 종류**가 정하며, `northStarBoard` 가 축마다 ceiling 열로 인쇄한다.

## 게이트 목록

아래 모든 게이트는 `board.toml` 에 등록되어 있어야 하고, 그 반대도 성립해야 한다. 현재 대부분은 **지어야 할 게이트의 명세**다. 게이트가 실재하고 러너가 부르기 전까지 그 축의 기반 층은 `none` 또는 `manual` (상한 3) 이다.

<!-- gates:begin -->

### 축을 떠받치는 게이트

| 게이트 | 무엇을 단언하는가 |
|---|---|
| `sessionLifecycleSmoke` | An operator-side local preflight starts and closes sessions from every installed real CLI, keeps their provider-native names in one list, survives a daemon restart, and reports native resume refusal instead of silently starting a replacement. This real-account evidence is operator-only and does not add score. |
| `providerTerminalParity` | An operator-side zero-turn gate opens the installed Claude and Codex TUIs through public Runtime. Two independent live viewers receive one provider-owned screen change, the first viewer closes, a new writer changes the screen observed by the remaining viewer, fresh snapshots stay byte-identical, delivery stays below 250 ms, and exact stop makes the terminal unattachable. Startup modals use reversible navigation without parsing provider text or assuming menu order. This real-account evidence is operator-only and does not add score. |
| `vscodeRealProviderMultiWindow` | An operator-side zero-turn gate opens each installed Claude and Codex TUI in a real VS Code editor terminal, attaches a second simultaneous isolated VS Code window to the exact Runtime generation and terminal generation, observes the first window's reversible input in both windows, closes the first window, writes from the second within 500 ms, and stops the exact terminal. It uses the production extension bundle, provider-owned TUI, and separate VS Code profiles, parses no provider text, and requires zero task-owned process survivors. This real-account evidence is operator-only and does not add score. |
| `phoneDrivesPcSmoke` | shipped PWA 의 WebCrypto, Noise, record, CoreClient 모듈을 헤드리스 폰 프로세스에서 그대로 실행한다. QR 승인 완료 상태만 테스트 행으로 주입하고 production 데몬의 기기 인증, 정확한 scope, workspace 및 provider 권한, 실물 Claude Code 시작, prompt, watch 출력, provider 종료, session 삭제를 관통한다. model 상대는 요청 본문을 버리는 결정론 loopback fixture 다 |
| `iosInstallAndPush` | iOS 홈화면 설치 + Web Push 수신의 기여자 operator evidence. 현재 실기기 관측은 없고 완료 범위와 점수에서 뺀다. 관측 전에는 통과로 주장하지 않는다 |
| `providerContract` | 저장소 밖 구현이 공개 `Provider` 와 `Agent` trait 를 구현하고 native command 를 처리하며 미지 event 를 `Unmapped` 로 보존할 수 있다. 코어의 provider 고유명사 격리는 별도 `providerIsolation` 게이트가 맡는다 |
| `agentSurfaceDrift` | scheduled hosted CI 가 최신 실물 CLI 를 무인증으로 설치하고, schema provider 의 바인딩 메서드와 stream-json provider 의 바인딩 플래그를 실제 생성 스키마와 인자 파서에 대조한다. built-in probe 전략 하나라도 실행되지 않으면 red 다. 인증과 턴이 필요한 event 및 control frame 호환성은 이 게이트의 주장이 아니다 |
| `genericAcpSmoke` | 공급자 코드 없이 외부 TOML 만 놓고 별도 ACP v1 실행 파일을 발견한다. 실물 데몬과 CLI 표면을 거쳐 시작 -> 프롬프트 -> 스트림 -> 공급자 선언 종료 -> load 를 Windows, macOS, Linux 에서 완주한다. fixture 이므로 공급자 실물 가산에는 세지 않는다 |
| `externalAcpSmoke` | 저장소 밖에서 독립 배포되는 ACP CLI 를 고정 판본으로 설치하고 외부 TOML 만으로 연결한다. 로컬 결정론 model endpoint 를 사용해 실물 데몬과 CLI 표면에서 두 번의 스트림과 공급자 선언 종료, daemon 재시작, 같은 native session load 를 완주한다. model 상대는 mock 이고 공급자 구현은 실물이다 |
| `claudeApprovalSmoke` | 설치된 실물 Claude Code 를 production stream-json 드라이버와 실물 daemon 으로 실행하고 로컬 결정론 Messages endpoint 에 연결한다. 실제 hidden `can_use_tool` 승인 수신, 정규화된 `rejectOnce` 선택, 원생 `control_response` 소비 뒤 두 번째 model 요청, provider 선언 `endTurn`, 대상 파일 부재를 한 여정에서 검증한다. model 상대는 mock 이고 계정 인증과 hosted model 동작은 주장하지 않는다 |
| `egressContract` | production 송신 정책으로 정확히 허용한 IP 와 port 만 실물 루프백 소켓에 연결된다. production `Noise_IK_25519_AESGCM_SHA256` 세션과 `Noise_IKpsk1_25519_AESGCM_SHA256` 페어링이 고정 static key, 링크 종류, relay origin, peer id 를 인증하며 변조와 잘못된 key, PSK, prologue 를 거절한다. 65,519 byte 경계 분할, `varint(len) || ciphertext`, REKEY 뒤 왕복까지 돈다. relay capture 와 `Debug` 에 prompt 표본이 평문으로 없고 transport 에 disk 또는 log API 가 없으며, **driver 와 store 에 벤더 세션 경로가 없다**는 정적 검사 포함 |
| `approvalRoundtripSmoke` | 실물 Claude Code 의 hidden Write permission 요청이 PWA watch 경로에 도달하고 같은 세션이 인증된 폰 catalogue에서 `waiting_on = person`으로 보인다. 폰이 완전한 subject, 유일한 `rejectOnce`, 32 byte digest를 확인해 답하고, CLI가 그 거부를 소비한 두 번째 model 요청과 provider 선언 `endTurn`까지 진행한 뒤 focus wait가 사라지며 대상 파일은 생기지 않는다 |
| `remoteResilienceFaultInjection` | shipped PWA의 WebCrypto, Noise, record, CoreClient 모듈과 설치된 실물 Claude Code를 production 서버에 연결한다. 폰 watch socket을 강제 절단해 bounded replay의 exact cursor와 중복 부재를 확인하고, 서버 작업을 강제 중단한 뒤 같은 durable home과 기기 권한으로 재구성한다. 이전 cursor의 명시적 cross-stream gap, provider native identity 보존, 공식 resume 뒤 새 turn, 전체 process cleanup을 검증한다. model 상대는 요청 본문을 버리는 결정론 loopback fixture다 |
| `idleFootprintRatchet` | hosted Windows, macOS, Linux 에서 실제 debug daemon 의 유휴 RSS 계약을 `memoryBudget` 정본으로 검사하고, 10 초 유휴 구간의 process CPU 누적 증가를 한 코어의 100 ms 이하로 제한한다. release live 비용이나 GUI 비용은 주장하지 않는다 |
| `crossPlatformContract` | 기존 `vscodePackage`의 단일 6대상 정본을 재사용해 각 대상의 정확한 executable, family, hosted runner를 검사하고, 일반 3 OS 행렬과 6대상 package 행렬에서 first-run 단계가 조건 없이 정확한 archive에 실행되는지 고정한다. Studio의 공개 새 대화 명령, 플랫폼 공통 shortcut, 빈 `runtrol.corePath` 기본값, 설치본 verifier의 새 draft 식별도 같은 사용자 방법으로 묶는다 |
| `crossPlatformMatrix` | 정확한 네이티브 VSIX를 깨끗한 실물 VS Code 프로필에 설치하고, 수동 Core 경로 없이 번들 Core를 발견·복사한 뒤 Runtrol을 연다. 같은 공개 `runtrol.startSession` 명령으로 `New chat` 작성 탭을 열고 그 정확한 draft를 닫는다. 일반 hosted CI의 Windows·macOS·Linux에서 동일 게이트가 돌고, 릴리스 행렬은 x64와 ARM64 6개 대상에서 반복한다. prompt나 provider process는 시작하지 않으며 model 동작, 다중 provider, 장애 복원은 주장하지 않는다 |
| `cliUpdateRehearsal` | Production update policy drives a deterministic package-tree fixture through target install and failed health verification, then reinstalls and verifies the exact starting release byte for byte. A second path makes rollback installation fail and requires a closed failure instead of an update claim. Package ownership and safe npm argument construction are covered separately by `channelVerdict`. Hosted CI does not mutate a developer's global provider installation or claim account-backed provider behavior. |
| `modelDetectionSmoke` | 자격증명을 제거한 hosted CI 에 최신 실물 CLI 를 설치하고 모든 built-in 실행을 강제한다. Codex 는 live `model/list`, Claude 는 격리한 provider-owned option cache sentinel 을 포함한 정직한 partial catalogue 를 반환해야 한다. 관측한 runtime model identifier 가 production source 에 리터럴로 있으면 red 다. 특정 계정의 사용 가능 모델은 주장하지 않는다 |
| `sessionOverlapGuard` | Real filesystem metadata resolves every requested directory to a Core-owned project and working-tree identity. Two subdirectories of one Git worktree are one writer, linked worktrees share a project but remain independent writers, and exclusive admission rejects overlapping opening, live, and closing claims atomically. Only an operator-marked shared start bypasses the refusal. The provider remains a deterministic fixture, so this gate does not claim an account-backed CLI journey. |
| `crossConsultSmoke` | 격리 홈에서 토글 켬 -> 배선 가능한 방향이 상대 서버의 tools/list 검증을 거쳐 CLI 공식 명령으로 등록되고, 등록 CLI 자신의 get 이 확증 -> 역방향은 실측 근거를 단 "지원 안 됨" -> 토글 끔 -> 설정이 정확히 원상복구되고 등록 항목에 명령 외 내용이 없음을 단언. **본문은 runtrol 을 지나지 않고, 설정 파일을 직접 쓰지 않는다** (배선은 CLI 공식 명령만. `configReadOnly` 바닥 게이트와 양립하는 것이 곧 설계다). 턴 중 실수신은 실물 턴 비용이라 게이트가 아니라 수기 실측이며 (2026-08-03, [cross consult](crossConsult.md)), 게이트 출력이 그 한계를 밝힌다. 실물 구독 CLI 를 몰므로 운영자 기계에서 돈다 |
| `uninstallLeavesNoTrace` | 공급자 소유 marker 를 `RUNTROL_HOME` 밖에 둔 채 한 턴을 끝내고 데몬 종료와 home 전체 삭제 뒤 runtrol 이 없는 상태에서 공급자 실행 파일로 같은 원생 세션을 직접 재개한다. 이어서 선택적 재설치와 manifest 재선언 뒤 같은 원생 세션을 load 해 두 번째 턴을 끝내며, Windows, macOS, Linux 에서 돈다. marker 는 transcript 가 아니라 native id 와 완료 횟수만 가진 fixture 상태다 |

### 바닥 게이트 (점수가 아니다. green/red 뿐이다)

강행규칙을 항목별로 쪼갠 것이다. **부분점수를 주지 않는 이유**: "클린코드 7/10" 은 "3 만큼 규칙을 어기는 중" 이라는 뜻이고, 그건 점수가 아니라 red 다. 총점에도 넣지 않는다 (사용자가 아무것도 못 받았는데 총점이 오르는 것이 곧 점수 부풀리기다).

| 게이트 | 무엇을 단언하는가 |
|---|---|
| `dependencyDirection` | crate 의존 방향이 선언된 간선만 갖고, 금지 쌍이 도달 불가이며, 제품 crate 에 순환이 없다 |
| `noScriptsDir` | repo 어디에도 `scripts/` 가 없다. 소유자 없는 폴더는 아무도 안 지운다 |
| `providerIsolation` | 코어 (`session`·`transport`·`api`) 에 provider 고유명사 분기가 없다. 새 CLI 는 manifest 또는 trait 구현만으로 붙는다 |
| `workspaceLints` | 어느 crate 가 워크스페이스 lint 표를 상속하고 어느 crate 가 자기 표를 쓰는지 고정한다 (`tests/audit` 는 후자다. 실측된 cargo 제약) |
| `vscodeExtension` | Studio contributes one provider-neutral native `runtrol.sidebar` tree containing project rows, conversation rows, first-run actions, and compact seven-day usage rows. The gate rejects extra view headers, webview sidebars, hardcoded provider branches, hidden coverage warnings, unavailable keyboard actions, invented usage percentages, an absent theme color, or a private terminal path. Source boundaries permit only bounded operational metadata and reviewed managed-Runtime installation writes, with no conversation-capable filesystem writer, transcript persistence, polling loop, or shipped Node runtime dependency. Type checking, tests, bundle size, exact command visibility, native icons, and segment-aware workspace collisions are gated. |
| `vscodePackage` | The extension release SSOT names one publishable SemVer and the exact native executable, family, and hosted runner for six targets. Fault injection rejects a missing target, a wrong architecture runner, missing or extra files, source and tooling leaks, wrong target or version metadata, license drift, stub binaries, unsafe paths, Core byte drift, a floating publisher CLI, unsupported Marketplace authentication, an unpinned workflow action, a missing automatic version trigger, and incomplete installation or support metadata. The static workflow contract requires the unconditional `crossPlatformMatrix` first-run step inside the six-target package job and binds it to the exact matrix archive. Release jobs resolve Core paths from the repository root and assemble in isolated temporary staging, so a running development Core is untouched. Each job inspects the built VSIX, installs it into a clean stable VS Code profile with no configured Core path, activates through the bundled Core, and requires exact process cleanup. A patch increment on `main` publishes through the Actions `VSCE_PAT` secret, verifies all six public package digests, installs and activates the exact Marketplace version on all six native runners, and creates the tag only after those public journeys pass. |
| `versionSsot` | `[workspace.package].version` owns the Runtime and Rust SDK version, while `release-policy.json` independently owns the Studio version. Every Cargo member inherits the workspace value, and VS Code packaging derives the policy value into temporary staging while checked-in npm manifests keep the neutral `0.0.0` placeholder. Studio stays on `0.1.x` from `0.1.1` onward, with an exact one-patch changelog sequence and predecessor tag required for every release. Fault injection rejects a hardcoded member, extension, lockfile, disconnected derivation module, series change, policy change, or skipped patch. |
| `runtimePublicBoundary` | Publishable Rust, TypeScript, and Python Runtime clients can import only the provider-neutral public protocol. Runtrol Studio uses the public TypeScript package for provider, session, approval, and terminal behavior; its private projection contains no terminal path. Independent administration cannot enter the public dispatcher, and generated schema bytes match their Rust source. |
| `runtimeClientSdk` | The TypeScript SDK regenerates and type-checks its protocol bindings, validates the checked schema, packs with no runtime dependency, installs offline into a repository-external consumer, exposes only the public, testing, and schema entry points, and rejects private imports or missing package documentation. |
| `runtimeRustClientSdk` | Cargo packages the public protocol and client crates, verifies their license, README, changelog, schema, registry dependency, and private-authority boundary, then extracts both archives and compiles a consumer outside the repository using only those packed sources. |
| `runtimeDistribution` | Six native standalone Runtime ZIP contracts derive from the shared target manifest, contain one real headless binary and an exact allowlist, reproduce checksums, carry protocol and rollback metadata, refuse running-locator uninstall, preserve provider-owned state, emit a machine-verifiable uninstall result, and require pinned keyless release attestations. The release manifest fails if any Runtime or SDK artifact is missing. |
| `runtimeDocumentation` | Public protocol, integration, security, operations, provider catalogue, package compatibility, and changelog documents are present and contain every shipped method, scope, error, finalized revision, install path, recovery action, and no-scan boundary from the code authorities. |
| `channelVerdict` | A provider update channel becomes executable only when its structured declaration, package-root ownership, resolved executable, safe discovered package identifier, and the argv owned by the closed channel adapter agree. Provider output is comparison data and is never executed. On-disk provider manifests cannot claim update authority, ghost installs remain distinct, and rollback selection uses semantic version order while refusing registries that do not contain the installed copy. |
| `vscodeHostPerformance` | A real isolated VS Code Extension Host runs the production bundle against a tracked product Core. Three isolated cold trials feed the field-wise fastest result into a shared JSON ratchet, while exact counts and zero-drop invariants must hold in every trial. The ratchet caps ready activation from one exact initial Runtime inventory, Runtrol navigation and editor conversation opening confirmed by both panel and active-tab state, 40-refresh p95, RSS growth, and real Webview animation, input, scroll, and queue growth while 3,000 raw frames per second cross the extension boundary with zero dropped frames in every trial. It also covers hiding the conversation behind a text editor and restoring the same editor tab before load begins. Each trial starts 30 external-manifest ACP sessions in independent workspaces while the Core keeps at most eight provider children hot, resumes one cold row through the provider-native surface within 3,500 ms, requires every hot switch to receive a new Core watch acknowledgement and Webview paint, and restarts VS Code in the selected workspace with the same profile to prove exact selection restoration. Animation records the runner's unloaded native cadence first, then enforces both a 40 ms absolute frame ceiling and an 8 ms p95 load-overrun ceiling. Switch p95 is capped at 175 ms and complete reload restoration at 2,500 ms. The gate runs on hosted Windows, macOS, and Linux with exact session and process cleanup. |
| `vscodeMultiWindowTerminal` | Two simultaneous real VS Code Extension Hosts load the production extension and attach editor terminal tabs to the exact same Runtime generation, terminal id, terminal generation, and provider TUI process. Input from the first window is observed in both windows. The first window then exits, the exact provider PID generation remains alive, the second window acquires the writer path and receives its own provider output, and its stop action ends that exact provider generation. A create-new PID marker makes a second fixture owner fail closed, delivery and handoff have bounded latency, and cleanup requires zero task-owned survivors. |
| `vscodeRealProviderJourney` | A real isolated VS Code Extension Host runs the production extension, Core, and an installed Claude Code process. In one journey it auto-discovers the CLI, starts two exact workspace sessions, sends a prompt, denies the provider-native hidden approval, reconnects the selected watch, interrupts a second turn, switches the same window to the second workspace, restores the exact selection, and closes both sessions. The deterministic loopback Messages endpoint discards request bodies immediately and spends no hosted model token. It does not claim account authentication or hosted model behavior. |
| `vscodeUpgradeRollback` | A native baseline VSIX starts an external ACP session through a Core materialized at one extension-global stable path. Real stable VS Code then installs the current VSIX and reinstalls the baseline through its official extension CLI. Every phase activates the installed production bundle, restores the exact session and workspace, completes another provider turn, and requires the original daemon and provider PIDs to remain in the same containment tree. The managed Core digest alone must move baseline to current to baseline, and cleanup requires zero exact survivors. The gate runs on hosted Windows, macOS, and Linux and again for every native release target before publication. |
| `vscodeEventCoverage` | All 19 names from `EventBody::wire_name` map to one bounded presentation contract in `assets/event-presentation.json`. VS Code consumes the shared kind, side, and localization-key SSOT while keeping localized human text and unknown-event fallback in the extension. Fault injection rejects missing, stale, malformed, opaque-content, and surface-local event maps. |
| `cargoFmt` | `cargo fmt --check` 통과. rustfmt 와 싸우지 않는다 |
| `cargoClippy` | `--all-targets -D warnings` 통과. 경고는 실패다 |
| `checkSilentFail` | `let _ = ...`, `.ok()`, 빈 `catch` 로 에러를 버리지 않는다. 근거 주석이 있는 것만 인정 |
| `silentFailSelftest` | 위 검출기가 **실패할 수 있음을 스스로 증명한다.** 결함을 심고 red 를 본다 |
| `cargoShear` | 미사용 의존성이 없다. `[workspace.dependencies]` 의 죽은 항목까지 (버전 SSOT 가 거기 산다) |
| `cargoDeny` | 공급망 advisory 와 `deny.toml` 의 기각 원장. 원장을 문서로만 두면 다음 사람은 읽지 않는다 |
| `noTranscriptCopy` | 대화를 담을 수 있는 타입이 저장소 crate 에 나타나지 않는다. 담을 수 있는 타입은 어휘에서 발견한다 (`Opaque` 필드를 가진 것 전부) 이므로 내일 생기는 타입도 그날부터 대상이다 |
| `scopeWall` | 모든 요청에 누가 할 수 있는지 규칙이 있고, 포괄 갈래가 거부하며, 벽이 디스패처의 다른 무엇보다 먼저 물어진다. 컴파일러는 crate 경계 너머로 빠진 요청을 말해주지 못한다 |
| `scopeGrantability` | 부여 불가 스코프 (`device.pair` · `config.write` · `approval.auto`) 를 원격에서 부여하려는 코드가 **컴파일되지 않는다** |
| `rebindingDefenses` | Host allowlist, Origin 기본 거부, 쿠키 인증 부재, CORS wildcard 부재를 실제 요청으로 확인 |
| `pairingLifecycle` | 128 bit QR PSK 가 120 초 뒤 만료되고, 다섯 번 실패하면 잠기며, 첫 유효 Noise 메시지에서 즉시 단일 사용 처리된다. Noise 로 인증된 static key 와 개별 attempt id, 검증된 기기명과 platform 을 PC prompt 와 witness 소비에 함께 결박한다. 일반 `device.pair` witness 나 다른 pairing witness 로는 message 2 와 channel 을 만들 수 없고, 정확한 현장 승인 뒤에만 locally minted device id 가 생긴다. 승인된 key, 단방향 credential fingerprint, scope 는 durable row 로 재개되며 bearer token 원문은 파일에 남지 않는다. Windows PC private key 는 CurrentUser DPAPI blob 으로만 남고 재시작 뒤 같은 public identity 를 복원한다. persisted grant constructor 는 daemon assembly 와 자체 security test 이외의 production 진입점을 거부한다 |
| `approvalAuthorization` | 승인 응답은 드라이버가 실제로 보관한 대기 요청의 id, subject digest, 선택지, 만료 시각, 구조적 위험도에 결박된다. wire 에 위험도를 싣지 않으므로 원격 장치가 필요 권한을 낮출 수 없고, 불완전한 subject 와 권한 밖 선택지는 공급자에게 전달되지 않는다 |
| `argumentEscaping` | Windows `.cmd` 실행 인자 이스케이프 (BatBadBut CVE-2024-24576) |
| `configReadOnly` | provider 설정 파일에 **쓰는** 코드가 없다 |
| `workspaceHygiene` | 루트 allowlist + `.tmp/` 7 일 부패 검출. stray log/tmp/trace 0 |
| `gateCoverage` | 저장소에 있는 게이트를 러너가 전부 부른다. 로컬 목록과 CI 목록이 서로를 검사한다 |
| `checkNoAiMarkers` | 커밋·태그·PR·주석에 AI 기여자 표식과 벤더명이 없다. 공개 artifact 는 주체 중립이다 |
| `noConsoleFlash` | Windows 데스크톱이 실행하는 provider 세션, 탐색 probe, 분리 daemon 이 콘솔 창을 만들지 않는다. 실제 자식 프로세스의 console handle 부재와 모든 제품 spawn 경계의 공통 정책 적용을 함께 확인한다 |
| `northStarBoard` | 점수판의 모든 숫자가 `board.toml` 에서 계산된다. manual 근거는 manual 층까지만 점수가 되고, 그보다 높은 층은 근거 게이트가 활성 hosted CI 작업에서 실행돼야 한다. `if: false` 작업과 로컬 전용 게이트는 manual 초과 근거에서 빠진다 |
| `readmeParity` | 4 개 언어 README 가 같은 축·같은 점수·같은 채점 규칙을 인쇄한다. 언어판이 낡으면 red |
| `memoryBudget` | 실제 daemon 의 idle RSS 만 platform 과 build profile 별 상한에 대조한다. session, subscriber, live payload 증분과 CPU 는 이 게이트의 주장이 아니다 |
| `liveMemoryBudget` | 실제 debug daemon 과 외부 ACP fixture 로 8개 hot idle session 전체를 동시에 유지해 baseline 대비 5 MiB 이하임을 먼저 검사하고, 별도 daemon 에 hot session 하나와 watcher 네 개를 연결한다. 900 KiB payload 의 완전 전달, baseline 대비 10 MiB 이하 peak 증가, Windows 및 macOS 48 MiB와 Linux 64 MiB total RSS, 종료 뒤 Windows 및 Linux 4 MiB와 macOS 6 MiB 이하 residual 을 검사한다. 종료 뒤 최대 2초 안에 250 ms 전체 관측 구간 하나가 정확한 residual 상한 아래에 있어야 한다. parser 입력 상한 아래의 15 MiB payload 는 live wire 에 싣지 않고 watcher 모두에게 explicit lag 를 내며, 같은 daemon 에서 session 을 세 번 연속 열어 거부한 뒤에도 residual 상한을 지켜야 한다 |
| `resilienceFaultInjection` | 실제 local IPC endpoint 를 끊은 동안 bounded ring 에 남은 fixture frame 이 exact cursor 뒤 한 번씩 replay 되는지 검사한다. 이어 daemon 을 강제 종료하고 같은 home 으로 재시작해 새 stream 의 explicit gap, provider-native identity 보존, 공식 resume 뒤 새 turn 을 확인한다. remote network, phone, transcript recovery, lossless history 는 주장하지 않는다 |
| `orphanReaping` | Windows 에서는 job handle 종료 뒤 자식 tree 부재를 확인한다. Unix 에서는 stable keeper 가 private control EOF 를 감지해 숫자 PID 나 PGID 없이 자기 group 을 종료한다. 다음 supervisor 는 PID, kernel start identity, boot identity 를 재검증하며 numeric group 을 signal 하지 않고 non-zombie member 부재만 확인한다. 실행 파일은 신원이 아니다 (갱신이 live keeper 뒤의 파일을 바꾼다). 다른 세대로 보이는 기록은 지우지 않고 보존하며, 갱신이 running image 를 rename 으로 덮은 뒤에도 세션 시작과 종료가 그대로 동작하는 것을 사본 격리 여정이 확인한다. 환경 변수와 대화 데이터는 guard 에 들어가지 않는다 |

<!-- gates:end -->

## 게이트가 어디서 도는가 (실행 환경의 정직성)

실물 CLI 게이트의 일부에는 **hosted CI 가 풀 수 없는 제약**이 있다. 계정 기반 턴의 구독 인증은 사람 로그인이 필요하고, 그 세션 자격을 CI 비밀로 실어 나르는 것은 하지 않는다. 반면 CLI 자기기술, credential-free model discovery, loopback model 여정은 hosted CI 에서 돈다. 이 경계를 숨기지 않고 실행 층을 가른다.

| 층 | 어디서 | 언제 | 무엇 |
|---|---|---|---|
| contract | hosted CI (GitHub Actions) | PR 마다 | 정적 검사 · mock 스모크 · 고정 판본 credential-free 실물 CLI |
| smoke (토큰 0) | hosted CI + 운영자 PC preflight | PR, schedule, 커밋 전 | parser/schema probe, model discovery, loopback provider wire. 계정 사용 가능성은 로컬에서만 관측 |
| smoke (턴 소모) | self-hosted runner | 스케줄 (미구성) | 실물 턴이 필요한 것 |
| bench | self-hosted runner | 스케줄 (미구성) | ratchet 실측 |
| operator | 사람 손 | 수시 | 실기기. 점수 제외 |

**계정 기반 실물 턴이 hosted CI 에서 못 도는 이유는 인증이다.** 그 자격을 CI 비밀로 실어 나르는 것은 runtrol 이 설계 전체를 걸고 거부해온 일이다. 하지만 설치된 실행 파일이 스스로 말하는 version, parser, schema, model surface 와 loopback endpoint 에 대한 provider wire 는 자격증명 없이 자동 검증한다. 로컬 preflight 는 계정 상태까지 관측하지만 그 추가 관측만으로 hosted 점수를 올리지 않는다.

**토큰을 쓰는 게이트와 안 쓰는 게이트를 가른다.** 프롬프트를 보내지 않는 실물 게이트와 local deterministic endpoint 여정은 돈도 rate limit 도 쓰지 않으므로 hosted CI 에서 돈다. 계정 model 의 실제 응답이 필요한 게이트는 그럴 수 없고, **그 층의 self-hosted 러너는 아직 없다.** 없는 것을 있는 것처럼 적지 않는다: 그 층에 기대는 축은 오늘 그만큼 검증되지 않았다.

### 활성 CI 판정

`gateCoverage.activeWorkflowText` 는 workflow 의 활성 job 과 step 만 남긴다. literal `if: false` 아래 명령은 파일에 적혀 있어도 실행이 아니므로 제거한다. Python 게이트는 그 활성 명령에서 직접 찾고, Rust 계약 게이트는 활성 `cargo test --all` 과 `tests/audit/Cargo.toml` 의 명시된 test target 을 함께 대조한다.

로컬 preflight 목록과 CI 목록의 대응은 `gateCoverage` 가 별도로 검사한다. 목적이 다르기 때문에 둘을 합쳐 점수 근거로 쓰지 않는다. 점수는 현재 커밋의 활성 CI 구조를 나타내고, 그 게이트가 red 면 workflow 전체가 red 다. 외부 CI 의 재시도나 과거 실행 상태를 저장소 점수 공식에 복제하지 않는다.

## 등록 규약

1. **기반 층 `realBothKinds` 는 static 게이트와 live 게이트를 둘 다 요구한다.** 한 종류뿐인 축은 천장이 6 이고, 그것은 실행을 더 해서가 아니라 없는 종류를 지어야 풀린다.
2. **`board.toml` 이 이름을 대는 게이트는 이 문서에 설명이 있어야 하고, 그 반대도 성립해야 한다.** 어긋나면 `northStarBoard` 가 red 다.
3. **기반 층이 `manual` 을 넘으면 게이트 파일이 실재하고 활성 hosted CI 작업이 불러야 한다.** 로컬 전용이거나 `if: false` 작업 아래 있으면 점수 근거가 아니다.
4. `operator` 종류는 총점 계산에서 빠지고, 그 사실이 `README.md` 에 보인다.
5. **가산은 최상 기반 층에서만 붙고, 각 가산은 그에 맞는 종류의 게이트를 요구한다.** `ratchet` 은 bench 없이, `faultInjection` 은 smoke 없이 주장할 수 없다.
6. **새 게이트는 통과를 보기 전에 실패할 수 있는지부터 확인한다.** `gateCoverage`, `northStarBoard`, `readmeParity` 도 각각 `--selftest` 에서 실행 누락, 비활성 증거, README drift 를 주입해 red 를 확인한다.
7. 바닥 게이트를 짓는 커밋은 `board.toml` 의 `planned` 를 `built` 로 같이 뒤집는다. 안 뒤집으면 red 다 (점수판이 자신을 과소평가하는 것도 부정확이다).
