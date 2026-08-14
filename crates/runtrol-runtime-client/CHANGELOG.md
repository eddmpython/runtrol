# Changelog

## 0.1.1

- Added the initial typed client for finalized Runtime revision `2026-08-13`.
- Added owner-validated locator discovery, signed enrollment, authenticated reconnect, and key rotation.
- Exposed read-only fields from the validated locator for native bootstrap adapters.
- Added provider, session, approval, control lease, mutation, and bounded watcher APIs.
- Added read-only reconnect helpers that preserve accepted cursors and never retry mutations.
- Added packed repository-external consumer verification.
