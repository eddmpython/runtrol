import type * as vscode from "vscode";

import type { ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";

/// The usage strip the last window drew, kept so the next one draws bars instead of "Checking usage".
///
/// # Why this is remembered at all
///
/// Nothing can answer this question quickly. Asking a service where its account stands means starting that
/// service's own process and waiting for it to reach its vendor, which is seconds even when everything is
/// well, and the Core deliberately waits a few more before its first round so that a daemon measured in its
/// first moments is not mid-round. For that whole time the strip said "Checking usage", every time a window
/// opened, about numbers that had not moved since the last one closed.
///
/// # Why drawing it is not a lie, which took more care than the list did
///
/// A remembered conversation list is safe because it is what the services keep on disk and disk does not
/// change while the machine is asleep. Usage is the opposite: it moves on its own. A bar restored without
/// thought is a number claimed, and this file exists in a product whose rule is that a number is only ever
/// shown because a service said it.
///
/// So two things are thrown away rather than drawn.
///
/// **A window whose reset has passed.** That is the case that would mislead worst: a week that stood at 100
/// yesterday reads 100 again this morning when the truth is nought. The instant is in the window itself, so
/// this is not a guess about staleness, it is the service's own word about when its own number stopped
/// being true.
///
/// **A snapshot older than [`MAX_AGE`].** What is left after the first rule is a number that is still about
/// the right window, and the only question is how far it has drifted. Fifteen minutes of drift is small
/// against the alternative of showing nothing, and past that the honest answer is that this window does not
/// know yet.
///
/// What is never thrown away is the instant the reading was taken. The row carries it, and the hover says
/// how long ago it was, so a restored bar is a bar somebody can tell is a moment old.
/// The key this window's remembered strip is stored under.
const KEY = "runtrol.usageMemory.v1";

/// How old a remembered strip may be and still be drawn.
///
/// Set from what the numbers do rather than from a round figure: the shortest window any of these services
/// meters is five hours, so a quarter of an hour is a twentieth of it, which is under a percentage point of
/// drift even for somebody working without pause. The fresh answer replaces it seconds later anyway; this
/// bound is about what a person sees while that is happening.
const MAX_AGE_MS = 15 * 60_000;

/// How long a burst of usage changes may collect before one write is made.
///
/// The strip moves whenever a turn ends, and this is a convenience for the *next* window, so it always
/// waits for this one to be quiet rather than putting a settings write between an event and the panel.
const WRITE_AFTER_MS = 2_000;

/// What is kept: the gauges as published, and nothing derived.
type Remembered = {
  readonly gauges: readonly ProviderUsageGauge[];
};

/// The write this window still owes, when a burst is in progress.
let pending: ReturnType<typeof setTimeout> | null = null;
/// What that owed write would store, so a test can force it without waiting out the burst.
let owed: (() => Promise<void>) | null = null;

/// Remember the strip this window is currently drawing.
///
/// Fire and forget: a failed write costs the next window its head start and nothing else, which is not
/// worth interrupting this one over.
export function rememberUsage(
  memento: vscode.Memento,
  gauges: readonly ProviderUsageGauge[],
): void {
  owed = async () => {
    try {
      await memento.update(KEY, { gauges } satisfies Remembered);
    } catch {
      // ok: the next window says "Checking usage" as it used to, and nobody asked for this write.
    }
  };
  if (pending) clearTimeout(pending);
  pending = setTimeout(() => {
    pending = null;
    const write = owed;
    owed = null;
    if (write) void write();
  }, WRITE_AFTER_MS);
  // Never holds the editor open: a write owed at shutdown is one the next window can do without.
  pending.unref?.();
}

/// The strip the last window drew, with everything that has stopped being true taken out of it.
///
/// Shape-checked rather than trusted, because this value survives extension updates and a strip written by
/// an older build must be discarded rather than drawn as if its fields were still what this one expects.
export function rememberedUsage(
  memento: vscode.Memento,
  nowMs: number,
): readonly ProviderUsageGauge[] {
  const stored = memento.get<Remembered>(KEY);
  if (!stored || !Array.isArray(stored.gauges)) return [];
  return stored.gauges
    .filter(isGauge)
    .filter((gauge) => nowMs - gauge.atMs <= MAX_AGE_MS && gauge.atMs <= nowMs)
    .map((gauge) => ({
      ...gauge,
      windows: (gauge.windows ?? []).filter((window) => stillTrue(window, nowMs)),
    }))
    .filter((gauge) => gauge.windows.length > 0 || gauge.cost !== undefined);
}

/// Whether a remembered window is still about the period it was read in.
///
/// A window past its own reset is the one case worth being strict about: its number is not stale, it is
/// wrong, and wrong in the direction that tells somebody they are out of room when they have just been
/// given a fresh window. A window that stated no reset is kept, because the age bound already covers it.
function stillTrue(window: ProviderUsageWindow, nowMs: number): boolean {
  return typeof window.resetsAtMs !== "number" || window.resetsAtMs > nowMs;
}

function isGauge(value: unknown): value is ProviderUsageGauge {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  if (typeof record.providerId !== "string" || record.providerId.length === 0) return false;
  if (typeof record.atMs !== "number" || !Number.isFinite(record.atMs)) return false;
  if (typeof record.reached !== "boolean") return false;
  const windows = record.windows;
  if (windows !== undefined && !Array.isArray(windows)) return false;
  return (windows ?? []).every((window) => {
    if (!window || typeof window !== "object") return false;
    return typeof (window as Record<string, unknown>).id === "string";
  });
}

/// Make the owed write happen now. For a test, which cannot wait out a burst it did not create.
export async function writeRememberedUsageNow(): Promise<void> {
  if (pending) {
    clearTimeout(pending);
    pending = null;
  }
  const write = owed;
  owed = null;
  if (write) await write();
}
