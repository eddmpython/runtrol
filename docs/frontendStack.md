# Frontend Stack

The public surfaces share product contracts and brand assets, not a mandatory component library.

## Surface choices

| Surface | Stack | Reason |
|---|---|---|
| GitHub Pages landing | Static HTML, CSS, and small browser JavaScript | Main content works without JavaScript, no package runtime is needed, and the complete output stays under a hard byte budget |
| VS Code extension | Native VS Code navigation plus one bounded editor Webview for the selected live session | Gives the conversation editor width while keeping renderer cost independent of logical session count |
| Phone PWA | Static HTML, CSS, browser JavaScript, WebCrypto, IndexedDB, and a service worker | Keeps the installed origin dependency-free while implementing the exact Noise record and reconnect contracts used by Core |

The previous desktop-window frontend was removed when the PC surface became VS Code-only. A static distribution page and the phone PWA do not carry an application component runtime. Both reuse the canonical brand assets and the PWA renders only one bounded current session view or bounded Mission metadata view.

## Shared contracts

All public surfaces must satisfy these rules:

- Reuse canonical files from [`assets/brand/`](../assets/brand/) and never redraw the mark.
- Use the canonical coral `#F56565`, graphite, white, and the two-tone mark whose ink arms follow the theme.
- Support light and dark color schemes without adding a glow, gradient, outline, or shadow to the mark.
- Keep navigation and input usable with a keyboard and expose meaningful accessible names.
- Respect reduced-motion preferences.
- Persist only small interface preferences such as locale and theme. Never persist conversation content.
- Render only bounded live data owned by the selected session.
- Treat a visible loading delay or input stutter as a failed product gate.
- Do not load public fonts, icon kits, CSS, or application scripts from a CDN.

## Landing implementation

[`site/`](../site/) contains the dependency-free source. `build.mjs` copies only the reviewed source and canonical brand assets into `site/dist`, creates `.nojekyll`, and rejects output above 600,000 bytes. The ceiling includes the locally vendored xterm browser terminal so the phone surface needs no CDN at runtime. `test.mjs` verifies the English static default, four locale dictionaries, the live Marketplace route, latest-release native VSIX selection, CDN independence, and forbidden punctuation. It also proves that five injected defects fail before the valid source passes.

The landing uses one source object for the repository and latest-release endpoint. A manual download becomes active only when the GitHub API returns a `.vsix` asset, and `release-assets.mjs` matches the runtime operating system and architecture to one of the six native packages. An unknown target opens the tagged release instead of guessing. The page never hardcodes a release version. The same build copies the reviewed PWA output to `/app/` and measures both surfaces against the one byte budget.

## VS Code implementation

The session TreeView and QuickPick own navigation for every logical session. Selecting a row opens one reusable
conversation tab in the editor area. Only the selected session owns the full watch and Webview renderer. A custom
operator name is primary when present. Otherwise Studio uses the provider's own catalogue title when it publishes
one, refreshing that metadata after a native identity appears and after each turn settles. A project and provider
fallback remains when the provider supplies no title, and a short stable discriminator separates collisions. Stable
selected-first ordering, fuzzy metadata search, workspace following, hot process bounds, and cold provider-native
resume are release contracts, not styling preferences.

Webview code must use the VS Code state and message boundary already covered by extension gates. It must not introduce a second transcript store or provider-specific product branch.

## Phone implementation

[`pwa/`](../pwa/) is an installable static application under the permanent Pages origin. It consumes pairing material only from a URL fragment, removes that fragment immediately, keeps its non-extractable X25519 private key and connection secrets in IndexedDB, and accepts the current device authority from each authenticated Core greeting. It stores no conversation content. The service worker caches only the application shell.

The phone uses the relay transport in the current release. Noise IKpsk1 protects the one-use pairing exchange and Noise IK protects every later session. The relay receives routing presence and encrypted records only. Web Push is a separate content-free wake path: the service worker renders one generic notification and reconnects through the normal encrypted stream when opened. Direct LAN and peer-to-peer routing remain later connection optimizations and must not be implied by the current UI.

On a narrow screen, the session list is the initial surface. Core's bounded `person` versus `quota` waiting fact drives
one `Needs you` count and a cyclic next action; conversation events are not parsed to choose a destination. A generic
notification carries only an attention intent, then the authenticated session catalogue decides the current target.
No session identifier enters the notification, launch URL, service-worker cache, or durable browser storage.

The Mission tab exists only when the authenticated greeting holds `mission.read`. It validates a closed set of
Mission and Task states, bounds the catalogue and snapshot, renders with text nodes, and shows pause, safe resume, or
cancel only when the current state and matching exact scope both permit that action. Mission creation, reviewed
start, Task instruction submission, integration, archive, Gate registration, and capability trust changes remain
local-only VS Code actions.

`phoneDrivesPcSmoke` and `approvalRoundtripSmoke` execute the same shipped WebCrypto, Noise, record, and CoreClient
modules in a headless phone process. They connect to the production daemon, drive an installed real CLI, observe
streamed output or a real permission request, expose and clear that request through the phone focus catalogue, close
the session, and verify process cleanup. The deterministic local model fixture discards request bodies, so these gates
do not claim account-backed model behavior.

`remoteResilienceFaultInjection` uses the same modules and CLI boundary. It cuts the authenticated watch socket,
requires exact bounded replay without duplicate cursors, recomposes the production Core over the same durable home,
requires an explicit cross-stream gap, and continues through the provider-native resume surface.
