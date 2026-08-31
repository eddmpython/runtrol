import type { NativeActivity } from "@runtrol/runtime-client";

import { nativeProcessKey } from "./conversationList";

export type NativeActivityAnswer = readonly [providerId: string, activity: NativeActivity | null];

export type NativeActivityProjection = {
  readonly live: ReadonlySet<string>;
  readonly attachable: ReadonlySet<string>;
  readonly active: ReadonlySet<string>;
  readonly unconfirmed: ReadonlySet<string>;
  readonly liveByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly attachableByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly activeByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly unconfirmedByProvider: ReadonlyMap<string, ReadonlySet<string>>;
  readonly discoveredProviders: ReadonlySet<string>;
};

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
  const active = new Set<string>();
  const unconfirmed = new Set<string>();
  const liveByProvider = new Map<string, ReadonlySet<string>>();
  const attachableByProvider = new Map<string, ReadonlySet<string>>();
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
    const providerActive = new Set(activity?.active ?? []);
    const providerUnconfirmed = activity === null ? prior : new Set<string>();
    liveByProvider.set(providerId, providerLive);
    attachableByProvider.set(providerId, providerAttachable);
    activeByProvider.set(providerId, providerActive);
    unconfirmedByProvider.set(providerId, providerUnconfirmed);
    for (const nativeId of providerLive) {
      live.add(nativeProcessKey(providerId, nativeId));
      if (!prior.has(nativeId)) discoveredProviders.add(providerId);
    }
    for (const nativeId of providerAttachable) attachable.add(nativeProcessKey(providerId, nativeId));
    for (const nativeId of providerActive) active.add(nativeProcessKey(providerId, nativeId));
    for (const nativeId of providerUnconfirmed) unconfirmed.add(nativeProcessKey(providerId, nativeId));
  }
  return {
    live,
    attachable,
    active,
    unconfirmed,
    liveByProvider,
    attachableByProvider,
    activeByProvider,
    unconfirmedByProvider,
    discoveredProviders,
  };
}
