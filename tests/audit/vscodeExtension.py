"""Gate: the VS Code surface stays thin, bounded, buildable, and package-shaped.

The gate deliberately checks the source contract before invoking the toolchain. A bundle that compiles can still
poll, persist conversation data, keep hidden renderers alive, or ship runtime Node dependencies. Product disk writes
are limited to the bounded selected-session scalar, reviewed Receipt Landing modules, and atomic Core installer.

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
    usage_view = next(
        (
            entry
            for entry in runtrol_views
            if isinstance(entry, dict) and entry.get("id") == "runtrol.usage"
        ),
        None,
    ) if isinstance(runtrol_views, list) else None
    if (
        not isinstance(usage_view, dict)
        or usage_view.get("visibility") != "visible"
        or usage_view.get("name") != "Agent Usage"
        or usage_view.get("type") != "webview"
    ):
        found.append(
            "every installed CLI's status and progress bars must be expanded at the bottom of the Runtrol sidebar"
        )
    for container, view_id in (
        ("runtrolPanel", "runtrol.conversationPanel"),
        ("runtrolSide", "runtrol.conversationSide"),
    ):
        entries = views.get(container) if isinstance(views, dict) else None
        chat_view = next(
            (
                entry
                for entry in entries
                if isinstance(entry, dict) and entry.get("id") == view_id
            ),
            None,
        ) if isinstance(entries, list) else None
        if not isinstance(chat_view, dict) or chat_view.get("name") != "Chat":
            found.append("the panel and secondary side bar must be named Chat without a product prefix")
    for area, container_id in (
        ("panel", "runtrolPanel"),
        ("secondarySidebar", "runtrolSide"),
    ):
        entries = view_containers.get(area) if isinstance(view_containers, dict) else None
        chat_container = next(
            (
                entry
                for entry in entries
                if isinstance(entry, dict) and entry.get("id") == container_id
            ),
            None,
        ) if isinstance(entries, list) else None
        if not isinstance(chat_container, dict) or chat_container.get("title") != "Chat":
            found.append("the panel and secondary side bar containers must be named Chat without a product prefix")
    welcome_entries = contributes.get("viewsWelcome") if isinstance(contributes, dict) else None
    welcomes = welcome_entries if isinstance(welcome_entries, list) else []
    has_ready_welcome = any(
        isinstance(entry, dict)
        and entry.get("view") == "runtrol.sessions"
        and entry.get("when") == "runtrol.hasUsableProvider"
        and "command:runtrol.startSession" in str(entry.get("contents", ""))
        for entry in welcomes
    )
    has_missing_welcome = any(
        isinstance(entry, dict)
        and entry.get("view") == "runtrol.sessions"
        and entry.get("when") == "!runtrol.hasUsableProvider && !runtrol.isVerifyingProvider"
        and "command:runtrol.refresh" in str(entry.get("contents", ""))
        for entry in welcomes
    )
    has_verifying_welcome = any(
        isinstance(entry, dict)
        and entry.get("view") == "runtrol.sessions"
        and entry.get("when") == "runtrol.isVerifyingProvider"
        and "Checking" in str(entry.get("contents", ""))
        for entry in welcomes
    )
    if not has_ready_welcome or not has_missing_welcome or not has_verifying_welcome:
        found.append("the empty sidebar must distinguish usable, verifying, and absent coding-agent CLIs")
    menus = contributes.get("menus") if isinstance(contributes, dict) else None
    title_entries = menus.get("view/title") if isinstance(menus, dict) else None
    title_commands = {
        entry.get("command")
        for entry in title_entries
        if isinstance(entry, dict)
        and "view == runtrol.sessions" in str(entry.get("when", ""))
        and str(entry.get("group", "")).startswith("navigation")
    } if isinstance(title_entries, list) else set()
    if title_commands != {"runtrol.startSession", "runtrol.createProject", "runtrol.switchSession"}:
        found.append("the Conversations title bar must keep exactly its three frequent actions visible")
    item_context = menus.get("view/item/context") if isinstance(menus, dict) else None
    winner_entries = item_context if isinstance(item_context, list) else []
    # The guard is the contract: deletion is offered only where the provider reported a deletion surface, and it
    # is offered on the row itself. Which inline slot it takes is a matter of button order, and pinning that here
    # would make adding a neighbouring action look like a broken invariant.
    delete_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.deleteConversation"
        and entry.get("when") == "view == runtrol.sessions && viewItem =~ /\\.delete/"
        and str(entry.get("group", "")).startswith("inline")
        for entry in winner_entries
    )
    if not delete_entry:
        found.append("conversation deletion must appear only on rows whose provider reports deletion available")
    conversation_inline = [
        entry.get("command")
        for entry in winner_entries
        if isinstance(entry, dict)
        and entry.get("command") in {
            "runtrol.archiveConversation",
            "runtrol.closeSession",
            "runtrol.deleteConversation",
        }
        and str(entry.get("group", "")).startswith("inline")
        and "view == runtrol.sessions" in str(entry.get("when", ""))
    ]
    if conversation_inline != ["runtrol.deleteConversation"]:
        found.append("a conversation row must expose one inline X for real deletion and no other buttons")
    winner_task_when = (
        "view == runtrol.missions && viewItem =~ "
        "/^runtrol\\.missionTask\\.passed(\\.session)?\\.chooseOne\\.integrating$/"
    )
    winner_task_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.reviewMissionLanding"
        and entry.get("when") == winner_task_when
        for entry in winner_entries
    )
    winner_mission_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.reviewMissionLanding"
        and entry.get("when") == "view == runtrol.missions && viewItem == runtrol.mission.integrating.chooseOne"
        for entry in winner_entries
    )
    if not winner_task_entry or not winner_mission_entry:
        found.append("Fleet winner Landing must be reachable from both the integrating Mission and a passed Task")
    recovery_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.recoverInterruptedMission"
        and entry.get("when") == (
            "view == runtrol.missions && viewItem =~ "
            "/^runtrol\\.mission\\.blocked(\\.chooseOne)?(\\.autoFlight)?$/"
        )
        for entry in winner_entries
    )
    if not recovery_entry:
        found.append("a recoverable blocked Mission must expose the exact interrupted-recovery action")

    command_entries = contributes.get("commands") if isinstance(contributes, dict) else None
    command_ids = {
        entry.get("command")
        for entry in command_entries if isinstance(entry, dict)
    } if isinstance(command_entries, list) else set()
    schedule_commands = {"runtrol.scheduleMission", "runtrol.cancelMissionSchedule"}
    if not schedule_commands.issubset(command_ids):
        found.append("Mission schedule and exact cancel commands must both be contributed")
    activation_events = package.get("activationEvents")
    activations = set(activation_events) if isinstance(activation_events, list) else set()
    if not {f"onCommand:{command}" for command in schedule_commands}.issubset(activations):
        found.append("Mission schedule and exact cancel commands must both activate the extension")
    delete_command = next(
        (
            entry for entry in command_entries
            if isinstance(entry, dict) and entry.get("command") == "runtrol.deleteConversation"
        ),
        None,
    ) if isinstance(command_entries, list) else None
    if (
        not isinstance(delete_command, dict)
        or delete_command.get("title") != "Delete Conversation"
        or delete_command.get("icon") != "$(close)"
    ):
        found.append("the row's single deletion action must be an unbranded X")
    schedule_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.scheduleMission"
        and entry.get("when") == "view == runtrol.missions && viewItem =~ /^runtrol\\.mission\\.validated/"
        for entry in winner_entries
    )
    cancel_schedule_entry = any(
        isinstance(entry, dict)
        and entry.get("command") == "runtrol.cancelMissionSchedule"
        and entry.get("when") == "view == runtrol.missions && viewItem =~ /schedulePending/"
        for entry in winner_entries
    )
    if not schedule_entry or not cancel_schedule_entry:
        found.append("a reviewed Mission and its pending schedule must expose schedule and exact cancel actions")
    if "runtrol.discoverServices" not in command_ids or "onCommand:runtrol.discoverServices" not in activations:
        found.append("the fixed sidebar service catalogue must have one activatable discovery command")

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
    }
    for token, meaning in forbidden.items():
        if token in all_source:
            found.append(f"{meaning} is reachable through `{token}`")
    for relative in ("mission/controller.ts", "mission/schedule.ts"):
        if "setTimeout(" in sources.get(relative, ""):
            found.append(f"Core-owned Mission wake must not be replaced by a timer in {relative}")

    writers = [relative for relative, source in sources.items() if "writeFile(" in source]
    expected_writers = {"selectionStore.ts"}
    if set(writers) != expected_writers or len(writers) != len(expected_writers):
        found.append(
            "direct writeFile calls must stay in selectionStore.ts, found "
            + (", ".join(writers) if writers else "none")
        )
    handleWriters = [
        relative
        for relative, source in sources.items()
        if "handle.write(" in source or re.search(r"\bopen\([^,\n]+,\s*[\"'][wax]", source)
    ]
    if handleWriters != ["mission/landing/atomicFile.ts"]:
        found.append(
            "write-capable file handles must stay in mission/landing/atomicFile.ts, found "
            + (", ".join(handleWriters) if handleWriters else "none")
        )
    coreWriters = [
        relative
        for relative, source in sources.items()
        if any(token in source for token in ("copyFile(", "link(", "rename(", "unlink("))
    ]
    expected_replacers = {"core/managedCore.ts", "mission/landing/atomicFile.ts"}
    if set(coreWriters) != expected_replacers or len(coreWriters) != len(expected_replacers):
        found.append(
            "atomic replacement must stay in managedCore.ts and mission/landing/atomicFile.ts, found "
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
    if 'mark.textContent = "R"' in sources.get("webview/main.ts", ""):
        found.append("the chat page must not place the product brand above the conversation")
    if "Runtrol sidebar" in sources.get("conversationSurface.ts", ""):
        found.append("an empty chat place must name the Conversations sidebar, not repeat the product brand")
    if "conversationDetail" in sources.get("trees.ts", ""):
        found.append("a conversation tree row must not derive or display state, age, service, or project detail")
    if "providerMark" in sources.get("usageDisplay.ts", "") or "providerMark" in sources.get("usageViewWebview.ts", ""):
        found.append("Agent Usage must render the provider's declared glyph instead of invented text initials")
    if "resources/symbol.svg" in "".join(
        sources.get(name, "") for name in ("conversationView.ts", "conversationPanels.ts", "conversationSurface.ts")
    ):
        found.append("an individual conversation surface must never use the product symbol as its speaker icon")
    if "`Chat ${" in sources.get("conversationList.ts", ""):
        found.append("a conversation title must never expose a shortened internal session identifier")

    contributes = package.get("contributes")
    views_containers = contributes.get("viewsContainers") if isinstance(contributes, dict) else None
    views = contributes.get("views") if isinstance(contributes, dict) else None
    neutral_icon = "resources/provider-icons/sparkle.svg"
    for container_kind, container_id in (("panel", "runtrolPanel"), ("secondarySidebar", "runtrolSide")):
        entries = views_containers.get(container_kind) if isinstance(views_containers, dict) else None
        entry = next((item for item in entries or [] if isinstance(item, dict) and item.get("id") == container_id), None)
        if not isinstance(entry, dict) or entry.get("icon") != neutral_icon:
            found.append("shared chat containers must use a neutral coding-service glyph, never the product symbol")
    for container_id in ("runtrolPanel", "runtrolSide"):
        entries = views.get(container_id) if isinstance(views, dict) else None
        if not isinstance(entries, list) or any(
            not isinstance(entry, dict) or entry.get("icon") != neutral_icon for entry in entries
        ):
            found.append("shared chat views must use a neutral coding-service glyph, never the product symbol")

    required = {
        "core/framing.ts": [
            "MAX_FRAME_BYTES",
            "MAX_QUEUED_FRAMES",
            "MAX_QUEUED_BYTES",
            "setImmediate",
            "this.socket.end()",
        ],
        "conversationView.ts": [
            "webviewReady",
            "createWebviewPanel",
            "focusSurface",
            "conversationTabIsActive",
            "onDidChangeTabs",
            "onDidChangeTabGroups",
            "MEASUREMENT_ATTEMPTS",
            "withinMeasurementStage",
            "waitForVisibleWebview",
            "retainContextWhenHidden: false",
            'aria-haspopup="listbox"',
            'aria-controls="commands"',
            'aria-expanded="false"',
            "surface.iconPath = this.panelIcon",
            "conversationIcon(this.extensionUri, this.iconOf(providerId))",
        ],
        "conversationIcon.ts": [
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`)',
            'const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle"',
            "existsSync(candidate.fsPath)",
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg")',
        ],
        "extension.ts": [
            "afterReady",
            "selfApproveIntegration(client, pendingId, signature)",
            'initializationStage = "runtime:bootstrap"',
            "missionController.startAutoFlights()",
            'executeCommand("runtrol.usage.focus")',
            '"runtrol.scheduleMission"',
            '"runtrol.cancelMissionSchedule"',
            '"runtrol.discoverServices"',
        ],
        "providerHealth.ts": [
            "the installed executable has not completed a verified probe",
            "export function awaitsVerification",
            "export function isBroken",
        ],
        "conversationPanels.ts": [
            'import { SerializedWatch } from "./serializedWatch"',
            "private readonly watch = new SerializedWatch()",
            "this.watch.pause()",
            "this.watch.dispose()",
        ],
        "conversationList.ts": [
            "const folderRows = rows.filter",
            '"open",',
            "if (left.current !== right.current) return left.current ? -1 : 1;",
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
        "trees.ts": [
            "conversation.serviceName",
            "this.description = undefined",
            'capabilities?.nativeSessionDelete?.availability === "available"',
            # Identity comes from the shipped mark, which exists for every service the build knows, rather
            # than from the editor's icon font, which carries a glyph for some services and none for others.
            # Motion while running is the editor's own spinner, because only its glyphs can spin.
            "conversationIcon(extensionUri, conversation.serviceIcon)",
            'new vscode.ThemeIcon("sync~spin")',
            "this.revealCurrentProject()",
            "this.state.incompleteDiscovery",
            "view.message = undefined",
            '"runtrol.hasUsableProvider"',
            '"runtrol.isVerifyingProvider"',
            "awaitsVerification",
        ],
        "webview/main.ts": [
            "MAX_VISIBLE_ITEMS",
            "MAX_VISIBLE_CHARACTERS",
            "MAX_BATCH",
            'setAttribute("aria-activedescendant"',
            'setAttribute("aria-expanded"',
            'removeAttribute("aria-activedescendant")',
            "(item ? prompt : chip)?.focus()",
            "paintDraftPrompt()",
            '"Choose a coding service above"',
            "`Message ${draft.service}`",
        ],
        "usageView.ts": [
            "implements vscode.WebviewViewProvider",
            "private gauges:",
            "usageRowsEqual(this.rows, next)",
            "Usage refresh failed: ${why}. Showing the last report.",
            "usageViewAction(message)",
            "isBroken(provider)",
            "this.installableCount",
            "this.postSnapshot()",
            "enableScripts: true",
            "img-src ${webview.cspSource}",
            'data-icon-base="${iconBase}"',
            '"resources", "provider-icons"',
        ],
        "usageViewWebview.ts": [
            'document.createElement("progress")',
            "bar.max = 100",
            "bar.value = meter.percent",
            'bar.setAttribute("aria-label"',
            'vscode.postMessage({ type: "fix"',
            'vscode.postMessage({ type: "discover"',
            "usage.replaceChildren",
            ".textContent =",
            'return /^[a-z0-9-]{1,64}$/u.test(value) ? value : "sparkle"',
            "image.src = `${iconBase}/${iconName(declared)}.svg`",
            "const fallback = `${iconBase}/sparkle.svg`",
        ],
        "usageViewMessage.ts": [
            "export function usageViewAction",
            "record.providerId.length <= 256",
        ],
        "usageView.css": [
            "progress::-webkit-progress-value",
            ".usage-row.limit-reached",
            "var(--vscode-progressBar-background",
        ],
        "usageDisplay.ts": [
            'provider.installation.state !== "missing"',
            'detail: "Ready"',
            'detail: "Checking"',
            'detail: "Unavailable · Fix"',
            'detail: `Disconnected · ${usageDetail(gauge, nowMs)}`',
            "export function usageMeters",
            "Math.max(0, Math.min(100",
            "export function installableProviders",
            "icon: providerIcon(providerId, providers)",
            'provider.installation.state === "missing" && Boolean(provider.help?.install)',
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
            "conversationSwitchDecision",
            "this.runtime.cool(",
            '"Stop and switch"',
            '"Keep both working"',
            '"Start here anyway"',
        ],
        "mission/autoFlight.ts": [
            "MAX_AUTO_FLIGHTS",
            "sessionGeneration",
            "recordAutoFlightSubmissions",
            "readAutoFlightArms",
            "pendingSignal",
            "stageSignal",
        ],
        "mission/controller.ts": [
            "AUTO_FLIGHTS_KEY",
            "runtimeState.onDidChange",
            "beforeSubmissions",
            "recordSubmissions",
            "startAutoFlights",
            "missionFlightSignal",
            "missionFlightSignalClear",
            "hasAutoFlightRecord",
            "Integrated winner Task",
            "Integrated winner Receipt",
            "recoverInterruptedMission",
            "assertInterruptedRecoveryAuthority",
            "reviewMissionSchedule",
            "assertMissionScheduleAuthority",
            "commitScheduleReview",
            'ask: "missionScheduleCancel"',
            "Core starts after Studio closes",
            "MissionWaveRunner",
            "for (const missionId of this.documents.openMissionIds())",
            "this.documents.update(await this.get(missionId))",
            "refreshRows",
        ],
        "mission/recovery.ts": [
            "interruptedRecoveryPlan",
            "assertInterruptedRecoveryAuthority",
            "missionSha256",
            "policySha256",
            "providerSelector",
            "baseCommit",
            "The previous provider input may already have caused external effects",
        ],
        "mission/schedule.ts": [
            "MIN_SCHEDULE_LEAD_MS",
            "MAX_SCHEDULE_LEAD_MS",
            "reviewMissionSchedule",
            "assertMissionScheduleAuthority",
            "missionSha256",
            "snapshot.policy_sha256",
            "task.instruction_sha256",
            "task.provider_selector",
            "task.workspace_mode",
            "replacesScheduleId",
            "dueUnixMs",
            "providers",
        ],
        "mission/waveRunner.ts": [
            "hasAmbiguousSubmission",
            "markAmbiguousSubmission",
            "resolveInstruction",
            "submit",
            "clearAmbiguousSubmission",
        ],
        "mission/landing/apply.ts": [
            "applyLandingTransaction",
            "landingCompletionProblem",
            "readMissionLanding",
            "createLandingDirectories",
            "writeAtomicLandingFile",
            "readLandingTarget",
        ],
        "mission/landing/model.ts": [
            "type LandingSelection",
            "missionWinnerLanding",
            'selection.kind === "chooseOne"',
            "snapshot.tasks.filter",
            "selection: landing.selection",
            "landingCompletionProblem",
            "selected_receipt_id",
        ],
        "mission/landing/review.ts": [
            "MAX_DIFF_TEXT",
            "document.isDirty",
            "tab.isDirty",
            "Receipt Artifact evidence mismatch",
        ],
        "mission/landing/localFile.ts": [
            "inspectSafeLocalFile",
            "readExactLocalFile",
            "opened.dev !== file.device",
            "named.isSymbolicLink()",
        ],
        "mission/landing/atomicFile.ts": [
            'open(temporary, "wx+"',
            "handle.write",
            "handle.sync",
            "beforeReplace",
            "rename(temporary, target)",
        ],
        "mission/landing/controller.ts": [
            "private currentReview",
            'state: "reviewed" | "appliedAwaitingCore"',
            "withProjectLease",
            "completeLandingWithRecovery",
            "review.landing.selection.taskId",
        ],
        "mission/projectLease.ts": [
            "runtrol-project-integration-leases",
            "acquireProcessLease",
            "ACTIVE_PROCESS_LEASES",
            "attempt < 3",
            "process.kill(pid, 0)",
        ],
        "core/client.ts": ["commandConnection", "commandTail"],
        "runtimeClient.ts": [
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
            "link(executable, preserved)",
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
            'controller.startResolvedSession(provider, workspace, model, reasoningEffort, "exclusive", false, permission)',
            "missions.scheduleMissionForJourney(missionId, dueUnixMs, operatorChoiceProvider)",
        ],
        "protocol.ts": [
            "MissionScheduleLine",
            'ask: "missionSchedule"',
            "replaces_schedule_id",
            'ask: "missionScheduleCancel"',
        ],
    }
    for relative, tokens in required.items():
        source = sources.get(relative, "")
        for token in tokens:
            if token not in source:
                found.append(f"{relative} does not contain required contract `{token}`")
    if "view.description" in sources.get("trees.ts", ""):
        found.append("the machine-wide conversation view must not look scoped under the current folder")
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
    package = {
        "engines": {"vscode": "^1.106.0"},
        "activationEvents": [
            "onCommand:runtrol.scheduleMission",
            "onCommand:runtrol.cancelMissionSchedule",
            "onCommand:runtrol.discoverServices",
        ],
        "contributes": {
            "viewsContainers": {
                "activitybar": [],
                "panel": [{
                    "id": "runtrolPanel",
                    "title": "Chat",
                    "icon": "resources/provider-icons/sparkle.svg",
                }],
                "secondarySidebar": [{
                    "id": "runtrolSide",
                    "title": "Chat",
                    "icon": "resources/provider-icons/sparkle.svg",
                }],
            },
            "views": {
                "runtrol": [
                    {
                        "id": "runtrol.usage",
                        "name": "Agent Usage",
                        "type": "webview",
                        "visibility": "visible",
                    }
                ],
                "runtrolPanel": [
                    {
                        "id": "runtrol.conversationPanel",
                        "name": "Chat",
                        "type": "webview",
                        "icon": "resources/provider-icons/sparkle.svg",
                    }
                ],
                "runtrolSide": [
                    {
                        "id": "runtrol.conversationSide",
                        "name": "Chat",
                        "type": "webview",
                        "icon": "resources/provider-icons/sparkle.svg",
                    }
                ],
            },
            "viewsWelcome": [
                {
                    "view": "runtrol.sessions",
                    "contents": "Look again (command:runtrol.refresh)",
                    "when": "!runtrol.hasUsableProvider && !runtrol.isVerifyingProvider",
                },
                {
                    "view": "runtrol.sessions",
                    "contents": "Checking",
                    "when": "runtrol.isVerifyingProvider",
                },
                {
                    "view": "runtrol.sessions",
                    "contents": "New Conversation (command:runtrol.startSession)",
                    "when": "runtrol.hasUsableProvider",
                },
            ],
            "commands": [
                {"command": "runtrol.scheduleMission"},
                {"command": "runtrol.cancelMissionSchedule"},
                {"command": "runtrol.discoverServices"},
                {
                    "command": "runtrol.deleteConversation",
                    "title": "Delete Conversation",
                    "icon": "$(close)",
                },
            ],
            "menus": {
                "view/title": [
                    {
                        "command": "runtrol.startSession",
                        "when": "view == runtrol.sessions",
                        "group": "navigation@0",
                    },
                    {
                        "command": "runtrol.createProject",
                        "when": "view == runtrol.sessions",
                        "group": "navigation@1",
                    },
                    {
                        "command": "runtrol.switchSession",
                        "when": "view == runtrol.sessions",
                        "group": "navigation@2",
                    },
                ],
                "view/item/context": [
                    {
                        "command": "runtrol.reviewMissionLanding",
                        "when": "view == runtrol.missions && viewItem == runtrol.mission.integrating.chooseOne",
                    },
                    {
                        "command": "runtrol.reviewMissionLanding",
                        "when": (
                            "view == runtrol.missions && viewItem =~ "
                            "/^runtrol\\.missionTask\\.passed(\\.session)?\\.chooseOne\\.integrating$/"
                        ),
                    },
                    {
                        "command": "runtrol.recoverInterruptedMission",
                        "when": (
                            "view == runtrol.missions && viewItem =~ "
                            "/^runtrol\\.mission\\.blocked(\\.chooseOne)?(\\.autoFlight)?$/"
                        ),
                    },
                    {
                        "command": "runtrol.scheduleMission",
                        "when": "view == runtrol.missions && viewItem =~ /^runtrol\\.mission\\.validated/",
                    },
                    {
                        "command": "runtrol.cancelMissionSchedule",
                        "when": "view == runtrol.missions && viewItem =~ /schedulePending/",
                    },
                    {
                        "command": "runtrol.deleteConversation",
                        "when": "view == runtrol.sessions && viewItem =~ /\\.delete/",
                        "group": "inline@1",
                    },
                ]
            },
        },
    }
    sources = {
        "core/framing.ts": (
            "MAX_FRAME_BYTES MAX_QUEUED_FRAMES MAX_QUEUED_BYTES setImmediate "
            "this.socket.end()"
        ),
        "webview/main.ts": (
            'MAX_VISIBLE_ITEMS MAX_VISIBLE_CHARACTERS MAX_BATCH '
            'setAttribute("aria-activedescendant" setAttribute("aria-expanded" '
            'removeAttribute("aria-activedescendant") (item ? prompt : chip)?.focus() '
            'paintDraftPrompt() "Choose a coding service above" `Message ${draft.service}`'
        ),
        "conversationView.ts": (
            "webviewReady createWebviewPanel focusSurface conversationTabIsActive "
            "onDidChangeTabs onDidChangeTabGroups MEASUREMENT_ATTEMPTS withinMeasurementStage "
            'waitForVisibleWebview retainContextWhenHidden: false aria-haspopup="listbox" '
            'aria-controls="commands" aria-expanded="false" '
            "surface.iconPath = this.panelIcon "
            "conversationIcon(this.extensionUri, this.iconOf(providerId))"
        ),
        "conversationIcon.ts": (
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", `${icon}.svg`) '
            'const icon = /^[a-z0-9-]{1,64}$/u.test(declared) ? declared : "sparkle" '
            'existsSync(candidate.fsPath) '
            'vscode.Uri.joinPath(extensionUri, "resources", "provider-icons", "sparkle.svg")'
        ),
        "extension.ts": (
            "afterReady selfApproveIntegration(client, pendingId, signature) "
            'initializationStage = "runtime:bootstrap" missionController.startAutoFlights() '
            'executeCommand("runtrol.usage.focus") '
            '"runtrol.scheduleMission" "runtrol.cancelMissionSchedule" "runtrol.discoverServices"'
        ),
        "providerHealth.ts": (
            "the installed executable has not completed a verified probe "
            "export function awaitsVerification export function isBroken"
        ),
        "conversationPanels.ts": (
            'import { SerializedWatch } from "./serializedWatch"; '
            "private readonly watch = new SerializedWatch(); this.watch.pause(); this.watch.dispose()"
        ),
        "conversationSurface.ts": "Conversations sidebar",
        "conversationList.ts": (
            'const folderRows = rows.filter "open", '
            'if (left.current !== right.current) return left.current ? -1 : 1; '
            'export function conversationDetail return ""; '
            '"Unnamed conversation" return "Cannot reopen"'
        ),
        "stateRows.ts": (
            'export function discoveryNotice names(partial, "partial for") '
            'names(unavailable, "unavailable for")'
        ),
        "trees.ts": (
            'conversation.serviceName this.description = undefined '
            'conversationIcon(extensionUri, conversation.serviceIcon) new vscode.ThemeIcon("sync~spin") '
            'capabilities?.nativeSessionDelete?.availability === "available" '
            'this.revealCurrentProject() '
            'this.state.incompleteDiscovery view.message = undefined '
            '"runtrol.hasUsableProvider" "runtrol.isVerifyingProvider" awaitsVerification'
        ),
        "usageView.ts": (
            "implements vscode.WebviewViewProvider private gauges: "
            '"Usage refresh failed: ${why}. Showing the last report." '
            "usageRowsEqual(this.rows, next) usageViewAction(message) isBroken(provider) "
            'this.installableCount this.postSnapshot() enableScripts: true '
            'img-src ${webview.cspSource} data-icon-base="${iconBase}" '
            '"resources", "provider-icons"'
        ),
        "usageViewWebview.ts": (
            'document.createElement("progress") bar.max = 100 bar.value = meter.percent '
            'bar.setAttribute("aria-label" vscode.postMessage({ type: "fix" '
            'vscode.postMessage({ type: "discover" usage.replaceChildren .textContent = '
            'return /^[a-z0-9-]{1,64}$/u.test(value) ? value : "sparkle" '
            'image.src = `${iconBase}/${iconName(declared)}.svg` '
            'const fallback = `${iconBase}/sparkle.svg`'
        ),
        "usageViewMessage.ts": (
            "export function usageViewAction record.providerId.length <= 256"
        ),
        "usageView.css": (
            "progress::-webkit-progress-value .usage-row.limit-reached "
            "var(--vscode-progressBar-background"
        ),
        "usageDisplay.ts": (
            'provider.installation.state !== "missing" detail: "Ready" '
            'detail: "Checking" detail: "Unavailable · Fix" '
            'detail: `Disconnected · ${usageDetail(gauge, nowMs)}` '
            "export function usageMeters Math.max(0, Math.min(100 "
            'export function installableProviders icon: providerIcon(providerId, providers) '
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
            'reconnect workspaceCollisions conversationSwitchDecision this.runtime.cool( '
            '"Stop and switch" "Keep both working" '
            '"Start here anyway"'
        ),
        "mission/autoFlight.ts": (
            "MAX_AUTO_FLIGHTS sessionGeneration recordAutoFlightSubmissions readAutoFlightArms "
            "pendingSignal stageSignal"
        ),
        "mission/controller.ts": (
            "AUTO_FLIGHTS_KEY runtimeState.onDidChange beforeSubmissions recordSubmissions startAutoFlights "
            "missionFlightSignal missionFlightSignalClear hasAutoFlightRecord "
            "Integrated winner Task Integrated winner Receipt recoverInterruptedMission "
            "assertInterruptedRecoveryAuthority reviewMissionSchedule assertMissionScheduleAuthority "
            'commitScheduleReview ask: "missionScheduleCancel" Core starts after Studio closes MissionWaveRunner '
            "for (const missionId of this.documents.openMissionIds()) "
            "this.documents.update(await this.get(missionId)) refreshRows"
        ),
        "mission/recovery.ts": (
            "interruptedRecoveryPlan assertInterruptedRecoveryAuthority missionSha256 policySha256 "
            "providerSelector baseCommit The previous provider input may already have caused external effects"
        ),
        "mission/schedule.ts": (
            "MIN_SCHEDULE_LEAD_MS MAX_SCHEDULE_LEAD_MS reviewMissionSchedule "
            "assertMissionScheduleAuthority missionSha256 snapshot.policy_sha256 task.instruction_sha256 "
            "task.provider_selector task.workspace_mode replacesScheduleId dueUnixMs providers"
        ),
        "mission/waveRunner.ts": (
            "hasAmbiguousSubmission markAmbiguousSubmission resolveInstruction submit clearAmbiguousSubmission"
        ),
        "mission/landing/apply.ts": (
            "applyLandingTransaction landingCompletionProblem readMissionLanding createLandingDirectories "
            "writeAtomicLandingFile readLandingTarget"
        ),
        "mission/landing/model.ts": (
            'type LandingSelection missionWinnerLanding selection.kind === "chooseOne" '
            "snapshot.tasks.filter selection: landing.selection landingCompletionProblem selected_receipt_id"
        ),
        "mission/landing/review.ts": (
            "MAX_DIFF_TEXT document.isDirty tab.isDirty Receipt Artifact evidence mismatch"
        ),
        "mission/landing/localFile.ts": (
            "inspectSafeLocalFile readExactLocalFile opened.dev !== file.device "
            "named.isSymbolicLink()"
        ),
        "mission/landing/atomicFile.ts": (
            'open(temporary, "wx+" handle.write handle.sync beforeReplace rename(temporary, target)'
        ),
        "mission/landing/controller.ts": (
            'private currentReview state: "reviewed" | "appliedAwaitingCore" withProjectLease '
            "completeLandingWithRecovery review.landing.selection.taskId"
        ),
        "mission/projectLease.ts": (
            "runtrol-project-integration-leases acquireProcessLease ACTIVE_PROCESS_LEASES "
            "attempt < 3 process.kill(pid, 0)"
        ),
        "core/client.ts": "commandConnection commandTail",
        "runtimeClient.ts": (
            "RuntimeLocator.system( RUNTIME_LOCATOR_SETTLE_MS isAbsolute(runtimeExecutable) withRuntimeLocator "
            "providerSnapshot sessionSnapshot async cool( watchSessions "
            "watchSessionIndexWithReconnect"
        ),
        "core/locator.ts": '["endpoint"] executable: "runtrol" runtimeExecutable',
        "core/managedCore.ts": (
            "createReadStream copyFile(source, incoming) link(executable, preserved) "
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
            'controller.startResolvedSession(provider, workspace, model, reasoningEffort, "exclusive", false, permission) '
            "missions.scheduleMissionForJourney(missionId, dueUnixMs, operatorChoiceProvider)"
        ),
        "protocol.ts": (
            'MissionScheduleLine ask: "missionSchedule" replaces_schedule_id ask: "missionScheduleCancel"'
        ),
    }
    if sourceViolations(package, sources):
        print("[vscodeExtension --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2

    hidden_usage = json.loads(json.dumps(package))
    hidden_usage["contributes"]["views"]["runtrol"][0]["visibility"] = "collapsed"
    non_progress_usage = json.loads(json.dumps(package))
    non_progress_usage["contributes"]["views"]["runtrol"][0].pop("type")
    branded_chat = json.loads(json.dumps(package))
    branded_chat["contributes"]["views"]["runtrolPanel"][0]["name"] = "Runtrol Chat"
    branded_chat_container = json.loads(json.dumps(package))
    branded_chat_container["contributes"]["viewsContainers"]["panel"][0]["title"] = "Runtrol"
    cluttered_toolbar = json.loads(json.dumps(package))
    cluttered_toolbar["contributes"]["menus"]["view/title"].append({
        "command": "runtrol.arrangeConversationGrid",
        "when": "view == runtrol.sessions",
        "group": "navigation@3",
    })
    merged_welcomes = json.loads(json.dumps(package))
    merged_welcomes["contributes"]["viewsWelcome"] = [
        {
            "view": "runtrol.sessions",
            "contents": "No coding-agent CLI was found. (command:runtrol.refresh)",
        }
    ]
    broad_delete = json.loads(json.dumps(package))
    for entry in broad_delete["contributes"]["menus"]["view/item/context"]:
        if entry.get("command") == "runtrol.deleteConversation":
            entry["when"] = "view == runtrol.sessions"
    cluttered_conversation = json.loads(json.dumps(package))
    cluttered_conversation["contributes"]["menus"]["view/item/context"].append({
        "command": "runtrol.closeSession",
        "when": "view == runtrol.sessions && viewItem =~ /^runtrol\\.conversation\\.live/",
        "group": "inline@2",
    })
    mutations = [
        ({**package, "dependencies": {"some-runtime": "1"}}, sources),
        (hidden_usage, sources),
        (non_progress_usage, sources),
        (branded_chat, sources),
        (branded_chat_container, sources),
        (cluttered_toolbar, sources),
        (merged_welcomes, sources),
        (broad_delete, sources),
        (cluttered_conversation, sources),
        ({**package, "activationEvents": []}, sources),
        ({**package, "contributes": {"viewsContainers": {"activitybar": []}}}, sources),
        ({"engines": {"vscode": "^1.100.0"}, "contributes": {"viewsContainers": {"activitybar": [], "secondarySidebar": []}}}, sources),
        (package, {**sources, "webview/main.ts": "localStorage MAX_VISIBLE_ITEMS"}),
        (package, {**sources, "webview/main.ts": sources["webview/main.ts"] + ' mark.textContent = "R"'}),
        (package, {**sources, "conversationSurface.ts": sources["conversationSurface.ts"] + " Runtrol sidebar"}),
        (package, {**sources, "controller.ts": "setInterval("}),
        (package, {**sources, "mission/controller.ts": sources["mission/controller.ts"] + " setTimeout("}),
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
        (package, {**sources, "conversationView.ts": "webviewReady"}),
        (package, {**sources, "conversationView.ts": sources["conversationView.ts"] + " resources/symbol.svg"}),
        (package, {**sources, "conversationIcon.ts": sources["conversationIcon.ts"].replace("provider-icons", "brand")}),
        (package, {**sources, "conversationList.ts": sources["conversationList.ts"] + " `Chat ${identity}`"}),
        (
            package,
            {
                **sources,
                "webview/main.ts": sources["webview/main.ts"].replace(
                    'setAttribute("aria-activedescendant"', ""
                ),
            },
        ),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace('document.createElement("progress")', "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace("image.src", "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace("const fallback", "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace("bar.max = 100", "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace("bar.value = meter.percent", "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"].replace('bar.setAttribute("aria-label"', "")}),
        (package, {**sources, "usageViewWebview.ts": sources["usageViewWebview.ts"] + " providerMark"}),
        (package, {**sources, "usageView.ts": sources["usageView.ts"].replace("img-src", "")}),
        (package, {**sources, "usageView.ts": sources["usageView.ts"].replace("data-icon-base", "")}),
        (package, {**sources, "usageView.ts": sources["usageView.ts"].replace("Usage refresh failed", "")}),
        (package, {**sources, "usageView.ts": sources["usageView.ts"].replace("usageRowsEqual(this.rows, next)", "false")}),
        (package, {**sources, "stateRows.ts": sources["stateRows.ts"].replace("discoveryNotice", "")}),
        (package, {**sources, "usageViewMessage.ts": sources["usageViewMessage.ts"].replace("record.providerId.length <= 256", "true")}),
        (package, {**sources, "trees.ts": sources["trees.ts"].replace("conversationIcon(extensionUri, conversation.serviceIcon)", "")}),
        (package, {**sources, "trees.ts": sources["trees.ts"].replace('new vscode.ThemeIcon("sync~spin")', "")}),
        (package, {**sources, "trees.ts": sources["trees.ts"] + " view.description"}),
        (package, {**sources, "trees.ts": sources["trees.ts"] + " conversationDetail"}),
        (package, {**sources, "trees.ts": sources["trees.ts"].replace("this.description = undefined", "")}),
        (
            package,
            {
                **sources,
                "trees.ts": sources["trees.ts"].replace(
                    'capabilities?.nativeSessionDelete?.availability === "available"', ""
                ),
            },
        ),
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
        (
            package,
            {
                **sources,
                "mission/landing/apply.ts": sources["mission/landing/apply.ts"].replace(
                    "applyLandingTransaction", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/controller.ts": sources["mission/controller.ts"].replace(
                    "this.documents.update(await this.get(missionId))", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/recovery.ts": sources["mission/recovery.ts"].replace(
                    "assertInterruptedRecoveryAuthority", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/schedule.ts": sources["mission/schedule.ts"].replace(
                    "task.instruction_sha256", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "protocol.ts": sources["protocol.ts"].replace("replaces_schedule_id", ""),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/waveRunner.ts": sources["mission/waveRunner.ts"].replace(
                    "markAmbiguousSubmission", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/landing/model.ts": sources["mission/landing/model.ts"].replace(
                    "missionWinnerLanding", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/landing/localFile.ts": sources["mission/landing/localFile.ts"].replace(
                    "named.isSymbolicLink()", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/landing/atomicFile.ts": sources["mission/landing/atomicFile.ts"].replace(
                    "beforeReplace", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/landing/controller.ts": sources["mission/landing/controller.ts"].replace(
                    "completeLandingWithRecovery", ""
                ),
            },
        ),
        (
            package,
            {
                **sources,
                "mission/projectLease.ts": sources["mission/projectLease.ts"].replace(
                    "acquireProcessLease", ""
                ),
            },
        ),
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
                "mission/autoFlight.ts": sources["mission/autoFlight.ts"].replace("sessionGeneration", ""),
            },
        ),
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
    failures = sourceViolations(package, sources)
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

    # The ceiling is a bloat tripwire, not a target. Raised 256 -> 272 KiB on 2026-08-19 when the minified
    # extension bundle crossed it with deliberate features (operator-created projects, the usage strip, and the
    # mid-conversation model switch), each reviewed at the crossing. Raised 272 -> 288 KiB on 2026-08-20 when
    # the nativeParity sweep crossed it with deliberate features again (the service remedy surface, the effort
    # chip and requested-suffix, message queueing, @file mentions, per-project start defaults, and the fan-out
    # gate picker), each reviewed at the crossing. Raised 288 -> 304 KiB on 2026-08-20 when the GUI identity
    # build crossed it with the draft conversation tab (project, service, model, effort and mode chips as
    # pickers), image attachments through sessions/submitBlocks, and the branch chip read off the folder's own
    # repository, each reviewed at the crossing. Raised 304 -> 320 KiB on 2026-08-21 when the places build
    # crossed it: a conversation surface contract (tab, bottom panel, secondary side bar) with two workbench
    # view providers, the one-command editor grid, and the per-place memory, each reviewed at the crossing.
    # Raised 320 -> 336 KiB on 2026-08-21 (evening) when the "know without opening, answer without opening" build
    # crossed it: the activity watch and its row word, sign-in and approval from the row, the back key and the
    # keyboard project switch, one prompt to N services, each reviewed at the crossing.
    # Raised 336 -> 352 KiB on 2026-08-22 when Fleet Compare crossed it: reviewed choose-one Missions, parallel
    # isolated launch, the conversation grid, native result diffs, and selected-Receipt completion, reviewed in
    # the real Extension Host at the crossing.
    # Raised 352 -> 368 KiB on 2026-08-22 when Mission Auto Flight crossed it at 374114 bytes: bounded local
    # authority, event-driven DAG waves, durable lifecycle-generation proof, immediate disarm, and explicit
    # Receipt Landing, reviewed in the real Extension Host at the crossing.
    # Raised 368 -> 384 KiB on 2026-08-22 when Receipt Landing apply crossed it at 382764 bytes: exact reviewed
    # Artifact writes, pre-apply drift and symlink defenses, bounded rollback, and one-action Gate completion.
    # Raised 384 -> 400 KiB in the same review when the proof found and closed missing Receipt digests, unbounded
    # pre-reads, non-atomic replacement, cross-window writer races, dirty non-text tabs, and Gate mutation races.
    # A dependency slipping in still trips it.
    bundles = [
        EXTENSION / "dist" / name
        for name in (
            "extension.js",
            "pairingQrVendor.js",
            "webview.js",
            "webview.css",
            "usageView.js",
            "usageView.css",
        )
    ]
    for bundle in bundles:
        if not bundle.is_file() or bundle.stat().st_size > 400 * 1024:
            failures.append(f"{bundle.relative_to(ROOT)} is missing or exceeds 400 KiB")
    qr_bundle = EXTENSION / "dist" / "pairingQrVendor.js"
    if qr_bundle.is_file() and qr_bundle.stat().st_size > 32 * 1024:
        failures.append(f"{qr_bundle.relative_to(ROOT)} exceeds its pairing-only 32 KiB budget")
    for name in ("usageView.js", "usageView.css"):
        usage_bundle = EXTENSION / "dist" / name
        if usage_bundle.is_file() and usage_bundle.stat().st_size > 16 * 1024:
            failures.append(f"{usage_bundle.relative_to(ROOT)} exceeds its fixed usage-view 16 KiB budget")
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
