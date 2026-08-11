# Runtrol Studio for VS Code

Run every repository, session, and installed coding-agent CLI from one VS Code window.

Runtrol Studio is the primary runtrol surface. It discovers supported CLIs already installed on your machine, keeps provider-owned sessions available, and moves the current VS Code window to the workspace or worktree bound to the selected session.

## Install

1. Open Extensions in VS Code with `Ctrl+Shift+X` on Windows or Linux, or `Cmd+Shift+X` on macOS.
2. Search for `Runtrol Studio`.
3. Select **Install**.
4. Open the Runtrol icon in the Activity Bar.

No Core path is required for a Marketplace installation. The extension verifies and materializes its bundled native Core, then discovers supported installed CLIs at runtime. Keep each CLI installed and authenticated through its own official flow.

## Use

- Select any session in the Runtrol view to focus it and follow its exact workspace.
- Run **Runtrol: Switch Session** for fast project, CLI, state, and path search.
- Run **Runtrol: Start Session** to choose a discovered CLI and workspace.
- Use **Open Session Workspace**, **Interrupt Turn**, and **Close Session** from the view or Command Palette.

Fifteen sessions are the daily-use baseline and 30 sessions are the release load. At most eight sessions own a hot process, while exactly one selected session owns the full event stream and active renderer. Cold rows respond immediately and resume through the provider-native session identity.

## Ownership and security

The installed provider CLI owns the conversation and repository changes. Runtrol supervises process, session, workspace, worktree, and collision boundaries.

Runtrol does not:

- read provider transcript files;
- keep a second conversation copy;
- hold or forward model API keys;
- hardcode provider versions, models, flags, or session paths;
- replace the provider CLI's agent loop.

Only the selected runtrol session identifier is retained across workspace reloads. Prompts, replies, approvals, provider state, and conversation frames are never written to extension storage.

Before another hot writer starts in the same, parent, or child path, the extension offers the existing session, a separate workspace or worktree, an explicit continue action, or cancellation.

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `runtrol.corePath` | Empty | Optional absolute Core path for local development. Marketplace packages use the bundled Core |
| `runtrol.followWorkspace` | `true` | Open the selected session's workspace or worktree in the current window |

## Open source

- [Product site](https://eddmpython.github.io/runtrol/)
- [Source and issue tracker](https://github.com/eddmpython/runtrol)
- [Security policy](https://github.com/eddmpython/runtrol/blob/main/SECURITY.md)

## Development

```text
npm install
npm run check
npm test
npm run build
```

Set `runtrol.corePath` to a local debug or release executable while developing. A packaged platform VSIX contains one matching runtrol Core under `resources/core/`. On activation that file is streamed into one stable path under the extension's global storage. The packaged file is release material and never owns a daemon lifetime.

### Package the native platform

Build the Rust release binary, then run:

```text
cargo build --release --bin runtrol --no-default-features --target-dir ../../target/vscode-release
npm run package:native
```

The packager resolves relative Core paths from the repository root and assembles the VSIX in an isolated temporary directory. It does not replace `resources/core` in the working extension, even when that development Core is running.

`RUNTROL_CORE_BINARY` may name a different verified binary. Packages are written under the repository `release/` directory, which is not tracked.

The release target map is `release-targets.json`. The manifest version is the release version SSOT and the package filename is derived from it. Every package contains one matching Core, the production bundles, brand resources, and the repository license. Source, tooling, dependencies, test budgets, and target metadata are excluded.

When Core bytes change, the extension preserves the currently mapped image with a hard link before atomically replacing the stable name. An Extension Host reload, VSIX upgrade, or VSIX rollback therefore reconnects to the same daemon and active provider processes instead of making the versioned extension directory their owner.

Inspect and clean-install a built package before publication:

```text
python -X utf8 ../../tests/audit/vscodePackage.py --archive ../../release/runtrol-studio-VERSION-win32-x64.vsix --target win32-x64 --core ../../target/vscode-release/release/runtrol.exe
node tooling/installed-package.mjs ../../release/runtrol-studio-VERSION-win32-x64.vsix
python -X utf8 ../../tests/audit/vscodeUpgradeRollback.py --archive ../../release/runtrol-studio-VERSION-win32-x64.vsix
```

The hosted release workflow repeats this journey on native Windows, macOS, and Linux runners. Visual Studio Marketplace signs published VSIX files, and VS Code verifies that signature when installing them.
