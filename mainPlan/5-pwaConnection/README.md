# pwaConnection

상태: 진행 중, 착수 순서 5 번 ([site deployment](../../docs/siteDeployment.md)가 불변 origin을 확정했다).
The permanent Pages origin, default relay shape, and device blast-radius rule are fixed in production code.

Phone-facing HTTP admission, rebinding defenses, exact egress allowlisting, Noise IK session encryption, the 120-second single-use Noise IKpsk1 pairing lifecycle with exact PC approval, durable exact device authority, fail-closed daemon restoration, native identity protection, relay service, reconnecting daemon listener, and the WebCrypto PWA relay client are implemented. Web Push and the optional direct LAN and peer-to-peer routes remain.

운영자가 첫 지시에서 "pwa 와 연결할 아이디어가 아직없다. 똑똑한 방법이 있을까 고민중이다" 라고 한 그 자리다. 이 문서가 그 답이다.

## 한 문장 정의

**origin 과 transport 를 분리한다.** PWA 는 영원히 바뀌지 않는 HTTPS origin 하나에 살고, 그 아래에서 전송 경로만 갈아끼운다. 보안 경계는 TLS 가 아니라 우리 자신의 Noise 계층 하나다.

## Tailnet-style device mesh, without a VPN dependency

The useful Tailscale idea is stable device identity over replaceable network paths, not making a VPN account a product
dependency. runtrol applies that idea at its existing thin Core boundary:

- the PC Core is the stable node and owns supervised process lifetime
- VS Code and the phone are paired control surfaces, never session owners
- a phone pairs once to the PC identity and receives durable, explicitly scoped device authorization
- a session is addressed by PC identity plus runtrol session identity, so changing windows or links does not rename it
- the transport ladder chooses direct LAN or P2P when reachable and falls back to an E2E encrypted relay
- the relay sees routing presence and ciphertext only, and never receives a model credential or readable conversation
- an installed Tailscale route may become an optional discovered direct path later, but pairing, push, and correctness
  cannot depend on Tailscale, an identity provider, or one vendor's pricing

The VS Code extension therefore connects to the local Core exactly like any other trusted surface. Closing or
reloading the extension never ends a provider process. The phone joins the same Core session index through its own
scoped device identity instead of tunnelling through a VS Code window.

## 최상위 결정 넷

### 1. origin 은 불변, transport 는 교체 가능

브라우저에서 다음이 **전부 origin 에 묶여 있다**: service worker 등록 (따라서 push 전체), `PushSubscription`, IndexedDB (따라서 기기 개인키), iOS 홈화면 설치, Chrome 142+ Local Network Access 권한 부여.

**연결 전략이 origin 을 바꾸면 사용자는 신원과 push 구독과 설치된 앱을 잃는다.** 그래서 "Cloudflare quick tunnel 쓰면 되지" 는 아키텍처가 아니라 데모다. trycloudflare 와 ngrok 무료는 URL 이 돌아가므로 **탈락**이다.

선행 프로젝트 대부분이 이걸 우연히 맞히거나 (전송이 하나뿐이라) 틀린다.

```
                   불변 ORIGIN (정적 파일만. 데이터 없음)
                   - service worker
                   - IndexedDB: 기기 Ed25519 + X25519 (non-extractable)
                   - PushSubscription (PC 의 VAPID 키에 결박)
                           |
                   페이지가 런타임에 링크를 고른다
    +----------------+----------------+----------------+
   T0 loopback      T1 LAN 직결      T2 P2P 직결      T3 릴레이
   127.0.0.1        WebRTC host      WebRTC srflx     WSS
   데스크톱만        또는 HTTP+TAS    ~70~80% 성공     항상 된다
    +----------------+----------------+----------------+
                           |
              Noise_IK_25519_AESGCM_SHA256   <- 유일한 보안 경계
                           |
                   runtrol 프레임 스트림 (이벤트 로그 + 멱등 RPC)
                           |
                   Rust daemon -> claude / codex 자식 프로세스
```

정적 호스트도 릴레이도 **데이터를 신뢰받지 않는다.** 공급망 신뢰점이지 데이터 신뢰점이 아니다.

### 2. 릴레이가 가용성 보장이다. LAN 은 최적화다

**직관과 반대로 뒤집는다.** mDNS + 로컬 HTTPS 를 먼저 짓지 않는다. E2E 암호화된 릴레이 경로를 먼저 짓는다. **셀룰러에서, 게스트 Wi-Fi 에서, 회사망에서, iOS 에서 첫날부터 되는 유일한 경로**이기 때문이다.

모두가 LAN 우선으로 짓고, 그게 가용성이 제일 나쁜 경로다.

릴레이는 Cloudflare Worker + Durable Object 무료 티어 (WebSocket hibernation 이라 유휴 연결은 비용 0). **TURN 은 영원히 안 쓴다.** 앱 계층 릴레이가 같은 fallback 일을 공짜로 하면서 presence 와 순서까지 준다.

### 3. 인증서 문제는 풀지 않는다. 우회한다

브라우저 페이지가 `192.168.1.5` 에 신뢰할 수 있는 TLS 를 얻는 방법은 **없다.** CA/Browser Forum Baseline Requirements 가 공개 CA 의 RFC1918 주소 발급을 금지한다.

검토하고 기각한 것들:

- **sslip.io / nip.io + DNS-01 Let's Encrypt**: 암호학적으로는 정당하지만 DNS rebinding 보호, IP 별 재발급, 소유 도메인 필요, 그리고 **Certificate Transparency 에 내 LAN 토폴로지가 영구 기록**된다. 기각.
- **자체 서명 + 수동 신뢰**: iOS 에서 앱 두 개를 오가며 탭 7 번. 데스크톱 프로그램에 루트 CA 를 넘긴다. 기각.

**대신 LAN 홉에 TLS 를 안 쓴다.**

- **Safari (iOS)**: WebRTC 데이터 채널. DTLS 가 WebPKI 가 아니라 **SDP fingerprint** 로 인증하고, 그 fingerprint 를 우리 Ed25519 키로 다시 인증한다. 공개 HTTPS origin 의 페이지가 CA 없이, mixed-content 차단 없이 `192.168.1.5` 에 닿는다.
- **Chromium**: 더 싼 지름길. Chrome 142 의 LNA 권한이 승인되면 **로컬 요청에 대해 mixed content 차단이 완화**되어 `fetch(url, {targetAddressSpace:"private"})` 로 평문 HTTP 를 쓴다.

기밀성은 WebPKI 가 아니라 우리 Noise 계층에서 온다.

브라우저에는 mDNS API 가 아예 없다 (`navigator.mdns` 없음). WICG Local Peer-to-Peer 제안이 존재하는 이유가 정확히 그 공백이다. PWA 는 **이미 아는 주소만 probe** 할 수 있고, 그 주소는 페어링 QR 에서 오고 PC 가 현재 링크로 갱신한다.

### 4. Web Push 는 백엔드 0 으로 된다. 이게 진짜 unlock 이다

Rust 데몬이 **자기 VAPID 키쌍을 생성**하고, 브라우저의 push 구독을 E2E 채널로 받고, `fcm.googleapis.com` / `web.push.apple.com` 에 **직접 아웃바운드 HTTPS POST** 한다.

**서버 없음. Firebase 프로젝트 없음. Apple 개발자 계정 없음.**

하루 걸릴 함정들:

- Apple 은 VAPID `sub` 가 진짜 `mailto:` 또는 `https://` 가 아니면 403 BadJwtToken 을 낸다 (`@localhost` 불가)
- `userVisibleOnly` 는 필수다
- **push 는 초인종이지 파이프가 아니다.** 내용을 실어 보내지 않는다
- iOS 는 16.4+ 이고 홈화면 추가가 필수다
- Declarative Web Push (Safari 18.4+) 는 v1.1 최적화

## 페어링과 신원

**비밀번호 없음. passkey 를 전송 신원으로 쓰지 않음.**

QR 이 128 비트 일회용 PSK 를 나르고 **PC 에서의 승인이 필수**다. 그 다음 `Noise_IKpsk1_25519_AESGCM_SHA256`. 이 조합을 고른 이유는 **모든 프리미티브가 WebCrypto 에 네이티브로 있고** (X25519 deriveBits, HKDF-SHA256, AES-GCM) Rust 쪽 `snow` 에도 있기 때문이다. 이후 세션은 링크마다 `Noise_IK` 를 새로 돌리고 prologue 로 전송을 결박한다.

WebAuthn 은 전송 신원으로 **기각** (재연결마다 사용자 제스처 필요, PRF 가 iOS 로밍·크로스디바이스에서 깨짐). 위험 작업의 step-up 으로만 선택적으로 남긴다.

키 저장: non-extractable WebCrypto `CryptoKey` 를 IndexedDB 에, Windows 는 DPAPI 로 감싼다.

## 프로토콜 모양

**append-only 이벤트 로그 (u64 오프셋) + 멱등 RPC (UUIDv7) 를 나르는 바이너리 프레임 스트림.**

전송 전환은 그냥 재연결 + `SUBSCRIBE{from_offset}` 이다. **마이그레이션 코드 경로가 존재하지 않는다.** 이것이 북극성 `끊겨도 살아남기` 축을 구조적으로 보장한다.

## 단계

| 단계 | 무엇 | 왜 이 순서 |
|---|---|---|
| v0.1 | 릴레이 전용 + push | 첫날부터 셀룰러에서 된다. 게다가 릴레이는 이후 모든 계층이 쓰는 시그널링 채널이다 |
| v0.2 | Chromium LAN 지름길 | 싸다. LNA 권한 하나 |
| v0.3 | WebRTC (iOS LAN + P2P) | 인증서 문제의 진짜 우회 |
| v0.4 | 자체 호스팅 릴레이 | "아무도 안 거침" 요구에 대한 답 |
| v2 후보 | WebTransport `serverCertificateHashes` | 자체 서명 (14 일 미만, ECDSA P-256) 으로 **ICE 스택 전체를 삭제** |

## PC 가 꺼져 있으면

정직하게: 아무것도 안 된다. 제품의 답은 "PC 오프라인" 을 명확히 보여주는 것이다.

## 크레이트

WebRTC 는 `str0m` 이지 `webrtc-rs` 가 아니다 (후자는 무겁고 콜백·락 기반이라 메모리 예산 계약과 충돌).

## 선행 기술에서 배운 것

| 프로젝트 | 잘한 것 | 틀린 것 |
|---|---|---|
| **Happy** (22.9k star) | 오픈소스, E2E, 자체 호스팅 릴레이, 멀티 provider, 음성용 WebRTC P2P + 릴레이 fallback | 제어 채널이 **항상** 릴레이 경유 (세션 데이터에 LAN·P2P 없음). 공개 보안 리뷰 (discussion #680) 에서 **vendor API key 가 E2E 가 아니라** 서버측 `HANDY_MASTER_SECRET` 로 암호화, 기기 단위가 아닌 계정 단위, 관리자 답변 없음 |
| **Omnara** (YC S25) | 이 카테고리 최고의 제품 프레이밍, push 를 핵심 기능으로, write-only 에이전트 API 키 | 자체 호스팅 안 하면 대화가 자기 서버에 평문. $9/mo. 출시 시점 프론트엔드 비공개 |
| **Tailscale + tmux/ttyd 계열** | 진짜 사설 WireGuard, 공개 노출 0, 올바른 NAT 계층화 | 제품이 아니라 VPN 이다 (설치·IdP 로그인·배터리·MDM 충돌). 남의 회사 가격정책에 의존 (2026-04-08 리프라이싱). **push 알림이 아예 없다.** `ttyd` 는 침입자에게 닫힌 RPC 표면이 아니라 풀 셸을 준다 |

**Happy 의 API 키 사고가 runtrol 의 불변식을 정당화한다: 자식 CLI 의 자격증명에 아예 손대지 않는다.**

## Fixed operator decisions

1. The permanent origin is the existing GitHub Pages origin.
2. v0.1 uses the default ciphertext relay, while the relay origin remains replaceable local configuration.
3. A phone cannot start or resume outside exact VS Code-approved workspace roots and runtime-discovered provider identities.

## 원본

`.claude/discussion/r1/pwaConnectivity.md` (약 11,000 단어. 전송 후보별 정밀 평가, Noise 핸드셰이크 전문, VAPID 와이어 포맷, CSP, 인용 URL)
