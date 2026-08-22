import * as vscode from "vscode";

import type { DiffDocuments } from "../../diffDocuments";
import type { MissionSnapshot } from "../../protocol";
import { applyReviewedLanding, assertReviewedLandingApplied } from "./apply";
import { completeLandingWithRecovery } from "./completion";
import {
  missionLanding,
  missionLandingQueue,
  missionWinnerLanding,
  type MissionLanding,
} from "./model";
import { openMissionLanding, type ReviewedMissionLanding } from "./review";

const APPLY_LANDING = "Apply, run Gates and complete";
const REVIEW_NEXT_LANDING = "Review next";
const LANDING_NOT_READY = "Mission cannot land";

export type LandingHost = {
  readonly getSnapshot: (missionId: string) => Promise<MissionSnapshot>;
  readonly listIntegratingSnapshots: () => Promise<readonly MissionSnapshot[]>;
  readonly complete: (snapshot: MissionSnapshot, selectedTaskId: string | null) => Promise<MissionSnapshot>;
  readonly withProjectLease: <T>(
    snapshot: MissionSnapshot,
    action: () => Promise<T>,
  ) => Promise<T>;
};

type LandingAuthority = {
  readonly review: ReviewedMissionLanding;
  state: "reviewed" | "appliedAwaitingCore";
};

export class MissionLandingController {
  private currentReview: LandingAuthority | null = null;
  private applying = false;
  private publicReviewActive = false;
  private reviewOpening = false;

  constructor(
    private readonly documents: DiffDocuments,
    private readonly host: LandingHost,
  ) {}

  async reviewAndApply(missionId?: string, taskId?: string): Promise<void> {
    if (this.publicReviewActive) throw new Error("A Mission Landing review is already open");
    this.publicReviewActive = true;
    try {
      let nextMissionId = missionId;
      for (;;) {
        const landing = await this.selectLanding(nextMissionId, taskId);
        if (!landing) return;
        const review = await this.openAndRemember(landing);
        const applyAction = landing.selection.kind === "chooseOne"
          ? `Apply ${landing.artifacts[0]?.task.key ?? "winner"}, run Gates and complete`
          : APPLY_LANDING;
        const sourceScope = landing.selection.kind === "chooseOne"
          ? ` from ${landing.artifacts[0]?.task.key ?? "the selected winner"} only`
          : "";
        const action = await vscode.window.showWarningMessage(
          `${landing.snapshot.mission.name}: apply ${sealedArtifactLabel(landing.artifacts.length)}${sourceScope}, run fixed Gates, and complete?`,
          applyAction,
        );
        if (action !== applyAction) {
          if (this.currentReview?.review === review) this.currentReview = null;
          return;
        }
        const completed = await this.applyAndComplete(review);
        if (landing.selection.kind === "chooseOne") {
          await vscode.window.showInformationMessage(
            `${completed.mission.name} completed with ${landing.artifacts[0]?.task.key ?? "the selected winner"}.`,
          );
          return;
        }
        const remaining = (await this.currentQueue()).length;
        const next = await vscode.window.showInformationMessage(
          `${completed.mission.name} completed. ${remaining > 0 ? `${remaining} ready to land.` : "Queue clear."}`,
          ...(remaining > 0 ? [REVIEW_NEXT_LANDING] : []),
        );
        if (next !== REVIEW_NEXT_LANDING) return;
        nextMissionId = undefined;
        taskId = undefined;
      }
    } finally {
      this.publicReviewActive = false;
    }
  }

  async reviewForJourney(missionId: string, taskId?: string): Promise<void> {
    const landing = await this.selectLanding(missionId, taskId);
    if (!landing) throw new Error(LANDING_NOT_READY);
    await this.openAndRemember(landing);
  }

  async applyForJourney(missionId: string, taskId?: string): Promise<MissionSnapshot> {
    const authority = this.currentReview;
    const selection = authority?.review.landing.selection;
    const exactSelection = taskId === undefined
      ? selection?.kind === "allTasks"
      : selection?.kind === "chooseOne" && selection.taskId === taskId;
    if (
      !authority
      || authority.review.landing.snapshot.mission.mission_id !== missionId
      || !exactSelection
    ) {
      throw new Error("Mission Landing must be the current review before apply");
    }
    return this.applyAndComplete(authority.review);
  }

  dispose(): void {
    this.currentReview = null;
  }

  private async selectLanding(missionId?: string, taskId?: string): Promise<MissionLanding | undefined> {
    if (missionId) {
      const snapshot = await this.host.getSnapshot(missionId);
      if (snapshot.mission.completion_policy !== "chooseOne") {
        const landing = missionLanding(snapshot);
        if (!landing) throw new Error(LANDING_NOT_READY);
        return landing;
      }
      if (taskId) {
        const landing = missionWinnerLanding(snapshot, taskId);
        if (!landing) throw new Error(LANDING_NOT_READY);
        return landing;
      }
      const winners = snapshot.tasks.flatMap((task) => {
        const landing = missionWinnerLanding(snapshot, task.task_id);
        return landing ? [{ task, landing }] : [];
      });
      if (winners.length === 0) throw new Error(LANDING_NOT_READY);
      if (winners.length === 1) return winners[0]?.landing;
      return vscode.window.showQuickPick(
        winners.map(({ task, landing }) => ({
          label: task.key,
          description: task.provider_selector,
          detail: `${sealedArtifactLabel(landing.artifacts.length)}  ${task.workspace ?? "workspace unavailable"}`,
          landing,
        })),
        {
          title: `Select one winner for ${snapshot.mission.name}`,
          placeHolder: "Only this exact Task Receipt will enter the project",
        },
      ).then((picked) => picked?.landing);
    }
    const queue = await this.currentQueue();
    if (queue.length === 0) {
      await vscode.window.showInformationMessage("Queue empty.");
      return undefined;
    }
    if (queue.length === 1) return queue[0];
    return vscode.window.showQuickPick(
      queue.map((entry) => ({
        label: entry.snapshot.mission.name,
        description: `${entry.artifacts.length} sealed`,
        detail: entry.snapshot.mission.project,
        entry,
      })),
      { title: "Landing Queue" },
    ).then((picked) => picked?.entry);
  }

  private async currentQueue(): Promise<readonly MissionLanding[]> {
    return missionLandingQueue(await this.host.listIntegratingSnapshots());
  }

  private async openAndRemember(landing: MissionLanding): Promise<ReviewedMissionLanding> {
    if (this.applying || this.reviewOpening) throw new Error("A Mission Landing review or apply is already running");
    this.reviewOpening = true;
    this.currentReview = null;
    try {
      const review = await openMissionLanding(landing, this.documents);
      this.currentReview = { review, state: "reviewed" };
      return review;
    } finally {
      this.reviewOpening = false;
    }
  }

  private async applyAndComplete(review: ReviewedMissionLanding): Promise<MissionSnapshot> {
    const authority = this.currentReview;
    if (!authority || authority.review !== review) throw new Error("A newer Mission Landing review replaced this one");
    if (this.applying) throw new Error("A Mission Landing apply is already running");
    this.applying = true;
    try {
      return await this.host.withProjectLease(review.landing.snapshot, async () => {
        const missionId = review.landing.snapshot.mission.mission_id;
        const latest = await this.host.getSnapshot(missionId);
        if (authority.state === "reviewed") {
          await applyReviewedLanding(review, latest);
          authority.state = "appliedAwaitingCore";
        } else {
          await assertReviewedLandingApplied(review, latest);
        }
        const completed = await completeLandingWithRecovery(
          latest,
          (snapshot) => this.host.complete(
            snapshot,
            review.landing.selection.kind === "chooseOne" ? review.landing.selection.taskId : null,
          ),
          () => this.host.getSnapshot(missionId),
          (snapshot) => assertReviewedLandingApplied(review, snapshot),
        );
        if (this.currentReview === authority) {
          this.currentReview = null;
          const tab = review.tab;
          if (tab && vscode.window.tabGroups.all.some((group) => group.tabs.includes(tab))) {
            await vscode.window.tabGroups.close(tab).then(undefined, () => undefined);
          }
        }
        return completed;
      });
    } finally {
      this.applying = false;
    }
  }
}

function sealedArtifactLabel(count: number): string {
  return `${count} sealed Artifact${count === 1 ? "" : "s"}`;
}
