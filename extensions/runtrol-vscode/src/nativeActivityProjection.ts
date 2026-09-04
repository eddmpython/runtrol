import type { NativeActivity } from "@runtrol/runtime-client";

import { nativeProcessKey } from "./conversationList";

export type NativeActivityAnswer = readonly [providerId: string, activity: NativeActivity | null];

export type NativeActivityProjection = {
  readonly live: ReadonlySet<string>;
  readonly attachable: ReadonlySet<string>;
  /// Live conversations a registered VS Code window can show and be brought forward for.
  readonly focusable: ReadonlySet<string>;
  readonly active: ReadonlySet<string>;
  readonly unconfirmed: ReadonlySet<string>;
  readonly liveByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly attachableByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly focusableByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly activeByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly unconfirmedByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly discoveredProviders: ReadonlySet<string>;
};

/// Providers that own a live conversation no row lists, each with the sorted identities that are missing.
///
/// A process that has not written its first message yet has no stored conversation, and one whose first message
/// came after the single refresh its discovery triggered has one that nobody has read. Nothing else would ever
/// ask again (measured 2026-09-04: a provider started in a terminal and answering its first message stayed on no
/// row in any window for as long as the journey waited). A conversation held by a daemon-owned terminal is asked
/// for the same way: its placeholder stands until the catalogue names it, and the terminal's own binding fired one
/// refresh that could still predate the provider's write (measured 2026-09-05).
export function unlistedLiveProviders(
  liveByProvider: ReadonlyMap<string, ReadonlySet<string>>,
  listed: ReadonlySet<string>,
): ReadonlyMap<string, string> {
  const unlisted = new Map<string, string>();
  for (const [providerId, live] of liveByProvider) {
    const missing = [...live]
      .filter((nativeId) => !listed.has(nativeProcessKey(providerId, nativeId)))
      .sort();
    if (missing.length > 0) unlisted.set(providerId, missing.join(" "));
  }
  return unlisted;
}

/// Project one complete roster round into sidebar keys.
///
/// A process is live only while the current round proves it. A failed request revokes the live badge, but carries
/// the previously observed identity into a deny-only `unconfirmed` set until a successful roster clears or restores
/// it. That prevents both a stale `Elsewhere` badge and a second owner racing a temporarily unreachable process.
export function projectNativeActivity(
  answers: readonly NativeActivityAnswer[],
  previousLive: ReadonlyMap<string, ReadonlySet<string>>,
  previousUnconfirmed: ReadonlyMap<string, ReadonlySet<string>> = new Map(),
): NativeActivityProjection {
  const live = new Set<string>();
  const attachable = new Set<string>();
  const focusable = new Set<string>();
  const active = new Set<string>();
  const unconfirmed = new Set<string>();
  const liveByProvider = new Map<string, ReadonlySet<string>>();
  const attachableByProvider = new Map<string, ReadonlySet<string>>();
  const focusableByProvider = new Map<string, ReadonlySet<string>>();
  const activeByProvider = new Map<string, ReadonlySet<string>>();
  const unconfirmedByProvider = new Map<string, ReadonlySet<string>>();
  const discoveredProviders = new Set<string>();
  for (const [providerId, activity] of answers) {
    const prior = new Set([
      ...(previousLive.get(providerId) ?? []),
      ...(previousUnconfirmed.get(providerId) ?? []),
    ]);
    const providerLive = new Set(activity?.live ?? activity?.active ?? []);
    const providerAttachable = new Set(
      (activity?.attachable ?? []).filter((nativeId) => providerLive.has(nativeId)),
    );
    const providerFocusable = new Set(
      (activity?.focusable ?? []).filter((nativeId) => providerLive.has(nativeId)),
    );
    const providerActive = new Set(activity?.active ?? []);
    const providerUnconfirmed = activity === null ? prior : new Set<string>();
    liveByProvider.set(providerId, providerLive);
    attachableByProvider.set(providerId, providerAttachable);
    focusableByProvider.set(providerId, providerFocusable);
    activeByProvider.set(providerId, providerActive);
    unconfirmedByProvider.set(providerId, providerUnconfirmed);
    for (const nativeId of providerLive) {
      live.add(nativeProcessKey(providerId, nativeId));
      if (!prior.has(nativeId)) discoveredProviders.add(providerId);
    }
    for (const nativeId of providerAttachable) attachable.add(nativeProcessKey(providerId, nativeId));
    for (const nativeId of providerFocusable) focusable.add(nativeProcessKey(providerId, nativeId));
    for (const nativeId of providerActive) active.add(nativeProcessKey(providerId, nativeId));
    for (const nativeId of providerUnconfirmed) unconfirmed.add(nativeProcessKey(providerId, nativeId));
  }
  return {
    live,
    attachable,
    focusable,
    active,
    unconfirmed,
    liveByProvider,
    attachableByProvider,
    focusableByProvider,
    activeByProvider,
    unconfirmedByProvider,
    discoveredProviders,
  };
}
