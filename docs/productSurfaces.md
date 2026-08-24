# Product Surfaces

This document is the source of truth for what users install and open.

## Decision

The PC product is the `Runtrol Studio` VS Code extension. There is no separately distributed desktop GUI.

The public product has three surfaces with distinct jobs:

| Surface | Job | Session ownership |
|---|---|---|
| VS Code extension | Discover the local Core, manage repositories and sessions, render the selected live stream, and follow the selected workspace | None. The Core owns process lifecycle |
| Phone PWA | Pair with the same Core and provide bounded remote control from a permanent HTTPS origin | None. The Core remains on the PC |
| GitHub Pages site | Explain the product and provide verified install entry points | None. It is static distribution metadata |

The Core is the only session owner. Closing VS Code, closing the PWA, or changing the network route does not transfer ownership to another surface.

Installed coding agents may also reach the Core through the project-scoped [Agent Tools](agentTools.md) MCP surface.
That is an agent-facing Runtime adapter, not a fourth user interface or another session owner. Studio enables and
revokes it; the provider-owned agent loop chooses calls; Core keeps the same workspace, lease, approval, and process
boundaries.

## Why there is no separate desktop GUI

A second PC window would duplicate session navigation, workspace selection, keyboard behavior, accessibility work, packaging, signing, update policy, and performance gates. It would also split the user's attention from the editor where repository work already happens.

VS Code already supplies the desktop shell, workspace model, command palette, keyboard conventions, and extension distribution channel. runtrol should spend its complexity budget on session supervision and remote continuity instead of another window manager.

The standalone desktop implementation and its execution path have been removed. The VS Code extension is the only PC user interface. Core remains a headless local supervisor and command endpoint.

## VS Code interaction contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight logical sessions may own a hot provider process.
- Exactly one selected session owns the full stream and active renderer.
- Selecting a session opens its own conversation tab in the editor area instead of compressing the conversation into
  the sidebar. Several conversations can remain open and use VS Code's editor groups.
- A conversation row contains only the coding-service icon and its actual conversation title. The icon spins only
  while that conversation is working. Session names use the operator's saved name when present, then the provider's
  own catalogue title, then a compact unique `Chat` handle. Studio refreshes provider title metadata when a native
  identity appears and when a turn settles. Project and provider names are not title fallbacks, and conversation
  content is never read to invent a title.
- The fixed `Agent Usage` area at the bottom of the sidebar keeps every installed service visible. Numeric account
  windows use bounded progress bars with exact percentages, and compact display-name marks keep similarly named
  services distinct; a service with no numeric report says `Ready` without
  inventing a zero value.
- New chats use a neutral greeting. The composer identifies project, branch, coding service, model, effort, and access
  mode. Its context visibly labels `Project`, `Branch`, and `Agent`, and its message field names the selected service so
  the destination remains explicit even when a project name matches the product name.
- Search covers project, provider metadata, state, and workspace path without reading conversation content.
- Selecting a cold row updates the UI immediately, resumes through the provider-native session identity, and follows the bound workspace.
- Installed providers, versions, models, flags, capabilities, and session paths are discovered at runtime.
- Adding a provider does not require a Core edit.

## Distribution contract

The GitHub Pages site uses English as its static default so installation instructions remain readable with JavaScript disabled. Korean, Chinese, and Japanese are optional client-side translations.

The primary PC action is the VS Code Marketplace listing. A manual action is enabled only when the latest GitHub Release contains an actual `.vsix` asset produced by the verified release workflow. An unpublished artifact must be shown as pending, never as a working download.

The phone action uses `/runtrol/app/` on the same permanent GitHub Pages origin. Its current production route is the end-to-end encrypted relay. Pairing starts from a one-use QR displayed in VS Code, and all later workspace, provider, and action authority remains editable only in VS Code. Content-free Web Push provides a generic wake signal without carrying conversation or approval data. Direct private-network routes are not part of the current release.

## Visual contract

The canonical assets in [`assets/brand/`](../assets/brand/) are reused without redrawing the mark. Public surfaces use graphite, ivory, and the canonical orange `#FF5A2F`. The landing page combines restrained editorial spacing with a real session-control panel. It does not simulate conversation content.
