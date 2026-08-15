import * as vscode from "vscode";

import { CoreClient } from "./core/client";
import { pairingQrDataUrl } from "./pairingQr";
import type { DeviceLine, PairingProposalLine, Response } from "./protocol";

const POLL_INTERVAL_MS = 750;

export async function pairPhone(client: CoreClient): Promise<void> {
  const invitation = expect(await client.once({ ask: "pairingBegin" }), "pairingInvitation");
  const qr = await pairingQrDataUrl(invitation.pairing_url);
  const panel = vscode.window.createWebviewPanel(
    "runtrol.pairPhone",
    "Pair a phone",
    vscode.ViewColumn.Active,
    { enableScripts: false, retainContextWhenHidden: false },
  );
  panel.webview.html = pairingHtml(
    panel.webview.cspSource,
    qr,
    invitation.pc_key_fingerprint,
    invitation.expires_at_ms,
  );
  try {
    const proposal = await waitForProposal(client, invitation.expires_at_ms, panel);
    if (!proposal) return;
    panel.dispose();
    await approveProposal(client, proposal);
  } finally {
    panel.dispose();
  }
}

export async function reviewPhonePairings(client: CoreClient): Promise<void> {
  const proposals = expect(await client.once({ ask: "pairingProposals" }), "pairingProposals");
  if (proposals.length === 0) {
    await vscode.window.showInformationMessage("Runtrol: no phone is waiting for approval.");
    return;
  }
  const selected = await vscode.window.showQuickPick(
    proposals.map((proposal) => ({
      label: proposal.name,
      description: proposal.platform,
      detail: `Authenticated key ${proposal.key_fingerprint}`,
      proposal,
    })),
    { title: "Phone pairing requests", placeHolder: "Choose the phone shown in front of you" },
  );
  if (selected) await approveProposal(client, selected.proposal);
}

export async function managePhones(client: CoreClient): Promise<void> {
  const devices = expect(await client.once({ ask: "devices" }), "devices");
  if (devices.length === 0) {
    await vscode.window.showInformationMessage("Runtrol: no phones are paired.");
    return;
  }
  const selected = await vscode.window.showQuickPick(
    devices.map((device) => ({
      label: device.name,
      description: device.platform,
      detail: `${device.scopes.length} permissions, ${device.roots.length} workspaces, ${device.providers.length} providers, key ${device.key_fingerprint}`,
      device,
    })),
    { title: "Paired phones", placeHolder: "Choose a phone to inspect or change" },
  );
  if (!selected) return;
  await showDevice(client, selected.device);
}

async function waitForProposal(
  client: CoreClient,
  expiresAtMs: number,
  panel: vscode.WebviewPanel,
): Promise<PairingProposalLine | undefined> {
  let disposed = false;
  const disposal = panel.onDidDispose(() => {
    disposed = true;
  });
  try {
    while (!disposed && Date.now() < expiresAtMs) {
      const proposals = expect(await client.once({ ask: "pairingProposals" }), "pairingProposals");
      if (proposals.length > 0) return proposals[0];
      await delay(POLL_INTERVAL_MS);
    }
    if (!disposed) {
      await vscode.window.showWarningMessage("Runtrol: the pairing QR expired. Start pairing again.");
    }
    return undefined;
  } finally {
    disposal.dispose();
  }
}

async function approveProposal(client: CoreClient, proposal: PairingProposalLine): Promise<void> {
  const decision = await vscode.window.showWarningMessage(
    `Pair ${proposal.name} on ${proposal.platform}? Authenticated key ${proposal.key_fingerprint}.`,
    { modal: true, detail: "Choose its exact permissions next. The phone receives nothing until you type the PC approval phrase." },
    "Review permissions",
    "Deny",
  );
  if (decision !== "Review permissions") {
    await client.once({ ask: "pairingDeny", with: { proposal_id: proposal.proposal_id } });
    return;
  }
  const picked = await vscode.window.showQuickPick(
    proposal.available_scopes.map((scope) => ({
      label: scope,
      description: scopeDescription(scope),
      picked: defaultScope(scope),
    })),
    {
      canPickMany: true,
      title: `Permissions for ${proposal.name}`,
      placeHolder: "Only selected permissions will be granted",
    },
  );
  if (!picked) {
    await client.once({ ask: "pairingDeny", with: { proposal_id: proposal.proposal_id } });
    return;
  }
  const challenge = expect(
    await client.once({
      ask: "pairingApprovalBegin",
      with: { proposal_id: proposal.proposal_id, scopes: picked.map((item) => item.label) },
    }),
    "pairingApprovalChallenge",
  );
  const answer = await vscode.window.showInputBox({
    title: `Approve ${proposal.name}`,
    prompt: challenge.prompt,
    placeHolder: "Type the exact three-word phrase",
    ignoreFocusOut: true,
    validateInput: (value) => value.trim().split(/\s+/u).length === 3 ? undefined : "Type all three words shown above.",
  });
  if (answer === undefined) {
    await client.once({ ask: "pairingDeny", with: { proposal_id: proposal.proposal_id } });
    return;
  }
  const finished = await client.once({
    ask: "pairingApprovalFinish",
    with: { challenge_id: challenge.challenge_id, answer },
  });
  expect(finished, "done");
  await vscode.window.showInformationMessage(`Runtrol: ${proposal.name} is paired with ${picked.length} permissions.`);
}

async function showDevice(client: CoreClient, device: DeviceLine): Promise<void> {
  const choice = await vscode.window.showInformationMessage(
    `${device.name} on ${device.platform}`,
    {
      modal: true,
      detail: deviceDetail(device),
    },
    "Change authority",
    "Revoke phone",
  );
  if (choice === "Change authority") {
    await changeAuthority(client, device);
    return;
  }
  if (choice !== "Revoke phone") return;
  const confirmed = await vscode.window.showWarningMessage(
    `Revoke ${device.name}?`,
    { modal: true, detail: "Its durable key and every permission will be removed immediately." },
    "Revoke",
  );
  if (confirmed !== "Revoke") return;
  expect(
    await client.once({ ask: "deviceRevoke", with: { device_id: device.device_id } }),
    "done",
  );
  await vscode.window.showInformationMessage(`Runtrol: ${device.name} was revoked.`);
}

async function changeAuthority(client: CoreClient, device: DeviceLine): Promise<void> {
  const scopes = await vscode.window.showQuickPick(
    device.available_scopes.map((scope) => ({
      label: scope,
      description: scopeDescription(scope),
      picked: device.scopes.includes(scope),
    })),
    {
      canPickMany: true,
      title: `Permissions for ${device.name}`,
      placeHolder: "This replaces the complete permission set",
    },
  );
  if (!scopes) return;

  const rootCandidates = new Map<string, { label: string; description: string; picked: boolean }>();
  for (const root of device.roots) {
    rootCandidates.set(root, { label: root, description: "Currently approved", picked: true });
  }
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    const path = folder.uri.fsPath;
    const current = rootCandidates.get(path);
    rootCandidates.set(path, {
      label: path,
      description: current ? "Currently approved and open" : `Open workspace: ${folder.name}`,
      picked: current?.picked ?? false,
    });
  }
  const roots = await vscode.window.showQuickPick([...rootCandidates.values()], {
    canPickMany: true,
    title: `Workspace roots for ${device.name}`,
    placeHolder: "Only these directory trees may be started or resumed",
  });
  if (!roots) return;

  const availableProviders = await client.availableProviders();
  const providers = await vscode.window.showQuickPick(
    availableProviders.map((provider) => ({
      label: provider.display_name,
      description: provider.id,
      provider: provider.id,
      picked: device.providers.includes(provider.id),
    })),
    {
      canPickMany: true,
      title: `Providers for ${device.name}`,
      placeHolder: "Only these discovered providers may be started or resumed",
    },
  );
  if (!providers) return;

  const challenge = expect(
    await client.once({
      ask: "deviceAuthorityBegin",
      with: {
        device_id: device.device_id,
        scopes: scopes.map((scope) => scope.label),
        roots: roots.map((root) => root.label),
        providers: providers.map((provider) => provider.provider),
      },
    }),
    "deviceAuthorityChallenge",
  );
  const answer = await vscode.window.showInputBox({
    title: `Replace authority for ${device.name}`,
    prompt: challenge.prompt,
    placeHolder: "Type the exact three-word phrase",
    ignoreFocusOut: true,
    validateInput: (value) => value.trim().split(/\s+/u).length === 3 ? undefined : "Type all three words shown above.",
  });
  if (answer === undefined) {
    await client.once({
      ask: "deviceAuthorityFinish",
      with: { challenge_id: challenge.challenge_id, answer: "" },
    }).catch(() => undefined);
    return;
  }
  expect(
    await client.once({
      ask: "deviceAuthorityFinish",
      with: { challenge_id: challenge.challenge_id, answer },
    }),
    "done",
  );
  await vscode.window.showInformationMessage(`Runtrol: ${device.name} authority was replaced.`);
}

function deviceDetail(device: DeviceLine): string {
  return [
    `Key ${device.key_fingerprint}`,
    `Paired ${new Date(device.paired_at_ms).toLocaleString()}`,
    "Permissions:",
    device.scopes.join("\n") || "none",
    "Workspace roots:",
    device.roots.join("\n") || "none",
    "Providers:",
    device.providers.join("\n") || "none",
  ].join("\n");
}

type ResponseWithValue = Exclude<Response, { say: "done" }>;

function expect(
  result: Awaited<ReturnType<CoreClient["once"]>>,
  say: "done",
): void;
function expect<T extends ResponseWithValue["say"]>(
  result: Awaited<ReturnType<CoreClient["once"]>>,
  say: T,
): Extract<ResponseWithValue, { say: T }>["with"];
function expect(
  result: Awaited<ReturnType<CoreClient["once"]>>,
  say: Response["say"],
): unknown {
  const response = result.response;
  if (response.say === "failed") throw new Error(response.with.message);
  if (response.say !== say) throw new Error(`the Core answered ${say} with ${response.say}`);
  return "with" in response ? response.with : undefined;
}

function pairingHtml(cspSource: string, qr: string, fingerprint: string, expiresAtMs: number): string {
  const seconds = Math.max(0, Math.ceil((expiresAtMs - Date.now()) / 1000));
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data: ${cspSource}; style-src ${cspSource} 'unsafe-inline'">
<title>Pair a phone</title></head>
<body style="margin:0;padding:32px;display:grid;place-items:center;background:var(--vscode-editor-background);color:var(--vscode-editor-foreground);font-family:var(--vscode-font-family)">
<main style="width:min(440px,100%);text-align:center"><h1 style="font-size:24px">Pair a phone</h1>
<p>Open the Runtrol phone app and scan this one-use code.</p>
<img src="${qr}" width="320" height="320" alt="One-use Runtrol phone pairing QR" style="max-width:100%;height:auto;background:#fff;border-radius:16px">
<p><strong>PC key</strong><br><code>${escapeHtml(fingerprint)}</code></p>
<p>This code expires in about ${seconds} seconds. The phone still needs a separate approval in VS Code.</p></main></body></html>`;
}

function defaultScope(scope: string): boolean {
  return ["session.list", "session.output.read", "session.input.write", "session.stop"].includes(scope);
}

function scopeDescription(scope: string): string {
  if (scope.endsWith(".read") || scope === "session.list") return "Read-only";
  if (scope.includes("approval.respond.high") || scope.includes("delete")) return "High impact";
  return "Changes or controls work";
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/gu, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "\"": "&quot;", "'": "&#39;",
  })[character] ?? character);
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
