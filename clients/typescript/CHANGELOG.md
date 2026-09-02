# Changelog

## Unreleased

- Added `checkpointAvailable` to the terminal view opened and lagged messages (regenerated bindings).
- Added `controlGeneration` and `controlHeld` to the terminal descriptor: exactly one view holds a terminal's
  control lease, and acquiring it transfers it (an earlier holder's next write answers `controlConflict`).

## 0.1.1

- Added finalized Runtime revision `2026-08-13` bindings and runtime message validation.
- Added owner-validated system locator discovery and signed integration identity helpers.
- Added optional exact-executable native Windows locator verification with post-validation record matching.
- Added provider, session, approval, control lease, mutation, and bounded watcher clients.
- Added read-only reconnect helpers that never retry mutations or reacquire control.
- Coalesced each framed request into one local transport write.
- Retire local transports with a graceful socket end so immediate Windows named-pipe reconnects reach a fresh server instance.
- Added packed external-consumer verification and the checked public schema.
