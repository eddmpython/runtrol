/// Which provider, if any, a shell command line starts: the pure half of the observed mirror
/// (`docs/vscodeSurface.md`, observed mirror). The command names come from the Runtime's provider inventory
/// (`ProviderDescriptor.commandNames`, the manifest's own names); nothing here knows a provider by heart.
export type ProviderCommandNames = ReadonlyMap<string, string>;

/// Build the lookup from the inventory: lowercase command name to provider id.
export function providerCommandNames(
  providers: ReadonlyArray<{ readonly providerId: string; readonly commandNames?: ReadonlyArray<string> }>,
): ProviderCommandNames {
  const names = new Map<string, string>();
  for (const provider of providers) {
    for (const name of provider.commandNames ?? []) names.set(programName(name), provider.providerId);
  }
  return names;
}

/// A command name reduced to what a shell resolves it by: lowercase, without a launcher extension.
function programName(file: string): string {
  return file.toLowerCase().replace(/\.(cmd|exe|bat|ps1|sh)$/, "");
}

/// The provider a command line invokes, or null. Only the program word is read: the first token, past a
/// PowerShell call operator or `call`, unquoted, reduced to its file name without a launcher extension. The
/// rest of the line is the provider's own business (the transparent shim carries it exactly; a mirror never
/// needs it).
export function providerOfCommand(commandLine: string, names: ProviderCommandNames): string | null {
  const tokens = commandLine.trim().split(/\s+/);
  let program = tokens[0] ?? "";
  if ((program === "&" || program.toLowerCase() === "call") && tokens.length > 1) program = tokens[1] ?? "";
  program = program.replace(/^["']+|["']+$/g, "");
  const name = programName(program.slice(Math.max(program.lastIndexOf("/"), program.lastIndexOf("\\")) + 1));
  if (name.length === 0) return null;
  return names.get(name) ?? null;
}

/// Split one captured string into chunks the Runtime accepts (64 KiB of UTF-8 each), as base64.
export function mirrorChunks(text: string, limitBytes = 64 * 1024): string[] {
  const bytes = Buffer.from(text, "utf8");
  const chunks: string[] = [];
  for (let at = 0; at < bytes.length; at += limitBytes) {
    chunks.push(bytes.subarray(at, Math.min(bytes.length, at + limitBytes)).toString("base64"));
  }
  return chunks;
}
