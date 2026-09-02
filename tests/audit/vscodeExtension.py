"""Gate: the VS Code surface stays thin, bounded, buildable, and package-shaped.

The gate deliberately checks the source contract before invoking the toolchain. A bundle that compiles can still
poll, persist conversation data, keep hidden renderers alive, or ship runtime Node dependencies. Product disk writes
are limited to the bounded selected-session scalar and atomic Core installer.

Usage::

    python -X utf8 tests/audit/vscodeExtension.py --selftest
    python -X utf8 tests/audit/vscodeExtension.py
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXTENSION = ROOT / "extensions" / "runtrol-vscode"


def sourceViolations(package: dict[str, object], sources: dict[str, str]) -> list[str]:
    """Return violations of the extension's static product contract."""
    found: list[str] = []
    dependencies = package.get("dependencies")
    if isinstance(dependencies, dict) and dependencies:
        found.append("the shipped extension has runtime Node dependencies")

    contributes = package.get("contributes")
    contribution_text = json.dumps(contributes, sort_keys=True)
    # A view container in the secondary side bar is a contribution VS Code accepts from 1.106 on (measured in
    # the 1.132 workbench's manifest schema, announced with 1.106). Contributing it under an older engine floor
    # would ship a manifest part of the floor's own VS Code cannot place.
    if "secondarySidebar" in contribution_text and not engineAtLeast(package, (1, 106)):
        found.append("the manifest contributes a secondary side bar container below the VS Code 1.106 engine floor")
    if '"activitybar"' not in contribution_text:
        found.append("the extension has no Activity Bar control surface")
    views = contributes.get("views") if isinstance(contributes, dict) else None
    view_containers = contributes.get("viewsContainers") if isinstance(contributes, dict) else None
    runtrol_views = views.get("runtrol") if isinstance(views, dict) else None
    # One view and only one, and it is the page Studio draws itself. VS Code draws a collapsible section header
    # for every view in a container as soon as there are two and moves the title actions into those headers
    # (measured 2026-08-27: "Runtrol" twice, the add buttons gone from the title bar). One webview view keeps the
    # title bar's two starting actions and lets the page draw zones, gauges and row density a tree cannot.
    if (
        not isinstance(runtrol_views, list)
        or len(runtrol_views) != 1
        or not isinstance(runtrol_views[0], dict)
        or runtrol_views[0].get("id") != "runtrol.sidebar"
        or runtrol_views[0].get("type") != "webview"
    ):
        found.append("Runtrol must contribute exactly one sidebar view, runtrol.sidebar, as a webview")
    if contributes.get("viewsWelcome") if isinstance(contributes, dict) else None:
        found.append("a webview sidebar draws its own empty states; viewsWelcome entries would never show")
    menus = contributes.get("menus") if isinstance(contributes, dict) else None
    title_entries = menus.get("view/title") if isinstance(menus, dict) else None
    def title_navigation(view: str) -> set[str]:
        return {
            entry.get("command")
            for entry in title_entries
            if isinstance(entry, dict)
            and f"view == {view}" in str(entry.get("when", ""))
            and str(entry.get("group", "")).startswith("navigation")
        } if isinstance(title_entries, list) else set()

    # The one header carries all three: create a conversation, add a project, and the vertical dots the rare
    # actions live behind. They were a strip the page drew under the title bar, which spent a whole row of a
    # narrow panel on one button (operator, 2026-08-28: "why make one more row when the first one is there").
    if title_navigation("runtrol.sidebar") != {
        "runtrol.createProject",
        "runtrol.startSession",
        "runtrol.moreActions",
    }:
        found.append("the sidebar title must carry exactly conversation, project and the more-actions dots")
    item_context = menus.get("view/item/context") if isinstance(menus, dict) else None
    if item_context:
        found.append("row actions are drawn by the sidebar page on hover; the manifest contributes no view/item/context menus")
    command_entries = contributes.get("commands") if isinstance(contributes, dict) else None
    command_ids = {
        entry.get("command")
        for entry in command_entries if isinstance(entry, dict)
    } if isinstance(command_entries, list) else set()
    activation_events = package.get("activationEvents")
    activations = set(activation_events) if isinstance(activation_events, list) else set()
    # Setting a service up is offered from the usage section's own title, in that section, and nowhere else. It
    # replaced a catalogue of services this product had never measured, advertised at the foot of the sidebar.
    if "runtrol.setUpServices" not in command_ids or "onCommand:runtrol.setUpServices" not in activations:
        found.append("the usage section must have one activatable set-up command")
    if "onView:runtrol.sidebar" not in activations:
        found.append("the unified sidebar view must activate the extension")
    colors = contributes.get("colors") if isinstance(contributes, dict) else None
    if not any(isinstance(entry, dict) and entry.get("id") == "runtrol.accent" for entry in colors or []):
        found.append("the native first-run actions must use the declared Runtrol accent")

    all_source = "\n".join(sources.values())
    forbidden = {
        "localStorage": "conversation-capable browser persistence",
        "sessionStorage": "conversation-capable browser persistence",
        "indexedDB": "conversation-capable browser persistence",
        "setInterval(": "polling loop",
        "scheduleRefresh": "session-list requery loop",
        "appendFile(": "filesystem write surface",
        "see the (i)": "sidebar coverage hidden behind an information action",
        "Untitled ·": "repeated non-name conversation fallback",
        "Resume anyway": "internal writer-collision copy on conversation switching",
        # The pages declare `style-src 'nonce-...'`, which covers their own style block and nothing else. A
        # style attribute carries no nonce, so the browser drops it without a word and the element renders
        # unstyled. It cost the project colour band days of invisibility and every usage bar its fill
        # (2026-08-28). Widths and colours belong to a class in the nonced stylesheet.
        'style="': "an inline style attribute the page's own policy silently drops",
    }
    for token, meaning in forbidden.items():
        if token in all_source:
            found.append(f"{meaning} is reachable through `{token}`")
    # The conversation pane carries the service's bytes and nothing else (`terminalTransportIntegrity`, Studio
    # presentation, 2026-09-02): no opening mark, no clear before a checkpoint, no exit or error sentence. What
    # the pseudoterminal module may write is what the Runtime sent, after the one viewer-edge mouse filter.
    pane = sources.get("runtimeTerminal.ts", "")
    for token, meaning in {
        "paintMark(": "the opening mark drawn into the conversation pane",
        "x1b[2J": "a clear-screen sequence Studio writes into the conversation pane",
        "x1b[31m": "an error sentence Studio writes into the conversation pane",
        "ended with code": "an exit sentence Studio writes into the conversation pane",
    }.items():
        if token in pane:
            found.append(f"{meaning} is back in runtimeTerminal.ts through `{token}`")
    if "this.writeEmitter.fire(text)" not in pane or pane.count("this.writeEmitter.fire(") != 1:
        found.append("runtimeTerminal.ts must write the pane from exactly one place, the filtered service bytes")

    # One view, one document. A second file that builds a whole page brings a second `body` rule with it, and
    # the last one concatenated wins. The usage strip had one from when it was its own webview; folding it into
    # the single page left that rule in place, where it quietly took the page's padding, colour and background
    # and handed the background back as `transparent`. So the panel sat on the browser's own dark canvas
    # (#121212) inside a sidebar the editor painted #252526, which is the black background the operator asked
    # about on 2026-08-28. It survived because the strip's unit tests rendered that second document, in which
    # the rule was correct. Whoever adds the next page fragment must not also add a page.
    documents = sorted(name for name, source in sources.items() if "<!DOCTYPE html>" in source)
    if documents != ["sidebarPage.ts"]:
        found.append(f"exactly one file may build the webview document, found {documents or ['none']}")

    writers = [relative for relative, source in sources.items() if "writeFile(" in source]
    # The selected-session scalar, the Core installer's digest memory (file identity -> sha256, so an
    # activation does not hash the Core twice; measured 2026-08-25 at 60 ms per hash), and the one fact the
    # `vscode:uninstall` hook cannot derive on its own: which global storage this Studio owned.
    expected_writers = {"selectionStore.ts", "core/managedCore.ts", "core/uninstallRecord.ts"}
    if set(writers) != expected_writers or len(writers) != len(expected_writers):
        found.append(
            "direct writeFile calls must stay in selectionStore.ts, core/managedCore.ts, and core/uninstallRecord.ts, found "
            + (", ".join(writers) if writers else "none")
        )
    handleWriters = [
        relative
        for relative, source in sources.items()
        if "handle.write(" in source or re.search(r"\bopen\([^,\n]+,\s*[\"'][wax]", source)
    ]
    # No source may open a write handle. The extension writes nothing of its own, and a new writer has to argue
    # for itself here first.
    if handleWriters:
        found.append(
            "no extension source may open a write-capable file handle, found "
            + ", ".join(handleWriters)
        )
    coreWriters = [
        relative
        for relative, source in sources.items()
        if any(token in source for token in ("copyFile(", "link(", "rename(", "unlink("))
    ]
    expected_replacers = {"core/managedCore.ts"}
    if set(coreWriters) != expected_replacers or len(coreWriters) != len(expected_replacers):
        found.append(
            "atomic replacement must stay in core/managedCore.ts, found "
            + (", ".join(coreWriters) if coreWriters else "none")
        )
    selection_source = sources.get("selectionStore.ts", "")
    for token in ("prompt", "reply", "approval", "transcript", "conversation", "frame"):
        if token in selection_source.lower():
            found.append(f"selectionStore.ts contains conversation-capable token `{token}`")
    runtime_source = sources.get("runtimeClient.ts", "")
    for token in (
        "connectSystemWithRetry",
        "watchEventsWithReconnectSystem",
        "watchProvidersWithReconnectSystem",
        "watchSessionIndexWithReconnectSystem",
    ):
        if token in runtime_source:
            found.append(f"runtimeClient.ts repeats system locator validation through `{token}`")
    if "conversationDetail" in sources.get("trees.ts", ""):
        found.append("a conversation tree row must not derive or display state, age, service, or project detail")
    if "providerMark" in sources.get("usageDisplay.ts", ""):
        found.append("Agent Usage must render the provider's declared glyph instead of invented text initials")
    if "`Chat ${" in sources.get("conversationList.ts", ""):
        found.append("a conversation title must never expose a shortened internal session identifier")

    contributes = package.get("contributes")
    views_containers = contributes.get("viewsContainers") if isinstance(contributes, dict) else None
    views = contributes.get("views") if isinstance(contributes, dict) else None
    # The conversation surface is the service's own terminal in an editor tab (docs/terminalSurface.md). A
    # chat container or webview view of ours would be that surface growing back.
    for container_kind in ("panel", "secondarySidebar"):
        if isinstance(views_containers, dict) and views_containers.get(container_kind):
            found.append(f"no chat container may be contributed to the {container_kind}; the conversation is a terminal tab")
    for view_group, entries in (views.items() if isinstance(views, dict) else []):
        for entry in entries if isinstance(entries, list) else []:
            if isinstance(entry, dict) and entry.get("type") == "webview" and entry.get("id") != "runtrol.sidebar":
                found.append(f"{view_group} contributes a webview view {entry.get('id')}; the only webview is the sidebar page")

    required = {
        "core/framing.ts": [
            "MAX_FRAME_BYTES",
            "MAX_QUEUED_FRAMES",
            "MAX_QUEUED_BYTES",
            "setImmediate",
            "this.socket.end()",
        ],
                "conversationIcon.ts": [
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`)',
            'const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle"',
            "existsSync(candidate.fsPath)",
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg")',
            "export function accentedConversationIcon",
            "data:image/svg+xml;base64",
        ],
        "terminalTabs.ts": [
            "projectAccentColor(",
            "iconPath: this.iconFor(",
            "vscode.ProgressLocation.Window",
        ],
        "runtimeTerminal.ts": [
            "this.presentation.opening(connecting)",
            "this.presentation.ended(notification.exitCode)",
            "this.presentation.failed(message)",
        ],
        "extension.ts": [
            "afterReady",
            "selfApproveIntegration(client, pendingId, signature)",
            'initializationStage = "core:currency"',
            'executeCommand("runtrol.sidebar.focus")',
            '"runtrol.setUpServices"',
            "registerWebviewViewProvider(SIDEBAR_VIEW_ID, sidebar",
            "await controller.signInProvider(provider)",
            "await controller.fixService(provider)",
        ],
        "providerHealth.ts": [
            "the installed executable has not completed a verified probe",
            "export function awaitsVerification",
            "export function isBroken",
        ],
                "conversationList.ts": [
            # The panel is the machine's, not this window's: only added projects are headings, and their
            # order is the person's rather than whatever ran most recently.
            "return qualified(records",
            "byAddedOrder",
            "if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;",
            "export function conversationDetail",
            'return "";',
            '"Unnamed conversation"',
            'return "Cannot reopen"',
        ],
        "stateRows.ts": [
            "export function discoveryNotice",
            'names(partial, "partial for")',
            'names(unavailable, "unavailable for")',
        ],
        "sidebarPage.ts": [
            # The three zones, each with its own title, in the operator's order.
            'aria-label="Projects"',
            'aria-label="Conversations"',
            'aria-label="Usage"',
            # Open rows and terminal tabs consume the same accented provider SVG. Work adds rotation without
            # changing that identity, and no row spends a separate left band on the same state.
            "readonly accent: string;",
            "readonly open: boolean;",
            "assets.accentIconUris.get",
            'row.open ? " open" : ""',
            ".conv.open .glyph, .conv.working .glyph { filter: none; opacity: 1; }",
            # Row actions appear on hover, and deletion only where the provider reports it.
            ".row:hover .actions",
            'row.canDelete ? action("runtrol.deleteConversation"',
            # A long name stays on one line and its tail fades; two-line rows made the list unreadable
            # (operator, 2026-08-28). Memory rides the row.
            "white-space: nowrap",
            "mask-image: linear-gradient(to right",
            # A running turn is unmistakable: one row-level state turns only the provider icon. The arc that
            # used to turn around the icon remains absent.
            ".conv.working .glyph { animation: spin",
            # A project shows five conversations and says how many more there are.
            'data-kind="more"',
            'class="memory"',
            "Content-Security-Policy",
            "script-src 'nonce-",
        ],
        "sidebarView.ts": [
            "enableScripts: true",
            "localResourceRoots",
            "usageRows(this.state.usage",
            # The empty list has four different reasons and the page says which.
            "Cannot reach the Runtrol Core.",
            "Connecting to the Runtrol Core...",
            "Checking the installed coding-agent CLI...",
            "No coding-agent CLI was found on this machine.",
            '"runtrol.hasUsableProvider"',
            '"runtrol.isVerifyingProvider"',
            "awaitsVerification",
            "this.state.incompleteDiscovery",
            "projectAccentColor(group.workspace)",
            "this.tabs.isOpen(row.key)",
            "accentedConversationIcon(",
            "ROWS_PER_PROJECT",
            "canDelete(row, capabilities)",
        ],
        "usageStrip.ts": [
            "export function usageChips",
            "primarySevenDayMeter(row.meters)",
            'role="progressbar"',
            'aria-expanded="false"',
            "export function escapeHtml",
        ],
        "usageDisplay.ts": [
            'provider.installation.state !== "missing"',
            'detail: "Not signed in · Sign in"',
            'detail: "Checking"',
            'detail: "Unavailable · Fix"',
            'detail: `Disconnected · ${usageDetail(gauge, nowMs)}`',
            "export function usageMeters",
            "export function primarySevenDayMeter",
            'meters.find((meter) => meter.label === "7d")',
            'meters.find((meter) => meter.label.startsWith("7d "))',
            "Math.max(0, Math.min(100",
            "export function setupRows",
            "icon: providerIcon(providerId, providers)",
            "export function usageAbsenceCause",
        ],
        "serializedWatch.ts": [
            "private active: AbortController",
            "const previous = this.tail",
            "const current = previous.then",
            "this.active?.abort()",
        ],
        "controller.ts": [
            "private indexAbort",
            "this.runtime.inventory()",
            "this.startSessionIndexWatch()",
            "this.startProviderVerification(",
            "this.runtime.verifyProvider(",
            "awaitsVerification(",
            "reconnect",
            "workspaceCollisions",
        ],
        "core/client.ts": ["commandConnection", "commandTail"],
        "runtimeClient.ts": [
            'this.reportInitialization("identity+core")',
            'this.reportInitialization("locator")',
            "RuntimeLocator.system(",
            "RUNTIME_LOCATOR_SETTLE_MS",
            "isAbsolute(runtimeExecutable)",
            "withRuntimeLocator",
            "providerSnapshot",
            "sessionSnapshot",
            "async cool(",
            "watchSessions",
            "watchSessionIndexWithReconnect",
        ],
        "core/locator.ts": ['["endpoint"]', 'executable: "runtrol"', "runtimeExecutable"],
        "core/managedCore.ts": [
            "createReadStream",
            "copyFile(source, incoming)",
            "imageName(sourceDigest)",
            "rename(incoming, executable)",
            "removeInactiveImages",
        ],
        "selectionStore.ts": [
            "MAX_FILE_BYTES",
            "MAX_SESSION_BYTES",
            "WRITE_ATTEMPTS",
            "schema: 1",
            "validSession",
            "retryTransientWrite",
            "writeFile(file",
        ],
        "journeyApi.ts": [
            "extensionMode !== vscode.ExtensionMode.Test",
            'process.env.RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY !== "1"',
            "return undefined",
            "sessions: () => [...state.sessions]",
        ],
    }
    for relative, tokens in required.items():
        source = sources.get(relative, "")
        for token in tokens:
            if token not in source:
                found.append(f"{relative} does not contain required contract `{token}`")
    if '<span class="bar' in sources.get("sidebarPage.ts", ""):
        found.append("sidebar conversation and project rows must not restore a left colour bar")
    if "view.description" in sources.get("trees.ts", ""):
        found.append("the machine-wide conversation view must not look scoped under the current folder")
    return found


def iconViolations(icons: dict[str, str]) -> list[str]:
    """Every shipped provider mark is a vector: no embedded raster, no external reference.

    A raster in an SVG wrapper is a bitmap that happens to have an .svg name: it blurs at the sizes the
    sidebar draws and cannot follow the theme. Measured 2026-08-25: the Grok mark shipped as a PNG in a
    wrapper, and it was also the wrong (2023) mark.
    """
    found: list[str] = []
    for name, text in sorted(icons.items()):
        lowered = text.lower()
        if "<image" in lowered or "data:image" in lowered or "xlink:href" in lowered:
            found.append(f"provider mark {name} embeds a raster or an external reference; ship a vector")
        if "<path" not in lowered and "<polygon" not in lowered and "<circle" not in lowered and "<rect" not in lowered:
            found.append(f"provider mark {name} draws no vector shape")
    return found


def engineAtLeast(package: dict, floor: tuple[int, int]) -> bool:
    """Whether the manifest's VS Code engine range starts at or above `floor` (major, minor)."""
    engines = package.get("engines")
    declared = engines.get("vscode") if isinstance(engines, dict) else None
    if not isinstance(declared, str):
        return False
    digits = declared.lstrip("^~>=v ").split(".")
    try:
        major, minor = int(digits[0]), int(digits[1])
    except (IndexError, ValueError):
        return False
    return (major, minor) >= floor


def selftest() -> int:
    """Prove the detector rejects each class of defect."""
    raster = '<svg xmlns="http://www.w3.org/2000/svg"><image xlink:href="data:image/png;base64,AAAA"/></svg>'
    vector = '<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1z"/></svg>'
    if not iconViolations({"grok.svg": raster}):
        print("[vscodeExtension] selftest FAIL. a raster wrapped in an SVG passed as a provider mark.", file=sys.stderr)
        return 2
    if iconViolations({"grok.svg": vector}):
        print("[vscodeExtension] selftest FAIL. a vector provider mark was refused.", file=sys.stderr)
        return 2
    package = {
        "engines": {"vscode": "^1.106.0"},
        "activationEvents": ["onView:runtrol.sidebar", "onCommand:runtrol.setUpServices"],
        "contributes": {
            "viewsContainers": {"activitybar": [{"id": "runtrol", "title": "Runtrol"}]},
            "views": {"runtrol": [{"id": "runtrol.sidebar", "name": "Runtrol", "type": "webview"}]},
            "commands": [
                {"command": "runtrol.setUpServices"},
                {"command": "runtrol.deleteConversation", "icon": "$(close)"},
            ],
            "menus": {
                "view/title": [
                    {
                        "command": "runtrol.startSession",
                        "when": "view == runtrol.sidebar",
                        "group": "navigation@0",
                    },
                    {
                        "command": "runtrol.createProject",
                        "when": "view == runtrol.sidebar",
                        "group": "navigation@1",
                    },
                    {
                        "command": "runtrol.moreActions",
                        "when": "view == runtrol.sidebar",
                        "group": "navigation@2",
                    },
                    {
                        "command": "runtrol.switchSession",
                        "when": "view == runtrol.sidebar",
                        "group": "1_attention@3",
                    },
                ],
            },
            "colors": [{"id": "runtrol.accent"}],
        },
    }
    sources = {
        "core/framing.ts": (
            "MAX_FRAME_BYTES MAX_QUEUED_FRAMES MAX_QUEUED_BYTES setImmediate "
            "this.socket.end()"
        ),
        "terminalTabs.ts": (
            "projectAccentColor( iconPath: this.iconFor( vscode.ProgressLocation.Window"
        ),
        "runtimeTerminal.ts": (
            "this.presentation.opening(connecting) this.presentation.ended(notification.exitCode) "
            "this.presentation.failed(message) this.writeEmitter.fire(text)"
        ),
        "conversationIcon.ts": (
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`) '
            'const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle" '
            'existsSync(candidate.fsPath) '
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg") '
            "export function accentedConversationIcon data:image/svg+xml;base64"
        ),
        "extension.ts": (
            "afterReady selfApproveIntegration(client, pendingId, signature) "
            'initializationStage = "core:currency" '
            'executeCommand("runtrol.sidebar.focus") '
            '"runtrol.setUpServices" registerWebviewViewProvider(SIDEBAR_VIEW_ID, sidebar '
            "await controller.signInProvider(provider) await controller.fixService(provider)"
        ),
        "providerHealth.ts": (
            "the installed executable has not completed a verified probe "
            "export function awaitsVerification export function isBroken"
        ),
        "conversationList.ts": (
            'return qualified(records byAddedOrder '
            'if (left.pinned !== right.pinned) return left.pinned ? -1 : 1; '
            'export function conversationDetail return ""; '
            '"Unnamed conversation" return "Cannot reopen"'
        ),
        "stateRows.ts": (
            'export function discoveryNotice names(partial, "partial for") '
            'names(unavailable, "unavailable for")'
        ),
        "sidebarPage.ts": (
            "<!DOCTYPE html> "
            'aria-label="Projects" aria-label="Conversations" aria-label="Usage" '
            'readonly accent: string; readonly open: boolean; assets.accentIconUris.get '
            'row.open ? " open" : "" .conv.open .glyph, .conv.working .glyph { filter: none; opacity: 1; } '
            '.row:hover .actions row.canDelete ? action("runtrol.deleteConversation" '
            "white-space: nowrap mask-image: linear-gradient(to right "
            '.conv.working .glyph { animation: spin '
            'data-kind="more" '
            'class="memory" Content-Security-Policy script-src \'nonce-'
        ),
        "sidebarView.ts": (
            "enableScripts: true localResourceRoots usageRows(this.state.usage "
            "Cannot reach the Runtrol Core. Connecting to the Runtrol Core... "
            "Checking the installed coding-agent CLI... No coding-agent CLI was found on this machine. "
            '"runtrol.hasUsableProvider" "runtrol.isVerifyingProvider" awaitsVerification '
            "this.state.incompleteDiscovery projectAccentColor(group.workspace) this.tabs.isOpen(row.key) "
            "accentedConversationIcon( ROWS_PER_PROJECT "
            "canDelete(row, capabilities)"
        ),
        "usageStrip.ts": (
            "export function usageChips primarySevenDayMeter(row.meters) "
            "role=\"progressbar\" Content-Security-Policy script-src 'nonce- aria-expanded=\"false\" "
            "export function escapeHtml"
        ),
        "usageDisplay.ts": (
            'provider.installation.state !== "missing" detail: "Not signed in · Sign in" '
            'detail: "Checking" detail: "Unavailable · Fix" '
            'detail: `Disconnected · ${usageDetail(gauge, nowMs)}` '
            'export function usageMeters export function primarySevenDayMeter '
            'meters.find((meter) => meter.label === "7d") '
            'meters.find((meter) => meter.label.startsWith("7d ")) '
            "Math.max(0, Math.min(100 "
            'export function setupRows export function usageAbsenceCause icon: providerIcon(providerId, providers) '
            'provider.installation.state === "missing" && Boolean(provider.help?.install)'
        ),
        "serializedWatch.ts": (
            "private active: AbortController; const previous = this.tail; "
            "const current = previous.then; this.active?.abort()"
        ),
        "controller.ts": (
            'private indexAbort; '
            'this.runtime.inventory(); this.startSessionIndexWatch(); this.startProviderVerification( '
            'this.runtime.verifyProvider( awaitsVerification( '
            'reconnect workspaceCollisions '
            ''
            '"Start here anyway"'
        ),
        "core/client.ts": "commandConnection commandTail",
        "runtimeClient.ts": (
            'this.reportInitialization("identity+core") this.reportInitialization("locator") '
            "RuntimeLocator.system( RUNTIME_LOCATOR_SETTLE_MS isAbsolute(runtimeExecutable) withRuntimeLocator "
            "providerSnapshot sessionSnapshot async cool( watchSessions "
            "watchSessionIndexWithReconnect"
        ),
        "core/locator.ts": '["endpoint"] executable: "runtrol" runtimeExecutable',
        "core/managedCore.ts": (
            "createReadStream copyFile(source, incoming) imageName(sourceDigest) writeFile( "
            "rename(incoming, executable) unlink(file) removeInactiveImages"
        ),
        "selectionStore.ts": (
            "MAX_FILE_BYTES MAX_SESSION_BYTES WRITE_ATTEMPTS schema: 1 validSession "
            "retryTransientWrite writeFile(file"
        ),
        "journeyApi.ts": (
            'extensionMode !== vscode.ExtensionMode.Test '
            'process.env.RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY !== "1" return undefined '
            'sessions: () => [...state.sessions] '
            'controller.startResolvedSession(provider, workspace, model, reasoningEffort, "exclusive", false, permission)'
        ),
    }
    rejected = sourceViolations(package, sources)
    if rejected:
        # Say which one. A self-test that only says the fixture was rejected sends the next reader hunting
        # through every contract entry by hand, which is what it cost to find this line.
        print("[vscodeExtension --selftest] FAIL. the green fixture was rejected:", file=sys.stderr)
        for violation in rejected:
            print(f"  - {violation}", file=sys.stderr)
        return 2

    second_view = json.loads(json.dumps(package))
    second_view["contributes"]["views"]["runtrol"].append({"id": "runtrol.usage", "name": "Usage", "type": "webview"})
    native_sidebar = json.loads(json.dumps(package))
    del native_sidebar["contributes"]["views"]["runtrol"][0]["type"]
    welcome_back = json.loads(json.dumps(package))
    welcome_back["contributes"]["viewsWelcome"] = [{"view": "runtrol.sidebar", "contents": "Connecting"}]
    row_menus = json.loads(json.dumps(package))
    row_menus["contributes"]["menus"]["view/item/context"] = [{"command": "runtrol.deleteConversation", "when": "view == runtrol.sidebar"}]
    chat_container = json.loads(json.dumps(package))
    chat_container["contributes"]["viewsContainers"]["panel"] = [{"id": "runtrolPanel", "title": "Chat"}]
    cluttered_toolbar = json.loads(json.dumps(package))
    cluttered_toolbar["contributes"]["menus"]["view/title"].append({
        "command": "runtrol.arrangeConversationGrid",
        "when": "view == runtrol.sidebar",
        "group": "navigation@3",
    })
    mutations = [
        ({**package, "dependencies": {"some-runtime": "1"}}, sources),
        (second_view, sources),
        (native_sidebar, sources),
        (welcome_back, sources),
        (row_menus, sources),
        (chat_container, sources),
        (cluttered_toolbar, sources),
        ({**package, "activationEvents": []}, sources),
        ({**package, "contributes": {"viewsContainers": {"activitybar": []}}}, sources),
        ({"engines": {"vscode": "^1.100.0"}, "contributes": {"viewsContainers": {"activitybar": [], "secondarySidebar": []}}}, sources),
        (package, {**sources, "controller.ts": "setInterval("}),
        (package, {**sources, "core/framing.ts": "MAX_FRAME_BYTES"}),
        (
            package,
            {
                **sources,
                "core/framing.ts": sources["core/framing.ts"].replace(
                    "this.socket.end()", "this.socket.destroy()"
                ),
            },
        ),
        (package, {**sources, "conversationIcon.ts": sources["conversationIcon.ts"].replace("provider-icons", "brand")}),
        (package, {**sources, "conversationList.ts": sources["conversationList.ts"] + " `Chat ${identity}`"}),
        (package, {**sources, "usageStrip.ts": sources["usageStrip.ts"].replace('role="progressbar"', "")}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"].replace("Content-Security-Policy", "")}),
        (package, {**sources, "sidebarView.ts": sources["sidebarView.ts"].replace("usageRows(this.state.usage", "")}),
        (package, {**sources, "sidebarView.ts": sources["sidebarView.ts"].replace("Cannot reach the Runtrol Core.", "")}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"].replace('row.canDelete ? action("runtrol.deleteConversation"', "")}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"].replace(".conv.working .glyph { animation: spin", "")}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"].replace("assets.accentIconUris.get", "")}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"] + '<span class="bar"></span>'}),
        (package, {**sources, "sidebarPage.ts": sources["sidebarPage.ts"].replace(".conv.open .glyph, .conv.working .glyph { filter: none; opacity: 1; }", "")}),
        (package, {**sources, "sidebarView.ts": sources["sidebarView.ts"].replace("projectAccentColor(group.workspace)", "")}),
        (package, {**sources, "sidebarView.ts": sources["sidebarView.ts"].replace("this.tabs.isOpen(row.key)", "")}),
        (package, {**sources, "extension.ts": sources["extension.ts"].replace('executeCommand("runtrol.sidebar.focus")', "")}),
        (package, {**sources, "stateRows.ts": sources["stateRows.ts"].replace("discoveryNotice", "")}),
        (
            package,
            {
                **sources,
                "conversationList.ts": sources["conversationList.ts"].replace('return "Cannot reopen"', ""),
            },
        ),
        (
            package,
            {
                **sources,
                "conversationList.ts": sources["conversationList.ts"].replace('return "";', ""),
            },
        ),
        (package, {**sources, "controller.ts": sources["controller.ts"].replace("workspaceCollisions", "")}),
        (package, {**sources, "controller.ts": sources["controller.ts"] + " writeFile("}),
        (package, {**sources, "controller.ts": sources["controller.ts"] + ' open(file, "w")'}),
        (package, {**sources, "controller.ts": sources["controller.ts"] + " copyFile("}),
        (package, {**sources, "selectionStore.ts": sources["selectionStore.ts"] + " prompt"}),
        (package, {**sources, "selectionStore.ts": sources["selectionStore.ts"].replace("retryTransientWrite", "")}),
        (package, {**sources, "journeyApi.ts": "return undefined sessions: () => [...state.sessions]"}),
        (package, {**sources, "runtimeClient.ts": sources["runtimeClient.ts"] + " connectSystemWithRetry"}),
        (package, {**sources, "runtimeClient.ts": sources["runtimeClient.ts"].replace("providerSnapshot", "")}),
        (package, {**sources, "controller.ts": sources["controller.ts"].replace("this.runtime.inventory()", "")}),
        (package, {**sources, "controller.ts": sources["controller.ts"].replace("this.runtime.verifyProvider(", "")}),
        (package, {**sources, "extension.ts": sources["extension.ts"].replace("selfApproveIntegration", "")}),
        (
            package,
            {
                **sources,
                "serializedWatch.ts": sources["serializedWatch.ts"].replace("this.active?.abort()", ""),
            },
        ),
    ]
    for index, (changed_package, changed_sources) in enumerate(mutations, start=1):
        if not sourceViolations(changed_package, changed_sources):
            print(f"[vscodeExtension --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(f"[vscodeExtension --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def npmCommand() -> list[str]:
    """Return an explicit npm launcher without asking a shell to interpret product input."""
    npm = shutil.which("npm.cmd" if sys.platform == "win32" else "npm") or shutil.which("npm")
    if npm is None:
        raise RuntimeError("npm is missing")
    if sys.platform == "win32":
        command = osCommand()
        return [command, "/d", "/c", npm]
    return [npm]


def osCommand() -> str:
    """Find the Windows command host used only to launch npm.cmd."""
    import os

    return os.environ.get("ComSpec", r"C:\Windows\System32\cmd.exe")


def run() -> int:
    """Inspect sources, type-check, test, and bundle the extension."""
    package_path = EXTENSION / "package.json"
    lock = EXTENSION / "package-lock.json"
    if not package_path.is_file() or not lock.is_file():
        print("[vscodeExtension] FAIL. package.json and package-lock.json are required.", file=sys.stderr)
        return 2
    package = json.loads(package_path.read_text(encoding="utf-8"))
    sources = {
        path.relative_to(EXTENSION / "src").as_posix(): path.read_text(encoding="utf-8")
        for path in (EXTENSION / "src").rglob("*")
        if path.is_file()
        and path.suffix in {".ts", ".css"}
        and not path.name.endswith(".test.ts")
        and path.name != "styles.d.ts"
    }
    icons = {
        path.name: path.read_text(encoding="utf-8")
        for path in (ROOT / "assets" / "brand" / "provider-icons").glob("*.svg")
    }
    failures = sourceViolations(package, sources) + iconViolations(icons)
    if failures:
        print("[vscodeExtension] FAIL. static contract violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2

    command = npmCommand()
    # The path-sensitive suites, as a POSIX platform sees them. Development happens on Windows and
    # CI runs three operating systems, so fixtures that hardcode a backslash pass here and fail
    # there; measured 2026-08-20, four such tests were red on the Linux runner for days. Running the
    # simulation beside the real suite makes that a local failure instead of a CI surprise.
    posix = subprocess.run(
        [*command, "run", "test:posix"],
        cwd=EXTENSION,
        check=False,
        text=True,
        capture_output=True,
        timeout=180,
    )
    if posix.returncode != 0:
        print(posix.stdout, file=sys.stderr)
        print(posix.stderr, file=sys.stderr)
        print("[vscodeExtension] FAIL. the POSIX-simulated suites are red.", file=sys.stderr)
        return 2

    for script in ("check", "test", "build"):
        result = subprocess.run(
            [*command, "run", script],
            cwd=EXTENSION,
            check=False,
            text=True,
            capture_output=True,
            timeout=180,
        )
        if result.returncode != 0:
            print(result.stdout, file=sys.stderr)
            print(result.stderr, file=sys.stderr)
            print(f"[vscodeExtension] FAIL. npm run {script} returned {result.returncode}.", file=sys.stderr)
            return 2

    # The current Studio surface fits inside 320 KiB. This is a bloat tripwire, not a target; a reviewed feature may
    # move it deliberately, while deleted surfaces cannot leave their former budget behind.
    bundles = [
        EXTENSION / "dist" / name
        for name in (
            "extension.js",
            "pairingQrVendor.js",
        )
    ]
    for bundle in bundles:
        if not bundle.is_file() or bundle.stat().st_size > 320 * 1024:
            failures.append(f"{bundle.relative_to(ROOT)} is missing or exceeds 320 KiB")
    qr_bundle = EXTENSION / "dist" / "pairingQrVendor.js"
    if qr_bundle.is_file() and qr_bundle.stat().st_size > 32 * 1024:
        failures.append(f"{qr_bundle.relative_to(ROOT)} exceeds its pairing-only 32 KiB budget")
    extension_bundle = EXTENSION / "dist" / "extension.js"
    if extension_bundle.is_file() and "RUNTROL_VSCODE_REAL_PROVIDER_JOURNEY" in extension_bundle.read_text(
        encoding="utf-8"
    ):
        failures.append("the production extension bundle contains the test-only Journey API")
    if failures:
        print("[vscodeExtension] FAIL. bundle contract violations:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 2

    total = sum(bundle.stat().st_size for bundle in bundles)
    print(f"[vscodeExtension] OK. thin source contract and {total} bundled bytes verified.")
    return 0


def main() -> int:
    """Select the selftest or the real gate."""
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: vscodeExtension.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
