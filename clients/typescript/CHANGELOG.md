# Changelog

## Unreleased

- Provider watches can opt into native activity and catalogue revisions when the connected Runtime advertises
  support. The subscription exposes whether negotiation succeeded so older Runtime observation remains explicit.
- Observe validated Runtime generation changes through one cancellable directory watch. Incarnation revisions
  distinguish same-build restarts, and cancellation joins pending verification before observation ends.
- Supporting Runtimes accept workspace-folder updates without replacing window registration or input proof.
- Add `session.input.priority` and `terminalInputPriority` capability discovery for owner-approved terminal
  input precedence. Earlier Runtime generations retain their original control policy.
- Terminal control acquisition can require that no unexpired holder exists through `onlyIfFree`.
- Add observed-terminal text receipts and a bounded, serial owner input subscription with explicit claim and receipt.
- Preserve unknown outcomes without replay and validate receipt identities and negotiated offer bounds.

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
