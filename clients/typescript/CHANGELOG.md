# Changelog

## Unreleased

- Added `TerminalView.setDialogue` and `TerminalDescriptor.dialogueEnabled` for local input-lease control of a
  live process's courier lifetime.
- Added `viewerCount` to the terminal descriptor: how many views are attached right now, a proved engine fact
  that changes the index when a view attaches or ends and never implies model work.
- `SessionDescriptor.looksStuck` is retired and now optional (always `false` from a current Runtime): a
  silence-based hint is not a proved state, so the Runtime no longer says it.
- Added `checkpointAvailable` to the terminal view opened and lagged messages (regenerated bindings).
- Added `controlGeneration` and `controlHeld` to the terminal descriptor: exactly one view holds a terminal's
  control lease, and acquiring it transfers it (an earlier holder's next write answers `controlConflict`).
- Added the window registry: `windows/register`, `windows/update`, `windows/list`, `windows/watchIndex` and the
  `windows/indexChanged` and `windows/indexEnded` notifications, with `WindowClient` and `WindowIndexSubscription`.
- Added `WindowClient.mirrorOpen`, `mirrorOutput` and `mirrorEnd`; `TerminalDescriptor.origin`,
  `ownerWindowSessionId` and `ownerTerminalKey`; `ProviderDescriptor.commandNames`.
- Added `WindowClient.reveal` and `watchReveals` with `WindowRevealSubscription`, and `WindowRegisterParams.hostPid`.
- Added `NativeActivity.focusable` and `ProviderClient.focusNative` (`providers/focusNative`, answering
  `NativeFocusResult`).

## 0.1.1

- Added finalized Runtime revision `2026-08-13` bindings and runtime message validation.
- Added owner-validated system locator discovery and signed integration identity helpers.
- Added optional exact-executable native Windows locator verification with post-validation record matching.
- Added provider, session, approval, control lease, mutation, and bounded watcher clients.
- Added read-only reconnect helpers that never retry mutations or reacquire control.
- Coalesced each framed request into one local transport write.
- Retire local transports with a graceful socket end so immediate Windows named-pipe reconnects reach a fresh server instance.
- Added packed external-consumer verification and the checked public schema.
