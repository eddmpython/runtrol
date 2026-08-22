import * as vscode from "vscode";

import type { DiffDocuments } from "../../diffDocuments";
import type { MissionSnapshot } from "../../protocol";
import { applyReviewedLanding, assertReviewedLandingApplied } from "./apply";
import { completeLandingWithRecovery } from "./completion";
import { missionLanding, missionLandingQueue, type MissionLanding } from "./model";
import { openMissionLanding, type ReviewedMissionLanding } from "./review";

const APPLY_LANDING = "Apply, run Gates and complete";
const REVIEW_NEXT_LANDING = "Review next";
const LANDING_NOT_READY = "Mission cannot land";

export type LandingHost = {
  readonly getSnapshot: (missionId: string) => Promise<MissionSnapshot>;
  readonly listIntegratingSnapshots: () => Promise<readonly MissionSnapshot[]>;
  readonly complete: (snapshot: MissionSnapshot) => Promise<MissionSnapshot>;
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

  async reviewAndApply(missionId?: string): Promise<void> {
    if (this.publicReviewActive) throw new Error("A Mission Landing review is already open");
    this.publicReviewActive = true;
    try {
      let nextMissionId = missionId;
      for (;;) {
        const landing = await this.selectLanding(nextMissionId);
        if (!landing) return;
        const review = await this.openAndRemember(landing);
        const action = await vscode.window.showWarningMessage(
          `${landing.snapshot.mission.name}: apply ${landing.artifacts.length} sealed Artifacts, run fixed Gates, and complete?`,
          APPLY_LANDING,
        );
        if (action !== APPLY_LANDING) {
          if (this.currentReview?.review === review) this.currentReview = null;
          return;
        }
        const completed = await this.applyAndComplete(review);
        const remaining = (await this.currentQueue()).length;
        const next = await vscode.window.showInformationMessage(
          `${completed.mission.name} completed. ${remaining > 0 ? `${remaining} ready to land.` : "Queue clear."}`,
          ...(remaining > 0 ? [REVIEW_NEXT_LANDING] : []),
        );
        if (next !== REVIEW_NEXT_LANDING) return;
        nextMissionId = undefined;
      }
    } finally {
      this.publicReviewActive = false;
    }
  }

  async reviewForJourney(missionId: string): Promise<void> {
    const landing = missionLanding(await this.host.getSnapshot(missionId));
    if (!landing) throw new Error(LANDING_NOT_READY);
    await this.openAndRemember(landing);
  }

  async applyForJourney(missionId: string): Promise<MissionSnapshot> {
    const authority = this.currentReview;
    if (!authority || authority.review.landing.snapshot.mission.mission_id !== missionId) {
      throw new Error("Mission Landing must be the current review before apply");
    }
    return this.applyAndComplete(authority.review);
  }

  dispose(): void {
    this.currentReview = null;
  }

  private async selectLanding(missionId?: string): Promise<MissionLanding | undefined> {
    if (missionId) {
      const landing = missionLanding(await this.host.getSnapshot(missionId));
      if (!landing) throw new Error(LANDING_NOT_READY);
      return landing;
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
          (snapshot) => this.host.complete(snapshot),
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
