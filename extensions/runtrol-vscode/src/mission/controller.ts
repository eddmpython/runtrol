import * as path from "node:path";

import * as vscode from "vscode";

import type { Controller } from "../controller";
import type { CoreClient } from "../core/client";
import type {
  MissionLine,
  MissionSnapshot,
  MissionTaskLine,
  Response,
} from "../protocol";
import type { RuntimeState } from "../state";

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
    const response = (await this.client.once({ ask: "missionList" })).response;
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

  async registerGate(): Promise<void> {
    const gateId = await requiredInput("Register deterministic Gate", "Stable Gate ID");
    if (!gateId) return;
    const program = await requiredInput("Register deterministic Gate", "Executable name, without shell text");
    if (!program) return;
    const argumentsText = await vscode.window.showInputBox({
      title: "Register deterministic Gate",
      prompt: "Fixed argument vector as a JSON string array",
      value: "[]",
      ignoreFocusOut: true,
    });
    if (argumentsText === undefined) return;
    const parsed: unknown = JSON.parse(argumentsText);
    if (!Array.isArray(parsed) || !parsed.every((value) => typeof value === "string")) {
      throw new Error("Gate arguments must be a JSON string array");
    }
    const timeoutText = await requiredInput("Register deterministic Gate", "Hard timeout in milliseconds", "60000");
    if (!timeoutText) return;
    const timeout = Number(timeoutText);
    if (!Number.isSafeInteger(timeout) || timeout <= 0) {
      throw new Error("the Gate timeout must be a positive integer");
    }
    requireDone((await this.client.once({
      ask: "missionRegisterGate",
      with: { gate_id: gateId, program, arguments: parsed, timeout_ms: timeout },
    })).response, "Gate registration");
    await vscode.window.showInformationMessage(`Registered Gate ${gateId}.`);
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
      `Receipt: ${inline(task.receipt_id ?? "not sealed")}`,
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
