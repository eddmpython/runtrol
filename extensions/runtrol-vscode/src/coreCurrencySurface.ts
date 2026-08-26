/// The window-facing half of Core currency: silence when the installed generation answers, one message and a
/// quiet re-check when a foreign build does. The decision itself lives in coreCurrency.ts, which knows nothing
/// about windows and is unit-tested there.

import type { CoreClient } from "./core/client";
import type { CoreLocator } from "./core/locator";
import { checkCoreCurrency } from "./coreCurrency";

const RECHECK_MS = 30_000;

/// Keep this window on the installed generation for its life.
///
/// Only the first check is awaited, so activation never waits on a foreign daemon: the hello stays readable
/// across builds either way. The foreign case is the daemon from before generations still holding the store
/// while it finishes a turn; the installed generation is already started and takes over the moment that
/// store is released, so the re-check re-runs `runtrol endpoint` until the installed build answers.
export async function superviseCoreCurrency(
  client: CoreClient,
  locator: CoreLocator,
  /// Told whether an older generation is serving this window, so the sidebar can keep saying so.
  ///
  /// A notification used to carry this, and a notification is gone in five seconds. The person then spends the
  /// rest of the update watching the behaviour of a build they thought they had replaced, with nothing on
  /// screen to explain it. There must be no moment where somebody cannot tell why they are seeing the old
  /// behaviour, and a line that stays is the whole point.
  report: (updating: boolean) => void = () => undefined,
): Promise<void> {
  const outcome = await attemptOnce(client, locator);
  if (outcome === null || outcome.state === "current") {
    report(false);
    return;
  }
  report(true);
  void recheckUntilCurrent(client, locator, report);
}

async function attemptOnce(client: CoreClient, locator: CoreLocator) {
  try {
    return await checkCoreCurrency(client, () => locator.managedDigest());
  } catch {
    // ok: connecting failed entirely; whoever needs the daemon next reports that failure with its
    // own words, and there is no build to compare against a daemon we cannot reach.
    return null;
  }
}

async function recheckUntilCurrent(
  client: CoreClient,
  locator: CoreLocator,
  report: (updating: boolean) => void,
): Promise<void> {
  for (;;) {
    await new Promise((resolve) => setTimeout(resolve, RECHECK_MS));
    locator.invalidate();
    await client.reset();
    const outcome = await attemptOnce(client, locator);
    if (outcome === null || outcome.state === "current") {
      report(false);
      return;
    }
  }
}
