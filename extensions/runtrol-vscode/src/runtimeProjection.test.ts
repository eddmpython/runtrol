import assert from "node:assert/strict";
import test from "node:test";

import { projectModelCatalog, projectProviders, projectSessions } from "./runtimeProjection";

test("public Runtime inventory projects every Studio presentation field", () => {
  assert.deepEqual(projectProviders({
    providers: [{
      providerId: "fixture",
      displayName: "Fixture",
      installation: { state: "unavailable", why: "probe pending" },
    }],
  }), [{
    id: "fixture",
    display_name: "Fixture",
    usable: false,
    why_not: "probe pending",
  }]);
  assert.deepEqual(projectSessions({
    sessions: [{
      sessionId: "session_fixture",
      providerId: "fixture",
      nativeSessionId: "native_fixture",
      workspace: "C:\\workspace",
      hot: true,
      lifecycle: "hotRunning",
      looksStuck: true,
      sessionGeneration: 3,
      label: "Operator label",
    }],
    warnings: [],
  }), [{
    session: "session_fixture",
    provider: "fixture",
    native: "native_fixture",
    label: "Operator label",
    workspace: "C:\\workspace",
    hot: true,
    doing: "busy",
    looks_stuck: true,
  }]);
});

test("public model coverage keeps provider choices opaque", () => {
  assert.deepEqual(projectModelCatalog({
    coverage: "partial",
    aliases: ["opaque-alias"],
    models: [{
      id: "opaque-model",
      displayName: "Provider label",
      description: "Provider description",
      isDefault: true,
      reasoningEfforts: [{ id: "opaque-effort", description: "Provider effort" }],
    }],
    why: "partial official surface",
  }), {
    kind: "partial",
    aliases: ["opaque-alias"],
    models: [{
      id: "opaque-model",
      displayName: "Provider label",
      description: "Provider description",
      isDefault: true,
      reasoningEfforts: [{ id: "opaque-effort", description: "Provider effort" }],
    }],
    why: "partial official surface",
  });
});
