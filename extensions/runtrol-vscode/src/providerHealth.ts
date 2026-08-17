import type { ProviderLine } from "./runtimeTypes";

/// The exact reason Runtime gives while a freshly seen executable has not been probed yet.
///
/// One spelling in one place. Two copies of this sentence would drift, and the copy that drifted would either
/// verify a provider forever or show a warning that never clears.
const AWAITING_PROBE = "the installed executable has not completed a verified probe";

/// A provider that is installed but has not yet been asked what it is.
///
/// Transient by construction: the surface answers it by running the probe, not by telling anyone about it.
export function awaitsVerification(provider: ProviderLine): boolean {
  return provider.installation.state === "unavailable" && provider.installation.why === AWAITING_PROBE;
}

/// A provider worth interrupting someone about.
///
/// Not installed is not a fault. It is the ordinary state of every coding service a person has not chosen, and
/// listing those would fill the entry point with absences. Only an installed service that still cannot run
/// qualifies, and only once its probe has actually finished.
export function isBroken(provider: ProviderLine): boolean {
  return provider.installation.state === "unavailable" && !awaitsVerification(provider);
}

export function isUsable(provider: ProviderLine): boolean {
  return provider.installation.state === "usable";
}
