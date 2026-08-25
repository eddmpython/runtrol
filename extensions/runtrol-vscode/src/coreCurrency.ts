/// Proving the daemon that answers is the Core this extension installed.
///
/// The daemon is a generation: `runtrol endpoint`, run from the installed Core, connects to the endpoint named
/// by that Core's own digest and starts that build when nothing listens there. A newer build starts beside an
/// older one and the older one drains, so there is no retire request, no idle judgement, and no button here.
/// What remains is the check: the hello names the daemon's digest, and it must be the installed one. A
/// difference means the locator handed this window a foreign build (an operator corePath, or a daemon from
/// before generations still holding the store), which is reported once and re-checked until it matches.

import type { CoreClient } from "./core/client";

export type CurrencyOutcome =
  /// The daemon that answered is the installed build (or the operator chose their own corePath).
  | { state: "current" }
  /// A daemon of another build answered; the installed generation is not serving this window yet.
  | { state: "foreign"; announced: string | null };

export async function checkCoreCurrency(
  client: CoreClient,
  managedDigest: () => Promise<string | null>,
): Promise<CurrencyOutcome> {
  // Reaching the daemon and measuring the installed file are independent, and both are on the
  // path activation waits for, so they overlap.
  const [, installed] = await Promise.all([client.ensureRuntime(), managedDigest()]);
  if (installed === null) {
    // Not our binary: an operator-configured corePath or a PATH fallback.
    return { state: "current" };
  }
  const announced = await client.announcedBuildDigest();
  return announced === installed ? { state: "current" } : { state: "foreign", announced };
}
