# Phone PWA

The phone application is a paired control surface for the same Core that Runtrol Studio uses. It never owns a provider process or provider transcript.

## Current release

The application is served from `/runtrol/app/` on the permanent GitHub Pages origin. The supported connection path is the ciphertext relay. Direct LAN and peer-to-peer routes, Web Push, and remote Mission views are not part of this release.

Pairing begins only from `Runtrol: Pair a Phone` in VS Code. The QR carries a 120-second one-use pairing secret in the URL fragment. The PWA validates the complete fragment contract and removes it from the visible URL and history before opening any durable storage. Noise IKpsk1 authenticates the first device exchange. VS Code then displays the authenticated phone key and exact initial scopes and requires the operator to type the local three-word presence phrase.

Later connections use Noise IK with the pinned PC and phone static keys. Relay tickets and bearer credentials admit a route but do not decrypt records. The relay receives peer presence, record sizes, timing, and ciphertext. It does not receive provider credentials or readable conversation content.

## Device authority

Initial pairing grants only selected plain scopes. Starting or resuming a session additionally requires a runtime-discovered provider identity and a canonical workspace root approved in VS Code. Each root is bound to a minted root identifier and the operating system's directory identity. Replacing the directory at the same path invalidates the grant.

`Runtrol: Manage Paired Phones` shows the complete plain scopes, workspace roots, and provider identities. Changing them creates a new local presence challenge and atomically replaces the durable authority. The authenticated Core greeting returns the current authority only to that same device, so a PWA reconnect immediately loses controls that were removed on the PC.

## Browser storage and rendering

IndexedDB contains only the non-extractable X25519 private key, its public key, relay admission values, and the latest authority metadata. There is no transcript table or conversation cache. The service worker caches only reviewed application-shell files and never caches relay or Core traffic.

The selected session owns one bounded live event view. Reconnect uses the exact Core cursor and reports an explicit gap when retained events are no longer available. ANSI control sequences are removed, bidirectional controls are expanded visibly, and approval options remain visible when unavailable so missing authority is not mistaken for a missing provider choice.

## Verification

- `npm --prefix pwa test` checks pairing-fragment handling, display hardening, canonical record framing, relay admission, explicit Core requests, and current-authority replacement.
- `cargo test -p runtrol-audit --test pwaInterop` runs a real WebCrypto peer against the production Rust transport for Noise IKpsk1 pairing, Noise IK reconnection, and bidirectional encrypted frames.
- daemon and security tests verify exact scope, root, provider, filesystem-identity, persistence, and Start and Resume enforcement.
- `npm --prefix site run build` publishes the reviewed application under `/app/` and holds the complete Pages artifact under the repository byte budget.
