// Run the path-sensitive suites the way a POSIX runner sees them, from a Windows machine.
//
// Development here happens on Windows and CI runs three operating systems, so a fixture that spells
// a path with a backslash passes locally and fails there. Measured 2026-08-20: four such tests were
// red on the Linux runner for days while every developer machine was green.
//
// Two substitutions are needed and one of them is easy to forget. `process.platform` is what the
// product branches on, and esbuild's define handles it. But `path.basename`, `path.resolve` and
// friends are bound to the *running* platform and no define can move them: on Windows they keep
// treating a backslash as a separator, which is exactly the difference that hides the bug. So the
// `path` module is replaced with its own posix half. Without this second substitution the check is
// a false green, proven by injecting the original defect and watching it pass.
import { build } from "esbuild";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));

// Suites whose fixtures or assertions depend on how a path is spelled.
const SUITES = [
  "conversationList",
  "projects",
  "workspaceCollision",
  "nativeChatCatalogue",
  "stateRows",
  "sessionDisplay",
];

const out = await mkdtemp(path.join(tmpdir(), "posix-suite-"));
try {
  const shim = path.join(out, "posixPathShim.mjs");
  await writeFile(
    shim,
    [
      "// `path`, as a POSIX process would have received it.",
      "import real from 'node:path';",
      "const posix = real.posix;",
      "export default posix;",
      "export const {",
      "  basename, delimiter, dirname, extname, format, isAbsolute, join, normalize,",
      "  parse, relative, resolve, sep, toNamespacedPath,",
      "} = posix;",
      "export const win32 = real.win32;",
      "export { posix };",
    ].join("\n"),
    "utf8",
  );

  const usePosixPath = {
    name: "posix-path",
    setup(pluginBuild) {
      pluginBuild.onResolve({ filter: /^(node:)?path$/ }, (args) => {
        // The shim itself imports the real module to borrow its posix half. Redirecting that
        // import too would point the shim at itself, and the resulting cycle leaves every importer
        // holding `undefined` (measured: "Cannot read properties of undefined (reading 'posix')").
        if (args.importer === shim) return { path: "node:path", external: true };
        return { path: shim };
      });
    },
  };

  await build({
    entryPoints: Object.fromEntries(
      SUITES.map((suite) => [suite, path.join(extensionRoot, "src", `${suite}.test.ts`)]),
    ),
    outdir: out,
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node20",
    external: ["vscode"],
    define: { "process.platform": '"linux"' },
    plugins: [usePosixPath],
    logLevel: "silent",
  });

  const run = spawnSync(
    process.execPath,
    ["--test", ...SUITES.map((suite) => path.join(out, `${suite}.js`))],
    { encoding: "utf8" },
  );
  const text = `${run.stdout}${run.stderr}`;
  const pass = /^# pass (\d+)/m.exec(text)?.[1] ?? "?";
  const fail = /^# fail (\d+)/m.exec(text)?.[1] ?? "?";
  console.log(`[posixSuite] pass=${pass} fail=${fail}`);
  if (fail !== "0") {
    for (const line of text.split("\n")) {
      if (line.includes("not ok") || line.trim().startsWith("✖") || line.includes("Error") || line.includes("AssertionError")) console.log("  " + line.trim().slice(0, 160));
    }
  }
  process.exitCode = fail === "0" && pass !== "?" ? 0 : 1;
} finally {
  await rm(out, { recursive: true, force: true });
}
