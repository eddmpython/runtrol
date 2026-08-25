/// The window-facing half of Core currency: silence when the installed generation answers, one message and a
/// quiet re-check when a foreign build does. The decision itself lives in coreCurrency.ts, which knows nothing
/// about windows and is unit-tested there.

import * as vscode from "vscode";

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
export async function superviseCoreCurrency(client: CoreClient, locator: CoreLocator): Promise<void> {
  const outcome = await attemptOnce(client, locator);
  if (outcome === null || outcome.state === "current") return;
  void vscode.window.showInformationMessage(
    "An older Runtrol Core is still finishing its running conversations. The installed build takes over when they end.",
  );
  void recheckUntilCurrent(client, locator);
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

async function recheckUntilCurrent(client: CoreClient, locator: CoreLocator): Promise<void> {
  for (;;) {
    await new Promise((resolve) => setTimeout(resolve, RECHECK_MS));
    locator.invalidate();
    await client.reset();
    const outcome = await attemptOnce(client, locator);
    if (outcome === null || outcome.state === "current") return;
  }
}
