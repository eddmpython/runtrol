import { execFile } from "node:child_process";
import { newMutationRequestId, type TerminalDescriptor, type TerminalView } from "@runtrol/runtime-client";

/// The installed courier owns its command vocabulary, environment names, and executable limits.
export function readDialogueGuide(executable: string, terminal?: TerminalDescriptor): Promise<string> {
  const words = ["courier", "--guide"];
  if (terminal?.spawnedBy && terminal.initialMessageId) {
    words.push("--from", terminal.spawnedBy, "--message-id", terminal.initialMessageId);
  }
  return new Promise((resolve, reject) => {
    execFile(executable, words,
      { windowsHide: true, timeout: 5_000, maxBuffer: 64 * 1024, encoding: "utf8" },
      (error, stdout) => {
        if (error) { reject(error); return; }
        const guide = stdout.trim();
        if (!guide) { reject(new Error("The installed courier returned an empty dialogue guide.")); return; }
        resolve(guide);
      });
  });
}

/// A visible operator action arms one live process and sends ordinary terminal input under the same lease.
export async function setTerminalDialogue(view: TerminalView, enabled: boolean, guide: string | null): Promise<void> {
  const terminal = view.opened.terminal;
  if (enabled && !terminal.dialogueEnabled && !guide) {
    throw new Error("A visible instruction is required to enable dialogue.");
  }
  const lease = await view.acquireControl({
    requestId: newMutationRequestId(), terminalId: terminal.terminalId,
    expectedTerminalGeneration: terminal.terminalGeneration,
  });
  const control = {
    terminalId: terminal.terminalId, leaseId: lease.leaseId, leaseGeneration: lease.leaseGeneration,
  };
  await view.setDialogue({ requestId: newMutationRequestId(), ...control, enabled });
  if (!enabled || terminal.dialogueEnabled) return;
  try {
    // End finishes native paste-burst handling before Enter. Cursor movement is deterministic and avoids
    // guessing how long a provider needs to consume the paste. Neither write is retried or inspected.
    await view.write({ requestId: newMutationRequestId(), ...control,
      bytesBase64: Buffer.from(`\x1b[200~${guide}\x1b[201~`, "utf8").toString("base64") });
    await view.write({ requestId: newMutationRequestId(), ...control,
      bytesBase64: Buffer.from("\x1b[F\r", "utf8").toString("base64") });
  } catch (error) {
    try {
      await view.setDialogue({ requestId: newMutationRequestId(), ...control, enabled: false });
    } catch (rollbackError) {
      throw new AggregateError([error, rollbackError],
        "The dialogue instruction failed and its activation could not be disabled. Check this session's dialogue state.");
    }
    throw error;
  }
}
