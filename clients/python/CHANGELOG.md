# Changelog

## Unreleased

- Add `session.input.priority` and `terminalInputPriority` capability discovery for owner-approved terminal
  input precedence. Earlier Runtime generations retain their original control policy.
- Terminal control acquisition can require that no unexpired holder exists through `onlyIfFree`.
- Add synchronous and asynchronous observed-terminal text input and typed owner window registration and duplex APIs.
- Keep owner operations serial and bounded, close cancelled receivers, and never replay an unknown input outcome.

- Add `setDialogue` to terminal views and the generated dialogue control parameters and descriptor state.

## 0.1.1

- Add the first official Python 3.11+ stable-ABI client for the shared Runtrol Runtime.
- Add exact-generation terminal listing and reattachment with typed continuity errors.
