import type { ProviderLine, SessionLine } from "./runtimeTypes";

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
