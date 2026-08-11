# Runtrol Studio for VS Code

Runtrol Studio is the primary runtrol surface. It connects one VS Code window to the existing thin Rust supervisor,
shows every runtime-discovered coding-agent CLI and session, keeps one selected event stream hot, and switches the
window to that session's exact workspace or worktree.

The extension does not read provider transcript files, keep a conversation copy, hold model credentials, or run an
agent loop. The installed provider CLI remains the owner of the conversation and repository changes.

## Development

```text
npm install
npm run check
npm test
npm run build
```

Set `runtrol.corePath` to a local debug or release executable while developing. A packaged platform VSIX contains one
matching runtrol core under `resources/core/`.

## Package Windows x64

Build the Rust release binary, then run:

```text
cargo build --release --bin runtrol
npm run package:win32-x64
```

`RUNTROL_CORE_BINARY` may name a different verified binary. Packages are written under the repository `release/`
directory, which is not tracked.

