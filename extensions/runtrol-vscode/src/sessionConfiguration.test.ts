import assert from "node:assert/strict";
import test from "node:test";

import type { ModelCatalog } from "./runtimeTypes";
import { modelOptions, reasoningOptions } from "./sessionConfiguration";

test("runtime-discovered models and aliases become one deduplicated choice list", () => {
  const catalog: ModelCatalog = {
    coverage: "partial",
    aliases: ["fast", "provider-model"],
    models: [{
      id: "provider-model",
      displayName: "Provider Model",
      description: "Discovered now",
      isDefault: true,
      reasoningEfforts: [],
    }],
    reasoningEfforts: [],
    why: "partial provider surface",
  };
  const options = modelOptions(catalog);
  assert.deepEqual(options.map((option) => option.id), ["provider-model", "fast"]);
  assert.equal(options[0]?.description, "CLI default");
});

test("model-specific effort choices win and catalogue choices support aliases", () => {
  const catalog: ModelCatalog = {
    coverage: "partial",
    aliases: ["fast"],
    models: [{
      id: "provider-model",
      displayName: "Provider Model",
      description: "",
      isDefault: false,
      reasoningEfforts: [{ id: "model-effort", description: "For this model" }],
    }],
    reasoningEfforts: [{ id: "global-effort", description: "For aliases" }],
    why: "partial provider surface",
  };
  const model = catalog.models[0] ?? null;
  assert.deepEqual(reasoningOptions(catalog, model).map((choice) => choice.id), ["model-effort"]);
  assert.deepEqual(reasoningOptions(catalog, null).map((choice) => choice.id), ["global-effort"]);
});
