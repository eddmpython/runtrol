# Runtrol Studio for VS Code

Manage every local coding-agent CLI chat, model, workspace, and session from one fast VS Code window.

Runtrol Studio discovers supported CLIs already installed on your computer, keeps provider-owned sessions available,
and follows the exact workspace or worktree bound to the selected chat. It does not replace the provider CLI or keep
a second conversation copy.

## Install

1. Open Extensions in VS Code with `Ctrl+Shift+X` on Windows or Linux, or `Cmd+Shift+X` on macOS.
2. Search for `Runtrol Studio`. If title search misses it, search for the exact identifier
   `@id:runtrol.runtrol-studio`.
3. Select **Install**.
4. Open the Runtrol icon in the Activity Bar.

No Core path is required for a Marketplace installation. Each platform package includes the matching native Core and
Runtrol materializes it automatically. Keep each coding-agent CLI installed and authenticated through its own official
flow.

## Updates

Marketplace installations use VS Code's built-in extension updates. VS Code checks for releases and updates enabled
extensions automatically. Its default update delay is two hours, and an immediate update is available from the
extension's **Update** action.

A manually installed VSIX has automatic updates disabled by VS Code. If an older Runtrol copy came from a VSIX,
open the extension's Manage menu and enable **Auto Update**, or uninstall it and install the Marketplace version once.
Runtrol never downloads or replaces extension packages outside VS Code's signed Marketplace flow.

An extension update does not take ownership of active chats. The Core binary lives at one extension-global managed
path, and upgrades reconnect to the same daemon and provider processes after the Extension Host restarts.

## Quick start

1. Open **Chats**.
2. Select **New chat** beside an available service.
3. Choose a local workspace, then a model and reasoning effort reported by that installed CLI. Provider default leaves
   the CLI's own choice unchanged.
4. Send the first message in the conversation editor.

Selecting an existing chat opens the same conversation tab and follows its exact workspace. Use **Rename Chat** for a
short label, and **Runtrol: Switch Chat** for fast project, service, state, and path search.

The conversation header keeps the active model, reasoning effort, provider mode, context use, provider-reported cost,
and available account-limit windows together. Missing provider telemetry is shown as unavailable instead of estimated.

## Agent Tools

Select the sparkle on a project heading, or run **Runtrol: Enable Agent Tools for This Project**, to let installed
coding agents delegate bounded project work through Runtrol. The project row says `Agent Tools` when ready. Runtrol
uses each provider CLI's official MCP registration command and discovers providers, models, and sessions at runtime.
No provider configuration file is edited directly.

Authority is limited to that canonical project root. Starts are exclusive, instructions and events pass unchanged,
and provider approvals always stay with you in Runtrol. Agent Tools cannot answer approvals, delete provider
conversations, silently share a working tree, hold an API key, keep a transcript copy, or run its own agent loop.

Run **Runtrol: Disable Agent Tools for This Project** to revoke the project's Runtime authority and remove its
protected local credential. The global provider registration is removed when the last enabled project is disabled.
Runtrol verifies the exact command through each provider's official readback before overwriting or removing
anything, so a same-named entry that points elsewhere is reported and left untouched.
See the complete [Agent Tools contract](https://github.com/eddmpython/runtrol/blob/main/docs/agentTools.md).

## Mission Auto Flight

Validate an ordinary reviewed Mission, then select **Runtrol: Arm Mission Auto Flight**. One local confirmation can
arm up to eight exact Mission digests. While that Studio window stays open, Runtrol starts each safe DAG wave, waits
for the real provider turn to finish, runs the fixed Gates, seals the Receipt, and starts the next eligible wave.
The row's rocket and `AUTO` marker show the arm; its stop action revokes future provider input immediately.

Auto Flight pauses for working sessions, person or quota waits, and a paused Mission. It stops and removes its arm on
authority drift, ambiguous delivery, missing sessions, recovery or failure, comparison flow, cancellation, or any
other unsafe state. Reaching `integrating` also removes the arm. Receipt review, applying Artifacts, final Gates, and
completion always remain explicit.

The durable arm contains only Mission, Task, session, provider, and lifecycle-generation identifiers. No instruction,
reply, transcript, Gate output, or Artifact content is stored. Runtime events drive it without polling, and it never
replaces the provider CLI's own agent loop.

Person waits, safety stops, and Receipt Landing first enter a durable idempotent signal outbox. Pending delivery
revokes automatic provider input, restart retries the same random UUID, and only Core acknowledgement removes the
outbox entry and arm. A paired phone then derives the exact current destination after authentication. Web Push stays
bodyless and carries no Mission ID, instruction, path, output, or Receipt content.

## Core-owned Mission schedule

Select **Runtrol: Schedule Reviewed Mission...** on a validated Mission. Pick 15 minutes, one hour, tomorrow at local
09:00, or an exact local `YYYY-MM-DD HH:mm` time, then review the Mission and policy digests plus every Task-to-provider
assignment. Core durably owns that one-shot wake, so the Studio window may close before it is due. A pending row shows
the local time and offers **Runtrol: Cancel Mission Schedule**. Scheduling the same Mission again uses the exact current
schedule as a compare-and-swap replacement instead of silently overwriting it.

At the due instant Core rechecks the Mission, policy, capability, Gate, Task, provider, and workspace authority. It
then uses the existing provider-neutral Mission Prepare, Start, Bind, Send-intent, and Prompt boundaries. It stores no
instruction or conversation copy and holds no provider credential. Restart can reclaim a launch that has not submitted
input, while an ambiguous submission becomes visible attention and is never repeated automatically.

## Parallel attempts

Run **Runtrol: Try One Instruction Several Ways...** to compose a reviewed comparison Mission from an instruction
file you already own. Choose two through four attempts, one registered deterministic Gate, the discovered coding
services, the Git base, and the allowed output roots. Runtrol opens the generated TOML without saving it so you can
read it and choose its project path before validation.

After validation, **Runtrol: Run All Reviewed Attempts** prepares one isolated linked worktree and native provider
session per attempt, rechecks the exact instruction bytes, sends them under one local confirmation, and arranges the
conversation tabs as a VS Code editor grid. When the attempts finish, verify their declared Artifacts and Gates.
**Runtrol: Compare Passing Results** opens the same Artifact from each passing worktree in native VS Code diff
editors. Apply the result you want to the project, then verify and complete from that passing Task row. Final
integration uses only the selected Task Receipt. Failed alternatives are results, not hidden retries.

## Interrupted Mission recovery

If Core restarts while a Mission Task is in flight, Runtrol blocks that Task rather than guessing whether provider
input took effect. Select **Runtrol: Recover Interrupted Mission** on the blocked Mission. The confirmation names the
exact reviewed digests, workspaces, and provider assignments and warns that fresh sessions may repeat external effects
from the interrupted attempt. Esc changes nothing. Confirming rechecks the same authority, reopens only interrupted
Tasks, safely resumes the scheduler, and starts fresh provider-native sessions with the unchanged instructions.

If the reviewed Mission, instruction, Gate, capability, or workspace contract changed across restart, recovery stays
unavailable. Validate the Mission again or cancel it instead. An uncertain Send is never repeated automatically.

## Requirements

- Desktop VS Code 1.106 or newer (the version that lets an extension place a view in the secondary side bar).
- Windows, macOS, or Linux on x64 or ARM64.
- A trusted local filesystem workspace. Virtual workspaces and browser-only VS Code cannot start local provider CLIs.
- At least one supported coding-agent CLI installed and authenticated with its own official account flow.

Runtrol runs near the local VS Code UI so it can supervise local CLI processes. A remote workspace still needs a local
filesystem workspace for chats that run on this computer.

## Troubleshooting

### The extension does not appear in search

Search for `@id:runtrol.runtrol-studio`, or open the
[public Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio).

### The installed version is old

Open the Runtrol Studio Manage menu and enable **Auto Update**. If the extension was installed from a VSIX, reinstall
it from the Marketplace once. Use VS Code's **Check for Extension Updates** command for an immediate check.

### No services appear

Install and authenticate a supported coding-agent CLI through its official flow, then select **Refresh services**.
Runtrol discovers versions, models, flags, capabilities, and provider-owned sessions at runtime.

### The extension view needs recovery

Run **Runtrol: Restart Extension Host**. The extension view restarts while the supervised Core and active provider
processes stay alive.

## Main commands

- **Runtrol: New Chat**
- **Runtrol: Switch Chat**
- **Runtrol: Open Current Chat**
- **Runtrol: Refresh Chats and Services**
- **Runtrol: Check Provider Updates**
- **Runtrol: Enable Agent Tools for This Project**
- **Runtrol: Disable Agent Tools for This Project**
- **Runtrol: Try One Instruction Several Ways...**
- **Runtrol: Run All Reviewed Attempts**
- **Runtrol: Compare Passing Results**
- **Runtrol: Arm Mission Auto Flight**
- **Runtrol: Disarm Mission Auto Flight**
- **Runtrol: Schedule Reviewed Mission...**
- **Runtrol: Cancel Mission Schedule**
- **Runtrol: Recover Interrupted Mission**
- **Runtrol: Verify and Complete Mission Integration**
- **Runtrol: Restart Extension Host**
- **Runtrol: Pair a Phone**

## Settings

| Setting | Default | Purpose |
|---|---|---|
| `runtrol.corePath` | Empty | Optional absolute Core path for local development. Marketplace packages use the bundled Core |
| `runtrol.followWorkspace` | `true` | Open the selected chat's workspace or worktree in the current window |
| `runtrol.relayOrigin` | Empty | Exact encrypted relay origin for phone connections. Empty disables phone connections |

## Ownership and security

The installed provider CLI owns the conversation and repository changes. Runtrol supervises process, session,
workspace, worktree, and collision boundaries.

Runtrol does not:

- read provider transcript files;
- keep a second conversation copy;
- hold or forward model API keys;
- hardcode provider versions, models, flags, or session paths;
- replace the provider CLI's agent loop.

Only the selected Runtrol session identifier is retained across workspace reloads. Prompts, replies, approvals,
provider state, and conversation frames are never written to extension storage.

## Support and development

- [Product site](https://eddmpython.github.io/runtrol/)
- [Source and issue tracker](https://github.com/eddmpython/runtrol)
- [Security policy](https://github.com/eddmpython/runtrol/blob/main/SECURITY.md)
- [Development and release internals](https://github.com/eddmpython/runtrol/blob/main/docs/vscodeSurface.md)
