import type { CoreClient } from "./core/client";

type ConnectionActions = typeof import("./connectionActions");
let actions: ConnectionActions | null = null;

function connectionActions(): ConnectionActions {
  // Connection, account, and restart UI are needed only after a person invokes their commands.
  actions ??= require("./connectionActions") as ConnectionActions;
  return actions;
}

export function pairPhone(client: CoreClient): Promise<void> {
  return connectionActions().pairPhone(client);
}

export function managePhones(client: CoreClient): Promise<void> {
  return connectionActions().managePhones(client);
}

export function reviewPhonePairings(client: CoreClient): Promise<void> {
  return connectionActions().reviewPhonePairings(client);
}

export function runAccountCommand(...args: Parameters<ConnectionActions["runAccountCommand"]>): Promise<void> {
  return connectionActions().runAccountCommand(...args);
}

export function restartExtensionHost(terminals: Parameters<ConnectionActions["restartExtensionHost"]>[0]): Promise<void> {
  return connectionActions().restartExtensionHost(terminals);
}
