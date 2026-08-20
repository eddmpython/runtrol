/// Rolling the running daemon forward to the Core this extension installed.
///
/// The extension replaces the Core binary on disk at every activation, but a daemon that was
/// already running keeps serving its old build, and old build means old dialect: on 2026-08-20
/// every installed daemon failed the freshly updated client's schema at hello. The hello is kept
/// eternally compatible so this comparison can happen at all; everything past the hello assumes
/// same-version peers, and this module is what makes that assumption true.
///
/// The daemon announces its own executable digest in the greeting. When it differs from the file
/// this extension installed, the daemon is asked to retire; it refuses while any conversation has
/// a live process, and this module retries when asked. A daemon too old to know "retire" is the
/// legacy case: the person at the machine gets one button that does the same thing by exact
/// process identity, because only a human should confirm replacing a build we cannot interrogate.

import { ask } from "./core/ask";
import type { CoreClient } from "./core/client";

export type SupersessionOutcome =
  /// The running daemon is the installed build (or the operator chose their own corePath).
  | { state: "current" }
  /// An older daemon retired and the installed build now answers.
  | { state: "superseded" }
  /// An older daemon keeps serving while conversations run; retry when the machine goes idle.
  | { state: "busy"; detail: string }
  /// The daemon predates the retire request (or refused it namelessly); only a person may act.
  | { state: "legacy"; detail: string };

export async function ensureCurrentCore(
  client: CoreClient,
  managedDigest: () => Promise<string | null>,
): Promise<SupersessionOutcome> {
  await client.ensureRuntime();
  const installed = await managedDigest();
  if (installed === null) {
    // Not our binary to roll: an operator-configured corePath or a PATH fallback.
    return { state: "current" };
  }
  if (await client.announcedBuildDigest() === installed) {
    return { state: "current" };
  }
  let retired;
  try {
    retired = await ask(client, { ask: "retire" });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    return detail.includes("live process")
      ? { state: "busy", detail }
      : { state: "legacy", detail };
  }
  if (retired.say !== "done") {
    return { state: "legacy", detail: `the daemon answered retire with ${retired.say}` };
  }
  // The old daemon exits after that answer. Reconnecting spawns the installed build the same way
  // any daemon is started, and the fresh greeting proves which build answered.
  await client.reset();
  await retryWhileTheOldDaemonExits(client);
  const successor = await client.announcedBuildDigest();
  if (successor !== installed) {
    return {
      state: "legacy",
      detail: "the daemon that answered after retirement is still not the installed build",
    };
  }
  return { state: "superseded" };
}

/// Connect to the successor, riding out the moment where the retiring daemon still owns the pipe.
export async function retryWhileTheOldDaemonExits(client: CoreClient): Promise<void> {
  const attempts = 10;
  for (let attempt = 1; ; attempt += 1) {
    try {
      await client.ensureRuntime();
      return;
    } catch (error) {
      if (attempt === attempts) throw error;
      await client.reset();
      await new Promise((resolve) => setTimeout(resolve, 200 * attempt));
    }
  }
}
