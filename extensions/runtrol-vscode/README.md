# Runtrol Studio for VS Code

Runtrol Studio is the primary runtrol surface. It connects one VS Code window to the existing thin Rust supervisor,
shows every runtime-discovered coding-agent CLI and session, keeps one selected event stream hot, and switches the
window to that session's exact workspace or worktree.

The Core pushes one current session snapshot and then only changed snapshots. Conversation traffic never triggers a
session-list query or rebuild, and every connected window shares the Core's once-encoded snapshot bytes.

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

## Package the native platform

Build the Rust release binary, then run:

```text
cargo build --release --bin runtrol
npm run package:native
```

`RUNTROL_CORE_BINARY` may name a different verified binary. Packages are written under the repository `release/`
directory, which is not tracked.

The release target map is `release-targets.json`. The manifest version is the release version SSOT and the package
filename is derived from it. Every package contains one matching Core, the production bundles, brand resources, and
the repository license. Source, tooling, dependencies, test budgets, and target metadata are excluded.

Inspect and clean-install a built package before publication:

```text
python -X utf8 ../../tests/audit/vscodePackage.py --archive ../../release/runtrol-studio-VERSION-win32-x64.vsix --target win32-x64 --core ../../target/release/runtrol.exe
node tooling/installed-package.mjs ../../release/runtrol-studio-VERSION-win32-x64.vsix
```

The hosted release workflow repeats this journey on native Windows, macOS, and Linux runners. The Visual Studio
Marketplace signs published VSIX files, and VS Code verifies that signature when installing them.
