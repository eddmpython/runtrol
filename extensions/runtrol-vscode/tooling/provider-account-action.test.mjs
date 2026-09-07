import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

test("account actions refresh only after their exact task ends, including an immediate exit", async () => {
  const compiled = await build({
    entryPoints: [fileURLToPath(new URL("../src/providerAccountAction.ts", import.meta.url))],
    bundle: true, platform: "node", format: "cjs", target: "node20", external: ["vscode"], write: false,
  });
  const listeners = new Set();
  const executions = [];
  let immediate = false;
  let fail = false;
  const end = (execution) => { for (const listener of listeners) listener({ execution, exitCode: 0 }); };
  const editor = {
    Task: class { constructor(definition, scope, name, source, execution) {
      Object.assign(this, { definition, scope, name, source, execution });
    } },
    ShellExecution: class { constructor(commandLine) { this.commandLine = commandLine; } },
    TaskScope: { Global: 1 }, TaskRevealKind: { Always: 1 }, TaskPanelKind: { Shared: 1 },
    tasks: {
      onDidEndTask: (listener) => {
        listeners.add(listener);
        return { dispose: () => listeners.delete(listener) };
      },
      onDidEndTaskProcess: (listener) => {
        listeners.add(listener);
        return { dispose: () => listeners.delete(listener) };
      },
      executeTask: async (task) => {
        if (fail) throw new Error("process launch failed");
        const execution = { task };
        executions.push(execution);
        if (immediate) end(execution);
        return execution;
      },
    },
  };
  const require = createRequire(import.meta.url);
  const module = { exports: {} };
  new Function("module", "exports", "require", compiled.outputFiles[0].text)(
    module, module.exports, (name) => name === "vscode" ? editor : require(name),
  );
  let refreshed = 0;
  const actions = new module.exports.ProviderAccountActions(() => { refreshed += 1; });
  const run = () => actions.run("fixture", "1.2.3", "Sign in", "fixture-cli auth login");
  await run();
  assert.equal(refreshed, 0);
  assert.equal(executions[0].task.execution.commandLine, "fixture-cli auth login");
  assert.equal(executions[0].task.definition.version, "1.2.3");
  await run();
  assert.equal(executions.length, 1, "repeated presses never interrupt or stack authentication");
  end({ task: { definition: { provider: "fixture", token: "unrelated" } } });
  end({ task: executions[0].task });
  assert.equal(refreshed, 0, "copied metadata is not the exact execution");
  end(executions[0]);
  assert.equal(refreshed, 1);
  end(executions[0]);
  assert.equal(refreshed, 1, "duplicate completion does not repeat the account request");
  immediate = true;
  await run();
  assert.equal(refreshed, 2, "an exit before executeTask resolves is retained");
  fail = true;
  await assert.rejects(run, /process launch failed/u);
  fail = false;
  await run();
  assert.equal(refreshed, 3, "failed launch releases the action for retry");
  immediate = false;
  await run();
  actions.dispose();
  end(executions.at(-1));
  assert.equal(refreshed, 3, "disposal releases the listener without owning the CLI's login");
  assert.equal(listeners.size, 0);
});
