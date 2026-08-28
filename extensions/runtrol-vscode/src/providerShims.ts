import { execFile } from "node:child_process";

/// Ask the Runtime to materialize every manifest-declared provider command in one dedicated directory.
///
/// The Runtime validates provider identities and owns the wrapper format. Studio only chooses its extension-owned
/// directory and places that directory at the front of future integrated terminal environments.
export function materializeProviderShims(executable: string, directory: string): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      ["shims", directory],
      { windowsHide: true, timeout: 20_000 },
      (error) => error ? reject(error) : resolve(),
    );
  });
}
