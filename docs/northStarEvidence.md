# 북극성 증거 등록부

**게이트가 무엇을 단언하는가의 정본이다.** 어느 게이트가 어느 축에 붙는지, 그 축이 지금 몇 점인지는 여기가 아니라 [`tests/audit/northStar/board.toml`](../tests/audit/northStar/board.toml) 이 정본이고, `northStarBoard` 게이트가 계산한다. 두 곳의 게이트 이름 집합이 어긋나면 red 다 (양방향).

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
| `sessionLifecycleSmoke` | 실물 CLI 를 몰아서: 시작 -> **두 provider 가 한 목록에** -> 목록이 재개에 필요한 이름을 싣는다 -> 닫기 -> 목록에서 사라짐. 프롬프트를 보내지 않으므로 토큰·rate limit 0 이고 그래서 매 preflight 에 돈다. **닿지 못하는 절반을 매 실행마다 말한다**: 턴이 한 번도 없던 대화는 provider 저장소에 없어서 재개할 수 없다 (한쪽은 이름을 안 주고, 한쪽은 `no rollout` 으로 거절한다. 둘 다 실측). 그래서 이 게이트가 지키는 것은 **성공한 재개가 아니라 실패한 재개가 이름을 갖고 거절되는 것** (조용히 새 대화를 시작해 재개인 척하지 않는 것) 이다. 성공한 재개는 턴 하나가 필요해 이 게이트 밖이다. provider 이름은 박지 않고 manifest 에서 발견한다 |
| `interactionLatencyBudget` | 실물 Edge 또는 Chrome 이 production bundle 을 열고 목록 첫 페인트, 저장된 꼬리 표시, 입력 반응의 상한을 지킨다. 전송 상대는 mock 이므로 5 점 층이다. 수치는 **내려가기만 하는 ratchet** |
| `scrollUnderLoadSmoke` | 실물 브라우저에 provider 모양 원시 프레임을 초당 3,000 개 넣고 처리량, p95 프레임, 입력 지연, DOM 창 상한을 함께 판정한다. 전송 상대는 mock 이므로 provider 시간은 섞이지 않는다 |
| `desktopConvenienceSmoke` | 실물 브라우저의 production bundle 에서 공급자를 고르지 않고 세션을 시작하고, 마지막 공급자가 다음 시작의 기본값이 되는지 확인한다. 드라이버가 이미 내는 문맥 사용량과 계정 한도 프레임도 별도 사본 없이 화면에 보이는지 판정한다 |
| `phoneDrivesPcSmoke` | headless 브라우저의 실물 PWA 가 실물 데몬을 통해 실물 `claude`/`codex` 세션에 프롬프트를 넣고 출력을 받는다 |
| `iosInstallAndPush` | iOS 홈화면 설치 + Web Push 수신. 실기기 필요. **점수에서 뺀다** |
| `providerContract` | 모든 어댑터가 같은 trait 계약을 통과. **코어에 provider 고유명사 분기가 없다**는 정적 검사 포함 |
| `agentSurfaceDrift` | 최신 CLI 를 받아 생성 스키마와 저장 스키마를 대조. 공급자가 표면을 바꾸면 사용자보다 먼저 red |
| `genericAcpSmoke` | 공급자 코드 없이 외부 TOML 만 놓고 별도 ACP v1 실행 파일을 발견한다. 실물 데몬과 CLI 표면을 거쳐 시작 -> 프롬프트 -> 스트림 -> 공급자 선언 종료 -> 데몬 재시작 뒤 load 까지 완주한다. fixture 이므로 공급자 실물 가산에는 세지 않는다 |
| `egressContract` | production 송신 정책으로 정확히 허용한 IP 와 port 만 실물 루프백 소켓에 연결된다. production `Noise_IK_25519_AESGCM_SHA256` 세션과 `Noise_IKpsk1_25519_AESGCM_SHA256` 페어링이 고정 static key, 링크 종류, relay origin, peer id 를 인증하며 변조와 잘못된 key, PSK, prologue 를 거절한다. 65,519 byte 경계 분할, `varint(len) || ciphertext`, REKEY 뒤 왕복까지 돈다. relay capture 와 `Debug` 에 prompt 표본이 평문으로 없고 transport 에 disk 또는 log API 가 없으며, **driver 와 store 에 벤더 세션 경로가 없다**는 정적 검사 포함 |
| `approvalRoundtripSmoke` | 실제 permission prompt 가 폰 표면에 도달하고, 폰의 응답이 세션을 재개시킨다 |
| `resilienceFaultInjection` | 네트워크 차단, 데몬 강제 종료, 폰 재연결 각각에서 세션이 살아남고 **출력 손실 0** |
| `idleFootprintRatchet` | idle RSS 와 CPU 상한. **내려가기만 하는 ratchet.** 기준은 데몬 단독이고 (상주하는 것은 데몬이다), GUI 창 열림은 별도 예산으로 병기한다 |
| `crossPlatformMatrix` | 같은 종단 스모크가 Windows·macOS·Linux 러너에서 전부 green. **Windows 잡은 WSL 없이 돈다** |
| `cliUpdateRehearsal` | 구버전 -> 업데이트 -> 세션 정상 -> 고의로 깨진 버전 -> 자동 롤백 |
| `appUpdateRehearsal` | 런처가 GitHub Releases 에서 서명된 업데이트를 받아 설치하고, 서명이 안 맞으면 거부한다 |
| `modelDetectionSmoke` | 실물 CLI 에서 모델 목록을 얻는다. **소스에 모델 이름 리터럴이 없다**는 정적 검사 포함 |
| `sessionOverlapGuard` | cwd 겹침이 목록에 구분돼 보이고, 같은 폴더에 두 번째 세션을 시작하면 경고가 선행하며, provider 가 내주는 워크트리 시작 옵션이 그대로 노출된다. **격리를 runtrol 이 직접 구현하는 것은 얇음 위반이라 하지 않는다** (겹침을 보이게 하고 provider 의 수단을 노출하는 것까지가 경계) |
| `crossConsultSmoke` | 토글 켬 -> 두 CLI 가 서로를 자기 공식 설정 명령 (MCP 등록) 으로 배선 -> 한 CLI 가 턴 중에 다른 CLI 의 의견을 실제로 받아옴 -> 토글 끔 -> 설정 원상복구. **본문은 runtrol 을 지나지 않고, 설정 파일을 직접 쓰지 않는다** (배선은 CLI 공식 명령만. `configReadOnly` 바닥 게이트와 양립하는 것이 곧 설계다) |
| `uninstallLeavesNoTrace` | runtrol 제거 후 `claude --resume` 과 `codex resume` 이 그 세션들을 그대로 연다 |

### 바닥 게이트 (점수가 아니다. green/red 뿐이다)

강행규칙을 항목별로 쪼갠 것이다. **부분점수를 주지 않는 이유**: "클린코드 7/10" 은 "3 만큼 규칙을 어기는 중" 이라는 뜻이고, 그건 점수가 아니라 red 다. 총점에도 넣지 않는다 (사용자가 아무것도 못 받았는데 총점이 오르는 것이 곧 점수 부풀리기다).

| 게이트 | 무엇을 단언하는가 |
|---|---|
| `dependencyDirection` | crate 의존 방향이 선언된 간선만 갖고, 금지 쌍이 도달 불가이며, 제품 crate 에 순환이 없다 |
| `noScriptsDir` | repo 어디에도 `scripts/` 가 없다. 소유자 없는 폴더는 아무도 안 지운다 |
| `providerIsolation` | 코어 (`session`·`transport`·`api`) 에 provider 고유명사 분기가 없다. 새 CLI 는 manifest 또는 trait 구현만으로 붙는다 |
| `workspaceLints` | 어느 crate 가 워크스페이스 lint 표를 상속하고 어느 crate 가 자기 표를 쓰는지 고정한다 (`tests/audit` 는 후자다. 실측된 cargo 제약) |
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
| `pairingLifecycle` | 128 bit QR PSK 가 120 초 뒤 만료되고, 다섯 번 실패하면 잠기며, 첫 유효 Noise 메시지에서 즉시 단일 사용 처리된다. Noise 로 인증된 static key 와 개별 attempt id, 검증된 기기명과 platform 을 PC prompt 와 witness 소비에 함께 결박한다. 일반 `device.pair` witness 나 다른 pairing witness 로는 message 2 와 channel 을 만들 수 없고, 정확한 현장 승인 뒤에만 locally minted device id 가 생긴다 |
| `argumentEscaping` | Windows `.cmd` 실행 인자 이스케이프 (BatBadBut CVE-2024-24576) |
| `configReadOnly` | provider 설정 파일에 **쓰는** 코드가 없다 |
| `workspaceHygiene` | 루트 allowlist + `.tmp/` 7 일 부패 검출. stray log/tmp/trace 0 |
| `gateCoverage` | 저장소에 있는 게이트를 러너가 전부 부른다. 로컬 목록과 CI 목록이 서로를 검사한다 |
| `checkNoAiMarkers` | 커밋·태그·PR·주석에 AI 기여자 표식과 벤더명이 없다. 공개 artifact 는 주체 중립이다 |
| `northStarBoard` | 점수판의 모든 숫자가 `board.toml` 에서 계산되고, 그 근거 게이트가 실재하며 러너가 부른다 |
| `readmeParity` | 4 개 언어 README 가 같은 축·같은 점수·같은 채점 규칙을 인쇄한다. 언어판이 낡으면 red |
| `memoryBudget` | daemon idle RSS 와 세션당 증분 상한. 예산을 올리는 것은 운영자 승인 사항이다 |
| `orphanReaping` | 데몬을 죽이면 자식 CLI 프로세스가 남지 않는다 |

<!-- gates:end -->

## 게이트가 어디서 도는가 (실행 환경의 정직성)

실물 CLI 게이트에는 **hosted CI 가 풀 수 없는 제약**이 있다. 두 CLI 의 구독 인증 (OAuth) 은 사람 로그인이 필요하고, 그 세션 자격을 CI 비밀로 실어 나르는 것은 하지 않는다. 실물 턴은 돈과 rate limit 도 쓴다 (한 턴 $0.03 수준 실측). 이 제약을 숨기지 않고 실행 층을 가른다.

| 층 | 어디서 | 언제 | 무엇 |
|---|---|---|---|
| contract | hosted CI (GitHub Actions) | PR 마다 | 정적 검사 · mock 스모크 |
| smoke (토큰 0) | **운영자 PC 의 preflight** | 커밋 전 매번 | 실물 CLI. 턴을 쓰지 않는 것 |
| smoke (턴 소모) | self-hosted runner | 스케줄 (미구성) | 실물 턴이 필요한 것 |
| bench | self-hosted runner | 스케줄 (미구성) | ratchet 실측 |
| operator | 사람 손 | 수시 | 실기기. 점수 제외 |

**실물 CLI 게이트가 hosted CI 에서 못 도는 이유는 인증이다.** 두 CLI 모두 사람의 구독 로그인으로 인증하고, 그 자격을 CI 비밀로 실어 나르는 것은 runtrol 이 설계 전체를 걸고 거부해온 일이다. 그래서 그 로그인이 사는 곳, 즉 운영자 PC 에서 돈다 (`gateCoverage.py` 의 `LOCAL_ONLY` 에 이유와 함께 선언).

**토큰을 쓰는 게이트와 안 쓰는 게이트를 가른다.** 프롬프트를 보내지 않는 실물 게이트는 돈도 rate limit 도 쓰지 않으므로 야간이 아니라 커밋 전 매번 돈다 (`sessionLifecycleSmoke` 가 그것이다). 실물 턴이 필요한 게이트는 그럴 수 없고, **그 층의 self-hosted 러너는 아직 없다.** 없는 것을 있는 것처럼 적지 않는다: 그 층에 기대는 축은 오늘 그만큼 검증되지 않았다.

채점 규칙과의 접점: 기반 층 `realOneKind`·`realBothKinds` 는 실제로 도는 실행으로만 인정한다. **게이트가 건너뛰면 `skipped` 상한 (5 점) 이 그대로 적용된다.** 러너가 죽은 채로, 또는 CLI 가 설치되지 않은 채로 점수를 유지하는 길은 없다.

## 등록 규약

1. **기반 층 `realBothKinds` 는 static 게이트와 live 게이트를 둘 다 요구한다.** 한 종류뿐인 축은 천장이 6 이고, 그것은 실행을 더 해서가 아니라 없는 종류를 지어야 풀린다.
2. **`board.toml` 이 이름을 대는 게이트는 이 문서에 설명이 있어야 하고, 그 반대도 성립해야 한다.** 어긋나면 `northStarBoard` 가 red 다.
3. **기반 층이 `manual` 을 넘으면 게이트 파일이 실재하고 러너가 불러야 한다.** 파일은 있는데 아무도 안 부르면 그것은 증거가 아니라 `unregistered` 상한 (0 점) 이다.
4. `operator` 종류는 총점 계산에서 빠지고, 그 사실이 `README.md` 에 보인다.
5. **가산은 최상 기반 층에서만 붙고, 각 가산은 그에 맞는 종류의 게이트를 요구한다.** `ratchet` 은 bench 없이, `faultInjection` 은 smoke 없이 주장할 수 없다.
6. **새 게이트는 통과를 보기 전에 실패할 수 있는지부터 확인한다.** 잡아야 할 결함을 일부러 심어 red 를 본다.
7. 바닥 게이트를 짓는 커밋은 `board.toml` 의 `planned` 를 `built` 로 같이 뒤집는다. 안 뒤집으면 red 다 (점수판이 자신을 과소평가하는 것도 부정확이다).
