# Security

runtrol supervises coding-agent CLI processes on a personal computer. A remote control surface for those
processes can read, write, and execute with the operator's account privileges. The security boundary therefore
starts before routing, stays default-deny after authentication, and never treats loopback as authentication.

## Current exposure

The shipping command and desktop surfaces use the operating system local endpoint. The browser-reachable phone
plane is not enabled by default and is not yet exposed by the product binary. Its HTTP admission layer exists so
the relay, pairing, and PWA work can only be connected behind the rules below.

## Invariants

- runtrol never asks for, holds, or forwards a model API key. Each child CLI owns its authentication.
- runtrol does not own a transcript and does not store a copy of conversation content.
- A remote device is denied every action until a PC-local presence flow grants explicit device scopes.
- A remote device can reduce its authority. It cannot grant itself authority.
- Device pairing, configuration writes, automatic approval, and dangerous permission modes are local-only types.
- Approval expiry is denial. An approval whose subject cannot be shown is denial-only.
- Stopping every supervised session is always available. Re-enabling work requires action at the PC.

## Default table

| Surface or control | Default | Can a phone widen it? |
|---|---|---|
| Local command endpoint | Owner-only OS endpoint | No |
| Browser-reachable listener | Disabled | No |
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
| WebSocket authentication | Before the HTTP 101 response | No |
| Unknown, duplicate, or malformed security header | Deny | No |
| Remote workspace | A PC-registered workspace root only | No |
| Provider credentials | Owned by the child CLI | No |

The phone-facing HTTP wrapper validates Host before Origin and routing. It accepts one exact header value, rejects
duplicates, requires an explicit HTTPS Origin, requires the non-simple protocol header, establishes a device caller
from an Authorization bearer credential, and removes Cookie before any handler runs. Every response removes
`Set-Cookie`, wildcard or handler-supplied CORS origin values, and `Access-Control-Allow-Credentials`. It then adds
only the exact configured origin and `Vary: Origin`.

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

## Reporting a vulnerability

Do not include credentials, conversation content, or private source code in a public report. Use the repository's
private security advisory channel when available. Include the affected version, operating system, minimal request
shape, observed result, and expected refusal.
