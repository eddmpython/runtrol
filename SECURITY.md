# Security

runtrol supervises coding-agent CLI processes on a personal computer. A remote control surface for those
processes can read, write, and execute with the operator's account privileges. The security boundary therefore
starts before routing, stays default-deny after authentication, and never treats loopback as authentication.

## Current exposure

The shipping command and VS Code surfaces use the operating system local endpoint. Remote access is disabled until
the operator configures one exact HTTPS relay origin in VS Code and approves a phone pairing. When enabled, the
daemon creates only an outbound relay connection and accepts a phone surface only after a relay-bound Noise
handshake authenticates a restored paired-device key. The relay receives routing metadata and ciphertext, not a
model credential or readable conversation.

The optional direct browser ingress accepts only an explicitly supplied loopback listener. The product binary does
not open a public inbound TCP listener, and the ingress constructor refuses a non-loopback address.

## Local endpoint boundary

Windows uses a named pipe with an owner-only DACL, an explicit network deny entry, remote-client rejection, and
client process token validation. Linux and macOS use a Unix socket under an owner-only runtime directory, create the
socket with owner-only permissions, and validate the peer user identifier. Loopback is never treated as local-user
authentication.

## Invariants

- runtrol never asks for, holds, or forwards a model API key. Each child CLI owns its authentication.
- runtrol does not own a transcript and does not store a copy of conversation content.
- A remote device is denied every action until a PC-local presence flow grants explicit device scopes.
- A remote device can reduce its authority. It cannot grant itself authority.
- Device pairing, configuration writes, automatic approval, dangerous permission modes, consult wiring, and explicit
  overlapping-workspace starts are local-only types.
- Approval expiry is denial. An approval whose subject cannot be shown is denial-only.
- Stopping every supervised session is always available. Re-enabling work requires action at the PC.

## Default table

| Surface or control | Default | Can a phone widen it? |
|---|---|---|
| Local command endpoint | Owner-only OS endpoint | No |
| Browser-reachable listener | Disabled | No |
| Outbound relay connection | Disabled until one exact HTTPS origin is configured locally | No |
| Browser bind address when enabled | Loopback | No |
| LAN bind | One explicit interface address | No |
| Host allowlist | Exact loopback names and assigned port | No |
| Origin allowlist | Empty until an HTTPS PWA origin is configured | No |
| Device credential registry | Empty | No |
| Cookie authentication | Never | No |
| CORS wildcard | Never | No |
| Credentialed CORS | Never | No |
| Browser protocol header | `X-Runtrol-Proto: 1` required | No |
| State-changing browser fetch metadata | `same-origin` or `none` required | No |
| WebSocket boundary | Host, Origin, fetch metadata, and subprotocol before 101; paired Noise key before Core data | No |
| Unknown, duplicate, or malformed security header | Deny | No |
| Outbound phone destination | Empty exact IP and port allowlist | No |
| Relay payload | Fresh Noise IK ciphertext for every link | No |
| Remote workspace | A PC-registered workspace root only | No |
| Provider credentials | Owned by the child CLI | No |

The phone-facing HTTP wrapper validates Host before Origin and routing. Routed HTTP requests accept one exact header
value, reject duplicates, require an explicit HTTPS Origin and the non-simple protocol header, establish a device
caller from an Authorization bearer credential, and remove Cookie before any handler runs. A WebSocket upgrade is
admitted only for the exact path, Origin, fetch metadata, and Noise subprotocol. It grants no caller. The paired
static key establishes the caller after the upgrade and before any Core request is accepted. Every HTTP response
removes `Set-Cookie`, wildcard or handler-supplied CORS origin values, and
`Access-Control-Allow-Credentials`, then adds only the exact configured origin and `Vary: Origin`.

The phone channel uses `Noise_IK_25519_AESGCM_SHA256` with pinned X25519 static keys. Its prologue binds the link
kind, relay origin, and peer identity so a captured relay handshake cannot move to a direct link. Pairing uses
`Noise_IKpsk1_25519_AESGCM_SHA256`; a domain-separated HKDF expands the 128-bit QR value into Noise's 32-byte PSK.
Messages are capped at 65,519 plaintext bytes, larger frames are bounded and chunked, and the channel rekeys after
2^24 messages or 15 minutes. Secret key material clears on drop and has no diagnostic representation.

A pairing QR is valid for 120 seconds, is destroyed on the first authenticated message, and locks after five bad
messages. Noise message two is withheld until a fresh PC presence witness names the exact attempt, authenticated
static key, validated device name, and platform. Generic pairing authority or a witness for another attempt is not
accepted. The device identifier is minted only after that witness matches.

Outbound TCP requires an exact IP address and port minted by an immutable `EgressPolicy`. The operating-system
connect call exists only inside that policy and an empty policy denies every destination.

`Sec-Fetch-Site` is a CSRF control, not a DNS rebinding control. After rebinding, a browser can still report
`same-origin`. Exact Host validation is the control that stops rebinding.

## Honest limits

- A supervised CLI runs as the operator and can read anything that account can read. runtrol cannot make an
  already-authorized coding agent harmless.
- CLI output can contain secrets. runtrol can keep an untrusted relay from reading encrypted traffic, but it cannot
  promise that a user's own paired device will never display sensitive output.
- A panic stop cannot undo files a CLI already changed.
- A compromised paired phone can act within the scopes and workspace roots the PC granted until that device is
  revoked.

## Verification

The `rebindingDefenses` gate sends real HTTP/1.1 requests through a real loopback TCP socket. It checks accepted
device identity, malicious Host rejection, missing and unknown Origin rejection, CSRF metadata, the mandatory
protocol header, cookie non-authentication, exact CORS preflight behavior, and authentication before WebSocket
upgrade. Scope grantability, request authorization coverage, process argument escaping, transcript-copy absence,
and provider configuration read-only behavior are separate merge-blocking gates.

The `egressContract` gate dials real allowed and refused loopback destinations, completes production Noise session
and pairing handshakes, crosses the Noise chunk boundary, round-trips length-prefixed ciphertext, rotates keys, and
injects wrong keys, PSKs, link bindings, and ciphertext corruption. It also checks that transport has no disk or
logging API and that drivers and storage name no provider-owned transcript path.

The `pairingLifecycle` gate proves the 120-second lifetime, single use, five-attempt lockout, prompt-injection
defense, exact witness binding, and an encrypted round trip that becomes possible only after PC approval.

The `phoneDrivesPcSmoke`, `approvalRoundtripSmoke`, and `remoteResilienceFaultInjection` gates run the shipped PWA
WebCrypto, Noise, record, and Core client modules in a headless phone process. They cross the production daemon and
an installed real CLI, prove exact device, workspace, provider, approval, reconnect, and resume authority, and require
every process they start to be gone afterward. The model endpoint is a deterministic loopback fixture that discards
request bodies.

Web Push contains no event or conversation body. The daemon stores each browser-issued capability endpoint as
device-bound protected ciphertext and sends an empty HTTP body only to the exact supported push-service origins.
The service worker displays one generic wake notification and fetches actual state through the authenticated Noise
channel.

## Reporting a vulnerability

Do not include credentials, conversation content, or private source code in a public report. Use the repository's
private security advisory channel when available. Include the affected version, operating system, minimal request
shape, observed result, and expected refusal.
