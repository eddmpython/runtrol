# Changelog

## Unreleased

- Advertise optional native activity on provider subscriptions. Explicit opt-in requires native-discovery authority;
  existing provider watches keep their notification vocabulary.
- Advertise optional workspace-folder updates that preserve window registration and owner input authority.
  Older peers omit the new capability and retain terminal-only updates.
- Advertise owner-approved terminal input precedence and negotiate grant scope projection for older clients.
  Missing capability fields retain the previous initialization and input behavior.
- Terminal control acquisition accepts `onlyIfFree` to atomically refuse replacing an unexpired holder.
  Omitting it preserves the existing request and explicit control transfer.
- `CatalogueSource` gains `providerStore`: a native session catalogue named from the provider's own store
  (identity, folder, the provider's own title and time) when the provider publishes no listing surface.
  Consumers that match the source exhaustively add the arm; the schema and generated bindings carry it.

## 0.1.1

- Finalized public Runtime revision `2026-08-13`.
- Added closed JSON-RPC methods, scopes, failures, session lifecycle, approvals, and integration identity DTOs.
- Added numeric limits, locator schema, native catalogue coverage, mutation identity, and event cursor contracts.
- Added deterministic checked JSON schema generation.
