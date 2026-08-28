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

/// The services to ask for their stored conversations now: the usable ones this window has not asked yet.
///
/// A service becomes usable at a moment nobody chose. Its CLI can be replacing itself while a window opens, and
/// the Runtime's probe of it lands whenever it lands: measured on the operator machine 2026-08-28, five and a half
/// minutes after activation. Every caller that asks for conversations runs before that, so without this the window
/// asked while nothing was usable, never asked again, and showed every project with nothing under it for as long
/// as it stayed open. `asked` is what keeps that from becoming a question on every listing the watch pushes.
export function unaskedUsable(
  providers: readonly ProviderLine[],
  asked: ReadonlySet<string>,
): string[] {
  return providers
    .filter((provider) => isUsable(provider) && !asked.has(provider.providerId))
    .map((provider) => provider.providerId);
}
