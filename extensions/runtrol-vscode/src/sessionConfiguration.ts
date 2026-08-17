import type { ModelCatalog, ModelChoice, ReasoningChoice } from "./runtimeTypes";

/// Where the coding service used for the last new conversation is remembered.
///
/// This is the whole of what makes New chat answer its own question. Global rather than per workspace, because a
/// person's preferred agent follows them between projects while a project rarely has an opinion.
export const RECENT_SERVICE_KEY = "runtrol.recentService";

export type ModelOption = {
  label: string;
  id: string;
  model: ModelChoice | null;
  description?: string;
  detail?: string;
};

export function modelOptions(catalog: ModelCatalog): ModelOption[] {
  const models: ModelChoice[] = catalog.coverage === "known" || catalog.coverage === "partial"
    ? [...catalog.models]
    : [];
  const aliases = catalog.coverage === "aliases" || catalog.coverage === "partial"
    ? catalog.aliases
    : [];
  return [
    ...models.map((model) => ({
      label: model.displayName,
      id: model.id,
      model,
      description: model.isDefault ? "CLI default" : undefined,
      detail: model.description || undefined,
    })),
    ...aliases
      .filter((alias) => !models.some((model) => model.id === alias))
      .map((alias) => ({
        label: alias,
        id: alias,
        model: null,
        description: "CLI alias",
      })),
  ];
}

export function reasoningOptions(
  catalog: ModelCatalog,
  model: ModelChoice | null,
): readonly ReasoningChoice[] {
  if (model && model.reasoningEfforts.length > 0) {
    return model.reasoningEfforts;
  }
  return catalog.coverage === "aliases" || catalog.coverage === "partial"
    ? catalog.reasoningEfforts
    : [];
}
