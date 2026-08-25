// The VS Code inspection tool: eyes and arms on a running VS Code window, for an AI developing runtrol and
// for an operator (or an AI) supervising the coding agents runtrol runs.
//
// It reuses the same Win32 primitives the eye pass has always used, promoted from one-shot test steps into a
// tool any session can call:
//
//   node tooling/inspect-vscode.mjs list                        # which VS Code windows exist right now
//   node tooling/inspect-vscode.mjs capture [--out shot.png]    # photograph one, occluded or not (eyes)
//   node tooling/inspect-vscode.mjs keys --keys "^k^b"          # type into it (arms)
//   node tooling/inspect-vscode.mjs click --x 120 --y 240       # click a client point (arms)
//
// Targeting: --title matches the window title (default "Visual Studio Code"), --command narrows to one
// isolated process family. Capture uses PrintWindow, so it renders the window's own surface without stealing
// focus: it is safe to run against the operator's live window and is how a message or toast on screen is
// "checked" (the caller reads the PNG). keys and click must bring the window forward, so they take focus.
//
// Windows only, like the primitives it wraps: PrintWindow and foreground-verified input are Win32. Elsewhere
// it says so and exits, rather than pretending to have looked.

import { execFile } from "node:child_process";
import { mkdir } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { parseInspectArgs } from "./inspectArgs.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

function fail(message, code = 1) {
  process.stderr.write(`${message}\n`);
  process.exit(code);
}

/// Run one of the sibling PowerShell primitives and return its stdout, or reject with its own words.
function powershell(script, args) {
  return new Promise((resolve, reject) => {
    execFile(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(here, script), ...args],
      { timeout: 30_000, windowsHide: true },
      (error, stdout, stderr) => {
        if (error) {
          reject(new Error((stderr || stdout || error.message).trim()));
        } else {
          resolve(stdout.trim());
        }
      },
    );
  });
}

async function main() {
  const request = parseInspectArgs(process.argv.slice(2));
  if (request.error) {
    fail(request.error, 2);
  }
  if (process.platform !== "win32") {
    fail("inspect-vscode looks at real Windows windows; it has nothing to look at on this platform", 3);
  }

  if (request.subcommand === "list") {
    // find-window prints the matched title, or nothing. Reused as an existence probe.
    const title = await powershell("find-window.ps1", [
      "-TitleMatch", request.title,
      ...(request.command ? ["-CommandLineMatch", request.command] : []),
    ]).catch((error) => fail(error.message, 4));
    if (!title) {
      process.stdout.write(`no VS Code window matches title "${request.title}"`
        + `${request.command ? ` and command "${request.command}"` : ""}\n`);
      process.exit(0);
    }
    process.stdout.write(`${title}\n`);
    return;
  }

  if (request.subcommand === "capture") {
    const out = request.out
      ? path.resolve(request.out)
      : path.join(os.tmpdir(), "runtrol-inspect", `vscode-${stamp()}.png`);
    await mkdir(path.dirname(out), { recursive: true });
    if (request.front) {
      // A caller that wants the window forward first (to dismiss an overlay, say) asks for it; capture itself
      // does not need it, which is why it is off by default.
      await powershell("press-keys.ps1", [
        "-TitleMatch", request.title,
        "-Keys", "",
        ...(request.command ? ["-CommandLineMatch", request.command] : []),
      ]).catch(() => undefined);
    }
    const said = await powershell("capture-window.ps1", [
      "-TitleMatch", request.title,
      "-OutPath", out,
      ...(request.command ? ["-CommandLineMatch", request.command] : []),
    ]).catch((error) => fail(error.message, 4));
    process.stdout.write(`${said}\n`);
    return;
  }

  if (request.subcommand === "keys") {
    const said = await powershell("press-keys.ps1", [
      "-TitleMatch", request.title,
      "-Keys", request.keys,
      ...(request.command ? ["-CommandLineMatch", request.command] : []),
    ]).catch((error) => fail(error.message, 4));
    process.stdout.write(`${said || `sent keys to "${request.title}"`}\n`);
    return;
  }

  // click
  const said = await powershell("click-window.ps1", [
    "-TitleMatch", request.title,
    "-X", String(request.x),
    "-Y", String(request.y),
  ]).catch((error) => fail(error.message, 4));
  process.stdout.write(`${said}\n`);
}

/// A filesystem-safe timestamp, without the arbitrary characters a locale string carries.
function stamp() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

await main();
