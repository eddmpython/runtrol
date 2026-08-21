import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import { extensionUnderTest } from "./extensionUnderTest.test";
import { Watcher, type EndFact } from "./realProviderWatch.test";
import type { ProviderLine, SessionLine } from "../runtimeTypes";

type JourneyApi = {
  providers(): readonly ProviderLine[];
  sessions(): readonly SessionLine[];
  start(
    provider: string,
    workspace: string,
    model?: string | null,
    reasoningEffort?: string | null,
    permission?: string | null,
  ): Promise<string>;
  select(session: string, follow?: boolean): Promise<void>;
  prompt(text: string): Promise<void>;
  switchModel(model: string): Promise<void>;
  switchMode(mode: string): Promise<void>;
  answerApproval(approval: string, option: number, subjectDigest: number[]): Promise<void>;
  interrupt(): Promise<void>;
  reconnect(): Promise<void>;
  openWorkspace(session: string): Promise<void>;
  close(session: string, now?: boolean): Promise<void>;
  verifySelected(session: string): Promise<void>;
};

type ExtensionApi = {
  readonly ready: Promise<void>;
  readonly journey?: JourneyApi;
};

type SwitchingCheckpoint = {
  stage: "switching";
  firstSession: string;
  secondSession: string;
  firstWorkspace: string;
  secondWorkspace: string;
  interruptStop: string;
  interruptDeclaredBy: string;
};

type FinalEvidence = Omit<SwitchingCheckpoint, "stage"> & {
  stage: "complete";
  vscode: string;
  providerDetected: boolean;
  approvalDenied: boolean;
  reconnected: boolean;
  interrupted: boolean;
  workspaceRestored: boolean;
  sessionsClosed: boolean;
};

let currentStage = "starting";
let currentFirstSession: string | undefined;
let currentSecondSession: string | undefined;

export async function run(): Promise<void> {
  const resultPath = requiredEnvironment("RUNTROL_VSCODE_RESULT");
  try {
    const checkpoint = await readCheckpoint(resultPath);
    if (checkpoint?.stage === "switching") {
      currentFirstSession = checkpoint.firstSession;
      currentSecondSession = checkpoint.secondSession;
      await restore(checkpoint, resultPath);
      return;
    }
    await journey(resultPath);
  } catch (error) {
    await writeFile(
      resultPath,
      JSON.stringify({
        stage: currentStage,
        failure: error instanceof Error ? error.message : String(error),
        stack: error instanceof Error ? error.stack : undefined,
        firstSession: currentFirstSession,
        secondSession: currentSecondSession,
      }),
      "utf8",
    );
    throw error;
  }
}

async function journey(resultPath: string): Promise<void> {
  const firstWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_ONE");
  const secondWorkspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE_TWO");
  const target = requiredEnvironment("RUNTROL_VSCODE_DENIED_TARGET");
  const provider = requiredEnvironment("RUNTROL_VSCODE_PROVIDER");
  const core = requiredEnvironment("RUNTROL_TEST_CORE");
  const api = await activate();

  if (!api.journey) {
    throw new Error("the real-provider journey API is unavailable");
  }
  const providerDetected = await waitForUsableProvider(api.journey, provider);

  currentStage = "starting-first-session";
  const firstSession = await within(
    api.journey.start(provider, firstWorkspace),
    30_000,
    "starting the first installed-provider session",
  );
  currentFirstSession = firstSession;
  const watch = new Watcher(core, firstSession);
  try {
    await watch.ready;
    await api.journey.verifySelected(firstSession);

    // The attach-time model announcement is deliberately not asserted here: it fires while the session is
    // starting, before any watcher can attach, and this watcher begins at the live boundary (measured: waiting
    // for it timed out on a healthy session). The switch below is therefore the first model fact this watcher
    // can ever see, which is exactly what makes its assertion unambiguous.
    currentStage = "approval-prompt";
    await api.journey.prompt("perform the requested deterministic action");
    const firstApproval = await watch.next("approval", 60_000);
    if (firstApproval.option < 0) throw new Error("the provider approval offered no single reject-once option");
    if (firstApproval.target !== target) {
      throw new Error(`the approval targeted ${firstApproval.target}, expected ${target}`);
    }
    await api.journey.answerApproval(
      firstApproval.approval,
      firstApproval.option,
      firstApproval.subjectDigest,
    );
    const firstEnd = await watch.next("end", 60_000);
    if (firstEnd.stop !== "endTurn" || firstEnd.declaredBy !== "provider") {
      throw new Error(`the denied turn ended as ${firstEnd.stop} by ${firstEnd.declaredBy}`);
    }

    currentStage = "model-switch";
    // The switch rides the whole spine to the real CLI's own control channel, and what comes back is the
    // CLI's confirmation event, not runtrol's assumption. "sonnet" is one of the aliases the manifest carries
    // with its measurement.
    await api.journey.switchModel("sonnet");
    // The CLI also announces its own answering model on its own schedule (measured: one such announcement
    // arrived ahead of the confirmation), and those announcements are correct events, not noise. So the
    // confirmation is found by draining a bounded few, not by assuming it arrives first.
    let confirmedSwitch = "";
    for (let attempt = 0; attempt < 3 && !confirmedSwitch.includes("sonnet"); attempt += 1) {
      confirmedSwitch = (await watch.next("model", 30_000)).model;
    }
    if (!confirmedSwitch.includes("sonnet")) {
      throw new Error(`the switch confirmed ${confirmedSwitch}, expected a sonnet model`);
    }

    currentStage = "mode-switch";
    // Same spine, other switch: the mode travels the CLI's own control channel, and the confirmation read
    // here is the CLI's own announcement (measured: a system/status frame carrying the mode follows every
    // accepted set_permission_mode, and the CLI also announces its current mode on its own schedule, so a
    // "default" announcement can precede the confirmation). Drained like the model switch above. "plan" is
    // in the manifest's switchable list; the modes that remove safety prompts are deliberately not, and the
    // daemon refuses them for every caller.
    await api.journey.switchMode("plan");
    let confirmedMode = "";
    for (let attempt = 0; attempt < 4 && confirmedMode !== "plan"; attempt += 1) {
      confirmedMode = (await watch.next("mode", 30_000)).mode;
    }
    if (confirmedMode !== "plan") {
      throw new Error(`the mode switch confirmed ${confirmedMode}, expected plan`);
    }
    // Returned to default so the rest of the journey exercises the ordinary approval path.
    await api.journey.switchMode("default");
    let restoredMode = "";
    for (let attempt = 0; attempt < 4 && restoredMode !== "default"; attempt += 1) {
      restoredMode = (await watch.next("mode", 30_000)).mode;
    }
    if (restoredMode !== "default") {
      throw new Error(`the mode restore confirmed ${restoredMode}, expected default`);
    }

    currentStage = "reconnecting";
    await api.journey.reconnect();
    await api.journey.verifySelected(firstSession);

    currentStage = "interrupt-prompt";
    await api.journey.prompt("wait for the operator decision");
    const secondApproval = await watch.next("approval", 60_000);
    if (secondApproval.target !== target) {
      throw new Error(`the interrupt approval targeted ${secondApproval.target}, expected ${target}`);
    }
    await api.journey.interrupt();
    const interruptEnd = await watch.next("end", 60_000);
    if (!interruptTerminal(interruptEnd)) {
      throw new Error(`the interrupt ended as ${interruptEnd.stop} by ${interruptEnd.declaredBy}`);
    }

    currentStage = "starting-second-session";
    // Started in plan mode through the public spine (StartSessionParams.permission -> daemon
    // vocabulary gate -> the CLI's own --permission-mode flag). The claude driver refuses an
    // explicit choice whose flag its parser did not confirm, and the CLI refuses an invalid value
    // loudly, so this start succeeding is itself evidence the choice traveled.
    const secondSession = await within(
      api.journey.start(provider, secondWorkspace, null, null, "plan"),
      30_000,
      "starting the second installed-provider session in plan mode",
    );
    currentSecondSession = secondSession;
    await api.journey.verifySelected(secondSession);
    const listed = api.journey.sessions();
    if (!listed.some(
      (session) => session.sessionId === firstSession && samePath(session.workspace, firstWorkspace),
    )) {
      throw new Error("the first installed-provider session lost its workspace binding");
    }
    if (!listed.some(
      (session) => session.sessionId === secondSession && samePath(session.workspace, secondWorkspace),
    )) {
      throw new Error("the second installed-provider session lost its workspace binding");
    }

    const checkpoint: SwitchingCheckpoint = {
      stage: "switching",
      firstSession,
      secondSession,
      firstWorkspace,
      secondWorkspace,
      interruptStop: interruptEnd.stop,
      interruptDeclaredBy: interruptEnd.declaredBy,
    };
    await writeFile(resultPath, JSON.stringify(checkpoint), "utf8");
    currentStage = "switching-workspace";
    await api.journey.openWorkspace(secondSession);
    await new Promise((resolve) => setTimeout(resolve, 1_000));
    const switched = vscode.workspace.workspaceFolders ?? [];
    if (switched.length !== 1 || !samePath(switched[0]?.uri.fsPath, secondWorkspace)) {
      throw new Error("the same-window workspace command returned without switching to the session workspace");
    }
  } finally {
    await watch.stop();
  }
}

async function waitForUsableProvider(journey: JourneyApi, providerId: string): Promise<boolean> {
  const deadline = Date.now() + 30_000;
  let observed: ProviderLine | undefined;
  while (Date.now() < deadline) {
    observed = journey.providers().find((provider) => provider.providerId === providerId);
    if (observed?.installation.state === "usable") return true;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(
    `the installed provider ${providerId} was not discovered as usable: `
    + `${observed?.installation.state ?? "absent"} ${observed?.installation.why ?? "without a reason"}`,
  );
}

async function restore(checkpoint: SwitchingCheckpoint, resultPath: string): Promise<void> {
  currentStage = "restoring-workspace";
  const folders = vscode.workspace.workspaceFolders ?? [];
  if (folders.length !== 1 || !samePath(folders[0]?.uri.fsPath, checkpoint.secondWorkspace)) {
    throw new Error(`workspace restored as ${folders[0]?.uri.fsPath ?? "none"}`);
  }
  const api = await activate();
  if (!api.journey) {
    throw new Error("the real-provider journey API is unavailable after the workspace switch");
  }
  await api.journey.verifySelected(checkpoint.secondSession);
  currentStage = "closing-sessions";
  await api.journey.close(checkpoint.secondSession, true);
  await api.journey.close(checkpoint.firstSession, true);
  if (api.journey.sessions().some(
    (session) => session.sessionId === checkpoint.firstSession
      || session.sessionId === checkpoint.secondSession,
  )) {
    throw new Error("a session remained listed after the extension closed both exact sessions");
  }
  const result: FinalEvidence = {
    ...checkpoint,
    stage: "complete",
    vscode: vscode.version,
    providerDetected: true,
    approvalDenied: true,
    reconnected: true,
    interrupted: true,
    workspaceRestored: true,
    sessionsClosed: true,
  };
  await writeFile(resultPath, JSON.stringify(result), "utf8");
}

async function activate(): Promise<ExtensionApi> {
  currentStage = "opening-view";
  const extension = extensionUnderTest<ExtensionApi>();
  await within(
    vscode.commands.executeCommand("workbench.view.extension.runtrol"),
    30_000,
    "opening the Runtrol view",
  );
  currentStage = "activating-extension";
  const api = await within(waitForActivation(extension), 30_000, "extension activation through the Runtrol view");
  currentStage = "initializing-extension";
  await within(api.ready, 30_000, "extension initialization");
  currentStage = "opening-conversation";
  await within(
    vscode.commands.executeCommand("runtrol.openConversation"),
    30_000,
    "focusing the Runtrol conversation",
  );
  return api;
}

async function waitForActivation(extension: vscode.Extension<ExtensionApi>): Promise<ExtensionApi> {
  while (!extension.isActive) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return extension.exports;
}

async function readCheckpoint(resultPath: string): Promise<SwitchingCheckpoint | null> {
  try {
    const value: unknown = JSON.parse(await readFile(resultPath, "utf8"));
    if (
      record(value)
      && value.stage === "switching"
      && typeof value.firstSession === "string"
      && typeof value.secondSession === "string"
      && typeof value.firstWorkspace === "string"
      && typeof value.secondWorkspace === "string"
      && typeof value.interruptStop === "string"
      && typeof value.interruptDeclaredBy === "string"
    ) {
      return value as SwitchingCheckpoint;
    }
    return null;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return null;
    }
    throw error;
  }
}

async function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      Promise.resolve(work),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      }),
    ]);
  } finally {
    if (timer) {
      clearTimeout(timer);
    }
  }
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is required`);
  }
  return value;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function interruptTerminal(fact: EndFact): boolean {
  return (
    fact.stop === "cancelled"
    && (fact.declaredBy === "interruptAcked" || fact.declaredBy === "provider")
  ) || (fact.stop === "failed" && fact.declaredBy === "provider");
}

function samePath(left: string | undefined, right: string): boolean {
  if (!left) {
    return false;
  }
  const normalizedLeft = path.resolve(left);
  const normalizedRight = path.resolve(right);
  return process.platform === "win32"
    ? normalizedLeft.toLocaleLowerCase("en-US") === normalizedRight.toLocaleLowerCase("en-US")
    : normalizedLeft === normalizedRight;
}
