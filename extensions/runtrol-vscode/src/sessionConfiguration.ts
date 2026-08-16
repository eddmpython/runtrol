import type { ModelCatalog, ModelChoice, ReasoningChoice } from "./runtimeTypes";

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
