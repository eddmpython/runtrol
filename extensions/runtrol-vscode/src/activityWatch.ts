import * as vscode from "vscode";

import { ActivityWatchGate } from "./activityWatchGate";
import type { StudioRuntimeClient } from "./runtimeClient";
import { activityAfter, sameActivity, type SessionActivity } from "./sessionActivity";
import type { RuntimeState } from "./state";

/// How long activity changes are coalesced before the sidebar repaints. A turn streams many tool frames a
/// second; the row needs the word, not every frame.
const REPAINT_COALESCE_MS = 200;
/// How long a failed watch rests before trying again.
const RETRY_MS = 2_000;

/// Keeps the sidebar's "what is it doing" word current for every running conversation, whether or not its
/// page is open.
///
/// One light watch per running session (the Runtime's hot ceiling bounds how many that can be), reading the
/// same event stream the page reads and keeping nothing but the reduced `SessionActivity`. A session that stops
/// running loses its watch and its tool word; its sign-in flag stays until a fresh attachment lowers it.
export class ActivityWatcher implements vscode.Disposable {
  private readonly watches = new Map<string, AbortController>();
  private readonly pending = new Map<string, SessionActivity>();
  private readonly openings = new ActivityWatchGate();
  private repaint: NodeJS.Timeout | null = null;
  private readonly subscription: vscode.Disposable;
  private disposed = false;

  constructor(
    private readonly runtime: StudioRuntimeClient,
    private readonly state: RuntimeState,
  ) {
    this.subscription = state.onDidChange((change) => {
      if (change === "rows") this.sync();
    });
    this.sync();
  }

  /// Start watches for sessions that began running and stop the ones that no longer are.
  private sync(): void {
    if (this.disposed) return;
    const running = new Set(
      this.state.sessions
        .filter((session) => session.lifecycle === "hotRunning")
        .map((session) => session.sessionId),
    );
    for (const [sessionId, abort] of this.watches) {
      if (running.has(sessionId)) continue;
      abort.abort();
      this.watches.delete(sessionId);
      const current = this.state.activity(sessionId);
      if (current.tool !== null) this.queue(sessionId, { ...current, tool: null });
    }
    for (const sessionId of running) {
      if (this.watches.has(sessionId)) continue;
      const abort = new AbortController();
      this.watches.set(sessionId, abort);
      void this.watch(sessionId, abort.signal);
    }
  }

  private async watch(sessionId: string, signal: AbortSignal): Promise<void> {
    while (!signal.aborted && !this.disposed) {
      const releaseOpening = await this.openings.acquire(signal);
      if (!releaseOpening) return;
      let opening = true;
      try {
        // No cursor: the Runtime replays its bounded recent window first, which is exactly the recent past
        // the word comes from, then the live stream.
        await this.runtime.watchEvents(
          sessionId,
          null,
          {
            started: () => {
              if (!opening) return;
              opening = false;
              releaseOpening();
            },
            event: (payload) => {
              const previous = this.pending.get(sessionId) ?? this.state.activity(sessionId);
              const next = activityAfter(previous, payload);
              if (!sameActivity(previous, next)) this.queue(sessionId, next);
              return true;
            },
            gap: () => {},
          },
          signal,
        );
      } catch {
        // The session may have just stopped or the Runtime reconnected; the next sync decides whether this
        // watch should still exist, and the wait keeps a refused watch from spinning.
      } finally {
        if (opening) releaseOpening();
      }
      await delay(RETRY_MS, signal);
    }
  }

  private queue(sessionId: string, activity: SessionActivity): void {
    this.pending.set(sessionId, activity);
    this.repaint ??= setTimeout(() => {
      this.repaint = null;
      const batch = [...this.pending];
      this.pending.clear();
      this.state.setActivities(batch);
    }, REPAINT_COALESCE_MS);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.subscription.dispose();
    for (const abort of this.watches.values()) abort.abort();
    this.watches.clear();
    if (this.repaint) clearTimeout(this.repaint);
    this.pending.clear();
  }
}

function delay(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve();
      return;
    }
    const timer = setTimeout(done, ms);
    function done(): void {
      signal.removeEventListener("abort", done);
      clearTimeout(timer);
      resolve();
    }
    signal.addEventListener("abort", done, { once: true });
  });
}
