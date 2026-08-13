import * as path from "node:path";

import * as vscode from "vscode";

import type { CoreClient } from "../core/client";
import type { CapabilityLine, Response } from "../protocol";

export class CandidateController implements vscode.Disposable {
  private rows: readonly CapabilityLine[] = [];

  constructor(private readonly client: CoreClient) {}

  async propose(): Promise<void> {
    const selected = await vscode.window.showOpenDialog({
      title: "Import a project capability candidate",
      openLabel: "Inspect Candidate",
      canSelectFiles: false,
      canSelectFolders: true,
      canSelectMany: false,
    });
    const candidate = selected?.[0];
    if (!candidate) return;
    const folder = vscode.workspace.getWorkspaceFolder(candidate);
    if (!folder) {
      throw new Error("the candidate directory must be inside an open VS Code workspace");
    }
    const candidateRef = path.relative(folder.uri.fsPath, candidate.fsPath).replaceAll("\\", "/");
    if (!candidateRef || candidateRef.startsWith("../") || path.isAbsolute(candidateRef)) {
      throw new Error("the candidate directory must be project relative");
    }
    this.rows = requireCapabilities((await this.client.once({
      ask: "capabilityPropose",
      with: { project: folder.uri.fsPath, candidate_ref: candidateRef },
    })).response);
    const imported = this.rows.find((row) => row.project === folder.uri.fsPath && row.source_ref === candidateRef);
    if (imported) await this.showReview(imported);
  }

  async inbox(): Promise<void> {
    const selected = await this.pick("Capability Candidate Inbox");
    if (selected) await this.showReview(selected);
  }

  async verify(): Promise<void> {
    const selected = await this.pick("Verify Capability Candidate", ["candidate"]);
    if (!selected) return;
    const action = await vscode.window.showWarningMessage(
      `Run independent fixed Gates for ${selected.capability_id}?`,
      {
        modal: true,
        detail: `Version SHA-256 ${selected.version_sha256}\nCandidate files are rechecked before verification.`,
      },
      "Verify exact version",
    );
    if (action !== "Verify exact version") return;
    this.rows = requireCapabilities((await this.client.once({
      ask: "capabilityVerify",
      with: identity(selected),
    })).response);
    await this.showUpdated(selected);
  }

  async approve(): Promise<void> {
    const selected = await this.pick("Approve Verified Capability", ["verified"]);
    if (!selected) return;
    await this.openNativeReview(selected);
    const action = await vscode.window.showWarningMessage(
      `Activate ${selected.capability_id} for this project only?`,
      {
        modal: true,
        detail: [
          `Version SHA-256 ${selected.version_sha256}`,
          `Verification Receipt ${selected.verification_receipt_id ?? "missing"}`,
          `Source Receipt ${selected.source_receipt_id}`,
          "Approval pins only this exact digest and never injects its text into a Task.",
        ].join("\n"),
      },
      "Approve exact digest",
    );
    if (action !== "Approve exact digest") return;
    this.rows = requireCapabilities((await this.client.once({
      ask: "capabilityApprove",
      with: identity(selected),
    })).response);
    await this.showUpdated(selected);
  }

  async reject(): Promise<void> {
    await this.simpleAction("Reject Capability Candidate", "capabilityReject", ["candidate", "verified"]);
  }

  async quarantine(): Promise<void> {
    await this.simpleAction("Quarantine Active Capability", "capabilityQuarantine", ["active", "rolledBack"]);
  }

  async rollback(): Promise<void> {
    const selected = await this.pick("Roll Back Capability", ["tampered", "quarantined"]);
    if (!selected) return;
    const version = await vscode.window.showQuickPick(
      selected.approved_versions
        .filter((candidate) => candidate !== selected.version_sha256)
        .map((candidate) => ({ label: candidate, version: candidate })),
      { title: `Prior approved version for ${selected.capability_id}` },
    );
    if (!version) return;
    const action = await vscode.window.showWarningMessage(
      `Restore exact approved version ${version.version}?`,
      { modal: true },
      "Roll back exact digest",
    );
    if (action !== "Roll back exact digest") return;
    this.rows = requireCapabilities((await this.client.once({
      ask: "capabilityRollback",
      with: {
        project: selected.project,
        capability_id: selected.capability_id,
        version_sha256: version.version,
      },
    })).response);
    await this.showUpdated(selected);
  }

  async archive(): Promise<void> {
    await this.simpleAction(
      "Archive Capability",
      "capabilityArchive",
      ["active", "rolledBack", "tampered", "quarantined", "stale", "rejected"],
    );
  }

  dispose(): void {
    this.rows = [];
  }

  private async simpleAction(
    title: string,
    ask: "capabilityReject" | "capabilityQuarantine" | "capabilityArchive",
    states: readonly string[],
  ): Promise<void> {
    const selected = await this.pick(title, states);
    if (!selected) return;
    const action = await vscode.window.showWarningMessage(
      `${title}: ${selected.capability_id}?`,
      { modal: true },
      title,
    );
    if (action !== title) return;
    this.rows = requireCapabilities((await this.client.once({
      ask,
      with: { project: selected.project, capability_id: selected.capability_id },
    })).response);
    await this.showUpdated(selected);
  }

  private async pick(title: string, states?: readonly string[]): Promise<CapabilityLine | null> {
    await this.refresh();
    const candidates = states ? this.rows.filter((row) => states.includes(row.state)) : this.rows;
    const selected = await vscode.window.showQuickPick(
      candidates.map((candidate) => ({
        label: candidate.capability_id,
        description: `${candidate.kind}  ${candidate.state}`,
        detail: `${candidate.project}  ${candidate.version_sha256}`,
        candidate,
      })),
      { title, placeHolder: candidates.length ? "Select one exact capability version" : "No matching capabilities" },
    );
    return selected?.candidate ?? null;
  }

  private async refresh(): Promise<void> {
    this.rows = requireCapabilities((await this.client.once({ ask: "capabilityList" })).response);
  }

  private async showUpdated(previous: CapabilityLine): Promise<void> {
    const updated = this.rows.find(
      (row) => row.project === previous.project && row.capability_id === previous.capability_id,
    );
    if (updated) await this.showReview(updated);
  }

  private async showReview(candidate: CapabilityLine): Promise<void> {
    const document = await vscode.workspace.openTextDocument({
      language: "markdown",
      content: candidateReview(candidate),
    });
    await vscode.window.showTextDocument(document, { preview: true, preserveFocus: false });
  }

  private async openNativeReview(candidate: CapabilityLine): Promise<void> {
    const source = vscode.Uri.file(path.join(candidate.project, candidate.source_ref, "SKILL.md"));
    const active = vscode.Uri.file(path.join(
      candidate.project,
      ".runtrol",
      "capabilities",
      "active",
      candidate.capability_id,
      "SKILL.md",
    ));
    try {
      await vscode.workspace.fs.stat(active);
      await vscode.commands.executeCommand(
        "vscode.diff",
        active,
        source,
        `${candidate.capability_id}: active to candidate`,
      );
    } catch {
      await vscode.window.showTextDocument(source, { preview: true, preserveFocus: false });
    }
  }
}

function identity(candidate: CapabilityLine) {
  return {
    project: candidate.project,
    capability_id: candidate.capability_id,
    version_sha256: candidate.version_sha256,
  };
}

function requireCapabilities(response: Response): CapabilityLine[] {
  if (response.say === "failed") throw new Error(response.with.message);
  if (response.say !== "capabilities") throw new Error(`Core returned ${response.say}, expected capabilities`);
  return response.with;
}

function candidateReview(candidate: CapabilityLine): string {
  return [
    "# Capability Candidate",
    "",
    `ID: ${inline(candidate.capability_id)}`,
    "",
    `Kind: ${inline(candidate.kind)}`,
    "",
    `State: ${inline(candidate.state)}`,
    "",
    `Project: ${inline(candidate.project)}`,
    "",
    `Source: ${inline(candidate.source_ref)}`,
    "",
    `Version SHA-256: ${inline(candidate.version_sha256)}`,
    "",
    `Source Receipt: ${inline(candidate.source_receipt_id)}`,
    "",
    `Verification Receipt: ${inline(candidate.verification_receipt_id ?? "not verified")}`,
    `Active version: ${inline(candidate.active_version_sha256 ?? "none")}`,
    `Verification Gates: ${candidate.verification_gates.map((gate) => `${inline(gate.gate_id)} ${gate.outcome} ${gate.duration_ms} ms`).join(", ") || "none"}`,
    "",
    `Approved rollback versions: ${candidate.approved_versions.map(inline).join(", ") || "none"}`,
    "",
    "The capability body remains in project files. Runtrol stores only exact local trust metadata and never injects the body into a Task.",
    "",
  ].join("\n");
}

function inline(value: string): string {
  return `\`${value.replaceAll("`", "'").replaceAll("\r", " ").replaceAll("\n", " ")}\``;
}
