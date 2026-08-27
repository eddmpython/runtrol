# Product surfaces

This document is the source of truth for what Runtrol is and what people or applications open.

## Product identity

Runtrol is a local Runtime for installed coding-agent CLIs. It discovers each CLI's executable, version, models,
capabilities, native sessions, and invocation surfaces at runtime. It supervises processes and transports events and
terminal bytes without interpreting conversation content. Provider CLIs keep ownership of accounts, transcripts,
native session records, model choices, approvals, and repository changes.

Runtrol is not an LLM wrapper. An embedding application does not inject a hidden prompt into a model connection that
Runtrol owns. It asks the installed provider CLI to start or resume work through the public Runtime contract, then
shows the provider's own result stream. Adding a provider is a manifest or driver change and never requires a Core
branch for that provider.

## Product family

| Surface | Job | Session ownership |
|---|---|---|
| Runtrol Runtime | Headless local supervisor, public integration endpoint, process and workspace authority, terminal host | Owns supervision, never the provider transcript |
| Rust, TypeScript, and Python clients | Locate, authenticate, authorize, and call the shared Runtime from another application | None |
| Runtrol Studio | Flagship VS Code client for projects, conversations, provider terminals, usage, approvals, and local administration | None |
| Phone PWA | Pair with the same Runtime and provide bounded remote continuity from a permanent HTTPS origin | None |
| GitHub Pages site | Explain the product and provide verified install entry points | None |

The Runtime is the only process-lifecycle authority. Closing Studio, closing the PWA, or replacing an SDK connection
does not transfer ownership. Draining Runtime generations continue only the live work they already own.

Installed coding agents may also call the Runtime through project-scoped [Agent Tools](agentTools.md). That is an
agent-facing adapter to the same public authority, not another agent loop and not a user interface.

## Studio decision

Studio is the distributed desktop GUI. There is no separately distributed desktop window. VS Code supplies the
desktop shell, workspace model, keyboard and accessibility conventions, editor tabs, and signed extension update
channel. Runtime and its SDKs remain usable without Studio, including independent command-line administration.

Studio contributes one sidebar view, `runtrol.sidebar`, drawn as a webview page with three zones: projects (added
folders, collapsible, each with its own colour that also marks its conversation rows and terminal tabs), the
conversations outside every project, and one usage chip per installed provider (icon inside a ring gauge with the
seven-day percentage). Row actions appear on hover; the rare actions sit behind the vertical dots at the top of the
page; the title bar keeps the two starting actions.

Each usage row makes the actual seven-day window the single primary line when the provider publishes one. Hover and
the keyboard-accessible vertical-menu action disclose every provider-reported window, plan, reset, report age, and
limit condition. No percentage is inferred. Provider additions appear through Runtime inventory without a Studio or
Core edit.

A conversation opens as the provider CLI's own terminal interface in an editor tab. Studio uses the public Runtime
terminal API and pins every reconnect to the generation that owns the terminal. The phone uses a paired,
device-scoped transport adapter into the same Core terminal host. That private device adapter is not an application
integration API.

## Interaction contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight logical sessions may own a hot provider process.
- Exactly one selected session owns the foreground subscription and full renderer.
- Search and ordering use operational metadata, never conversation content.
- A working conversation changes its declared provider icon to a spinning state without adding repeated labels.
- A provider capability controls whether resume, archive, native delete, models, usage, and remedies exist.
- Cold resume uses the provider-native identity and the exact bound workspace.
- Closing a viewer does not end a provider process.
- New providers are discovered from manifests and drivers at runtime.

## Distribution contract

Studio is published as six native Marketplace packages for Windows, macOS, and Linux on x64 and ARM64. Each package
contains the matching Runtime binary. Standalone Runtime releases and SDK artifacts are separate public integration
products and do not depend on installing Studio.

The GitHub Pages site uses English as its static default. Korean, Chinese, and Japanese are optional client-side
translations. A download action is enabled only for an artifact that the release workflow produced and verified.

The phone action uses `/runtrol/app/` on the same permanent HTTPS origin. Pairing starts with a one-use QR in Studio.
All later root, provider, and action authority remains editable only at the PC. The relay receives ciphertext and
bodyless Web Push carries no conversation or approval content.

## Visual contract

[`assets/brand/`](../assets/brand/) is the canonical brand source. Public full-color surfaces use graphite, white,
and coral `#F56565`. The Marketplace icon, landing page, phone, and generated public images use the two-tone mark.
VS Code masks Activity Bar icons to a theme foreground by design, so Studio supplies the canonical silhouette there
and uses `runtrol.accent` for theme-aware coral actions inside the sidebar.
