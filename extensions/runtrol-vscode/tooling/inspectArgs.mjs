// The argument grammar for the VS Code inspection tool, split out so it can be tested without a window.
//
// One subcommand per line, each with its own required arguments. Parsing is total: a bad command or a missing
// argument returns an error the caller prints, never a half-built request that reaches a window.

/// The subcommands the tool answers, and what each needs.
export const SUBCOMMANDS = ["list", "capture", "keys", "click"];

/// Parse argv (already sliced past node and the script) into a request, or an { error } to print.
///
/// `--title` matches a window's title (default "Visual Studio Code", which every VS Code window ends with).
/// `--command` narrows to a process family whose command line contains the string, for one isolated window
/// among several. `capture` also takes `--out` (a PNG path) and `--front` (bring the window forward first,
/// which capture does not otherwise need). `keys` takes `--keys` (SendKeys vocabulary: ^ Ctrl, + Shift,
/// {ENTER}). `click` takes `--x` and `--y` (client-relative pixels).
export function parseInspectArgs(argv) {
  const [subcommand, ...rest] = argv;
  if (!subcommand) {
    return { error: `a subcommand is required: ${SUBCOMMANDS.join(", ")}` };
  }
  if (!SUBCOMMANDS.includes(subcommand)) {
    return { error: `unknown subcommand "${subcommand}"; use one of ${SUBCOMMANDS.join(", ")}` };
  }
  const flags = {};
  for (let index = 0; index < rest.length; index += 1) {
    const token = rest[index];
    if (!token.startsWith("--")) {
      return { error: `unexpected argument "${token}"` };
    }
    const name = token.slice(2);
    if (name === "front") {
      flags.front = true;
      continue;
    }
    const value = rest[index + 1];
    if (value === undefined || value.startsWith("--")) {
      return { error: `--${name} needs a value` };
    }
    flags[name] = value;
    index += 1;
  }

  const title = flags.title ?? "Visual Studio Code";
  const command = flags.command ?? "";
  if (typeof title !== "string" || title.length === 0) {
    return { error: "--title must not be empty" };
  }

  if (subcommand === "list") {
    return { subcommand, title, command };
  }
  if (subcommand === "capture") {
    return { subcommand, title, command, out: flags.out ?? null, front: Boolean(flags.front) };
  }
  if (subcommand === "keys") {
    if (!flags.keys) {
      return { error: "keys needs --keys (SendKeys vocabulary, e.g. \"^k^b\" or \"{ESC}\")" };
    }
    return { subcommand, title, command, keys: flags.keys };
  }
  // click
  const x = Number(flags.x);
  const y = Number(flags.y);
  if (!Number.isInteger(x) || !Number.isInteger(y) || x < 0 || y < 0) {
    return { error: "click needs --x and --y as non-negative integer client pixels" };
  }
  return { subcommand, title, command, x, y };
}
