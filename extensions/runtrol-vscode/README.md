# Runtrol Studio for VS Code

Run the coding-agent CLIs already installed on your computer from one native VS Code sidebar.

[Product site](https://eddmpython.github.io/runtrol/) | [Source](https://github.com/eddmpython/runtrol) | [Phone app](https://eddmpython.github.io/runtrol/app/)

Runtrol Studio is the flagship graphical client for the local Runtrol Runtime. Runtime discovers provider CLIs,
supervises their processes, and connects each provider-owned conversation to its exact workspace. Studio presents
that public Runtime contract inside VS Code. It does not replace a provider CLI, call a model API, or keep a second
copy of a conversation.

## Install

1. Open Extensions in VS Code.
2. Search for `Runtrol Studio` or the exact identifier `@id:runtrol.runtrol-studio`.
3. Select **Install**.
4. Open the Runtrol symbol in the Activity Bar.

The Marketplace package includes the native Runtime for the selected platform. Keep each coding-agent CLI installed
and authenticated through its own official flow. No Core path is required for a Marketplace installation, and no
model API key is ever entered into Studio.

## Updates

Marketplace installations use VS Code's built-in extension updates. VS Code checks for releases and updates enabled
extensions automatically. Its default update delay is two hours, and an immediate update is available from the
extension's **Update** action.

A manually installed VSIX has automatic updates disabled by VS Code. If an older Runtrol copy came from a VSIX,
open the extension's Manage menu and enable **Auto Update**, or uninstall it and install the Marketplace version once.
Runtrol never downloads or replaces extension packages outside VS Code's signed Marketplace flow.

An extension update does not take ownership of live conversations. The Runtime lives at one extension-global managed
path, and an upgrade reconnects to the exact Runtime generation that owns each terminal after the Extension Host
restarts.

## One sidebar

Studio contributes one native tree named **Runtrol**. Projects, conversations, first-run actions, and compact usage
rows share that list instead of occupying separate VS Code view headers.

- Add a project folder and its provider-owned conversations appear below it.
- Conversations outside added projects remain ordinary top-level rows.
- Start, open, pin, rename, archive, close, or delete through the provider capabilities Runtime discovered.
- Opening a Studio window never continues or resumes a conversation. Live terminals attach to their exact process;
  cold conversations start only after an explicit open action.
- Provider commands started in a new integrated terminal pass through Runtime's transparent shim and appear in every
  open sidebar as the same PTY stream. A later provider title replaces the project placeholder in the row and tab.
- A process already running outside that broker is preserved and shown as external. Studio blocks duplicate resume
  and attaches only when the provider or original terminal host publishes an official channel.
- A working conversation spins its coding-service icon without adding a permanent status sentence.
- Every installed service contributes one compact `7d` usage row. Hover shows every reported limit window, plan,
  reset time, and report age. Press the row or its vertical menu action for the same keyboard-accessible detail.
- A service that does not publish a seven-day number says so. Studio never invents capacity or usage.

The accent uses the `runtrol.accent` theme color. VS Code intentionally masks Activity Bar symbols to the active
theme foreground, while the Marketplace icon and other public brand surfaces use the canonical coral and white mark.

## Provider terminal, not another chat page

Opening a conversation creates an editor-area terminal tab showing that coding service's own terminal interface.
Model selection, effort, permissions, approvals, and history remain the provider CLI's own controls. Split, grid,
keyboard input, and full screen remain VS Code controls.

The local Runtime owns the pseudo terminal and a bounded screen snapshot. Closing a tab detaches that viewer without
ending the provider process. If an update creates a new Runtime generation, Studio reconnects a tab only to the exact
generation that owns its terminal. It never redirects input to a different process or retries uncertain input.

## Agent Tools

Select the sparkle on a project or run **Runtrol: Enable Agent Tools for This Project** to let installed coding agents
use Runtrol's bounded Runtime tools for that canonical project root. Each provider is registered only through its
official CLI command. Disabling Agent Tools revokes the protected local credential and removes provider registration
when no enabled project still needs it.

Agent Tools cannot answer approvals, delete provider conversations, silently share a working tree, hold an API key,
keep a transcript copy, or run a Runtrol-owned agent loop. See the complete
[Agent Tools contract](https://github.com/eddmpython/runtrol/blob/main/docs/agentTools.md).

## Phone continuity

Pair the phone PWA from Studio's one-use QR to view the same Core-hosted conversations remotely. Remote authority is
default deny, is narrowed to explicit roots and providers, and can be revoked from the PC. The relay sees ciphertext;
Runtrol does not place provider credentials or model API keys in the relay.

## Requirements

- Desktop VS Code 1.106 or newer.
- Windows, macOS, or Linux on x64 or ARM64.
- A trusted local filesystem workspace for project conversations.
- At least one supported coding-agent CLI installed and authenticated through its official account flow.

Virtual workspaces and browser-only VS Code cannot start local provider CLI processes.

## Main commands

- **Runtrol: New Conversation**
- **Runtrol: Switch Conversation**
- **Runtrol: Open Next Waiting Conversation**
- **Runtrol: Refresh Conversations**
- **Runtrol: Add Project**
- **Runtrol: Set Up Coding Services**
- **Runtrol: Check Provider Updates**
- **Runtrol: Enable Agent Tools for This Project**
- **Runtrol: Review Integration Requests**
- **Runtrol: Manage Runtime Integrations**
- **Runtrol: Review Runtime Requests**
- **Runtrol: Pair a Phone**
- **Runtrol: Restart Extension Host**

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `runtrol.corePath` | Empty | Optional absolute Runtime path for local development. Marketplace packages use the bundled Runtime |
| `runtrol.relayOrigin` | Empty | Exact encrypted relay origin for phone connections. Empty disables phone connections |

## Ownership and security

The installed provider CLI owns its account, conversation, terminal interface, native session record, and repository
changes. Runtrol owns only supervision metadata, process and workspace boundaries, authority, and bounded transport.

Runtrol does not:

- read or copy provider transcripts;
- interpret, summarize, or rewrite terminal bytes;
- hold or forward model API keys;
- hardcode provider versions, models, flags, or session paths;
- replace the provider CLI's agent loop.

Only bounded identifiers and operational metadata required for reconnect and authority survive a restart. Prompts,
replies, terminal frames, drafts, and approval content are not written to Studio storage.

## Troubleshooting

If no service appears, install and authenticate its CLI through the provider's official flow, then run
**Runtrol: Refresh Conversations**. If Studio needs recovery, run **Runtrol: Restart Extension Host**. The Extension
Host restarts while Runtime and its supervised provider processes remain alive.

For an exact identifier search, use `@id:runtrol.runtrol-studio` or open the
[Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio).

## Support and development

- [Product site](https://eddmpython.github.io/runtrol/)
- [Source and issue tracker](https://github.com/eddmpython/runtrol)
- [Security policy](https://github.com/eddmpython/runtrol/blob/main/SECURITY.md)
- [Runtime integration guide](https://github.com/eddmpython/runtrol/blob/main/docs/runtimeIntegration.md)
- [Studio operating contract](https://github.com/eddmpython/runtrol/blob/main/docs/vscodeSurface.md)
