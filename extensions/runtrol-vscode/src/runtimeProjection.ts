import type {
  ManagedSessionList,
  ProviderList,
  RuntimeModelCatalog,
  SessionDescriptor,
} from "@runtrol/runtime-client";

export function projectProviders(snapshot: ProviderList) {
  return snapshot.providers.map((provider) => ({
    id: provider.providerId,
    display_name: provider.displayName,
    usable: provider.installation.state === "usable",
    why_not: provider.installation.why ?? null,
  }));
}

export function projectSessions(snapshot: ManagedSessionList) {
  return snapshot.sessions.map(projectSession);
}

export function projectSession(session: SessionDescriptor) {
  return {
    session: session.sessionId,
    provider: session.providerId,
    native: session.nativeSessionId ?? null,
    label: session.label ?? null,
    workspace: session.workspace,
    hot: session.hot,
    doing: lifecycleName(session),
    looks_stuck: session.looksStuck,
  };
}

export function projectModelCatalog(catalogue: RuntimeModelCatalog) {
  switch (catalogue.coverage) {
    case "known":
      return { kind: "known" as const, models: catalogue.models.map(projectModel) };
    case "aliases":
      return { kind: "aliases" as const, aliases: [...catalogue.aliases], why: catalogue.why };
    case "partial":
      return {
        kind: "partial" as const,
        aliases: [...catalogue.aliases],
        models: catalogue.models.map(projectModel),
        why: catalogue.why,
      };
    case "unknown":
    case "unsupported":
      return { kind: "unknown" as const, why: catalogue.why };
  }
}

function projectModel(model: Extract<RuntimeModelCatalog, { coverage: "known" }>["models"][number]) {
  return {
    id: model.id,
    displayName: model.displayName,
    description: model.description,
    isDefault: model.isDefault,
    reasoningEfforts: model.reasoningEfforts.map((effort) => ({ ...effort })),
  };
}

function lifecycleName(session: SessionDescriptor): string {
  switch (session.lifecycle) {
    case "hotIdle":
      return "idle";
    case "hotRunning":
      return "busy";
    case "failed":
      return "failed";
    case "cold":
      return "detached";
  }
}
