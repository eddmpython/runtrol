import type { CoreClient } from "./client";
import type { Response } from "../protocol";

/// One question to the local daemon, with its failure raised as the daemon's own sentence.
///
/// Lives beside the client rather than in the administration flows that first needed it, because everything that
/// speaks the private dialect asks the same way, and a copy per caller is how two surfaces come to handle the
/// same failure differently.
export async function ask(
  client: CoreClient,
  request: Parameters<CoreClient["once"]>[0],
): Promise<Response> {
  const { response } = await client.once(request);
  if (response.say === "failed") {
    throw new Error(response.with.message);
  }
  return response;
}

/// Insist a mutation was acknowledged as done, naming the action a person would recognise.
export function expectDone(response: Response, action: string): void {
  if (response.say !== "done") {
    throw new Error(`the daemon answered ${action} with ${response.say}`);
  }
}
