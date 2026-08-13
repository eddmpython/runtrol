import type { ProviderLine, SessionLine } from "./protocol";

export function sessionRowsEqual(left: readonly SessionLine[], right: readonly SessionLine[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((value, index) => {
    const candidate = right[index];
    return candidate !== undefined
      && value.session === candidate.session
      && value.provider === candidate.provider
      && value.native === candidate.native
      && value.label === candidate.label
      && value.workspace === candidate.workspace
      && value.hot === candidate.hot
      && value.doing === candidate.doing
      && value.looks_stuck === candidate.looks_stuck;
  });
}

export function providerRowsEqual(left: readonly ProviderLine[], right: readonly ProviderLine[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  return left.every((value, index) => {
    const candidate = right[index];
    return candidate !== undefined
      && value.id === candidate.id
      && value.display_name === candidate.display_name
      && value.usable === candidate.usable
      && value.why_not === candidate.why_not;
  });
}
