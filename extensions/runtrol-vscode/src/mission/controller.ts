import * as path from "node:path";

import * as vscode from "vscode";

import type { Controller } from "../controller";
import type { CoreClient } from "../core/client";
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
import { branchFromHead, gitdirTarget } from "./gitHead";

/// Where each project's last fan-out shape is remembered (output roots only, never instruction text).
const FAN_OUT_DEFAULTS_KEY = "runtrol.fanOutDefaults";

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

export class MissionController implements vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  private readonly documents = new MissionDocumentProvider();
  private rows: readonly MissionLine[] = [];
  private readonly snapshots = new Map<string, MissionSnapshot>();

  readonly onDidChange = this.changedEmitter.event;

  constructor(
    private readonly client: CoreClient,
    private readonly sessions: Controller,
    private readonly runtimeState: RuntimeState,
    private readonly context: vscode.ExtensionContext,
  ) {}

  get missions(): readonly MissionLine[] {
    return this.rows;
  }

  documentProvider(): vscode.TextDocumentContentProvider {
    return this.documents;
  }

  async initialize(): Promise<void> {
    await this.refresh();
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
  /// The extension writes exactly two things to disk (the selected-session scalar and the managed Core), and an
  /// instruction is the prompt an operator gives an agent. Writing that would make this surface a place where
  /// conversation starts living, which is the boundary the writer contract exists to hold.
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
    const instruction = new TextDecoder().decode(bytes);
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
    const projectKey = workspaceIdentity(folder.uri.fsPath);
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
    await this.rememberFanOutRoots(projectKey, outputRoots);

    const document = await vscode.workspace.openTextDocument({
      language: "toml",
      content: missionSpec(fanOutName(instruction, new Date()), instructionDigest(instruction), {
        instruction,
        instructionRef,
        branches: Number(branches),
        gateId,
        baseRef: baseRef.trim(),
        outputRoots,
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

  private async rememberFanOutRoots(projectKey: string, outputRoots: string[]): Promise<void> {
    const stored = this.context.globalState.get(FAN_OUT_DEFAULTS_KEY);
    const record = stored !== null && typeof stored === "object" && !Array.isArray(stored)
      ? { ...(stored as Record<string, unknown>) }
      : {};
    record[projectKey] = { outputRoots };
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

  async prepareTask(selection?: MissionSelection): Promise<void> {
    const selected = await this.resolveTask(selection, ["reserved"], "Prepare Mission Task");
    if (!selected) return;
    const workspace = requireResponse((await this.client.once({
      ask: "missionPrepareTask",
      with: { mission_id: selected.mission.mission_id, task_id: selected.task.task_id },
    })).response, "missionWorkspace");
    const provider = await this.chooseProvider(selected.task.provider_selector);
    if (!provider) return;
    const access = selected.task.workspace_mode === "readOnlyBase" ? "shared" : "exclusive";
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
        mission_id: selected.mission.mission_id,
        task_id: selected.task.task_id,
        session_id: session.sessionId,
        provider_runtime_id: session.providerId,
        native_session_id: session.nativeSessionId ?? null,
        workspace: session.workspace,
      },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
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
    const instruction = requireResponse((await this.client.once({
      ask: "missionSendTaskInstruction",
      with: {
        mission_id: selected.mission.mission_id,
        task_id: selected.task.task_id,
        instruction_sha256: selected.task.instruction_sha256,
      },
    })).response, "missionInstruction");
    await this.sessions.submitResolvedInput(instruction.session_id, instruction.instruction);
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

  async completeIntegration(selection?: MissionSelection): Promise<void> {
    const mission = selection?.mission ?? await this.pickMission("Complete Mission Integration");
    if (!mission) return;
    if (mission.state !== "integrating") {
      throw new Error("the Mission is not waiting for integrated-tree verification");
    }
    const action = await vscode.window.showWarningMessage(
      `Verify the integrated project tree and complete ${mission.name}?`,
      {
        modal: true,
        detail: "Current project Artifacts must exactly match passing Task Receipts. Every reviewed Gate runs again before completion.",
      },
      "Verify and complete",
    );
    if (action !== "Verify and complete") return;
    const snapshot = requireResponse((await this.client.once({
      ask: "missionCompleteIntegration",
      with: { mission_id: mission.mission_id },
    })).response, "mission");
    await this.acceptSnapshot(snapshot, true);
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
    if (show) await this.documents.show(snapshot.mission.mission_id);
  }
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
