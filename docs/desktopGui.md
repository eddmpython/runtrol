# Desktop GUI

The desktop GUI is a Tauri v2 window embedded in the `runtrol` executable. It is a local view and control surface for the daemon. It does not supervise provider processes, discover transcript paths, or own a conversation copy.

## Runtime ownership

`runtrol gui` starts or reuses the local daemon through the same CLI boundary as every other local caller. The daemon owns provider discovery, sessions, bounded live frames, and cleanup. Closing the window closes the GUI and its WebView tree while the daemon and its sessions continue under their own lifecycle.

Product builds enable the `runtrol-gui/custom-protocol` feature and embed the production frontend. A canonical build attestation binds the source revision and workspace state, build-relevant source tree, embedded `dist` tree, product executable, and ACP workload fixture. Actual-shell, console, IME, and memory journeys validate that attestation before and after using the product.

The GUI persists only two scalar preferences: the light or dark theme and the last successfully used provider identifier. Session rows come from the daemon. Conversation frames and drafts are not written to GUI storage, and a reload removes the bounded live view rather than reconstructing a transcript.

## Session surface

The left rail is one provider-neutral session list. A row carries the provider as metadata, not as a separate product silo. The production surface supports:

- metadata search across the current list;
- starting with the last successfully used provider without reopening the provider picker;
- opening a hot session immediately from its bounded live tail;
- cold resume through the provider's discovered native surface;
- editing the next draft while a previous prompt is in flight;
- usage and rate-limit status outside the conversation frame stream;
- confirmed removal of the runtrol list pointer without deleting the provider-owned conversation.

The current provider boundary has no structured title capability. A row label uses the opaque provider-native name when one exists and otherwise a short runtrol session identifier. Workspace sections use the platform-normalized final path component. Metadata search covers provider, native name, session identifier, workspace, and folder without reading conversation text.

The actual product journey uses a real Tauri window, Rust commands, local IPC, Tauri events, the daemon, and a deterministic external ACP fixture. Hosted browser lifecycle tests use the production bundle with a mocked Tauri transport, so they do not claim a real provider account or provider-owned history.

## Bounded rendering and continuity

Every view owns its watch cursor and generation. Reconnect drains the accepted generation, rejects stale and cancelled watches, and resumes from an exact cursor or shows an explicit gap. The renderer never infers transcript offsets.

The retained conversation view is bounded to 400 items and 256 KiB of text. When one oversized item crosses the byte bound, the oldest prefix is trimmed so the newest reply tail and later status frames remain visible. A Web Worker applies frames outside the input path. Production memory evidence requires one checkpoint identity to appear first in the worker-applied trace and then in an actual `.verbatim` DOM paint after two animation frames, with both sides at or above the 256 KiB retained bound.

The browser interaction ratchets currently require list paint p95 at or below 900 ms, session open at or below 100 ms, and input p95 at or below 50 ms. The load smoke injects about 3,000 frames per second and requires at least 29,300 frames, frame p95 at or below 24 ms, frame maximum at or below 120 ms, input p95 at or below 50 ms, and a bounded DOM. These measurements use the production bundle and a transport mock, so model and network time are intentionally excluded.

## Text input and Korean IME

The composer keeps editing independent from an in-flight prompt and preserves the next draft. Composition state uses both the native `isComposing` signal and explicit composition lifecycle events. WebView2 may report `compositionend` immediately before the Enter that commits the last Korean preedit, so a short one-shot guard stops that Enter from reaching the submit handler.

The guard does not cancel the keydown default, because Windows IME needs it to finish the native commit. Instead, a native capture listener on the exact contenteditable consumes only the matching cancelable `beforeinput` event whose type is `insertParagraph` or `insertLineBreak`. It leaves composition text, normal line breaks, token nodes, later Enter presses, and other sessions untouched. Session changes, editable replacement, and unmount clear composition state, timer generations, and listeners.

`desktopTextInputSmoke` proves the browser event and lifecycle cases with synthetic composition plus real browser key input. `desktopImeSmoke` is a separate Windows product journey. It sends physical virtual keys through the installed Korean IME, requires exact selected and copied Korean text, checks composition and commit-break traces, and requires zero submissions. The driver restores pending key state, the original IME mode and open state, the exact keyboard-layout handle, and the operator's multi-format clipboard before it closes every process it started.

## Console and cleanup policy

A GUI launched into a private new console hides that console before the first production list paint. A GUI that inherits a console shared with PowerShell or another parent leaves the shared console visible. In both cases the Tauri window must remain visible.

Actual product gates capture process identity as PID, creation time, and image identity. Cleanup attempts GUI, session, daemon, fixture, and WebView actions independently, revalidates identity immediately before forced termination, and requires zero exact survivors. Daemon readiness failure uses terminate, bounded wait, kill, and reap before ownership can escape the caller.

## Memory contracts

GUI memory is separate from the daemon idle budget. The measurement includes the production GUI root and every descendant WebView2 process. It excludes the separately started daemon and workload fixture. Both private bytes and working set are retained because WebView shared pages make either value misleading alone. Process topology is a separate ceiling, while sample and churn continuity use fixed cadence-derived limits rather than becoming memory headroom.

The pull-request smoke budget is seeded from five independent 60-second records of one clean commit and one canonical product. Its exact provenance and ceilings live in [`guiMemorySmokeBudget.json`](../tests/audit/guiMemorySmokeBudget.json). The 24-hour campaign uses the same measurement core at a longer cadence and has its own tracked [`guiMemoryCampaignBudget.json`](../tests/audit/guiMemoryCampaignBudget.json). Raw records remain CI artifacts; the reviewed budgets are the repository contracts. Neither budget adds an invented multiplier or silent headroom.

The current actual-shell, console, IME, and memory evidence is Windows WebView2 evidence. Linux and macOS compile and browser gates are active, but no equivalent actual Tauri-window, OS IME, console, or GUI process-tree claim is made for those platforms yet.

## Supply-chain cost

Tauri is the measured shell choice because it met the Korean text floor, stayed inside the burst-render frame budget, and shares the Astryx React component layer with the web surfaces. Its WebView and Linux backend dependencies are a real supply-chain cost. [`deny.toml`](../deny.toml) is the rejection ledger for the inherited unmaintained advisories. New vulnerabilities remain merge failures.

## Evidence map

- `desktopLifecycleSmoke`: production browser metadata search, native-name and session fallback labels, session start, hot and cold open, editable next draft, and confirmed list removal with mocked transport.
- `actualShellSmoke`: actual Tauri window, embedded production bundle, Rust commands, local IPC, events, daemon, and external ACP fixture.
- `desktopTextInputSmoke`: browser composition, native paragraph suppression, selection, copy, token preservation, and lifecycle reset.
- `desktopImeSmoke`: actual Windows Korean IME physical-key journey and operator-state restoration.
- `interactionLatencyBudget`, `scrollUnderLoadSmoke`, and `reconnectContinuitySmoke`: local responsiveness, bounded rendering, and cursor continuity.
- `guiMemoryContract`: canonical build provenance, same-ID 256 KiB DOM paint, WebView2 process-tree budgets, continuity, and exact cleanup.
- `desktopThinBoundary`, `desktopPersistenceSmoke`, and `desktopConsolePolicy`: thin authority, non-persistence, and Windows console presentation floors.
