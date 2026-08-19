# Phone PWA

The phone application is a paired control surface for the same Core that Runtrol Studio uses. It never owns a provider process or provider transcript.

## Current release

The application is served from `/runtrol/app/` on the permanent GitHub Pages origin. The supported connection path is the ciphertext relay. Content-free Web Push wakes the installed application when Core publishes an event that needs attention. A scope-gated Mission view exposes bounded metadata plus pause, safe resume, and cancel actions. Direct LAN and peer-to-peer routes are not part of this release.

Pairing begins only from `Runtrol: Pair a Phone` in VS Code. The QR carries a 120-second one-use pairing secret in the URL fragment. The PWA validates the complete fragment contract and removes it from the visible URL and history before opening any durable storage. Noise IKpsk1 authenticates the first device exchange. VS Code then displays the authenticated phone key and exact initial scopes and requires the operator to type the local three-word presence phrase.

Later connections use Noise IK with the pinned PC and phone static keys. Relay tickets and bearer credentials admit a route but do not decrypt records. The relay receives peer presence, record sizes, timing, and ciphertext. It does not receive provider credentials or readable conversation content.

## Device authority

Initial pairing grants only selected plain scopes. Starting or resuming a session additionally requires a runtime-discovered provider identity and a canonical workspace root approved in VS Code. Each root is bound to a minted root identifier and the operating system's directory identity. Replacing the directory at the same path invalidates the grant. Workspace roots bound both disclosure and action: the session index a phone lists or watches contains exactly the sessions inside its live approved roots (with no storage warnings), every session command it sends is verified against the same roots (so a session identity learned before a root was revoked stops working the moment the root does), and Mission reads answer only the Missions of those roots, with a Mission outside them indistinguishable from one that does not exist.

`Runtrol: Manage Paired Phones` shows the complete plain scopes, workspace roots, and provider identities. Changing them creates a new local presence challenge and atomically replaces the durable authority. The authenticated Core greeting returns the current authority only to that same device, so a PWA reconnect immediately loses controls that were removed on the PC.

## Browser storage and rendering

IndexedDB contains only the non-extractable X25519 private key, its public key, relay admission values, and the latest authority metadata. There is no transcript table or conversation cache. The service worker caches only reviewed application-shell files and never caches relay or Core traffic.

The selected session owns one bounded live event view. Reconnect uses the exact Core cursor and reports an explicit gap when retained events are no longer available. ANSI control sequences are removed, bidirectional controls are expanded visibly, and approval options remain visible when unavailable so missing authority is not mistaken for a missing provider choice.

The active `remoteResilienceFaultInjection` gate cuts an authenticated phone watch socket and requires the retained
suffix to resume from the exact cursor without a duplicate. It then aborts and recomposes the production server over
the same durable home and device authorization, requires a cross-stream gap for the old cursor, and continues the
installed real CLI through its provider-native resume surface. The deterministic model counterpart is local and
discards request bodies.

## Web Push

Notifications are enabled only by an explicit phone action. The browser subscription is bound to the stable P-256 VAPID public key returned by the authenticated PC. The PWA sends only the browser-issued endpoint through the existing Noise channel. It does not send content-encryption keys because Core deliberately sends an empty push body.

Core derives separate VAPID signing and endpoint-storage keys from the operating-system-protected PC identity. The subscription endpoint is a bearer capability, so it is validated against the reviewed FCM and Apple push hosts, encrypted with device-bound authenticated encryption, and authenticated again during daemon restoration. Plain endpoint bytes are never stored.

For an approval, blocked session, failure, or abnormal detach, Core selects only subscribed devices that hold `session.output.read`, resolves the reviewed push host, rejects the complete DNS answer set if any address is non-public, applies exact egress admission, authenticates WebPKI TLS, and sends an empty HTTP/2 POST with bounded VAPID authorization. The service worker ignores push data and always renders the same generic notification. Session, provider, workspace, prompt, approval subject, output, and identifiers never enter the push request or notification. The reconnect stream remains authoritative if delivery fails.

iOS and iPadOS require adding the application to the Home Screen before notification permission can be granted. Browser subscription removal and `Forget this PC` both clear the server-side capability when Core is reachable. A locally removed browser subscription is already unusable if Core is offline.

## Physical iOS verification status

The production surface and automated contracts are complete. This repository does not currently hold an observation of a physical iPhone installing the PWA from the public origin and receiving an Apple Web Push notification. That operator-only observation is outside the current completion scope and score. It must not be reported as passed until a contributor supplies the evidence.

A useful contributor receipt records the date, iOS version, public application origin, Home Screen launch, notification permission state, receipt of the generic notification, and successful reconnect after opening it. It must not contain a subscription endpoint, device key, QR fragment, pairing secret, conversation content, or a sensitive screenshot.

## Verification

- `npm --prefix pwa test` checks pairing-fragment handling, display hardening, canonical record framing, relay admission, explicit Core requests, current-authority replacement, VAPID binding, and subscription removal.
- `cargo test -p runtrol-audit --test pwaInterop` runs a real WebCrypto peer against the production Rust transport for Noise IKpsk1 pairing, Noise IK reconnection, and bidirectional encrypted frames.
- `cargo test -p runtrol-audit --test webPushContract` verifies device-bound encrypted endpoint storage, stable VAPID public-key shape, the empty production request body, and the generic service-worker notification boundary.
- `python -X utf8 tests/audit/remoteResilienceFaultInjection.py --require-external` verifies exact replay after a network cut and explicit recovery through provider-native resume after a production Core restart.
- daemon and security tests verify exact scope, root, provider, filesystem-identity, persistence, and Start and Resume enforcement.
- `npm --prefix site run build` publishes the reviewed application under `/app/` and holds the complete Pages artifact under the repository byte budget.
