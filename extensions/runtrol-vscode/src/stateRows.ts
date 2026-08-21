import type { NativeChatCatalogue, ProviderLine, SessionLine } from "./runtimeTypes";

export function sessionRowsEqual(left: readonly SessionLine[], right: readonly SessionLine[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((value, index) => {
    const candidate = right[index];
    return candidate !== undefined
      && value.sessionId === candidate.sessionId
      && value.providerId === candidate.providerId
      && value.nativeSessionId === candidate.nativeSessionId
      && value.label === candidate.label
      && value.workspace === candidate.workspace
      && value.hot === candidate.hot
      && value.lifecycle === candidate.lifecycle
      && value.looksStuck === candidate.looksStuck
      && value.sessionGeneration === candidate.sessionGeneration;
  });
}

export function providerRowsEqual(left: readonly ProviderLine[], right: readonly ProviderLine[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((value, index) => {
    const candidate = right[index];
    return candidate !== undefined
      && value.providerId === candidate.providerId
      && value.displayName === candidate.displayName
      && value.installation.state === candidate.installation.state
      && value.installation.version === candidate.installation.version
      && value.installation.why === candidate.installation.why;
  });
}

/// Why the conversation list is not everything, in each service's own words, or null when it is.
///
/// The daemon and the drivers already build these sentences ("some stored conversations name no
/// folder and are not shown"), and the extension used to keep them for a single error path. A reader who cannot see yesterday's conversation and is told nothing concludes the
/// list is complete and goes looking somewhere else, so the honest sentence belongs beside the list
/// it qualifies. Named per service, because "some chats are missing" tells nobody which ones.
///
/// Pure on purpose: this is the sentence the sidebar prints, and it is worth a test that does not
/// need an Extension Host to run.
export function incompleteDiscovery(
  catalogues: readonly NativeChatCatalogue[],
  providers: readonly ProviderLine[],
): string | null {
  const reasons: string[] = [];
  for (const catalogue of catalogues) {
    const coverage = catalogue.coverage;
    if (!coverage || coverage.kind === "complete") continue;
    const name = providers.find(
      (provider) => provider.providerId === catalogue.providerId,
    )?.displayName ?? catalogue.providerId;
    reasons.push(`${name}: ${coverage.why}`);
  }
  return reasons.length === 0 ? null : [...new Set(reasons)].sort().join(" · ");
}
