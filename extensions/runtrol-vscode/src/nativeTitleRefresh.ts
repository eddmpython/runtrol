import type { SessionLine } from "./runtimeTypes";

/// Providers whose own conversation catalogue may now carry a fresher title.
///
/// Runtime deliberately stores no conversation title. A provider-owned title therefore arrives through that
/// provider's native catalogue, not through the session index. Two index changes tell Studio that reading the
/// catalogue again is useful: a provider identity first appears, or a running turn settles after the provider had
/// a chance to name its conversation. The set keeps simultaneous sessions from spawning duplicate catalogue reads.
export function nativeTitleRefreshProviders(
  previous: readonly SessionLine[],
  current: readonly SessionLine[],
): string[] {
  const before = new Map(previous.map((session) => [session.sessionId, session]));
  const providers = new Set<string>();
  for (const session of current) {
    if (!session.nativeSessionId) continue;
    const prior = before.get(session.sessionId);
    const gainedNativeIdentity = !prior?.nativeSessionId;
    const turnSettled = prior?.lifecycle === "hotRunning" && session.lifecycle !== "hotRunning";
    if (gainedNativeIdentity || turnSettled) providers.add(session.providerId);
  }
  return [...providers];
}

type TitleBinding = {
  readonly session: SessionLine | null;
  updateSession(session: SessionLine): void;
};

/// Fan a newly read provider title into its open conversation surfaces without disturbing their watches.
export function refreshProviderTitleBindings(
  bindings: Iterable<TitleBinding>,
  providerId: string,
): void {
  for (const binding of bindings) {
    const session = binding.session;
    if (session?.providerId === providerId) binding.updateSession(session);
  }
}
