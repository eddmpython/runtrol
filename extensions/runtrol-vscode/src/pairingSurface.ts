import type { CoreClient } from "./core/client";

type PairingActions = typeof import("./pairingAdministration");
let actions: PairingActions | null = null;

function pairingActions(): PairingActions {
  // Pairing UI and its encoder are needed only after a person invokes a phone command.
  actions ??= require("./pairingQrVendor") as PairingActions;
  return actions;
}

export function pairPhone(client: CoreClient): Promise<void> {
  return pairingActions().pairPhone(client);
}

export function managePhones(client: CoreClient): Promise<void> {
  return pairingActions().managePhones(client);
}

export function reviewPhonePairings(client: CoreClient): Promise<void> {
  return pairingActions().reviewPhonePairings(client);
}
