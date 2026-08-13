# Runtrol Runtime protocol

This crate is the provider-neutral public wire vocabulary for Runtrol Runtime. Its Rust DTOs generate the checked
JSON schema shipped with Runtime and the TypeScript client package.

The protocol contains no provider implementation, provider credential, model API, transcript store, daemon control,
or consumer UI. Consumers should normally use `runtrol-runtime-client` instead of constructing JSON-RPC frames.

Protocol revisions are negotiated independently from crate SemVer. The finalized revision inventory is exported as
`FINALIZED_REVISIONS` and recorded in `schema/runtime.schema.json`.

Version 0.1.1 defines finalized revision `2026-08-13` and is tested with Runtime and client packages 0.1.1.
