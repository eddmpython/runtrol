import * as vscode from "vscode";

import type { MissionLine, MissionTaskLine } from "../protocol";
import { MissionController, type MissionSelection } from "./controller";

type MissionNode = MissionItem | MissionTaskItem;

export class MissionItem extends vscode.TreeItem implements MissionSelection {
  readonly mission: MissionLine;

  constructor(mission: MissionLine) {
    super(mission.name, vscode.TreeItemCollapsibleState.Collapsed);
    this.mission = mission;
    this.description = `${mission.state}  ${mission.passed_tasks}/${mission.total_tasks}`;
    this.tooltip = `${mission.name}\n${mission.project}\nState: ${mission.state}\n${mission.awaiting_input} awaiting local Send`;
    this.contextValue = `runtrol.mission.${mission.state}`;
    this.iconPath = new vscode.ThemeIcon(missionIcon(mission.state));
    this.command = {
      command: "runtrol.openMission",
      title: "Open Mission",
      arguments: [this],
    };
  }
}

export class MissionTaskItem extends vscode.TreeItem implements MissionSelection {
  readonly mission: MissionLine;
  readonly task: MissionTaskLine;

  constructor(mission: MissionLine, task: MissionTaskLine) {
    super(task.key, vscode.TreeItemCollapsibleState.None);
    this.mission = mission;
    this.task = task;
    this.description = task.state;
    this.tooltip = [
      task.key,
      `State: ${task.state}`,
      `Instruction: ${task.instruction_ref}`,
      `Workspace: ${task.workspace ?? task.workspace_mode}`,
      `Gates: ${task.passed_gates} passed, ${task.failed_gates} failed`,
      `Receipt: ${task.receipt_id ?? "not sealed"}`,
    ].join("\n");
    this.contextValue = `runtrol.missionTask.${task.state}${task.session_id ? ".session" : ""}`;
    this.iconPath = new vscode.ThemeIcon(taskIcon(task.state));
    this.command = task.session_id
      ? { command: "runtrol.openTaskSession", title: "Open Task Session", arguments: [this] }
      : { command: "runtrol.openMission", title: "Open Mission", arguments: [this] };
  }
}

export class MissionTree implements vscode.TreeDataProvider<MissionNode>, vscode.Disposable {
  private readonly changedEmitter = new vscode.EventEmitter<void>();
  private readonly subscription: vscode.Disposable;
  readonly onDidChangeTreeData = this.changedEmitter.event;

  constructor(private readonly controller: MissionController) {
    this.subscription = controller.onDidChange(() => this.changedEmitter.fire());
  }

  getTreeItem(element: MissionNode): vscode.TreeItem {
    return element;
  }

  async getChildren(element?: MissionNode): Promise<MissionNode[]> {
    if (!element) {
      return [...this.controller.missions]
        .sort((left, right) => left.name.localeCompare(right.name) || left.mission_id.localeCompare(right.mission_id))
        .map((mission) => new MissionItem(mission));
    }
    if (element instanceof MissionTaskItem) return [];
    const response = await this.controllerSnapshot(element.mission.mission_id);
    return response.tasks.map((task) => new MissionTaskItem(response.mission, task));
  }

  dispose(): void {
    this.subscription.dispose();
    this.changedEmitter.dispose();
  }

  private async controllerSnapshot(missionId: string) {
    return this.controller.snapshot(missionId);
  }
}

function missionIcon(state: string): string {
  if (state === "integrating" || state === "completed") return "pass-filled";
  if (state === "failed" || state === "cancelled") return "error";
  if (state === "paused" || state === "blocked") return "debug-pause";
  return "type-hierarchy-sub";
}

function taskIcon(state: string): string {
  if (state === "passed") return "pass";
  if (state === "failed" || state === "cancelled") return "error";
  if (state === "awaitingInput" || state === "retryable") return "bell";
  if (state === "running" || state === "verifying") return "sync";
  return "circle-outline";
}
