# Changelog

## Unreleased

- Negotiate terminal input priority without breaking older Runtime authentication or older grant vocabularies.
- Allow a bounded initialization retry while a draining generation receives its current owner-approved grant.
  Primary authentication failures remain immediate, and ordinary requests are never replayed.
- Add typed observed-terminal text receipts and a bounded owner input duplex with cancellation-safe teardown.
- Preserve unknown input outcomes without replay and reject mismatched receipts or queue overflow.

## 0.1.1

- Added the initial typed client for finalized Runtime revision `2026-08-13`.
- Added owner-validated locator discovery, signed enrollment, authenticated reconnect, and key rotation.
- Exposed read-only fields from the validated locator for native bootstrap adapters.
- Added provider, session, approval, control lease, mutation, and bounded watcher APIs.
- Added read-only reconnect helpers that preserve accepted cursors and never retry mutations.
- Added packed repository-external consumer verification.
