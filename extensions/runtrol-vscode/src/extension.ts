import path from "node:path";

import * as vscode from "vscode";

import { AgentToolsController, type AgentToolsAction } from "./agentTools";
import { conversations as conversationRows, namedPlaceholders } from "./conversationList";
import { ActivityWatcher } from "./activityWatch";
import { WatchLifecycleGate } from "./watchLifecycleGate";
import { DiffDocuments } from "./diffDocuments";
import { Controller } from "./controller";
import { CoreClient } from "./core/client";
import { CoreLocator } from "./core/locator";
import { superviseCoreCurrency } from "./coreCurrencySurface";
import { readGitBranch } from "./gitBranch";
import { GitChangesWatch } from "./gitChanges";
import { ProviderUpdateWatch } from "./providerUpdateWatch";
import { ProviderHelpCache } from "./providerHelpCache";
import { followGitExtension } from "./gitExtensionFollow";
import {
  confirmRuntimeForget,
  confirmRuntimeSharedOpen,
  manageIntegrations,
  reviewIntegrationEnrollments,
  reviewRuntimeRequests,
  selfApproveIntegration,
} from "./integrationAdministration";
import { journeyApi, type JourneyApi } from "./journeyApi";
import { projectlessRoot } from "./projectlessWorkspace";
import { ProjectStore } from "./projects";
import { isBroken, isUsable } from "./providerHealth";
import { materializeProviderShims } from "./providerShims";
import { managePhones, pairPhone, reviewPhonePairings } from "./pairingAdministration";
import type { RemoteConnection } from "./protocol";
import { SelectionStore } from "./selectionStore";
import { ServiceTroubleReported } from "./serviceHelp";
import { providerDisplayName, providerIcon, sessionTitle, workspaceName } from "./sessionDisplay";
import { RuntimeState } from "./state";
import { StudioRuntimeClient } from "./runtimeClient";
import { workspaceCovers, workspaceIdentity } from "./workspaceCollision";
import type { Conversation } from "./conversationList";
import { rememberedList, rememberList } from "./listMemory";
import { rememberedUsage, rememberUsage } from "./usageMemory";
import { setupRows } from "./usageDisplay";
import { WorkspaceRootFollowing } from "./workspaceRoots";
import { conversationIcon } from "./conversationIcon";
import { TerminalTabs } from "./terminalTabs";
import { ConversationItem, ProjectItem, ServiceChoiceItem, icon } from "./sidebarTargets";
import { SIDEBAR_VIEW_ID, SidebarView } from "./sidebarView";
import { showMoreActions } from "./moreActions";

declare const RUNTROL_INCLUDE_TEST_JOURNEY: boolean;

export type RuntrolExtensionApi = {
  readonly ready: Promise<void>;
  readonly initializationStage?: string;
  refresh(): Promise<void>;
  measureSessionManagement?(
    sessionIds: readonly string[],
    progress?: (stage: string) => void,
  ): Promise<SessionManagementPerformance>;
  verifyRestoredSession?(sessionId: string): Promise<void>;
  hasConversationIn?(folder: string): Promise<boolean>;
  waitForConversationIn?(folder: string, deadlineMs: number): Promise<number>;
  seedProject?(folder: string): Promise<void>;
  openFirstConversation?(): Promise<void>;
  openCrossProjectConversation?(): Promise<void>;
  readonly journey?: JourneyApi;
};

export type SessionManagementPerformance = {
  sessionCount: number;
  hotSessionCount: number;
  coldResumeMs: number;
  sessionSwitchP95Ms: number;
  resumedFrom: string;
  resumedTo: string;
  restoreSession: string;
  restoreWorkspace: string;
};

// Eight hot sessions over five rounds keep nearest-rank p95 from collapsing to the single maximum sample.
const SESSION_SWITCH_ROUNDS = 5;

/// Whether the performance-only measurement surface is on, asked once. One name for one flag.
const MEASURED_HOST = process.env.RUNTROL_VSCODE_PERFORMANCE === "1";

/// How long after reaching a Core the release inspection waits, so discovery goes first.
const RELEASE_CHECK_AFTER_REACH_MS = 15_000;

export function activate(context: vscode.ExtensionContext): RuntrolExtensionApi {
  // Declared below; the private locator only asks it after activation has built it.
  let runtime: StudioRuntimeClient;
  const locator = new CoreLocator(
    context,
    () => runtime.warmLocator().then((listed) => listed?.controlEndpoint ?? null),
  );
  const client = new CoreClient(locator);
  const providerShimDirectory = vscode.Uri.joinPath(
    context.globalStorageUri,
    "provider-shims",
  ).fsPath;
  // Every integrated terminal opened after activation resolves manifest-declared provider commands through the
  // transparent bridge. The wrapper removes this leading directory before starting Core, so Core resolves the real
  // provider executable and never recurses through its own shim.
  context.environmentVariableCollection.prepend("PATH", `${providerShimDirectory}${path.delimiter}`);
  context.environmentVariableCollection.replace("RUNTROL_PROVIDER_SHIM_PATH", providerShimDirectory);
  const agentTools = new AgentToolsController(() => locator.runtimeExecutable());
  let initializationStage = "runtime:bootstrap";
  runtime = new StudioRuntimeClient(
    context,
    async () => {
      const located = await locator.locate();
      return { runtimeExecutable: located.executable, preferDigest: located.managedDigest };
    },
    async () => {
      const expected = await locator.firstCandidate();
      return { runtimeExecutable: expected.executable, preferDigest: expected.managedDigest };
    },
    (pendingId, signature) => selfApproveIntegration(client, pendingId, signature),
    (confirmationId, sessionId) => confirmRuntimeForget(client, confirmationId, sessionId),
    (confirmationId, workspace) => confirmRuntimeSharedOpen(client, confirmationId, workspace),
    testIntegrationRoots(context),
    (stage) => {
      initializationStage = `runtime:${stage}`;
    },
  );
  // Conversations started with no project run in the extension's own scratch folder; the state knows it so
  // every derived row agrees on which conversations are projectless.
  const state = new RuntimeState(projectlessRoot(context.globalStorageUri.fsPath));
  // Before anything is located, connected to or asked: the list this window drew last time. Reading it is a
  // synchronous memento lookup, so the panel has rows in its first paint rather than a sentence about
  // connecting. A person who can see the wait is the failure.
  state.restoreRemembered(rememberedList(context.globalState));
  state.onRemember((catalogues) => rememberList(context.globalState, catalogues));
  // The strip the last window left, so bars are there before anything has been asked. Everything that has
  // stopped being true is taken out of it first; see `usageMemory`.
  state.restoreRememberedUsage(rememberedUsage(context.globalState, Date.now()));
  state.onRememberUsage((usage) => rememberUsage(context.globalState, usage));
  const selection = new SelectionStore(context.globalStorageUri.fsPath);
  let settleReady: ((error?: unknown) => void) | null = null;
  let lifecycle: Promise<void> = new Promise<void>((resolve, reject) => {
    settleReady = (error) => {
      settleReady = null;
      if (error === undefined) {
        resolve();
      } else {
        reject(error);
      }
    };
  });
  const afterReady = async <T>(action: () => Promise<T>): Promise<T> => {
    await lifecycle;
    return action();
  };
  let controller: Controller;
  // The operator's own projects. Global state, because the panel manages the whole machine from any window.
  // Built before the controller because a draft's project picker offers them first.
  const projectStore = new ProjectStore(context.globalState);
  // Another window may have added, renamed or removed a project while this one sat unfocused. The list is the
  // machine's, so the moment this window is looked at is the moment it has to be current.
  context.subscriptions.push(vscode.window.onDidChangeWindowState((window) => {
    if (window.focused) projectStore.reload();
  }));
  const watchLifecycle = new WatchLifecycleGate();
  // The sidebar's "what is it doing" word for every running conversation, page open or not.
  context.subscriptions.push(new ActivityWatcher(runtime, state, watchLifecycle));
  const diffDocuments = new DiffDocuments();
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(DiffDocuments.scheme, diffDocuments),
  );
  // What each project holds uncommitted and unpushed. Measured when an agent in it writes (it may have edited
  // files) and when the editor's git extension sees a change in a folder this window has open; never polled.
  const changes = new GitChangesWatch();
  context.subscriptions.push(changes, followGitExtension((root) => changes.touchUnder(root)));
  // The conversation surface: the service's own terminal interface in an editor tab, hosted by the Core.
  const terminals = new TerminalTabs(
    runtime,
    (row) => icon(row, context.extensionUri),
    (providerId) => conversationIcon(context.extensionUri, providerIcon(providerId, state.providers)),
    () => state.setStarted(terminals.startedConversations()),
    async (key) => {
      await controller.refreshChats();
      return state.conversations.find((candidate) => candidate.key === key) ?? null;
    },
    (key) => {
      state.markStreaming(key);
      const home = state.conversations.find((row) => row.key === key)?.homeWorkspace;
      // The project the conversation is filed under, not the folder it runs in: a conversation in a
      // subfolder is a row of the project above it, and that is the chip to move.
      if (home) changes.touchContaining(home);
    },
    (key) => state.conversations.find((row) => row.key === key)?.title ?? null,
  );
  context.subscriptions.push(terminals);
  // A tab started from here is filed under a placeholder until its service writes the conversation. The list
  // rebuild that drops the placeholder is the moment the tab can move onto the real one, and it is the same
  // event that repaints the sidebar, so no row can be clicked before its tab has moved.
  context.subscriptions.push(state.onDidChange((change) => {
    if (change !== "rows") return;
    terminals.retire(namedPlaceholders(state.conversations, terminals.startedConversations()));
    terminals.reconcileHosted(state.conversations);
  }));
  controller = new Controller(context, client, runtime, state, selection, projectStore, terminals);
  const offerServices = (): readonly { providerId: string; displayName: string; icon: string }[] =>
    controller.startableServices().map((provider) => ({
      providerId: provider.providerId,
      displayName: provider.displayName,
      icon: providerIcon(provider.providerId, state.providers),
    }));
  // The window's folders follow into the grant's roots. Enrollment read them once; without this, every folder
  // opened after first activation stayed outside conversation discovery, silently.
  const rootFollowing = new WorkspaceRootFollowing({
    client,
    integrationId: () => runtime.integrationId(),
    refreshRoots: () => controller.refreshAfterRootWidened(),
    // The window's folders and every added project: a conversation listed under a project heading must also
    // open, and opening is root-bounded, so adding a project is also asking for its folder (rootDenied
    // otherwise, measured 2026-08-27 on a window that had codaro as a heading but not as a folder).
    openFolders: () => [
      ...(vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath),
      ...projectStore.all().map((record) => record.workspace),
    ],
    warn: (message) => void vscode.window.showWarningMessage(message),
  });
  // One page owns the whole sidebar: projects with their conversations, the conversations outside every project,
  // and one usage chip per installed service, in that order and with visible edges (`docs/vscodeSurface.md`).
  const providerNamed = (providerId: string) => {
    const provider = state.providers.find((candidate) => candidate.providerId === providerId);
    if (!provider) throw new Error(`${providerId} is not an installed service`);
    return provider;
  };
  // Whether a newer release of each service exists, asked of the Core whenever this window reaches one (a
  // fresh activation, or a reconnect after an update or restart). No clock: the sidebar puts the installed
  // version beside the service and an Update button when the Core confirmed a rollback-safe release.
  let releasesAsked = false;
  // A command connection is serial, and the shared one carries every ask that a person is waiting on (the
  // list, closing, answering). The slow asks the sidebar makes on its own (the release inspection's registry
  // calls, a provider install, the help lines) run on a connection of their own so nothing waits behind them.
  const sideChannel = new CoreClient(locator);
  const releases = new ProviderUpdateWatch(() => controller.inspectProviderUpdates(sideChannel));
  // Each service's private help line (its sign-out command), asked once per set of usable services.
  const help = new ProviderHelpCache((providerId) => controller.providerHelpLine(providerId, sideChannel));
  context.subscriptions.push(releases, help, sideChannel);
  const sidebar = new SidebarView(context, state, projectStore, agentTools, changes, releases, help, {
    signIn: (providerId) => afterReady(async () => {
      await controller.signInProvider(providerNamed(providerId));
    }),
    signOut: (providerId) => afterReady(async () => {
      await controller.signOutProvider(providerNamed(providerId));
    }),
    fix: (providerId) => afterReady(async () => {
      await controller.fixService(providerNamed(providerId));
    }),
    update: (providerId) => afterReady(async () => {
      const line = releases.get(providerId);
      if (!line) throw new Error(`${providerId} has no update inspection to act on`);
      await controller.updateProvider(line, providerNamed(providerId).displayName, sideChannel);
      await releases.check(true);
    }),
  }, (error) => void vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error)));
  context.subscriptions.push(state.onDidChange((change) => {
    if (change !== "rows") return;
    if (state.coreReach !== "reached") {
      releasesAsked = false;
      return;
    }
    void help.refresh(state.providers.filter(isUsable).map((provider) => provider.providerId));
    // Each time the Core is reached anew (activation, or a reconnect after an update), ask about releases
    // once, a moment later: the inspection holds each service's discovery lane while it asks the package
    // registry, and the conversation listing that reach starts must not queue behind a network call.
    // Between reaches nothing polls; the manual check command and a finished update ask on their own.
    if (!releasesAsked) {
      releasesAsked = true;
      setTimeout(() => void releases.check(), RELEASE_CHECK_AFTER_REACH_MS);
    }
  }));

  context.subscriptions.push(
    state,
    controller,
    agentTools,
    sidebar,
    vscode.window.registerWebviewViewProvider(SIDEBAR_VIEW_ID, sidebar, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
    vscode.commands.registerCommand(
      "runtrol.refresh",
      () => run(() => afterReady(() => controller.refreshChats())),
    ),
    vscode.commands.registerCommand(
      "runtrol.startSessionWith",
      (item: ServiceChoiceItem) => run(() => afterReady(async () => {
        sidebar.clearServiceChoice();
        await controller.startSessionWith(item.providerId, item.workspace);
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.restartExtensionHost",
      () => run(restartExtensionHost),
    ),
    vscode.commands.registerCommand(
      "runtrol.checkProviderUpdates",
      () => run(() => afterReady(() => controller.checkProviderUpdates())),
    ),
    vscode.commands.registerCommand(
      "runtrol.remoteConnectionStatus",
      () => run(() => afterReady(async () => {
        const connection = await remoteConnection(client);
        await vscode.window.showInformationMessage(remoteConnectionMessage(connection));
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.pairPhone",
      () => run(() => afterReady(() => pairPhone(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewPhonePairings",
      () => run(() => afterReady(() => reviewPhonePairings(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.managePhones",
      () => run(() => afterReady(() => managePhones(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewIntegrations",
      () => run(() => afterReady(() => reviewIntegrationEnrollments(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.manageIntegrations",
      () => run(() => afterReady(async () => {
        if (await manageIntegrations(client)) await controller.reconnect();
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.reviewRuntimeRequests",
      () => run(() => afterReady(() => reviewRuntimeRequests(client))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openNextWaiting",
      () => run(() => afterReady(() => controller.openNextWaiting())),
    ),
    vscode.commands.registerCommand(
      "runtrol.switchSession",
      () => run(() => afterReady(() => controller.switchSession())),
    ),
    vscode.commands.registerCommand(
      "runtrol.startSession",
      (options?: unknown) => run(() => afterReady(() => controller.startSession(
        options !== null && typeof options === "object" && (options as { interactive?: unknown }).interactive === false
          ? { interactive: false }
          : {},
      ))),
    ),
    vscode.commands.registerCommand(
      "runtrol.newConversationInProject",
      (item: unknown) => run(() => afterReady(async () => {
        // Inline on the project heading only, so the argument is always the heading. Guarded anyway, because a
        // command invoked with the wrong thing must refuse rather than start a session somewhere surprising.
        if (!(item instanceof ProjectItem)) return;
        await controller.startSessionInWorkspace(item.group.workspace);
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.createProject",
      // No name prompt: the folder's own name is the default and rename is one right-click away. Several
      // folders can be picked at once, and each becomes its own project.
      () => run(async () => {
        const chosen = await vscode.window.showOpenDialog({
          canSelectFiles: false,
          canSelectFolders: true,
          canSelectMany: true,
          openLabel: "Add Project",
          title: "Choose the folder each new project stands on",
          defaultUri: vscode.workspace.workspaceFolders?.[0]?.uri,
        });
        if (!chosen) return;
        for (const folder of chosen) {
          await projectStore.create(folder.fsPath);
        }
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.createProjectHere",
      // The open folder added as a project, in one click from its heading. The folder is already answered by
      // the heading, so no dialog; rename is one right-click away afterwards.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.create(item.group.workspace);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.moveProjectUp",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.move(item.group.workspace, -1);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.moveProjectDown",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.move(item.group.workspace, 1);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.pinProject",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.setPinned(item.group.workspace, true);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.unpinProject",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await projectStore.setPinned(item.group.workspace, false);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.enableAgentTools",
      (item?: unknown) => run(() => afterReady(
        () => changeAgentTools(agentTools, "enable", item instanceof ProjectItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.disableAgentTools",
      (item?: unknown) => run(() => afterReady(
        () => changeAgentTools(agentTools, "disable", item instanceof ProjectItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.renameProject",
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        const name = await vscode.window.showInputBox({
          prompt: "Project name",
          value: item.group.name,
        });
        if (name === undefined) return;
        await projectStore.setName(item.group.workspace, name);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.removeProject",
      // Removal only takes the heading away: conversations stay, the folder on disk stays, and adding the
      // project again is one click. That reversibility is why there is no confirmation dialog in the way; the
      // toast's Undo covers the misclick without making everyone else answer a question first.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        const { workspace, name } = item.group;
        await projectStore.remove(workspace);
        // Not awaited as a gate: the removal is done, and the toast lives on its own time.
        void vscode.window.showInformationMessage(`Removed the project ${name}.`, "Undo").then((choice) => {
          if (choice === "Undo") {
            return run(() => projectStore.create(workspace, name).then(() => undefined));
          }
          return undefined;
        });
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.fixService",
      // From the Command Palette. The fixed usage surface routes its already visible service directly.
      () => run(async () => {
        await afterReady(async () => {
          const broken = state.providers.filter(isBroken);
          if (broken.length === 0) {
            void vscode.window.showInformationMessage("All installed coding services are available.");
            return;
          }
          let provider = broken.at(0) ?? null;
          if (broken.length > 1) {
            const picked = await vscode.window.showQuickPick(
              broken.map((candidate) => ({
                label: candidate.displayName,
                description: "Unavailable",
                detail: candidate.installation.why ?? undefined,
                provider: candidate,
              })),
              {
                title: "Fix coding service",
                placeHolder: "Choose the service that needs attention",
              },
            );
            provider = picked?.provider ?? null;
          }
          if (provider) await controller.fixService(provider);
        });
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.setUpServices",
      () => run(() => afterReady(async () => {
        const picked = await vscode.window.showQuickPick(
          setupRows(state.providers).map((row) => ({
            label: row.name,
            description: row.state === "ready" ? "Ready" : row.detail,
            detail: row.state === "ready" ? row.detail : undefined,
            providerId: row.providerId,
            actionable: row.actionable,
          })),
          {
            title: "Coding services",
            placeHolder: "Choose a service that needs setup",
          },
        );
        if (!picked?.actionable) return;
        const provider = state.providers.find((candidate) => candidate.providerId === picked.providerId);
        if (!provider) return;
        if (provider.account?.status === "signedOut") {
          await controller.signInProvider(provider);
        } else if (isBroken(provider)) {
          await controller.fixService(provider);
        } else {
          await controller.setUpService(provider);
        }
      })),
    ),
    vscode.commands.registerCommand(
      "runtrol.selectSession",
      (item) => run(() => afterReady(() => controller.select(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openConversation",
      // The sidebar page names the row it was asked to open; selection is what opens a row's terminal, so a
      // named row selects (and therefore opens) even in a window that has never selected anything. Without the
      // argument (the keybinding, the palette) the command still means "the selected conversation's tab".
      (item) => run(() => afterReady(() =>
        item instanceof ConversationItem ? controller.select(item) : controller.openConversation())),
    ),
    vscode.commands.registerCommand(
      "runtrol.renameSession",
      (item) => run(() => afterReady(() => controller.nameSession(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openWorkspace",
      (item) => run(() => afterReady(() => controller.openWorkspace(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.openProjectWorkspace",
      (item) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        // The explicit move the contract requires: only this button changes what the window is open on.
        await controller.switchWindowTo(item.group.workspace);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.returnToPreviousProject",
      () => run(() => controller.returnToPreviousProject()),
    ),
    vscode.commands.registerCommand(
      "runtrol.switchProject",
      () => run(() => afterReady(() => controller.switchProject())),
    ),
    vscode.commands.registerCommand(
      "runtrol.signInFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.signInFromRow(item instanceof ConversationItem ? item : undefined),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.allowFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "allow"),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.declineFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "decline"),
      )),
    ),
    vscode.commands.registerCommand(
      "runtrol.answerFromRow",
      (item?: unknown) => run(() => afterReady(
        () => controller.answerFromRow(item instanceof ConversationItem ? item : undefined, "choose"),
      )),
    ),
    vscode.commands.registerCommand("runtrol.interrupt", () => run(() => afterReady(() => controller.interrupt()))),
    vscode.commands.registerCommand(
      "runtrol.closeSession",
      (item) => run(() => afterReady(() => controller.close(item))),
    ),
    vscode.commands.registerCommand(
      "runtrol.deleteProjectConversations",
      // Destructive, so guarded: invoked with anything but a project it refuses rather than guessing one.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        await afterReady(() => controller.deleteProjectConversations(item));
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.projectMenu",
      // The project row's fuller menu, from a right click on the row: the hover actions plus the one action
      // too destructive to sit among them. The menu names the project so a misread row is caught before
      // anything runs.
      (item: unknown) => run(async () => {
        if (!(item instanceof ProjectItem)) return;
        const created = item.group.kind === "created";
        type Entry = vscode.QuickPickItem & { command: string };
        const entries: Entry[] = [
          { label: "$(add) New conversation here", command: "runtrol.newConversationInProject" },
          ...(created
            ? [
                { label: "$(edit) Rename project", command: "runtrol.renameProject" },
                {
                  label: item.agentToolsEnabled ? "$(sparkle-filled) Turn Agent Tools off" : "$(sparkle) Turn Agent Tools on",
                  command: item.agentToolsEnabled ? "runtrol.disableAgentTools" : "runtrol.enableAgentTools",
                },
                {
                  label: item.group.pinned ? "$(pinned) Unpin" : "$(pin) Pin to the top",
                  command: item.group.pinned ? "runtrol.unpinProject" : "runtrol.pinProject",
                },
              ]
            : [{ label: "$(folder-library) Keep this folder as a project", command: "runtrol.createProjectHere" }]),
          { label: "$(link-external) Open this folder in a window", command: "runtrol.openProjectWorkspace" },
          { label: "$(trash) Delete all conversations...", command: "runtrol.deleteProjectConversations" },
          ...(created
            ? [{ label: "$(close) Remove from the sidebar (the folder stays)", command: "runtrol.removeProject" }]
            : []),
        ];
        const picked = await vscode.window.showQuickPick(entries, {
          title: item.group.name,
          placeHolder: "Project actions",
        });
        if (picked) await vscode.commands.executeCommand(picked.command, item);
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.moreActions",
      () => run(() => showMoreActions(sidebar.listingReasons())),
    ),
    vscode.commands.registerCommand(
      "runtrol.explainListing",
      () => {
        const reasons = sidebar.listingReasons();
        void vscode.window.showInformationMessage(
          reasons
            ? `Not every chat is listed. ${reasons}`
            : "Every conversation the installed coding services list is shown.",
        );
      },
    ),
    vscode.commands.registerCommand(
      "runtrol.archiveConversation",
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.archiveConversation(item));
      }),
    ),
    vscode.commands.registerCommand(
      "runtrol.deleteConversation",
      // From the row's X. Guarded, because a command invoked with the wrong thing must refuse rather than
      // delete something surprising.
      (item: unknown) => run(async () => {
        if (!(item instanceof ConversationItem)) return;
        await afterReady(() => controller.deleteConversation(item));
      }),
    ),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("runtrol.corePath")) {
        const previous = lifecycle;
        lifecycle = previous.catch(() => undefined).then(async () => {
          locator.invalidate();
          await controller.reconnect();
        });
        void run(async () => {
          await lifecycle;
          await configureRemoteConnection(client);
        });
      } else if (event.affectsConfiguration("runtrol.relayOrigin")) {
        void run(() => afterReady(async () => {
          await configureRemoteConnection(client);
        }));
      }
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => {
      void run(() => afterReady(() => rootFollowing.follow()));
    }),
  );
  // Before the Runtime integration speaks: proving the daemon that answered is the installed generation
  // is what lets everything past the hello assume the daemon and this extension are the same build.
  context.subscriptions.push(projectStore.onDidChange(() => {
    void run(() => afterReady(() => rootFollowing.follow()));
  }));
  // The public locator's native verification starts now; the private locator reads the control endpoint off
  // its answer instead of spawning `endpoint`, and `initialize` finds it settled. See `warmLocator`.
  void runtime.warmLocator();
  // The generation supervision (re-check and reconnect to the installed build) runs, but says nothing on the
  // sidebar: the "update applies when the running conversations end" line read as out of nowhere on a machine
  // with several generations alive, especially when this window sees none of them running (operator, 2026-08-29).
  const runtimeInitialization = superviseCoreCurrency(client, locator).then(() => runtime.initialize());
  const controllerInitialization = runtimeInitialization.then(async () => {
    initializationStage = "controller";
    await controller.initialize();
  });
  const readyInitialization = controllerInitialization;
  readyInitialization.then(
    () => {
      initializationStage = "ready";
      settleReady?.();
      // After ready rather than at enrollment, so a window opened onto a not-yet-approved folder catches up on
      // its own activation, which is the same physical act the first enrollment trusted.
      void run(() => rootFollowing.follow());
      void run(async () => {
        await materializeProviderShims(await locator.runtimeExecutable(), providerShimDirectory);
      });
    },
    (error: unknown) => {
      // Activation itself failed. If nothing was ever listed the Core never answered, so say that rather than
      // leaving the sidebar on "Connecting..." for the rest of the window's life: a wait with no end is the
      // same lie as the wrong sentence it replaced, just quieter. A failure after the first listing is
      // something else failing, and it must not rewrite a Core that demonstrably answered.
      if (state.coreReach !== "reached") state.setCoreReach("unreachable");
      settleReady?.(error);
    },
  );
  sidebar.offerServices(offerServices);
  controller.chooseService = (workspace) => {
    sidebar.chooseService(workspace);
    void vscode.commands.executeCommand("runtrol.sidebar.focus");
  };
  // Whether this window knows the views this build declares.
  //
  // The editor reads a container's set of views when the window opens and keeps it: a view this build
  // deleted is still drawn as an empty box, and one it added is missing. Nothing here can re-register them,
  // and the sidebar that results reads as broken rather than as behind. The focus command the editor makes
  // for each declared view is the honest test, because it exists exactly when the registration does.
  void vscode.commands.getCommands(true).then((known) => {
    if (known.includes("runtrol.sidebar.focus")) return;
    sidebar.setStaleWindow(
      "This window opened before the unified Runtrol sidebar was registered. Open a new window to use it.",
    );
  });
  void run(() => lifecycle);
  void run(async () => {
    await lifecycle;
    await configureRemoteConnection(client);
  });
  void run(async () => {
    await lifecycle;
    const workingDirectory = vscode.workspace.workspaceFolders
      ?.find((folder) => folder.uri.scheme === "file")
      ?.uri.fsPath ?? vscode.env.appRoot;
    await agentTools.refresh(workingDirectory);
  });
  return {
    get ready() {
      return lifecycle;
    },
    get initializationStage() {
      return MEASURED_HOST ? initializationStage : undefined;
    },
    refresh: () => afterReady(() => controller.refresh()),
    measureSessionManagement: MEASURED_HOST
      ? (sessionIds, progress = () => {}) => afterReady(async () => {
        const expected = new Set(sessionIds);
        const managed = state.sessions.filter((session) => expected.has(session.sessionId));
        const initialHot = managed.filter((session) => session.hot);
        const cold = managed.find((session) => !session.hot);
        if (expected.size !== 30 || managed.length !== expected.size || initialHot.length !== 8 || !cold) {
          throw new Error(
            `expected 30 named sessions with eight hot and a cold choice, found ${managed.length} and ${initialHot.length}`,
          );
        }

        const resumeStarted = performance.now();
        progress("cold-select");
        // The extension-host integration owns the whole measurement hang guard. Per-phase timers here would
        // abort a valid trial on a saturated runner before the three-trial performance ratchet can score it.
        await controller.select(cold.sessionId);
        progress("cold-watch-and-render");
        const coldResumeMs = performance.now() - resumeStarted;
        const resumed = state.selected;
        // Named one by one. "did not heat" covered six different failures, and every one of them sent the
        // reader to look at resume when the mismatch might have been the row, the provider or the folder.
        const openedRow = state.conversations.find(
          (candidate) => candidate.session?.sessionId === cold.sessionId,
        );
        const mismatch = describeResumeMismatch(resumed, cold, openedRow, terminals);
        if (mismatch) {
          throw new Error(`selecting a cold row did not heat the same Runtime-managed session: ${mismatch}`);
        }
        const current = state.sessions.filter((session) => expected.has(session.sessionId));
        const hot = current.filter((session) => session.hot);
        if (current.length !== 30 || hot.length !== 8) {
          throw new Error(`cold resume changed the 30-session and eight-hot bounds to ${current.length} and ${hot.length}`);
        }
        const samples: number[] = [];
        for (let round = 0; round < SESSION_SWITCH_ROUNDS; round += 1) {
          for (const [index, session] of hot.entries()) {
            const started = performance.now();
            progress(`round-${round + 1}-session-${index + 1}-select`);
            await controller.select(session.sessionId);
            samples.push(performance.now() - started);
          }
        }
        progress("selection-persistence");
        await controller.selectionPersisted();
        return {
          sessionCount: current.length,
          hotSessionCount: hot.length,
          coldResumeMs,
          sessionSwitchP95Ms: percentile(samples, 0.95),
          resumedFrom: cold.sessionId,
          resumedTo: resumed?.sessionId ?? "",
          restoreSession: state.selected?.sessionId ?? "",
          restoreWorkspace: state.selected?.workspace ?? "",
        };
      })
      : undefined,
    verifyRestoredSession: MEASURED_HOST
      ? (sessionId) => afterReady(async () => {
        if (state.selected?.sessionId !== sessionId) {
          throw new Error(`restored ${state.selected?.sessionId ?? "no session"}, expected ${sessionId}`);
        }
      })
      : undefined,
    // The two follow probes exist for the live root-following proof: a real window opens a second folder and the
    // harness watches that folder's provider-owned stored conversation arrive. Managed sessions are deliberately
    // machine-wide on this owner-only local surface, so they cannot prove that a discovery root widened.
    hasConversationIn: MEASURED_HOST
      ? (folder) => afterReady(async () => nativeConversationVisibleIn(state, folder))
      : undefined,
    waitForConversationIn: MEASURED_HOST
      ? (folder, deadlineMs) => afterReady(() => new Promise<number>((resolve, reject) => {
        const arrived = () => nativeConversationVisibleIn(state, folder);
        if (arrived()) {
          resolve(0);
          return;
        }
        const started = performance.now();
        const timer = setTimeout(() => {
          subscription.dispose();
          reject(new Error(`no conversation arrived for ${folder} within ${deadlineMs} ms`));
        }, deadlineMs);
        const subscription = state.onDidChange(() => {
          if (!arrived()) return;
          clearTimeout(timer);
          subscription.dispose();
          resolve(performance.now() - started);
        });
      }))
      : undefined,
    // The harness's way to stand a created project up without driving the folder-picker dialog. Same code path
    // as the command, minus the picking.
    seedProject: MEASURED_HOST || RUNTROL_INCLUDE_TEST_JOURNEY
      ? async (folder) => {
        await projectStore.create(folder);
      }
      : undefined,
    openFirstConversation: MEASURED_HOST || RUNTROL_INCLUDE_TEST_JOURNEY
      ? async () => {
        // The eye pass photographs a real conversation, so it opens the first one the tree would show,
        // through the same selection path a click takes.
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null, state.projectlessRoot);
        const openable = rows.find((row) => row.canOpen);
        if (!openable) {
          throw new Error("no openable conversation for the eye pass");
        }
        await controller.select(openable);
        await vscode.commands.executeCommand("runtrol.openConversation");
      }
      : undefined,
    openCrossProjectConversation: MEASURED_HOST
      ? async () => {
        // The contract in `docs/vscodeSurface.md`, provable: a conversation whose folder this window never
        // opened selects and opens as a tab right here. Managed rows only, because native rows still ride
        // the enrollment roots until discovery goes machine-wide.
        const open = (vscode.workspace.workspaceFolders ?? []).map((folder) => folder.uri.fsPath);
        const rows = conversationRows(state.sessions, state.providers, state.nativeChats, null, state.projectlessRoot);
        const away = rows.find((row) =>
          row.canOpen
          && row.session !== null
          && !open.some((folder) => workspaceCovers(folder, row.workspace)));
        if (!away) {
          throw new Error("no conversation outside the open folders to prove the contract with");
        }
        await controller.select(away);
        await vscode.commands.executeCommand("runtrol.openConversation");
        if (state.selected?.sessionId !== away.session?.sessionId) {
          throw new Error("selecting the away conversation did not take");
        }
      }
      : undefined,
    journey: RUNTROL_INCLUDE_TEST_JOURNEY
      ? journeyApi(
        controller,
        state,
        afterReady,
        context.extensionMode,
        (sessionId) => sidebar.revealSession(sessionId),
        (key) => sidebar.revealConversation(key),
        () => sidebar.treeItemIds(),
      )
      : undefined,
  };
}

/// Why a cold row that was selected is not the hot session it should now be, or null when it is.
///
/// One sentence per way it can be wrong. The single message this replaced covered six failures at once, and each
/// of them sent the reader to look at resume when the mismatch might have been the row, the service or the folder.
function describeResumeMismatch(
  resumed: RuntimeState["selected"],
  cold: { sessionId: string; providerId: string; nativeSessionId?: string | null; workspace: string },
  row: Conversation | undefined,
  terminals: TerminalTabs,
): string | null {
  if (!resumed) return "nothing is selected";
  // What selecting a stored conversation promises is its tab, not a structured resume: this surface opens the
  // service's own terminal and nothing else (`docs/terminalSurface.md`). The measurement asked for a session
  // that had turned hot, which is the older model's promise and stopped being made when that model was removed.
  if (!row) return "the selected session has no row to open";
  if (!terminals.isOpen(row.key)) return "the selected conversation has no open tab";
  if (resumed.sessionId !== cold.sessionId) {
    return `a different session is selected: ${resumed.sessionId} instead of ${cold.sessionId}`;
  }
  if (resumed.providerId !== cold.providerId) {
    return `the selected session changed service: ${resumed.providerId} instead of ${cold.providerId}`;
  }
  if (resumed.nativeSessionId !== cold.nativeSessionId) {
    return "the selected session changed its service-side identity";
  }
  if (resumed.workspace !== cold.workspace) {
    return `the selected session changed folder: ${resumed.workspace} instead of ${cold.workspace}`;
  }
  return null;
}

/// Whether provider discovery has made a stored conversation in this folder visible.
function nativeConversationVisibleIn(state: RuntimeState, folder: string): boolean {
  const identity = workspaceIdentity(folder);
  return state.nativeChats.some((chat) => workspaceIdentity(chat.cwd) === identity);
}

function testIntegrationRoots(context: vscode.ExtensionContext): readonly string[] {
  if (context.extensionMode !== vscode.ExtensionMode.Test) return [];
  const raw = process.env.RUNTROL_TEST_INTEGRATION_ROOTS;
  if (!raw) return [];
  const value: unknown = JSON.parse(raw);
  if (
    !Array.isArray(value)
    || value.length > 32
    || !value.every((root) => typeof root === "string" && path.isAbsolute(root))
  ) {
    throw new Error("RUNTROL_TEST_INTEGRATION_ROOTS must contain at most 32 absolute paths");
  }
  return [...new Set(value)];
}

async function changeAgentTools(
  controller: AgentToolsController,
  action: AgentToolsAction,
  item?: ProjectItem,
): Promise<void> {
  const workspace = item?.group.workspace ?? await chooseAgentToolsProject();
  if (!workspace) return;
  const name = path.basename(workspace) || workspace;
  const changing = action === "enable" ? "Enabling" : "Disabling";
  const result = await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: `${changing} Agent Tools for ${name}`,
      cancellable: false,
    },
    () => action === "enable" ? controller.enable(workspace) : controller.disable(workspace),
  );
  const warning = result.lines.find((line) => line.startsWith("warning:"));
  if (warning) void vscode.window.showWarningMessage(warning);
  if (action === "enable") {
    void vscode.window.showInformationMessage(
      `Agent Tools are ready for ${name}. Coding agents can now delegate through Runtrol; approvals stay with you.`,
    );
  } else {
    void vscode.window.showInformationMessage(
      result.alreadySettled
        ? `Agent Tools were already off for ${name}.`
        : `Agent Tools are off for ${name}. Runtime authority and local credentials were removed.`,
    );
  }
}

async function chooseAgentToolsProject(): Promise<string | null> {
  const folders = (vscode.workspace.workspaceFolders ?? [])
    .filter((folder) => folder.uri.scheme === "file");
  if (folders.length === 0) {
    await vscode.window.showWarningMessage("Open a local project folder before enabling Agent Tools.");
    return null;
  }
  if (folders.length === 1) return folders[0]?.uri.fsPath ?? null;
  const picked = await vscode.window.showQuickPick(
    folders.map((folder) => ({
      label: folder.name,
      detail: folder.uri.fsPath,
      workspace: folder.uri.fsPath,
    })),
    {
      title: "Project for Agent Tools",
      placeHolder: "Choose the one project root coding agents may orchestrate",
      matchOnDetail: true,
    },
  );
  return picked?.workspace ?? null;
}

export function deactivate(): void {}

async function configureRemoteConnection(client: CoreClient): Promise<RemoteConnection> {
  const configured = vscode.workspace
    .getConfiguration("runtrol")
    .get<string>("relayOrigin", "")
    .trim();
  const { response } = await client.once({
    ask: "remoteConfigure",
    with: { relay_origin: configured || null },
  });
  return readRemoteConnection(response);
}

async function remoteConnection(client: CoreClient): Promise<RemoteConnection> {
  const { response } = await client.once({ ask: "remoteConnection" });
  return readRemoteConnection(response);
}

function readRemoteConnection(response: Awaited<ReturnType<CoreClient["once"]>>["response"]): RemoteConnection {
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  if (response.say !== "remoteConnection") {
    throw new Error(`the Core answered remote connection status with ${response.say}`);
  }
  return response.with;
}

function remoteConnectionMessage(connection: RemoteConnection): string {
  if (connection.state === "disabled") {
    return "Runtrol phone connection is disabled. Set runtrol.relayOrigin to enable it.";
  }
  if (connection.state === "online") {
    return `Runtrol phone connection is online through ${connection.relay_origin ?? "the configured relay"}.`;
  }
  if (connection.state === "connecting") {
    return `Runtrol phone connection is connecting to ${connection.relay_origin ?? "the configured relay"}.`;
  }
  return `Runtrol phone connection is retrying after ${connection.stage ?? "relay"} failure.`;
}

async function run(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    // Already explained, with the coding service's own next steps offered as buttons. A second message
    // underneath that one reads as a second problem, and the bare protocol string is the less useful of
    // the two.
    if (error instanceof ServiceTroubleReported) return;
    await vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error));
  }
}

async function restartExtensionHost(): Promise<void> {
  const confirmed = await vscode.window.showWarningMessage(
    "Restart the VS Code Extension Host? Other extensions in this window will restart too.",
    { modal: true },
    "Restart extensions",
  );
  if (confirmed !== "Restart extensions") return;
  await vscode.commands.executeCommand("workbench.action.restartExtensionHost");
}

function percentile(values: readonly number[], at: number): number {
  const ordered = [...values].sort((left, right) => left - right);
  return ordered[Math.ceil(ordered.length * at) - 1] ?? Number.POSITIVE_INFINITY;
}

function within<T>(work: Promise<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) {
      clearTimeout(timer);
    }
  });
}
