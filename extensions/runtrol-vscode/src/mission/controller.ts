import { randomUUID } from "node:crypto";
import * as path from "node:path";

import * as vscode from "vscode";

import type { Controller } from "../controller";
import type { CoreClient } from "../core/client";
import type { DiffDocuments } from "../diffDocuments";
import type {
  GateLine,
  MissionLine,
  MissionSnapshot,
  MissionTaskLine,
  Response,
} from "../protocol";
import type { RuntimeState } from "../state";
import { workspaceIdentity } from "../workspaceCollision";
import {
  MAX_BRANCHES,
  MIN_BRANCHES,
  branchProblem,
  fanOutName,
  instructionDigest,
  missionSpec,
} from "./fanOut";
import { AmbiguousSubmissions } from "./ambiguousSubmissions";
import {
  MAX_AUTO_FLIGHTS,
  AutoFlights,
  createAutoFlightArm,
  decideAutoFlight,
  restrictAutoFlightMomentum,
  type AutoFlightArm,
  type AutoFlightSignal,
  type AutoFlightTurn,
} from "./autoFlight";
import {
  missionFlightDeck,
  type MissionFlightDeck,
} from "./flightDeck";
import { branchFromHead, gitdirTarget } from "./gitHead";
import { MissionLandingController } from "./landing/controller";
import { safeArtifactPath } from "./landing/model";
import {
  hasMissionMomentumWork,
  missionMomentum,
  type MissionMomentum,
} from "./momentum";
import { MissionProjectLeases } from "./projectLease";

/// Where each project's last fan-out shape is remembered, never instruction text.
const FAN_OUT_DEFAULTS_KEY = "runtrol.fanOutDefaults";
/// Task identities whose durable local Send intent exists without a confirmed public Runtime delivery.
const AMBIGUOUS_TASK_SUBMISSIONS_KEY = "runtrol.ambiguousMissionTaskSubmissions";
/// Exact local authority and Runtime generation markers for at most eight armed Mission Auto Flights.
const AUTO_FLIGHTS_KEY = "runtrol.missionAutoFlights";
/// The current branch of the repository at `folder`, or null when it cannot be read honestly.
///
/// Read-only: `.git/HEAD` is the repository's own statement, and a linked worktree's one-line
/// `gitdir:` indirection is followed exactly one step because one step is what git writes there.
async function currentBranch(folder: string): Promise<string | null> {
  const read = async (file: string): Promise<string | null> => {
    try {
      return new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.file(file)));
    } catch {
      // A folder without a readable repository is the normal miss; the caller's prefilled
      // question is the handling.
      return null;
    }
  };
  const dotGit = path.join(folder, ".git");
  const stat = await vscode.workspace.fs.stat(vscode.Uri.file(dotGit)).then(
    (value) => value,
    () => null,
  );
  if (!stat) return null;
  let headFile = path.join(dotGit, "HEAD");
  if (stat.type === vscode.FileType.File) {
    const target = gitdirTarget((await read(dotGit)) ?? "");
    if (!target) return null;
    headFile = path.join(path.isAbsolute(target) ? target : path.join(folder, target), "HEAD");
  }
  const head = await read(headFile);
  return head === null ? null : branchFromHead(head);
}

export type MissionSelection = {
  readonly mission?: MissionLine;
  readonly task?: MissionTaskLine;
};

type MissionAdvanceResult = {
  readonly snapshot: MissionSnapshot;
  readonly verified: number;
  readonly sessionIds: readonly string[];
  readonly providerSelectionCancelled: boolean;
};

type MissionProviderChooser = (
  tasks: readonly MissionTaskLine[],
) => Promise<ReadonlyMap<string, string> | null>;

type MissionAdvanceOptions = {
  readonly showProgress?: boolean;
  readonly verifyTaskIds?: ReadonlySet<string>;
  readonly beforeSubmissions?: (submissions: readonly AutoFlightTurn[]) => Promise<void>;
};

type MissionFlightFailure = {
  readonly missionId: string;
  readonly name: string;
  readonly project: string;
  readonly message: string;
};

type MissionFlightAdvanceResult = {
  readonly advanced: number;
  readonly verified: number;
  readonly sessionIds: readonly string[];
  readonly failures: readonly MissionFlightFailure[];
  readonly providerSelectionCancelled: boolean;
  readonly remainingReady: number;
};

export class MissionController implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  private readonly documents = new MissionDocumentProvider();
  private rows: readonly MissionLine[] = [];
  private readonly snapshots = new Map<string, MissionSnapshot>();
  /// A durable Mission Send intent can exist when the public Runtime delivery failed or lost its answer. The
  /// convenience path must not mistake the still-idle session for completed provider work, including after restart.
  private readonly ambiguousTaskSubmissions: AmbiguousSubmissions;
  private readonly autoFlights: AutoFlights;
  private readonly landings: MissionLandingController;
  private readonly projectLeases = new MissionProjectLeases();
  private readonly autoFlightSubscription: vscode.Disposable;
  /// An operator disarm takes effect in memory before the durable removal finishes. This closes the only await
  /// boundary before provider input while keeping the persisted arm conservative if storage itself refuses.
  private readonly autoFlightRevocations = new Set<string>();
  private autoFlightsStarted = false;
  private autoFlightPending = false;
  private autoFlightRun: Promise<void> | null = null;
  private disposed = false;

  readonly onDidChange = this.changedEmitter.event;

  constructor(
    private readonly client: CoreClient,
    private readonly sessions: Controller,
    private readonly runtimeState: RuntimeState,
    private readonly context: vscode.ExtensionContext,
    diffDocuments: DiffDocuments,
  ) {
    this.ambiguousTaskSubmissions = new AmbiguousSubmissions(
      context.globalState.get<readonly string[]>(AMBIGUOUS_TASK_SUBMISSIONS_KEY, []),
      (taskIds) => context.globalState.update(AMBIGUOUS_TASK_SUBMISSIONS_KEY, taskIds),
    );
    this.autoFlights = new AutoFlights(
      context.globalState.get<unknown>(AUTO_FLIGHTS_KEY, []),
      (arms) => context.globalState.update(AUTO_FLIGHTS_KEY, arms),
    );
    this.landings = new MissionLandingController(diffDocuments, {
      getSnapshot: (missionId) => this.get(missionId),
      listIntegratingSnapshots: async () => {
        await this.refresh();
        return Promise.all(
          this.rows
            .filter((mission) => mission.state === "integrating")
            .map((mission) => this.get(mission.mission_id)),
        );
      },
      complete: (snapshot) => this.verifyAndCompleteIntegration(snapshot, null),
      withProjectLease: (snapshot, action) => this.projectLeases.run(
        snapshot.mission.project,
        snapshot.mission.mission_id,
        action,
      ),
    });
    this.autoFlightSubscription = runtimeState.onDidChange((change) => {
      if (change === "rows") this.scheduleAutoFlights();
    });
  }

  get missions(): readonly MissionLine[] {
    return this.rows;
  }

  documentProvider(): vscode.TextDocumentContentProvider {
    return this.documents;
  }

  async initialize(): Promise<void> {
    await this.refresh();
  }

  /// Restored authority is observed only after both Core and Runtime have completed their first exact listings.
  startAutoFlights(): void {
    this.autoFlightsStarted = true;
    this.scheduleAutoFlights();
  }

  isAutoFlightArmed(missionId: string): boolean {
    return !this.autoFlightRevocations.has(missionId)
      && this.autoFlights.get(missionId)?.pendingSignal === null;
  }

  hasAutoFlightRecord(missionId: string): boolean {
    return this.autoFlights.get(missionId) !== null;
  }

  async refresh(): Promise<void> {
    const response = (await this.client.read({ ask: "missionList" })).response;
    this.rows = requireResponse(response, "missions");
    this.changedEmitter.fire();
  }

  async validateMission(): Promise<void> {
    const selected = await vscode.window.showOpenDialog({
      title: "Validate a reviewed Mission file",
      openLabel: "Validate Mission",
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
      filters: { "Mission TOML": ["toml"] },
    });
    const source = selected?.[0];
    if (!source) return;
    await this.validateMissionFile(source);
  }

  async validateMissionFile(source: vscode.Uri): Promise<MissionSnapshot> {
    const folder = vscode.workspace.getWorkspaceFolder(source);
    if (!folder) {
      throw new Error("the Mission file must be inside an open VS Code workspace");
    }
    const missionRef = path.relative(folder.uri.fsPath, source.fsPath).replaceAll("\\", "/");
    if (!missionRef || missionRef.startsWith("../") || path.isAbsolute(missionRef)) {
      throw new Error("the Mission file must be project relative");
    }
    const response = (await this.client.once({
      ask: "missionValidate",
      with: { project: folder.uri.fsPath, mission_ref: missionRef },
    })).response;
    const snapshot = requireResponse(response, "mission");
    await this.acceptSnapshot(snapshot, true);
    return snapshot;
  }

  /// Write the Mission that tries one instruction several ways at once, each attempt in its own worktree.
  ///
  /// Reaching this flow used to mean composing a Mission document by hand: schema line, limits, and one task block
  /// per attempt with a matching instruction digest. That is the part nobody should be typing, and it is the whole
  /// reason the flow went unused. So this composes it and hands it over.
  ///
  /// # Why it hands the document over instead of saving it
  ///
  /// Two reasons that turned out to be the same reason.
  ///
  /// The extension never writes instruction or conversation content. Its audited disk mutations are limited to
  /// small local preference state, the managed Core, exact reviewed Receipt Landing bytes, and its bounded writer
  /// lease. Writing an instruction here would make this surface a place where conversation starts living, which is
  /// the boundary the writer contract exists to hold.
  ///
  /// And Mission binds a task to the exact bytes of its instruction because those bytes are meant to have been
  /// reviewed. A document generated, saved and validated without the operator seeing it would keep every mechanism
  /// and discard the thing the mechanism is for. Handing it to an editor means the review is real: they read it,
  /// they choose where it lives, and they save it.
  async fanOutInstruction(): Promise<void> {
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      throw new Error("open the project you want to try this in before composing a fan-out");
    }
    const chosen = await vscode.window.showOpenDialog({
      title: "Which instruction should every attempt follow?",
      openLabel: "Use this instruction",
      canSelectFiles: true,
      canSelectFolders: false,
      canSelectMany: false,
    });
    const instructionUri = chosen?.[0];
    if (!instructionUri) return;
    const instructionRef = path
      .relative(folder.uri.fsPath, instructionUri.fsPath)
      .replaceAll("\\", "/");
    if (!instructionRef || instructionRef.startsWith("../") || path.isAbsolute(instructionRef)) {
      throw new Error("the instruction file must live inside this project");
    }
    // Read, never written. The digest has to be over the bytes already on disk, because those are the bytes the
    // operator reviewed and the ones Mission will bind the task to.
    const bytes = await vscode.workspace.fs.readFile(instructionUri);
    let instruction: string;
    try {
      instruction = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      throw new Error("the instruction file must be valid UTF-8");
    }
    if (!instruction.trim()) {
      throw new Error("that instruction file is empty");
    }

    const branches = await vscode.window.showInputBox({
      title: "Try one instruction several ways",
      prompt: `How many attempts at once? (${MIN_BRANCHES} to ${MAX_BRANCHES})`,
      value: "3",
      ignoreFocusOut: true,
      validateInput: branchProblem,
    });
    if (!branches) return;
    const branchCount = Number(branches);
    const projectKey = workspaceIdentity(folder.uri.fsPath);
    const providerIds = await this.chooseFanOutProviders(projectKey, branchCount);
    if (!providerIds) return;
    // Offered from the registry instead of typed from memory. Asked rather than invented, still: a
    // Gate decides whether an attempt succeeded, and inventing one that checks nothing would
    // produce a fan-out whose attempts all pass regardless of what they did.
    const gateId = await this.chooseGate();
    if (!gateId) return;

    // The repository's own word on where it stands, prefilled rather than assumed: a detached HEAD
    // or an unreadable repository falls back to the question with the old default.
    const detectedBranch = await currentBranch(folder.uri.fsPath);
    const baseRef = await vscode.window.showInputBox({
      title: "Try one instruction several ways",
      prompt: "Which ref do the attempts branch from?",
      value: detectedBranch ?? "main",
      ignoreFocusOut: true,
    });
    if (!baseRef?.trim()) return;

    // The blast radius somebody declared last time, offered again. The project's own source
    // directory stays the first-time default: an attempt allowed to write anywhere is an attempt
    // whose blast radius nobody declared.
    const rootsText = await vscode.window.showInputBox({
      title: "Try one instruction several ways",
      prompt: "Which directories may the attempts write to? (comma separated, project relative)",
      value: this.fanOutRoots(projectKey).join(", "),
      ignoreFocusOut: true,
    });
    if (rootsText === undefined) return;
    const outputRoots = rootsText
      .split(",")
      .map((root) => root.trim())
      .filter(Boolean);
    if (outputRoots.length === 0) return;
    await this.rememberFanOutDefaults(projectKey, outputRoots, providerIds);

    const document = await vscode.workspace.openTextDocument({
      language: "toml",
      content: missionSpec(fanOutName(instruction, new Date()), instructionDigest(bytes), {
        instruction,
        instructionRef,
        branches: branchCount,
        gateId,
        baseRef: baseRef.trim(),
        outputRoots,
        providerIds,
      }),
    });
    await vscode.window.showTextDocument(document, { preview: false });
    await vscode.window.showInformationMessage(
      `Read this, save it in ${folder.name}, then run Validate Mission on it.`,
    );
  }

  /// The Gate for this fan-out: registered ones offered first, registering chained in, typing kept
  /// as the escape for an identity the registry does not hold yet.
  private async chooseGate(): Promise<string | null> {
    const response = (await this.client.read({ ask: "missionListGates" })).response;
    const gates: GateLine[] = response.say === "missionGates" ? response.with : [];
    const picked = await vscode.window.showQuickPick(
      [
        ...gates.map((gate) => ({
          label: gate.gate_id,
          description: gate.program,
          detail: `hard timeout ${gate.timeout_ms} ms`,
          choice: "registered" as const,
        })),
        {
          label: "Register a new Gate...",
          description: "one registration per project, then every fan-out reuses it",
          detail: undefined,
          choice: "register" as const,
        },
        {
          label: "Type a Gate ID...",
          description: "for an identity this registry does not hold yet",
          detail: undefined,
          choice: "type" as const,
        },
      ],
      {
        title: "Which registered Gate decides whether an attempt worked?",
        placeHolder: gates.length === 0
          ? "No Gate is registered yet; register one once and every fan-out reuses it"
          : "A Gate decides whether an attempt worked; nothing passes without one",
      },
    );
    if (!picked) return null;
    if (picked.choice === "registered") return picked.label;
    if (picked.choice === "register") return this.registerGate();
    const typed = await vscode.window.showInputBox({
      title: "Try one instruction several ways",
      prompt: "Which registered Gate decides whether an attempt worked?",
      placeHolder: "the Gate ID you registered for this project",
      ignoreFocusOut: true,
    });
    return typed?.trim() || null;
  }

  private fanOutRoots(projectKey: string): string[] {
    const stored = this.context.globalState.get(FAN_OUT_DEFAULTS_KEY);
    if (stored === null || typeof stored !== "object" || Array.isArray(stored)) return ["src"];
    const entry = (stored as Record<string, unknown>)[projectKey];
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return ["src"];
    const roots = (entry as { outputRoots?: unknown }).outputRoots;
    return Array.isArray(roots) && roots.length > 0 && roots.every((root) => typeof root === "string")
      ? roots
      : ["src"];
  }

  private fanOutProviderIds(projectKey: string): string[] {
    const stored = this.context.globalState.get(FAN_OUT_DEFAULTS_KEY);
    if (stored === null || typeof stored !== "object" || Array.isArray(stored)) return [];
    const entry = (stored as Record<string, unknown>)[projectKey];
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return [];
    const providers = (entry as { providerIds?: unknown }).providerIds;
    return Array.isArray(providers) && providers.every((provider) => typeof provider === "string")
      ? providers
      : [];
  }

  private async chooseFanOutProviders(projectKey: string, branches: number): Promise<string[] | null> {
    const usable = this.runtimeState.providers.filter((provider) => provider.installation.state === "usable");
    if (usable.length === 0) throw new Error("no installed coding-agent CLI is currently usable");
    const remembered = new Set(this.fanOutProviderIds(projectKey));
    const defaults = remembered.size > 0
      ? remembered
      : new Set(usable.slice(0, Math.min(branches, usable.length)).map((provider) => provider.providerId));
    while (true) {
      const selected = await vscode.window.showQuickPick(
        usable.map((provider) => ({
          label: provider.displayName,
          description: provider.providerId,
          id: provider.providerId,
          picked: defaults.has(provider.providerId),
        })),
        {
          title: "Which coding services should try it?",
          placeHolder: "Selected services are assigned round-robin to the reviewed attempts",
          canPickMany: true,
          ignoreFocusOut: true,
        },
      );
      if (!selected) return null;
      if (selected.length === 0) {
        await vscode.window.showWarningMessage("Select at least one runtime-discovered coding service.");
        continue;
      }
      if (selected.length > branches) {
        await vscode.window.showWarningMessage(`Select no more than the ${branches} attempts being created.`);
        continue;
      }
      return selected.map((provider) => provider.id);
    }
  }

  private async rememberFanOutDefaults(
    projectKey: string,
    outputRoots: string[],
    providerIds: string[],
  ): Promise<void> {
    const stored = this.context.globalState.get(FAN_OUT_DEFAULTS_KEY);
    const record = stored !== null && typeof stored === "object" && !Array.isArray(stored)
      ? { ...(stored as Record<string, unknown>) }
      : {};
    record[projectKey] = { outputRoots, providerIds };
    await this.context.globalState.update(FAN_OUT_DEFAULTS_KEY, record);
  }

  /// Register one Gate and hand back its identity, so a fan-out can chain straight into using it.
  async registerGate(): Promise<string | null> {
    const gateId = await requiredInput("Register deterministic Gate", "Stable Gate ID");
    if (!gateId) return null;
    const program = await requiredInput("Register deterministic Gate", "Executable name, without shell text");
    if (!program) return null;
    const argumentsText = await vscode.window.showInputBox({
      title: "Register deterministic Gate",
      prompt: "Fixed argument vector as a JSON string array",
      value: "[]",
      ignoreFocusOut: true,
    });
    if (argumentsText === undefined) return null;
    const parsed: unknown = JSON.parse(argumentsText);
    if (!Array.isArray(parsed) || !parsed.every((value) => typeof value === "string")) {
      throw new Error("Gate arguments must be a JSON string array");
    }
    const timeoutText = await requiredInput("Register deterministic Gate", "Hard timeout in milliseconds", "60000");
    if (!timeoutText) return null;
    const timeout = Number(timeoutText);
    if (!Number.isSafeInteger(timeout) || timeout <= 0) {
      throw new Error("the Gate timeout must be a positive integer");
    }
    requireDone((await this.client.once({
      ask: "missionRegisterGate",
      with: { gate_id: gateId, program, arguments: parsed, timeout_ms: timeout },
    })).response, "Gate registration");
    await vscode.window.showInformationMessage(`Registered Gate ${gateId}.`);
    return gateId;
  }

  async openMission(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Open Mission");
    if (!mission) return;
    const snapshot = await this.get(mission.mission_id);
    await this.acceptSnapshot(snapshot, true);
  }

  snapshot(missionId: string): Promise<MissionSnapshot> {
    return this.get(missionId);
  }

  async startMission(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Start Mission");
    if (!mission) return;
    const snapshot = await this.get(mission.mission_id);
    const action = await vscode.window.showWarningMessage(
      `Start ${snapshot.mission.name} with ${snapshot.tasks.length} reviewed Tasks?`,
      {
        modal: true,
        detail: `Mission SHA-256 ${snapshot.mission_sha256}\nNo Task instruction is sent until its exact local Send action.`,
      },
      "Start reviewed Mission",
    );
    if (action !== "Start reviewed Mission") return;
    const started = requireResponse((await this.client.once({
      ask: "missionStart",
      with: {
        mission_id: snapshot.mission.mission_id,
        mission_sha256: snapshot.mission_sha256,
      },
    })).response, "mission");
    await this.acceptSnapshot(started, true);
  }

  async armMissionAutoFlight(selection?: MissionSelection): Promise<void> {
    const available = MAX_AUTO_FLIGHTS - this.autoFlights.current().length;
    if (available <= 0) {
      await vscode.window.showInformationMessage(
        `The local limit of ${MAX_AUTO_FLIGHTS} armed Mission Auto Flights is already in use.`,
      );
      return;
    }

    let selected: readonly MissionLine[];
    if (selection?.mission) {
      if (this.autoFlights.isArmed(selection.mission.mission_id)) {
        await vscode.window.showInformationMessage(`${selection.mission.name} already has Auto Flight armed.`);
        return;
      }
      selected = [selection.mission];
    } else {
      await this.refresh();
      const candidates = this.rows.filter((mission) =>
        (mission.state === "validated" || mission.state === "running")
        && mission.completion_policy === "allTasks"
        && !this.autoFlights.isArmed(mission.mission_id)
      ).sort((left, right) => left.name.localeCompare(right.name) || left.mission_id.localeCompare(right.mission_id));
      if (candidates.length === 0) {
        await vscode.window.showInformationMessage("No reviewed ordinary Mission is available for Auto Flight.");
        return;
      }
      const picks = await vscode.window.showQuickPick(
        candidates.map((mission) => ({
          label: mission.name,
          description: mission.state,
          detail: `${mission.passed_tasks}/${mission.total_tasks}  ${mission.project}`,
          mission,
        })),
        {
          title: "Arm Mission Auto Flight",
          placeHolder: candidates.length
            ? `Select up to ${available} reviewed ordinary Missions`
            : "No reviewed ordinary Mission is available",
          canPickMany: true,
        },
      );
      if (!picks || picks.length === 0) return;
      if (picks.length > available) {
        await vscode.window.showWarningMessage(`Select no more than ${available} Missions for this Auto Flight arm.`);
        return;
      }
      selected = picks.map((pick) => pick.mission);
    }

    const reviewed: MissionSnapshot[] = [];
    for (const mission of selected) {
      const snapshot = await this.get(mission.mission_id);
      createAutoFlightArm(snapshot, null, this.runtimeState.sessions, this.momentum(snapshot), Date.now());
      reviewed.push(snapshot);
    }
    const needsOperatorChoice = reviewed.some((snapshot) => snapshot.tasks.some((task) =>
      task.provider_selector === "operatorChoice"
      && (task.state === "pending" || task.state === "reserved")
    ));
    const operatorChoiceProvider = needsOperatorChoice
      ? await this.chooseProvider("operatorChoice")
      : null;
    if (needsOperatorChoice && !operatorChoiceProvider) return;
    for (const snapshot of reviewed) {
      this.resolveWaveProviders(
        snapshot.tasks.filter((task) => task.state === "pending" || task.state === "reserved"),
        operatorChoiceProvider,
      );
    }

    const providerDetail = operatorChoiceProvider
      ? `Operator-choice Tasks: ${operatorChoiceProvider}`
      : "Providers: reviewed fixed selectors or already bound sessions";
    const action = await vscode.window.showWarningMessage(
      `Arm Auto Flight for ${reviewed.length} reviewed Mission${reviewed.length === 1 ? "" : "s"}?`,
      {
        modal: true,
        detail: [
          providerDetail,
          "Every later safe DAG wave may start while this Studio window is open. Integration stays explicit.",
          "",
          ...reviewed.flatMap((snapshot) => [
            `${snapshot.mission.name}  ${snapshot.mission.project}`,
            `Mission SHA-256 ${snapshot.mission_sha256}`,
          ]),
        ].join("\n"),
      },
      "Arm Auto Flight",
    );
    if (action !== "Arm Auto Flight") return;

    const arms = [];
    for (const reviewedSnapshot of reviewed) {
      const exact = await this.get(reviewedSnapshot.mission.mission_id);
      if (exact.mission_sha256 !== reviewedSnapshot.mission_sha256) {
        throw new Error(`Mission ${reviewedSnapshot.mission.name} changed after review; Auto Flight was not armed`);
      }
      arms.push(createAutoFlightArm(
        exact,
        operatorChoiceProvider,
        this.runtimeState.sessions,
        this.momentum(exact),
        Date.now(),
      ));
    }
    for (const arm of arms) await this.clearMissionFlightSignals(arm.missionId, arm.missionSha256);
    await this.autoFlights.armMany(arms);
    for (const arm of arms) this.autoFlightRevocations.delete(arm.missionId);
    this.changedEmitter.fire();
    this.scheduleAutoFlights();
    await vscode.window.showInformationMessage(
      `${arms.length} Mission Auto Flight${arms.length === 1 ? " is" : "s are"} armed on this PC.`,
    );
  }

  async disarmMissionAutoFlight(selection?: MissionSelection): Promise<void> {
    const current = this.autoFlights.current().filter((arm) => (
      !this.autoFlightRevocations.has(arm.missionId) && arm.pendingSignal === null
    ));
    let missionId = selection?.mission?.mission_id ?? null;
    let name = selection?.mission?.name ?? null;
    if (!missionId) {
      if (current.length === 0) {
        await vscode.window.showInformationMessage("No Mission Auto Flight is armed.");
        return;
      }
      const pick = await vscode.window.showQuickPick(
        current.map((arm) => {
          const mission = this.rows.find((candidate) => candidate.mission_id === arm.missionId);
          return {
            label: mission?.name ?? arm.missionId,
            description: mission?.state ?? "restored",
            detail: mission?.project ?? `Mission SHA-256 ${arm.missionSha256}`,
            missionId: arm.missionId,
          };
        }),
        { title: "Disarm Mission Auto Flight", placeHolder: current.length ? "Select one arm" : "No Auto Flight is armed" },
      );
      if (!pick) return;
      missionId = pick.missionId;
      name = pick.label;
    }
    if (!this.autoFlights.isArmed(missionId)) {
      await vscode.window.showInformationMessage(`${name ?? missionId} does not have Auto Flight armed.`);
      return;
    }
    this.autoFlightRevocations.add(missionId);
    await this.autoFlights.disarm(missionId);
    this.changedEmitter.fire();
    await vscode.window.showInformationMessage(`${name ?? missionId} Auto Flight is disarmed.`);
  }

  async armMissionAutoFlightForJourney(
    missionId: string,
    operatorChoiceProvider: string | null,
  ): Promise<void> {
    const snapshot = await this.get(missionId);
    this.resolveWaveProviders(
      snapshot.tasks.filter((task) => task.state === "pending" || task.state === "reserved"),
      operatorChoiceProvider,
    );
    const arm = createAutoFlightArm(
      snapshot,
      operatorChoiceProvider,
      this.runtimeState.sessions,
      this.momentum(snapshot),
      Date.now(),
    );
    await this.clearMissionFlightSignals(arm.missionId, arm.missionSha256);
    await this.autoFlights.arm(arm);
    this.autoFlightRevocations.delete(missionId);
    this.changedEmitter.fire();
    await this.runAutoFlights();
  }

  async continueMission(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Continue Reviewed Mission");
    if (!mission) return;
    let current = await this.get(mission.mission_id);
    let momentum = this.momentum(current);
    if (momentum.stopped === "specialized mission flow") {
      throw new Error("a choose-one Mission uses Run All Reviewed Attempts and its explicit result comparison");
    }
    if (momentum.stopped) {
      await this.showStoppedMomentum(current, momentum.stopped);
      return;
    }
    if (momentum.manual.length > 0 || !hasMissionMomentumWork(momentum)) {
      await this.showWaitingMomentum(current, momentum);
      return;
    }

    const action = await vscode.window.showWarningMessage(
      `Continue ${current.mission.name} through every currently safe reviewed step?`,
      {
        modal: true,
        detail: momentumConfirmation(current, momentum),
      },
      "Continue reviewed Mission",
    );
    if (action !== "Continue reviewed Mission") return;

    const result = await this.advanceMomentum(current, (tasks) => this.chooseWaveProviders(tasks));
    if (result.providerSelectionCancelled) {
      await this.acceptSnapshot(result.snapshot, true);
      return;
    }
    const remaining = this.momentum(result.snapshot);
    if (remaining.manual.length > 0) {
      await this.showWaitingMomentum(result.snapshot, remaining);
      return;
    }
    if (remaining.stopped && remaining.stopped !== "integrating") {
      await this.showStoppedMomentum(result.snapshot, remaining.stopped);
      return;
    }
    await this.showMomentumResult(result.snapshot, result.verified, result.sessionIds.length);
  }

  async continueReadyMissions(): Promise<void> {
    const deck = await this.currentFlightDeck();
    if (deck.batch.length === 0) {
      await vscode.window.showInformationMessage(flightDeckEmptyMessage(deck));
      return;
    }
    const action = await vscode.window.showWarningMessage(
      `Continue ${deck.batch.length} reviewed Missions through every currently safe step?`,
      {
        modal: true,
        detail: flightDeckConfirmation(deck),
      },
      "Continue reviewed Missions",
    );
    if (action !== "Continue reviewed Missions") return;

    const result = await this.advanceFlightDeck(deck, this.flightDeckProviderChooser());
    await this.showFlightDeckResult(result, deck.batch.length);
  }

  async continueMissionForJourney(
    missionId: string,
    operatorChoiceProvider: string,
  ): Promise<{ snapshot: MissionSnapshot; sessionIds: readonly string[]; verified: number }> {
    const current = await this.get(missionId);
    const momentum = this.momentum(current);
    if (momentum.stopped) {
      throw new Error(`the Mission cannot continue from ${momentum.stopped}`);
    }
    if (momentum.manual.length > 0 || !hasMissionMomentumWork(momentum)) {
      throw new Error("the Mission has no deterministic reviewed step to continue");
    }
    const result = await this.advanceMomentum(
      current,
      (tasks) => Promise.resolve(this.resolveWaveProviders(tasks, operatorChoiceProvider)),
    );
    if (result.providerSelectionCancelled) {
      throw new Error("the journey provider assignment was cancelled");
    }
    return {
      snapshot: result.snapshot,
      sessionIds: result.sessionIds,
      verified: result.verified,
    };
  }

  async continueReadyMissionsForJourney(
    operatorChoiceProvider: string,
  ): Promise<{ missions: number; sessionIds: readonly string[]; verified: number; remainingReady: number }> {
    const deck = await this.currentFlightDeck();
    if (deck.batch.length === 0) {
      throw new Error("no reviewed ordinary Mission has a deterministic step to continue");
    }
    const result = await this.advanceFlightDeck(
      deck,
      (tasks) => Promise.resolve(this.resolveWaveProviders(tasks, operatorChoiceProvider)),
    );
    if (result.providerSelectionCancelled) {
      throw new Error("the Mission Flight Deck provider assignment was cancelled");
    }
    if (result.failures.length > 0) {
      throw new Error(result.failures.map((failure) =>
        `${failure.name} (${failure.project}, ${failure.missionId}): ${failure.message}`
      ).join("; "));
    }
    return {
      missions: result.advanced,
      sessionIds: result.sessionIds,
      verified: result.verified,
      remainingReady: result.remainingReady,
    };
  }

  private async currentFlightDeck(nowUnixMs = Date.now()): Promise<MissionFlightDeck> {
    await this.refresh();
    const active = this.rows.filter((mission) => (
      mission.state === "validated"
      || mission.state === "ready"
      || mission.state === "running"
      || mission.state === "paused"
      || mission.state === "blocked"
      || mission.state === "integrating"
    ));
    // MissionList is bounded at 100. Read every exact snapshot through the command connection's existing serialized
    // ownership instead of trusting an older editor document or Mission row summary.
    const snapshots: MissionSnapshot[] = [];
    for (const mission of active) snapshots.push(await this.get(mission.mission_id));
    return missionFlightDeck(
      snapshots.map((snapshot) => ({ snapshot, momentum: this.momentum(snapshot) })),
      nowUnixMs,
    );
  }

  private async advanceFlightDeck(
    deck: MissionFlightDeck,
    chooseProviders: MissionProviderChooser,
  ): Promise<MissionFlightAdvanceResult> {
    let advanced = 0;
    let verified = 0;
    let providerSelectionCancelled = false;
    let remainingReady = deck.remainingReady.length;
    const sessionIds: string[] = [];
    const failures: MissionFlightFailure[] = [];

    for (const [index, entry] of deck.batch.entries()) {
      try {
        const result = await this.advanceMomentum(entry.snapshot, chooseProviders, false);
        if (result.providerSelectionCancelled) {
          await this.acceptSnapshot(result.snapshot, false);
          providerSelectionCancelled = true;
          remainingReady += deck.batch.length - index;
          break;
        }
        advanced += 1;
        verified += result.verified;
        sessionIds.push(...result.sessionIds);
      } catch (error) {
        failures.push({
          missionId: entry.snapshot.mission.mission_id,
          name: entry.snapshot.mission.name,
          project: entry.snapshot.mission.project,
          message: error instanceof Error ? error.message : String(error),
        });
      }
    }

    await this.refresh();
    await this.placeMissionSessions(sessionIds);
    return {
      advanced,
      verified,
      sessionIds,
      failures,
      providerSelectionCancelled,
      remainingReady,
    };
  }

  private async advanceMomentum(
    initial: MissionSnapshot,
    chooseProviders: MissionProviderChooser,
    placeSessions = true,
    options: MissionAdvanceOptions = {},
  ): Promise<MissionAdvanceResult> {
    let current = initial;
    const exactMomentum = (snapshot: MissionSnapshot): MissionMomentum => {
      const value = this.momentum(snapshot);
      return options.verifyTaskIds
        ? restrictAutoFlightMomentum(value, options.verifyTaskIds)
        : value;
    };
    let momentum = exactMomentum(current);

    let verified = 0;
    if (momentum.start) {
      current = requireResponse((await this.client.once({
        ask: "missionStart",
        with: {
          mission_id: current.mission.mission_id,
          mission_sha256: current.mission_sha256,
        },
      })).response, "mission");
      momentum = exactMomentum(current);
    }

    if (momentum.verify.length > 0) {
      current = await withMissionProgress(
        options.showProgress !== false,
        `Checking finished Tasks in ${current.mission.name}`,
        async (progress) => {
          let snapshot = current;
          for (const task of momentum.verify) {
            if (snapshot.mission.state !== "running") break;
            const exact = snapshot.tasks.find((candidate) => candidate.task_id === task.task_id);
            if (exact?.state !== "running") continue;
            progress.report({
              message: `Running fixed Gates for ${exact.key}`,
              increment: 100 / momentum.verify.length,
            });
            snapshot = requireResponse((await this.client.once({
              ask: "missionVerifyTask",
              with: { mission_id: snapshot.mission.mission_id, task_id: exact.task_id },
            })).response, "mission");
            verified += 1;
            if (this.momentum(snapshot).manual.length > 0) break;
          }
          return snapshot;
        },
      );
    }

    momentum = exactMomentum(current);
    if (momentum.manual.length > 0) {
      await this.acceptSnapshot(current, false);
      return {
        snapshot: current,
        verified,
        sessionIds: [],
        providerSelectionCancelled: false,
      };
    }
    if (momentum.stopped) {
      await this.acceptSnapshot(current, false);
      return {
        snapshot: current,
        verified,
        sessionIds: [],
        providerSelectionCancelled: false,
      };
    }
    const assignments = await chooseProviders(momentum.prepare);
    if (!assignments) {
      return {
        snapshot: current,
        verified,
        sessionIds: [],
        providerSelectionCancelled: true,
      };
    }

    const sentSessions = await withMissionProgress(
      options.showProgress !== false,
      `Starting the next reviewed wave in ${current.mission.name}`,
      async (progress) => {
        let snapshot = current;
        for (const task of momentum.prepare) {
          progress.report({
            message: `Preparing ${task.key}`,
            increment: momentum.prepare.length === 0 ? 0 : 45 / momentum.prepare.length,
          });
          const prepared = await this.prepareTaskSession(
            snapshot.mission.mission_id,
            task,
            requireProviderAssignment(assignments, task),
          );
          snapshot = prepared.snapshot;
        }

        const ready = this.momentum(snapshot).send;
        const instructions = [];
        for (const task of ready) {
          progress.report({
            message: `Rechecking ${task.instruction_ref}`,
            increment: ready.length === 0 ? 0 : 25 / ready.length,
          });
          await this.markSubmissionAmbiguous(task.task_id);
          instructions.push({
            task,
            instruction: requireResponse((await this.client.once({
              ask: "missionSendTaskInstruction",
              with: {
                mission_id: snapshot.mission.mission_id,
                task_id: task.task_id,
                instruction_sha256: task.instruction_sha256,
              },
            })).response, "missionInstruction"),
          });
        }

        progress.report({ message: "Sending exact reviewed instructions", increment: 30 });
        if (instructions.length > 0 && options.beforeSubmissions) {
          const turns = instructions.map(({ task, instruction }) => {
            const session = this.runtimeState.sessions.find(
              (candidate) => candidate.sessionId === instruction.session_id,
            );
            if (!session
              || session.lifecycle !== "hotIdle"
              || (session.waitingOn !== null && session.waitingOn !== undefined)) {
              throw new Error(`Task ${task.key} no longer has an exact idle Runtime session before provider input`);
            }
            return {
              taskId: task.task_id,
              sessionId: session.sessionId,
              sessionGeneration: session.sessionGeneration,
            };
          });
          await options.beforeSubmissions(turns);
        }
        const submissions = await Promise.allSettled(instructions.map(async ({ task, instruction }) => {
          await this.sessions.submitResolvedInput(instruction.session_id, instruction.instruction);
          await this.clearAmbiguousSubmission(task.task_id);
          return instruction.session_id;
        }));
        const sessionIds = submissions.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
        const failed = submissions.length - sessionIds.length;
        current = await this.get(snapshot.mission.mission_id);
        if (failed > 0) {
          await this.acceptSnapshot(current, false);
          throw new Error(
            `${failed} of ${submissions.length} provider submissions are ambiguous; automatic verification is disabled for those Tasks`,
          );
        }
        return sessionIds;
      },
    );

    current = await this.get(current.mission.mission_id);
    await this.acceptSnapshot(current, false);
    if (placeSessions) await this.placeMissionSessions(sentSessions);
    return {
      snapshot: current,
      verified,
      sessionIds: sentSessions,
      providerSelectionCancelled: false,
    };
  }

  async launchFleet(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Run Reviewed Attempts");
    if (!mission) return;
    const reviewed = await this.get(mission.mission_id);
    if (reviewed.mission.state !== "validated" || reviewed.mission.completion_policy !== "chooseOne") {
      throw new Error("only a validated choose-one Mission can launch all reviewed attempts");
    }
    const assignments = new Map<string, string>();
    for (const task of reviewed.tasks) {
      const provider = await this.chooseProvider(task.provider_selector);
      if (!provider) return;
      assignments.set(task.task_id, provider);
    }
    const assignmentText = reviewed.tasks
      .map((task) => `${task.key}: ${assignments.get(task.task_id)}`)
      .join("\n");
    const action = await vscode.window.showWarningMessage(
      `Run all ${reviewed.tasks.length} reviewed attempts for ${reviewed.mission.name}?`,
      {
        modal: true,
        detail: `Mission SHA-256 ${reviewed.mission_sha256}\n${assignmentText}\n\nEvery exact instruction is rechecked, then sent to its prepared session.`,
      },
      "Run all attempts",
    );
    if (action !== "Run all attempts") return;
    await this.runFleet(reviewed, assignments);
  }

  async launchFleetForJourney(missionId: string): Promise<string[]> {
    const reviewed = await this.get(missionId);
    if (reviewed.mission.state !== "validated" || reviewed.mission.completion_policy !== "chooseOne") {
      throw new Error("the journey requires one validated choose-one Mission");
    }
    const assignments = new Map<string, string>();
    for (const task of reviewed.tasks) {
      const provider = await this.chooseProvider(task.provider_selector);
      if (!provider) throw new Error("the reviewed provider selection was cancelled");
      assignments.set(task.task_id, provider);
    }
    return this.runFleet(reviewed, assignments);
  }

  async registerGateForJourney(gateId: string, program: string, arguments_: string[]): Promise<void> {
    requireDone((await this.client.once({
      ask: "missionRegisterGate",
      with: { gate_id: gateId, program, arguments: arguments_, timeout_ms: 30_000 },
    })).response, "Gate registration");
  }

  async verifyTaskForJourney(missionId: string, taskId: string): Promise<MissionSnapshot> {
    const snapshot = requireResponse((await this.client.once({
      ask: "missionVerifyTask",
      with: { mission_id: missionId, task_id: taskId },
    })).response, "mission");
    await this.clearAmbiguousSubmission(taskId);
    await this.acceptSnapshot(snapshot, false);
    return snapshot;
  }

  private async runFleet(
    reviewed: MissionSnapshot,
    assignments: ReadonlyMap<string, string>,
  ): Promise<string[]> {
    return vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: `Running ${reviewed.tasks.length} reviewed attempts`,
        cancellable: false,
      },
      async (progress) => {
        const sessionIds: string[] = [];
        try {
          let current = requireResponse((await this.client.once({
            ask: "missionStart",
            with: {
              mission_id: reviewed.mission.mission_id,
              mission_sha256: reviewed.mission_sha256,
            },
          })).response, "mission");
          for (const [index, task] of reviewed.tasks.entries()) {
            progress.report({ message: `Preparing ${task.key}`, increment: 45 / reviewed.tasks.length });
            const prepared = await this.prepareTaskSession(
              current.mission.mission_id,
              task,
              requireProviderAssignment(assignments, task),
            );
            current = prepared.snapshot;
            sessionIds.push(prepared.sessionId);
          }

          const instructions = [];
          for (const task of reviewed.tasks) {
            progress.report({ message: `Checking ${task.instruction_ref}`, increment: 20 / reviewed.tasks.length });
            await this.markSubmissionAmbiguous(task.task_id);
            instructions.push({
              task,
              instruction: requireResponse((await this.client.once({
                ask: "missionSendTaskInstruction",
                with: {
                  mission_id: reviewed.mission.mission_id,
                  task_id: task.task_id,
                  instruction_sha256: task.instruction_sha256,
                },
              })).response, "missionInstruction"),
            });
          }
          progress.report({ message: "Sending reviewed instructions", increment: 20 });
          const submissions = await Promise.allSettled(
            instructions.map(async ({ task, instruction }) => {
              await this.sessions.submitResolvedInput(instruction.session_id, instruction.instruction);
              await this.clearAmbiguousSubmission(task.task_id);
            }),
          );
          const failed = submissions.filter((result) => result.status === "rejected").length;
          if (failed > 0) {
            throw new Error(`${failed} of ${submissions.length} provider submissions failed; Mission state was kept for explicit recovery`);
          }
          progress.report({ message: "Opening the attempt grid", increment: 15 });
          for (const sessionId of sessionIds) await this.sessions.select(sessionId);
          await this.sessions.arrangeConversationGrid();
          current = await this.get(reviewed.mission.mission_id);
          await this.acceptSnapshot(current, false);
          void vscode.window.showInformationMessage(
            `${sessionIds.length} reviewed attempts are running in isolated worktrees.`,
          );
          return sessionIds;
        } catch (error) {
          await this.refresh().catch(() => undefined);
          throw error;
        }
      },
    );
  }

  async prepareTask(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, ["reserved"], "Prepare Mission Task");
    if (!selected) return;
    const provider = await this.chooseProvider(selected.task.provider_selector);
    if (!provider) return;
    const prepared = await this.prepareTaskSession(selected.mission.mission_id, selected.task, provider);
    await this.acceptSnapshot(prepared.snapshot, true);
  }

  async sendTaskInstruction(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, ["awaitingInput"], "Send Task Instruction");
    if (!selected) return;
    const action = await vscode.window.showWarningMessage(
      `Send ${selected.task.instruction_ref} to its exact Runtime session?`,
      {
        modal: true,
        detail: `Instruction SHA-256 ${selected.task.instruction_sha256}\nThe project file bytes are rechecked before transport.`,
      },
      "Send Task Instruction",
    );
    if (action !== "Send Task Instruction") return;
    await this.markSubmissionAmbiguous(selected.task.task_id);
    const instruction = requireResponse((await this.client.once({
      ask: "missionSendTaskInstruction",
      with: {
        mission_id: selected.mission.mission_id,
        task_id: selected.task.task_id,
        instruction_sha256: selected.task.instruction_sha256,
      },
    })).response, "missionInstruction");
    await this.sessions.submitResolvedInput(instruction.session_id, instruction.instruction);
    await this.clearAmbiguousSubmission(selected.task.task_id);
    await this.openMission({ mission: selected.mission });
  }

  async verifyTask(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, ["running"], "Verify Mission Task");
    if (!selected) return;
    const session = selected.task.session_id
      ? this.runtimeState.sessions.find((candidate) => candidate.sessionId === selected.task.session_id)
      : null;
    if (session?.lifecycle === "hotRunning") {
      throw new Error("the provider turn is still running, so Task evidence cannot be sealed yet");
    }
    const action = await vscode.window.showWarningMessage(
      `Seal declared Artifacts and run ${selected.task.gate_refs.length} fixed Gates?`,
      { modal: true },
      "Verify Task",
    );
    if (action !== "Verify Task") return;
    const snapshot = requireResponse((await this.client.once({
      ask: "missionVerifyTask",
      with: { mission_id: selected.mission.mission_id, task_id: selected.task.task_id },
    })).response, "mission");
    await this.clearAmbiguousSubmission(selected.task.task_id);
    await this.acceptSnapshot(snapshot, true);
  }

  async retryTask(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, ["retryable"], "Retry Mission Task");
    if (!selected) return;
    const action = await vscode.window.showWarningMessage(
      `Prepare one bounded retry for ${selected.task.key}?`,
      { modal: true },
      "Retry Task",
    );
    if (action !== "Retry Task") return;
    const snapshot = requireResponse((await this.client.once({
      ask: "missionRetryTask",
      with: { mission_id: selected.mission.mission_id, task_id: selected.task.task_id },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
  }

  async pauseMission(selection?: MissionSelection): Promise<void> {
    await this.missionAction(selection, "Pause Mission", "missionPause");
  }

  async resumeMission(selection?: MissionSelection): Promise<void> {
    await this.missionAction(selection, "Resume Mission", "missionResumeSafe");
  }

  async cancelMission(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Cancel Mission");
    if (!mission) return;
    const action = await vscode.window.showWarningMessage(
      `Cancel ${mission.name} and release its exact reservations?`,
      { modal: true },
      "Cancel Mission",
    );
    if (action !== "Cancel Mission") return;
    const snapshot = requireResponse((await this.client.once({
      ask: "missionCancel",
      with: { mission_id: mission.mission_id },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
  }

  async reviewMissionLanding(selection?: MissionSelection): Promise<void> {
    await this.landings.reviewAndApply(selection?.mission?.mission_id);
  }

  async completeIntegration(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Complete Mission Integration");
    if (!mission) return;
    const snapshot = await this.get(mission.mission_id);
    if (snapshot.mission.state !== "integrating") {
      throw new Error("the Mission is not waiting for integrated-tree verification");
    }
    let selectedTask: MissionTaskLine | null = null;
    if (snapshot.mission.completion_policy === "chooseOne") {
      selectedTask = selection?.task
        ? snapshot.tasks.find((task) => task.task_id === selection.task?.task_id && task.state === "passed") ?? null
        : null;
      if (!selectedTask) {
        const picked = await vscode.window.showQuickPick(
          snapshot.tasks
            .filter((task) => task.state === "passed")
            .map((task) => ({
              label: task.key,
              description: task.provider_selector,
              detail: `${task.artifact_paths.length} sealed Artifacts  ${task.workspace ?? "workspace unavailable"}`,
              task,
            })),
          {
            title: "Which passing result did you apply to the project?",
            placeHolder: "Final verification uses only this exact Task Receipt",
          },
        );
        if (!picked) return;
        selectedTask = picked.task;
      }
    }
    const action = await vscode.window.showWarningMessage(
      selectedTask
        ? `Verify the project with ${selectedTask.key} and complete ${snapshot.mission.name}?`
        : `Verify the integrated project tree and complete ${snapshot.mission.name}?`,
      {
        modal: true,
        detail: selectedTask
          ? "Current project Artifacts must exactly match the selected passing Task Receipt. Its reviewed Gates run again before completion."
          : "Current project Artifacts must exactly match passing Task Receipts. Every reviewed Gate runs again before completion.",
      },
      "Verify and complete",
    );
    if (action !== "Verify and complete") return;
    await this.projectLeases.run(
      snapshot.mission.project,
      snapshot.mission.mission_id,
      () => this.verifyAndCompleteIntegration(snapshot, selectedTask),
    );
  }

  async reviewMissionLandingForJourney(missionId: string): Promise<void> {
    await this.landings.reviewForJourney(missionId);
  }

  async applyMissionLandingForJourney(missionId: string): Promise<MissionSnapshot> {
    return this.landings.applyForJourney(missionId);
  }

  async compareResults(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Compare Passing Results");
    if (!mission) return;
    const snapshot = await this.get(mission.mission_id);
    if (snapshot.mission.completion_policy !== "chooseOne") {
      throw new Error("result comparison is available for choose-one Missions");
    }
    const passed = snapshot.tasks.filter((task) =>
      task.state === "passed" && task.workspace !== null && task.artifact_paths.length > 0
    );
    if (passed.length === 0) throw new Error("no passing result has sealed Artifact paths to compare");
    const paths = [...new Set(passed.flatMap((task) => task.artifact_paths))].sort();
    const artifact = paths.length === 1
      ? paths[0]
      : (await vscode.window.showQuickPick(
        paths.map((candidate) => ({
          label: candidate,
          description: `${passed.filter((task) => task.artifact_paths.includes(candidate)).length} passing results`,
          artifact: candidate,
        })),
        { title: "Compare one declared Artifact across passing results" },
      ))?.artifact;
    if (!artifact) return;
    if (!safeArtifactPath(artifact)) throw new Error("Core returned an unsafe Artifact path");
    const candidates = passed.filter((task) => task.artifact_paths.includes(artifact));
    const projectUri = fileUnder(snapshot.mission.project, artifact);
    const projectExists = await vscode.workspace.fs.stat(projectUri).then(
      (stat) => (stat.type & vscode.FileType.File) !== 0,
      () => false,
    );
    const baseline = projectExists ? projectUri : fileUnder(candidates[0].workspace as string, artifact);
    const comparisons = projectExists ? candidates : candidates.slice(1);
    if (comparisons.length === 0) {
      await vscode.window.showTextDocument(baseline, { preview: false });
      await vscode.window.showInformationMessage(
        `${artifact} is new and only one passing attempt produced it, so its file was opened directly.`,
      );
      return;
    }
    for (const [index, task] of comparisons.entries()) {
      await vscode.commands.executeCommand(
        "vscode.diff",
        baseline,
        fileUnder(task.workspace as string, artifact),
        `${task.key}: ${artifact}`,
        {
          preview: false,
          preserveFocus: index + 1 < comparisons.length,
          viewColumn: index + 1,
        },
      );
    }
  }

  private async verifyAndCompleteIntegration(
    snapshot: MissionSnapshot,
    selectedTask: MissionTaskLine | null,
  ): Promise<MissionSnapshot> {
    const completed = requireResponse((await this.client.once({
      ask: "missionCompleteIntegration",
      with: { mission_id: snapshot.mission.mission_id, task_id: selectedTask?.task_id ?? null },
    })).response, "mission");
    await this.acceptSnapshot(completed, true);
    return completed;
  }

  async archiveMission(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Archive Mission");
    if (!mission) return;
    if (!["completed", "failed", "cancelled"].includes(mission.state)) {
      throw new Error("only a completed, failed, or cancelled Mission can be archived");
    }
    const action = await vscode.window.showWarningMessage(
      `Compact ${mission.name} into immutable local history?`,
      { modal: true },
      "Archive Mission",
    );
    if (action !== "Archive Mission") return;
    const snapshot = requireResponse((await this.client.once({
      ask: "missionArchive",
      with: { mission_id: mission.mission_id },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
  }

  async openTaskSession(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, [], "Open Task Session");
    if (!selected?.task.session_id) {
      throw new Error("the Task has no prepared Runtime session");
    }
    await this.sessions.select(selected.task.session_id);
  }

  dispose(): void {
    this.disposed = true;
    this.autoFlightPending = false;
    this.autoFlightSubscription.dispose();
    this.landings.dispose();
    this.projectLeases.clear();
    this.documents.dispose();
    this.changedEmitter.dispose();
  }

  private async missionAction(
    selection: MissionSelection | undefined,
    title: string,
    ask: "missionPause" | "missionResumeSafe",
  ): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission(title);
    if (!mission) return;
    const snapshot = requireResponse((await this.client.once({
      ask,
      with: { mission_id: mission.mission_id },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
  }

  private async resolveTask(
    selection: MissionSelection | undefined,
    states: readonly string[],
    title: string,
  ): Promise<{ mission: MissionLine; task: MissionTaskLine } | null> {
    const mission = selection?.mission ?? await this.pickMission(title);
    if (!mission) return null;
    const snapshot = await this.get(mission.mission_id);
    const selectedTask = selection?.task
      ? snapshot.tasks.find((task) => task.task_id === selection.task?.task_id)
      : undefined;
    if (selectedTask && (states.length === 0 || states.includes(selectedTask.state))) {
      return { mission: snapshot.mission, task: selectedTask };
    }
    const candidates = snapshot.tasks.filter((task) => states.length === 0 || states.includes(task.state));
    const task = await vscode.window.showQuickPick(
      candidates.map((candidate) => ({
        label: candidate.key,
        description: candidate.state,
        detail: `${candidate.workspace_mode}  ${candidate.instruction_ref}`,
        task: candidate,
      })),
      { title, placeHolder: candidates.length ? "Select one exact Task" : "No Task is available for this action" },
    );
    return task ? { mission: snapshot.mission, task: task.task } : null;
  }

  private async chooseProvider(selector: string): Promise<string | null> {
    const usable = this.runtimeState.providers.filter((provider) => provider.installation.state === "usable");
    if (selector !== "operatorChoice") {
      if (!usable.some((provider) => provider.providerId === selector)) {
        throw new Error(`the reviewed provider ${selector} is not currently usable`);
      }
      return selector;
    }
    const selected = await vscode.window.showQuickPick(
      usable.map((provider) => ({ label: provider.displayName, description: provider.providerId, id: provider.providerId })),
      { title: "Provider for Mission Task", placeHolder: "Select a runtime-discovered provider" },
    );
    return selected?.id ?? null;
  }

  private async chooseWaveProviders(
    tasks: readonly MissionTaskLine[],
  ): Promise<ReadonlyMap<string, string> | null> {
    const assignments = new Map<string, string>();
    const undecided: MissionTaskLine[] = [];
    for (const task of tasks) {
      if (task.provider_selector === "operatorChoice") {
        undecided.push(task);
        continue;
      }
      const provider = await this.chooseProvider(task.provider_selector);
      if (!provider) return null;
      assignments.set(task.task_id, provider);
    }
    if (undecided.length === 0) return assignments;
    if (undecided.length === 1) {
      const provider = await this.chooseProvider("operatorChoice");
      if (!provider) return null;
      assignments.set(undecided[0].task_id, provider);
      return assignments;
    }

    const usable = this.runtimeState.providers.filter((provider) => provider.installation.state === "usable");
    const individual = { label: "Assign each Task separately", description: "Choose per reviewed Task", id: null };
    const selected = await vscode.window.showQuickPick(
      [
        ...usable.map((provider) => ({
          label: provider.displayName,
          description: provider.providerId,
          detail: `Use for all ${undecided.length} ready Tasks`,
          id: provider.providerId as string | null,
        })),
        individual,
      ],
      {
        title: `Provider for ${undecided.length} ready Mission Tasks`,
        placeHolder: "Use one runtime-discovered provider for this wave, or assign Tasks individually",
      },
    );
    if (!selected) return null;
    if (selected.id) {
      for (const task of undecided) assignments.set(task.task_id, selected.id);
      return assignments;
    }
    for (const task of undecided) {
      const provider = await this.chooseProvider("operatorChoice");
      if (!provider) return null;
      assignments.set(task.task_id, provider);
    }
    return assignments;
  }

  private flightDeckProviderChooser(): MissionProviderChooser {
    let commonProvider: string | undefined;
    let individual = false;
    return async (tasks) => {
      if (individual || !tasks.some((task) => task.provider_selector === "operatorChoice")) {
        return this.chooseWaveProviders(tasks);
      }
      if (commonProvider) return this.resolveWaveProviders(tasks, commonProvider);
      const usable = this.runtimeState.providers.filter((provider) => provider.installation.state === "usable");
      const selected = await vscode.window.showQuickPick(
        [
          ...usable.map((provider) => ({
            label: provider.displayName,
            description: provider.providerId,
            detail: "Use for every operator-choice Task in this flight",
            providerId: provider.providerId as string | null,
          })),
          {
            label: "Assign each Task separately",
            description: "Keep provider choices independent",
            providerId: null,
          },
        ],
        { title: "Provider for this Mission flight" },
      );
      if (!selected) return null;
      if (selected.providerId === null) {
        individual = true;
        return this.chooseWaveProviders(tasks);
      }
      commonProvider = selected.providerId;
      return this.resolveWaveProviders(tasks, commonProvider);
    };
  }

  private async placeMissionSessions(sessionIds: readonly string[]): Promise<void> {
    for (const sessionId of sessionIds) await this.sessions.select(sessionId);
    if (sessionIds.length > 1) await this.sessions.arrangeConversationGrid();
  }

  private resolveWaveProviders(
    tasks: readonly MissionTaskLine[],
    operatorChoiceProvider: string | null,
  ): ReadonlyMap<string, string> {
    const usable = new Set(
      this.runtimeState.providers
        .filter((provider) => provider.installation.state === "usable")
        .map((provider) => provider.providerId),
    );
    if (tasks.some((task) => task.provider_selector === "operatorChoice")
      && (!operatorChoiceProvider || !usable.has(operatorChoiceProvider))) {
      throw new Error(`the selected provider ${operatorChoiceProvider} is not currently usable`);
    }
    return new Map(tasks.map((task) => {
      const provider = task.provider_selector === "operatorChoice"
        ? operatorChoiceProvider as string
        : task.provider_selector;
      if (!usable.has(provider)) {
        throw new Error(`the reviewed provider ${provider} is not currently usable`);
      }
      return [task.task_id, provider];
    }));
  }

  private scheduleAutoFlights(): void {
    if (!this.autoFlightsStarted || this.disposed || !this.hasLiveAutoFlight()) return;
    void this.runAutoFlights().catch((error: unknown) => {
      if (this.disposed) return;
      void vscode.window.showWarningMessage(
        `Mission Auto Flight stopped locally: ${error instanceof Error ? error.message : String(error)}`,
      );
    });
  }

  private runAutoFlights(): Promise<void> {
    if (!this.autoFlightsStarted || this.disposed || !this.hasLiveAutoFlight()) return Promise.resolve();
    this.autoFlightPending = true;
    if (this.autoFlightRun) return this.autoFlightRun;
    const running = this.drainAutoFlights();
    this.autoFlightRun = running;
    void running.then(
      () => this.finishAutoFlightRun(running, true),
      () => this.finishAutoFlightRun(running, false),
    );
    return running;
  }

  private finishAutoFlightRun(running: Promise<void>, continuePending: boolean): void {
    if (this.autoFlightRun !== running) return;
    this.autoFlightRun = null;
    if (!continuePending) this.autoFlightPending = false;
    if (continuePending && this.autoFlightPending) this.scheduleAutoFlights();
  }

  private async drainAutoFlights(): Promise<void> {
    while (this.autoFlightPending && this.hasLiveAutoFlight() && !this.disposed) {
      this.autoFlightPending = false;
      for (const listed of this.autoFlights.current()) {
        if (this.autoFlightRevocations.has(listed.missionId) && !listed.pendingSignal) continue;
        const arm = this.autoFlights.get(listed.missionId);
        if (!arm) continue;
        try {
          const snapshot = await this.get(arm.missionId);
          const decision = decideAutoFlight(
            arm,
            snapshot,
            this.momentum(snapshot),
            this.runtimeState.sessions,
            Date.now(),
          );
          if (decision.kind === "signal") {
            await this.deliverAutoFlightSignal(arm, decision.signal);
            continue;
          }
          if (decision.kind === "discard") {
            await this.finishAutoFlightSignal(arm.missionId);
            continue;
          }
          if (decision.kind === "wait") {
            await this.autoFlights.reconcile(arm.missionId, snapshot);
            continue;
          }
          if (decision.kind === "arrived") {
            await this.arriveAutoFlight(snapshot);
            continue;
          }
          if (decision.kind === "disarm") {
            await this.stopAutoFlight(snapshot.mission.name, arm.missionId, decision.reason);
            continue;
          }

          const verifyTaskIds = new Set(decision.momentum.verify.map((task) => task.task_id));
          const result = await this.advanceMomentum(
            snapshot,
            (tasks) => Promise.resolve(this.resolveWaveProviders(tasks, arm.operatorChoiceProvider)),
            false,
            {
              showProgress: false,
              verifyTaskIds,
              beforeSubmissions: async (submissions) => {
                await this.autoFlights.recordSubmissions(arm.missionId, submissions);
                if (!this.isAutoFlightArmed(arm.missionId)) {
                  throw new Error("the operator disarmed Auto Flight before provider input");
                }
              },
            },
          );
          await this.autoFlights.reconcile(arm.missionId, result.snapshot);
          this.changedEmitter.fire();
          this.autoFlightPending = true;
        } catch (error) {
          await this.stopAutoFlight(
            this.rows.find((mission) => mission.mission_id === arm.missionId)?.name ?? arm.missionId,
            arm.missionId,
            error instanceof Error ? error.message : String(error),
          );
        }
      }
    }
  }

  private hasLiveAutoFlight(): boolean {
    return this.autoFlights.current().some((arm) => (
      arm.pendingSignal !== null || !this.autoFlightRevocations.has(arm.missionId)
    ));
  }

  private async stopAutoFlight(name: string, missionId: string, reason: string): Promise<void> {
    this.autoFlightRevocations.add(missionId);
    const arm = this.autoFlights.get(missionId);
    if (arm && !arm.pendingSignal) {
      await this.autoFlights.stageSignal(missionId, { signalId: randomUUID(), kind: "stopped" });
      this.autoFlightPending = true;
    }
    this.changedEmitter.fire();
    void vscode.window.showWarningMessage(`Auto Flight stopped for ${name}: ${reason}.`);
  }

  private async arriveAutoFlight(snapshot: MissionSnapshot): Promise<void> {
    const missionId = snapshot.mission.mission_id;
    this.autoFlightRevocations.add(missionId);
    const arm = this.autoFlights.get(missionId);
    if (arm && !arm.pendingSignal) {
      await this.autoFlights.stageSignal(missionId, { signalId: randomUUID(), kind: "landing" });
      this.autoFlightPending = true;
    }
    this.changedEmitter.fire();
    void vscode.window.showInformationMessage(
      `${snapshot.mission.name} Auto Flight arrived at explicit Receipt Landing.`,
      "Review Landing",
    ).then((action) => {
      if (action === "Review Landing") {
        void vscode.commands.executeCommand("runtrol.reviewMissionLanding", { mission: snapshot.mission });
      }
    }, () => undefined);
  }

  private async deliverAutoFlightSignal(
    arm: AutoFlightArm,
    signal: AutoFlightSignal,
  ): Promise<void> {
    requireResponse((await this.client.once({
      ask: "missionFlightSignal",
      with: {
        signal_id: signal.signalId,
        mission_id: arm.missionId,
        mission_sha256: arm.missionSha256,
        kind: signal.kind,
      },
    })).response, "missionFlightSignalRecorded");
    await this.finishAutoFlightSignal(arm.missionId);
  }

  private async finishAutoFlightSignal(missionId: string): Promise<void> {
    this.autoFlightRevocations.add(missionId);
    await this.autoFlights.disarm(missionId);
    this.changedEmitter.fire();
  }

  private async clearMissionFlightSignals(missionId: string, missionSha256: string): Promise<void> {
    requireDone((await this.client.once({
      ask: "missionFlightSignalClear",
      with: { mission_id: missionId, mission_sha256: missionSha256 },
    })).response, "clearing prior Mission Flight Signals");
  }

  private markSubmissionAmbiguous(taskId: string): Promise<void> {
    return this.ambiguousTaskSubmissions.mark(taskId);
  }

  private clearAmbiguousSubmission(taskId: string): Promise<void> {
    return this.ambiguousTaskSubmissions.clear(taskId);
  }

  private momentum(snapshot: MissionSnapshot): MissionMomentum {
    return missionMomentum(snapshot, this.runtimeState.sessions, this.ambiguousTaskSubmissions.current());
  }

  private async showStoppedMomentum(snapshot: MissionSnapshot, reason: string): Promise<void> {
    if (reason === "integrating") {
      await vscode.window.showInformationMessage(
        `${snapshot.mission.name} is ready for explicit project integration and final Gate verification.`,
      );
      return;
    }
    await vscode.window.showInformationMessage(
      `${snapshot.mission.name} is ${reason}; no reviewed Task wave can advance from that state.`,
    );
  }

  private async showWaitingMomentum(snapshot: MissionSnapshot, momentum: MissionMomentum): Promise<void> {
    if (momentum.manual.length > 0) {
      await vscode.window.showWarningMessage(
        `${momentum.manual.length} Tasks require explicit retry or recovery: ${momentum.manual.map((task) => task.key).join(", ")}.`,
      );
      return;
    }
    if (momentum.waiting.length > 0) {
      await vscode.window.showInformationMessage(
        `${momentum.waiting.length} Tasks are still working or waiting. Continue again when their exact sessions are Ready.`,
      );
      return;
    }
    await vscode.window.showInformationMessage(
      `${snapshot.mission.name} has no deterministic next step. Open its review for the exact state.`,
    );
  }

  private async showMomentumResult(snapshot: MissionSnapshot, verified: number, sent: number): Promise<void> {
    const changes = [
      verified > 0 ? `${verified} finished Tasks sealed` : "",
      sent > 0 ? `${sent} reviewed Tasks started` : "",
    ].filter(Boolean).join("; ");
    const prefix = changes || "The reviewed Mission advanced";
    const suffix = snapshot.mission.state === "integrating"
      ? " It is ready for explicit project integration."
      : "";
    await vscode.window.showInformationMessage(`${prefix}.${suffix}`);
  }

  private async showFlightDeckResult(result: MissionFlightAdvanceResult, selected: number): Promise<void> {
    const summary = [
      `${result.advanced} of ${selected} reviewed Missions advanced`,
      `${result.verified} finished Tasks sealed`,
      `${result.sessionIds.length} reviewed Tasks started`,
    ].join("; ");
    const suffix = result.remainingReady > 0
      ? ` ${result.remainingReady} ready Missions remain for the next review.`
      : "";
    if (result.failures.length > 0) {
      await vscode.window.showWarningMessage(
        `${summary}. ${result.failures.length} Missions stopped at an explicit failure.${suffix}`,
        {
          modal: true,
          detail: result.failures.map((failure) =>
            `${failure.name} (${failure.project}, ${failure.missionId}): ${failure.message}`
          ).join("\n"),
        },
      );
      return;
    }
    if (result.providerSelectionCancelled) {
      await vscode.window.showInformationMessage(`${summary}. Provider assignment was cancelled.${suffix}`);
      return;
    }
    await vscode.window.showInformationMessage(`${summary}.${suffix}`);
  }

  private async prepareTaskSession(
    missionId: string,
    task: MissionTaskLine,
    provider: string,
  ): Promise<{ snapshot: MissionSnapshot; sessionId: string }> {
    const workspace = requireResponse((await this.client.once({
      ask: "missionPrepareTask",
      with: { mission_id: missionId, task_id: task.task_id },
    })).response, "missionWorkspace");
    const access = task.workspace_mode === "readOnlyBase" ? "shared" : "exclusive";
    const sessionId = await this.sessions.startResolvedSession(
      provider,
      workspace.workspace,
      null,
      null,
      access,
      false,
    );
    const session = this.runtimeState.sessions.find((candidate) => candidate.sessionId === sessionId);
    if (!session) {
      throw new Error("Runtime started the Task session but did not list its exact descriptor");
    }
    const snapshot = requireResponse((await this.client.once({
      ask: "missionBindSession",
      with: {
        mission_id: missionId,
        task_id: task.task_id,
        session_id: session.sessionId,
        provider_runtime_id: session.providerId,
        native_session_id: session.nativeSessionId ?? null,
        workspace: session.workspace,
      },
    })).response, "mission");
    return { snapshot, sessionId };
  }

  private async pickMission(title: string): Promise<MissionLine | null> {
    await this.refresh();
    const selected = await vscode.window.showQuickPick(
      this.rows.map((mission) => ({
        label: mission.name,
        description: mission.state,
        detail: `${mission.passed_tasks}/${mission.total_tasks}  ${mission.project}`,
        mission,
      })),
      { title, placeHolder: this.rows.length ? "Select a Mission" : "No validated Missions" },
    );
    return selected?.mission ?? null;
  }

  private async get(missionId: string): Promise<MissionSnapshot> {
    const snapshot = requireResponse((await this.client.once({
      ask: "missionGet",
      with: { mission_id: missionId },
    })).response, "mission");
    this.snapshots.set(missionId, snapshot);
    return snapshot;
  }

  private async acceptSnapshot(snapshot: MissionSnapshot, show: boolean): Promise<void> {
    this.snapshots.set(snapshot.mission.mission_id, snapshot);
    this.documents.update(snapshot);
    await this.refresh();
    this.scheduleAutoFlights();
    if (show) await this.documents.show(snapshot.mission.mission_id);
  }
}

type MissionProgress = vscode.Progress<{ readonly message?: string; readonly increment?: number }>;

async function withMissionProgress<T>(
  visible: boolean,
  title: string,
  work: (progress: MissionProgress) => Promise<T>,
): Promise<T> {
  if (!visible) return work({ report: () => undefined });
  return await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title,
      cancellable: false,
    },
    work,
  );
}

class MissionDocumentProvider implements vscode.TextDocumentContentProvider, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<vscode.Uri>();
  private readonly snapshots = new Map<string, MissionSnapshot>();
  readonly onDidChange = this.changedEmitter.event;

  provideTextDocumentContent(uri: vscode.Uri): string {
    const id = decodeURIComponent(uri.path.slice(1));
    const snapshot = this.snapshots.get(id);
    return snapshot ? missionDocument(snapshot) : "# Mission\n\nThe Mission snapshot is unavailable.\n";
  }

  update(snapshot: MissionSnapshot): void {
    this.snapshots.set(snapshot.mission.mission_id, snapshot);
    this.changedEmitter.fire(this.uri(snapshot.mission.mission_id));
  }

  async show(missionId: string): Promise<void> {
    const opened = await vscode.workspace.openTextDocument(this.uri(missionId));
    const document = await vscode.languages.setTextDocumentLanguage(opened, "markdown");
    await vscode.window.showTextDocument(document, { preview: false, preserveFocus: false });
  }

  dispose(): void {
    this.snapshots.clear();
    this.changedEmitter.dispose();
  }

  private uri(missionId: string): vscode.Uri {
    return vscode.Uri.parse(`runtrol-mission:/${encodeURIComponent(missionId)}`);
  }
}

function missionDocument(snapshot: MissionSnapshot): string {
  const lines = [
    "# Mission",
    "",
    `Name: ${inline(snapshot.mission.name)}`,
    "",
    `State: ${inline(snapshot.mission.state)}`,
    "",
    `Completion policy: ${inline(snapshot.mission.completion_policy)}`,
    "",
    `Project: ${inline(snapshot.mission.project)}`,
    "",
    `Source: ${inline(snapshot.mission_ref)}`,
    "",
    `Mission SHA-256: ${inline(snapshot.mission_sha256)}`,
    "",
    `Policy SHA-256: ${inline(snapshot.policy_sha256)}`,
    "",
    `Start approval expires: ${inline(new Date(snapshot.approval_expires_unix_ms).toISOString())}`,
    "",
    `Progress: ${snapshot.mission.passed_tasks}/${snapshot.mission.total_tasks}`,
    "",
    "## Tasks",
    "",
  ];
  for (const task of snapshot.tasks) {
    lines.push(
      `### ${inline(task.key)}`,
      "",
      `State: ${inline(task.state)}`,
      "",
      `Instruction: ${inline(task.instruction_ref)}  SHA-256 ${inline(task.instruction_sha256)}`,
      "",
      `Workspace: ${inline(task.workspace ?? task.workspace_mode)}`,
      "",
      `Base: ${inline(task.base_commit ?? "not prepared")}`,
      "",
      `Provider: ${inline(task.provider_selector)}`,
      "",
      `Outputs: ${task.output_roots.map(inline).join(", ")}`,
      "",
      `Artifacts: ${task.artifact_paths.map(inline).join(", ") || "not sealed"}`,
      "",
      `Gates: ${task.gate_refs.map(inline).join(", ")}  Passed ${task.passed_gates}  Failed ${task.failed_gates}`,
      "",
      `Capabilities: ${task.capability_versions.map((capability) => `${inline(capability.capability_id)} at ${inline(capability.version_sha256)}`).join(", ") || "none"}`,
      "",
      `Receipt: ${inline(task.receipt_id ?? "not sealed")}`,
      "",
      `Run: ${inline(task.run_id ?? "not sealed")}`,
      "",
    );
  }
  lines.push(
    "## Boundary",
    "",
    "Task instructions remain project files. This view contains metadata only. Each instruction requires its exact local Send action.",
    "",
  );
  return lines.join("\n");
}

function inline(value: string): string {
  return `\`${value.replaceAll("`", "'").replaceAll("\r", " ").replaceAll("\n", " ")}\``;
}

function requireProviderAssignment(
  assignments: ReadonlyMap<string, string>,
  task: MissionTaskLine,
): string {
  const provider = assignments.get(task.task_id);
  if (!provider) throw new Error(`no provider was assigned to ${task.key}`);
  return provider;
}

function momentumConfirmation(snapshot: MissionSnapshot, momentum: MissionMomentum): string {
  const actions = momentum.start
    ? `Start the Mission and send its first eligible wave from ${snapshot.tasks.length} reviewed Tasks.`
    : [
      momentum.verify.length > 0 ? `Seal ${momentum.verify.length} exact idle Tasks with their fixed Gates.` : "",
      momentum.prepare.length > 0 ? `Prepare ${momentum.prepare.length} newly eligible Tasks.` : "",
      momentum.send.length > 0 ? `Send ${momentum.send.length} already prepared exact instructions.` : "",
    ].filter(Boolean).join("\n");
  return [
    `Mission SHA-256 ${snapshot.mission_sha256}`,
    actions,
    "Running, waiting, failed, retryable, and integration boundaries stay explicit.",
  ].join("\n\n");
}

function flightDeckEmptyMessage(deck: MissionFlightDeck): string {
  return [
    "No reviewed ordinary Mission has a safe deterministic step now.",
    `${deck.waiting.length} are working or waiting; ${deck.manual.length} require review or recovery; ${deck.stopped.length} use a specialized or stopped flow.`,
  ].join(" ");
}

function flightDeckConfirmation(deck: MissionFlightDeck): string {
  const missions = deck.batch.map((entry, index) => [
    `${index + 1}. ${entry.snapshot.mission.name}`,
    `Project ${entry.snapshot.mission.project}`,
    `Mission SHA-256 ${entry.snapshot.mission_sha256}`,
    `Safe now: ${flightDeckAction(entry.momentum)}`,
  ].join("\n")).join("\n\n");
  const outside = [
    deck.remainingReady.length > 0 ? `${deck.remainingReady.length} additional ready Missions stay queued.` : "",
    deck.waiting.length > 0 ? `${deck.waiting.length} Missions are working or waiting.` : "",
    deck.manual.length > 0 ? `${deck.manual.length} Missions require individual review or recovery.` : "",
    deck.stopped.length > 0 ? `${deck.stopped.length} Missions use a specialized or stopped flow.` : "",
  ].filter(Boolean).join("\n");
  return [
    missions,
    outside,
    "Operator-choice Tasks ask once for a shared discovered provider, with individual assignment still available.",
    "Working, waiting, failed, retryable, ambiguous, comparison, and integration boundaries stay explicit.",
  ].filter(Boolean).join("\n\n");
}

function flightDeckAction(momentum: MissionMomentum): string {
  if (momentum.start) return "start the Mission and send its first eligible wave";
  return [
    momentum.verify.length > 0 ? `seal ${momentum.verify.length} finished Tasks` : "",
    momentum.prepare.length > 0 ? `prepare ${momentum.prepare.length} newly eligible Tasks` : "",
    momentum.send.length > 0 ? `send ${momentum.send.length} reviewed instructions` : "",
  ].filter(Boolean).join("; ");
}

function fileUnder(root: string, relative: string): vscode.Uri {
  return vscode.Uri.file(path.join(root, ...relative.split(/[\\/]/u)));
}

async function requiredInput(title: string, prompt: string, value?: string): Promise<string | null> {
  const entered = await vscode.window.showInputBox({ title, prompt, value, ignoreFocusOut: true });
  const normalized = entered?.trim();
  return normalized ? normalized : null;
}

function requireDone(response: Response, operation: string): void {
  if (response.say === "failed") throw new Error(response.with.message);
  if (response.say !== "done") throw new Error(`${operation} returned ${response.say}`);
}

type ResponseWithBody = Exclude<Response, { say: "done" } | { say: "failed" }>;
type ResponsePayload<S extends ResponseWithBody["say"]> =
  Extract<ResponseWithBody, { say: S }> extends { with: infer Payload } ? Payload : never;

function requireResponse<S extends ResponseWithBody["say"]>(
  response: Response,
  say: S,
): ResponsePayload<S> {
  if (response.say === "failed") throw new Error(response.with.message);
  if (response.say !== say) throw new Error(`Core returned ${response.say}, expected ${say}`);
  return (response as { say: S; with: ResponsePayload<S> }).with;
}
