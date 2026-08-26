import type * as vscode from "vscode";

import type { NativeChatCatalogue } from "./runtimeTypes";

/// The last list this window drew, kept so the next one draws it before anything is asked.
///
/// # Why the list is remembered at all
///
/// Opening the panel used to mean waiting: the window appeared, the Core was located, a connection was made, an
/// integration was checked, every service was asked what it had, and only then did a conversation appear. Each of
/// those is fast on its own and the sum is not, and the person watching has nothing to look at while it happens.
/// A list that is already there when the panel opens is the whole difference between an editor panel and an app
/// somebody has to launch.
///
/// # Why this is safe to draw
///
/// What is remembered is what the coding services keep on disk, which is the part of the list that does not
/// change while this machine is asleep. It is not what is running: nothing here claims a conversation is live,
/// because that is exactly the claim only the daemon can make. So the first paint is the true shape of the
/// person's work with no live badges, and the badges arrive a moment later without the rows moving.
///
/// A conversation deleted from another window shows for the moment before the first listing lands. That is the
/// cost, and it is smaller than the cost of showing nothing: the row disappears on its own, and no action on a
/// stale row can do harm, because every action is served by the daemon against the service's own store.
///
/// # Why `globalState`
///
/// It is read synchronously, so the restore costs nothing measurable and can happen during activation before any
/// I/O. It is per profile, which is the same scope the rest of this window's memory has.
/// The key this window's remembered list is stored under.
const KEY = "runtrol.listMemory.v1";

/// How many conversations are worth remembering.
///
/// Enough for a busy machine (the operator's has around forty), bounded because this is written to a settings
/// file on every change and an unbounded list there is a slow leak nobody would notice until it was large.
const LIMIT = 600;

/// What is kept for one service: exactly the catalogue it published, trimmed.
type Remembered = {
  readonly catalogues: readonly NativeChatCatalogue[];
};

/// How long a burst of catalogue changes is allowed to collect before one write is made.
///
/// Discovery does not arrive once. Each service answers on its own schedule and a busy machine produces a run of
/// changes in a second or two; writing on every one of them put a settings-file write between the daemon's
/// events and the panel that was waiting for them. This is a convenience for the *next* window, so it can always
/// wait for the current one to be quiet.
const WRITE_AFTER_MS = 2_000;

/// The write this window still owes, when a burst is in progress.
let pending: ReturnType<typeof setTimeout> | null = null;
/// What that owed write would store, so it can be forced without waiting out the burst.
let owed: (() => Promise<void>) | null = null;

/// Remember the catalogues this window is currently drawing.
///
/// Fire and forget: the write is a convenience for the next window, so a failed write must never interrupt this
/// one. A rejection is swallowed deliberately, and the only consequence is that the next window waits as it used
/// to (the caller has nothing better to do about it, and telling the person their sidebar cache did not save
/// would be noise about something they never asked for).
export function rememberList(
  memento: vscode.Memento,
  catalogues: readonly NativeChatCatalogue[],
): void {
  let left = LIMIT;
  const trimmed: NativeChatCatalogue[] = [];
  for (const catalogue of catalogues) {
    if (left <= 0) break;
    trimmed.push({ ...catalogue, chats: catalogue.chats.slice(0, left) });
    left -= catalogue.chats.length;
  }
  owed = async () => {
    try {
      await memento.update(KEY, { catalogues: trimmed } satisfies Remembered);
    } catch {
      // ok: the next window waits as it used to, and nobody asked for this write in the first place.
    }
  };
  if (pending) clearTimeout(pending);
  pending = setTimeout(() => {
    pending = null;
    const write = owed;
    owed = null;
    if (write) void write();
  }, WRITE_AFTER_MS);
  // Never holds the window open: a write owed when the editor is closing is a write the next window can do
  // without.
  pending.unref?.();
}

/// The catalogues the last window drew, or an empty list when there is nothing remembered yet.
///
/// Shape-checked rather than trusted: this value survives extension updates, so a list written by an older
/// version must be discarded rather than drawn as if its fields were still what this build expects.
export function rememberedList(memento: vscode.Memento): readonly NativeChatCatalogue[] {
  const stored = memento.get<Remembered>(KEY);
  if (!stored || !Array.isArray(stored.catalogues)) return [];
  return stored.catalogues.filter(isCatalogue);
}

function isCatalogue(value: unknown): value is NativeChatCatalogue {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return typeof record.providerId === "string"
    && record.providerId.length > 0
    && Array.isArray(record.chats)
    && record.chats.every((chat) => {
      if (!chat || typeof chat !== "object") return false;
      const line = chat as Record<string, unknown>;
      return typeof line.providerId === "string" && typeof line.nativeSessionId === "string";
    });
}

/// Make the owed write happen now. For a test, which cannot wait out a burst it did not create.
export async function writeRememberedNow(): Promise<void> {
  if (pending) {
    clearTimeout(pending);
    pending = null;
  }
  const write = owed;
  owed = null;
  if (write) await write();
}
