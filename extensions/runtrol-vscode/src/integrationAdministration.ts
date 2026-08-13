import * as vscode from "vscode";

import type { CoreClient } from "./core/client";
import type { IntegrationEnrollmentLine, IntegrationLine, Response } from "./protocol";

export async function reviewRuntimeRequests(client: CoreClient): Promise<void> {
  const forgets = await ask(client, { ask: "runtimeForgetRequests" });
  if (forgets.say !== "runtimeForgetRequests") {
    throw new Error(`the daemon answered Runtime forget listing with ${forgets.say}`);
  }
  const rotations = await ask(client, { ask: "runtimeKeyRotationRequests" });
  if (rotations.say !== "runtimeKeyRotationRequests") {
    throw new Error(`the daemon answered Runtime key rotation listing with ${rotations.say}`);
  }
  const requests = [
    ...forgets.with.map((request) => ({
      label: request.integration_label,
      description: "Forget Runtime session pointer",
      detail: `${request.session_id}  ${request.integration_id}`,
      requestKind: "forget" as const,
      request,
    })),
    ...rotations.with.map((request) => ({
      label: request.integration_label,
      description: "Rotate Runtime integration key",
      detail: `Generation ${request.current_key_generation}  New key ${request.new_key_fingerprint}`,
      requestKind: "rotation" as const,
      request,
    })),
  ];
  if (requests.length === 0) {
    await vscode.window.showInformationMessage("Runtrol: No Runtime request is waiting for review.");
    return;
  }
  const selected = await vscode.window.showQuickPick(
    requests,
    {
      title: "Review a Runtrol Runtime request",
      placeHolder: "Choose the exact local Runtime request to inspect",
      ignoreFocusOut: true,
    },
  );
  if (!selected) {
    return;
  }
  if (selected.requestKind === "rotation") {
    const confirmed = await vscode.window.showWarningMessage(
      `Allow ${selected.request.integration_label} to replace integration key generation ${selected.request.current_key_generation} with ${selected.request.new_key_fingerprint}? Existing key credentials stop authenticating immediately.`,
      { modal: true },
      "Allow Key Rotation",
    );
    if (confirmed !== "Allow Key Rotation") {
      return;
    }
    const decided = await ask(client, {
      ask: "runtimeKeyRotationConfirm",
      with: { confirmation_id: selected.request.confirmation_id },
    });
    expectDone(decided, "Runtime integration key rotation confirmation");
    await vscode.window.showInformationMessage("Runtrol: The integration key rotation was confirmed.");
    return;
  }
  const confirmed = await vscode.window.showWarningMessage(
    `Allow ${selected.request.integration_label} to forget Runtime session ${selected.request.session_id}? Provider-owned conversation state is not deleted.`,
    { modal: true },
    "Allow Forget",
  );
  if (confirmed !== "Allow Forget") {
    return;
  }
  const decided = await ask(client, {
    ask: "runtimeForgetConfirm",
    with: { confirmation_id: selected.request.confirmation_id },
  });
  expectDone(decided, "Runtime session forget confirmation");
  await vscode.window.showInformationMessage("Runtrol: The Runtime metadata removal request was confirmed.");
}

export async function reviewIntegrationEnrollments(client: CoreClient): Promise<void> {
  const response = await ask(client, { ask: "integrationEnrollments" });
  if (response.say !== "integrationEnrollments") {
    throw new Error(`the daemon answered integration enrollment listing with ${response.say}`);
  }
  if (response.with.length === 0) {
    await vscode.window.showInformationMessage("Runtrol: No integration enrollment is waiting for review.");
    return;
  }
  const selected = await vscode.window.showQuickPick(
    response.with.map((enrollment) => ({
      label: enrollment.client_name,
      description: enrollment.client_version,
      detail: `${enrollment.client_instance_id}  ${enrollment.key_fingerprint}`,
      enrollment,
    })),
    {
      title: "Review a Runtrol Runtime integration",
      placeHolder: "Choose the local integration request to inspect",
      ignoreFocusOut: true,
    },
  );
  if (!selected) {
    return;
  }
  await decideEnrollment(client, selected.enrollment);
}

export async function reviewIntegrationEnrollment(
  client: CoreClient,
  pendingId: string,
): Promise<boolean> {
  const response = await ask(client, { ask: "integrationEnrollments" });
  if (response.say !== "integrationEnrollments") {
    throw new Error(`the daemon answered integration enrollment listing with ${response.say}`);
  }
  const enrollment = response.with.find((candidate) => candidate.pending_id === pendingId);
  if (!enrollment) {
    throw new Error("the Studio Runtime enrollment is no longer pending");
  }
  return await decideEnrollment(client, enrollment) === "approved";
}

export async function manageIntegrations(client: CoreClient): Promise<boolean> {
  const response = await ask(client, { ask: "integrations" });
  if (response.say !== "integrations") {
    throw new Error(`the daemon answered integration listing with ${response.say}`);
  }
  const active = response.with.filter((integration) => !integration.revoked);
  if (active.length === 0) {
    await vscode.window.showInformationMessage("Runtrol: No active Runtime integration is enrolled.");
    return false;
  }
  const selected = await pickIntegration(active);
  if (!selected) {
    return false;
  }
  const action = await vscode.window.showQuickPick(
    [
      {
        label: "Edit permissions and project roots",
        description: `Current grant generation ${selected.grant_generation}`,
        action: "change" as const,
      },
      {
        label: "Revoke integration",
        description: "Refuse every future Runtime request",
        action: "revoke" as const,
      },
    ],
    { title: `Manage ${selected.label}`, ignoreFocusOut: true },
  );
  if (!action) return false;
  if (action.action === "change") {
    return changeIntegrationGrant(client, selected);
  }
  const confirmed = await vscode.window.showWarningMessage(
    `Revoke ${selected.label}? Its current Runtime connection will be refused on its next request. Supervised sessions will keep running.`,
    { modal: true },
    "Revoke",
  );
  if (confirmed !== "Revoke") {
    return false;
  }
  const revoked = await ask(client, {
    ask: "integrationRevoke",
    with: { integration_id: selected.integration_id },
  });
  expectDone(revoked, "integration revocation");
  await vscode.window.showInformationMessage(`Runtrol: Revoked ${selected.label}.`);
  return true;
}

async function changeIntegrationGrant(
  client: CoreClient,
  integration: IntegrationLine,
): Promise<boolean> {
  const scopes = await vscode.window.showQuickPick(
    integration.available_scopes.map((scope) => ({
      label: scope,
      picked: integration.scopes.includes(scope),
    })),
    {
      title: `Choose permissions for ${integration.label}`,
      canPickMany: true,
      ignoreFocusOut: true,
      placeHolder: "Select the complete replacement permission set",
    },
  );
  if (!scopes) return false;
  if (scopes.length === 0) {
    await vscode.window.showWarningMessage("Runtrol: At least one integration permission is required.");
    return false;
  }

  let roots = [...integration.roots];
  const rootAction = await vscode.window.showQuickPick(
    ["Keep current project roots", "Review project roots"],
    { title: `Project authority for ${integration.label}`, ignoreFocusOut: true },
  );
  if (!rootAction) return false;
  if (rootAction === "Review project roots") {
    const retained = integration.roots.length === 0
      ? []
      : await vscode.window.showQuickPick(
        integration.roots.map((root) => ({ label: root, picked: true })),
        {
          title: "Choose project roots to retain",
          canPickMany: true,
          ignoreFocusOut: true,
        },
      );
    if (!retained) return false;
    roots = retained.map(({ label }) => label);
    const add = await vscode.window.showInformationMessage(
      "Add more project roots to this integration?",
      { modal: true },
      "Add Project Roots",
      "Use Selected Roots",
    );
    if (!add) return false;
    if (add === "Add Project Roots") {
      const selected = await vscode.window.showOpenDialog({
        title: "Add Runtime integration project roots",
        canSelectFiles: false,
        canSelectFolders: true,
        canSelectMany: true,
        openLabel: "Add Project Roots",
      });
      if (!selected) return false;
      roots.push(...selected.map((uri) => uri.fsPath));
    }
  }
  roots = uniquePaths(roots);
  const selectedScopes = scopes.map(({ label }) => label);
  if (selectedScopes.some(scopeNeedsRoot) && roots.length === 0) {
    await vscode.window.showWarningMessage(
      "Runtrol: Session and approval permissions require at least one project root.",
    );
    return false;
  }
  const confirmed = await vscode.window.showWarningMessage(
    `Replace ${integration.label}'s Runtime authority with permissions ${selectedScopes.join(", ")} and projects ${roots.length === 0 ? "none" : roots.join(", ")}? Active connections must authenticate again.`,
    { modal: true },
    "Replace Authority",
  );
  if (confirmed !== "Replace Authority") return false;
  const changed = await ask(client, {
    ask: "integrationGrantChange",
    with: {
      integration_id: integration.integration_id,
      expected_grant_generation: integration.grant_generation,
      scopes: selectedScopes,
      roots,
    },
  });
  expectDone(changed, "integration authority replacement");
  await vscode.window.showInformationMessage(`Runtrol: Updated ${integration.label}'s Runtime authority.`);
  return true;
}

async function decideEnrollment(
  client: CoreClient,
  enrollment: IntegrationEnrollmentLine,
): Promise<"approved" | "denied" | "cancelled"> {
  const summary = [
    `${enrollment.client_name} ${enrollment.client_version}`,
    `Instance: ${enrollment.client_instance_id}`,
    `Key: ${enrollment.key_fingerprint}`,
    `Scopes: ${enrollment.scopes.join(", ")}`,
    `Projects: ${enrollment.roots.length === 0 ? "none" : enrollment.roots.join(", ")}`,
  ].join("\n");
  const action = await vscode.window.showWarningMessage(
    summary,
    { modal: true, detail: "Approve only the scopes and project roots this integration needs." },
    "Review and Approve",
    "Deny",
  );
  if (action === "Deny") {
    const denied = await ask(client, {
      ask: "integrationEnrollmentDeny",
      with: { pending_id: enrollment.pending_id },
    });
    expectDone(denied, "integration denial");
    await vscode.window.showInformationMessage(`Runtrol: Denied ${enrollment.client_name}.`);
    return "denied";
  }
  if (action !== "Review and Approve") {
    return "cancelled";
  }
  const scopes = await pickSubset(
    "Choose integration permissions",
    enrollment.scopes,
    "At least one permission must remain selected",
  );
  if (!scopes) {
    return "cancelled";
  }
  const roots = enrollment.roots.length === 0
    ? []
    : await pickSubset(
      "Choose project roots",
      enrollment.roots,
      "At least one project root must remain selected",
    );
  if (!roots) {
    return "cancelled";
  }
  const begun = await ask(client, {
    ask: "integrationApprovalBegin",
    with: { pending_id: enrollment.pending_id, scopes, roots },
  });
  if (begun.say !== "integrationApprovalChallenge") {
    throw new Error(`the daemon answered integration approval with ${begun.say}`);
  }
  const typed = await vscode.window.showInputBox({
    title: `Approve ${enrollment.client_name}`,
    prompt: begun.with.prompt,
    placeHolder: "Type the exact three-word phrase",
    ignoreFocusOut: true,
  });
  if (typed === undefined) {
    return "cancelled";
  }
  const approved = await ask(client, {
    ask: "integrationApprovalFinish",
    with: { challenge_id: begun.with.challenge_id, answer: typed },
  });
  if (approved.say !== "integrationApproved") {
    throw new Error(`the daemon answered completed integration approval with ${approved.say}`);
  }
  await vscode.window.showInformationMessage(
    `Runtrol: Approved ${enrollment.client_name} as ${approved.with.integration_id}.`,
  );
  return "approved";
}

async function pickSubset(
  title: string,
  values: readonly string[],
  emptyMessage: string,
): Promise<string[] | undefined> {
  const chosen = await vscode.window.showQuickPick(
    values.map((value) => ({ label: value, picked: true })),
    {
      title,
      canPickMany: true,
      ignoreFocusOut: true,
      placeHolder: "Clear anything this integration should not receive",
    },
  );
  if (!chosen) {
    return undefined;
  }
  if (chosen.length === 0) {
    await vscode.window.showWarningMessage(`Runtrol: ${emptyMessage}.`);
    return undefined;
  }
  return chosen.map(({ label }) => label);
}

async function pickIntegration(integrations: readonly IntegrationLine[]): Promise<IntegrationLine | undefined> {
  const selected = await vscode.window.showQuickPick(
    integrations.map((integration) => ({
      label: integration.label,
      description: integration.client_instance_id,
      detail: `${integration.scopes.join(", ")}  ${integration.roots.join(", ")}`,
      integration,
    })),
    {
      title: "Manage Runtrol Runtime integrations",
      placeHolder: "Choose an integration to revoke",
      ignoreFocusOut: true,
    },
  );
  return selected?.integration;
}

function scopeNeedsRoot(scope: string): boolean {
  return scope.startsWith("session.") || scope.startsWith("approval.");
}

function uniquePaths(paths: readonly string[]): string[] {
  const seen = new Set<string>();
  return paths.filter((path) => {
    const key = process.platform === "win32" ? path.toLocaleLowerCase("en-US") : path;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

async function ask(client: CoreClient, request: Parameters<CoreClient["once"]>[0]): Promise<Response> {
  const { response } = await client.once(request);
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  return response;
}

function expectDone(response: Response, action: string): void {
  if (response.say !== "done") {
    throw new Error(`the daemon answered ${action} with ${response.say}`);
  }
}
