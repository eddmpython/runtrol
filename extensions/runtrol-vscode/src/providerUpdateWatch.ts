import type { ProviderUpdateLine } from "./protocol";

/// The Core's last answer about each service's release.
///
/// The sidebar reads it to put the installed version beside a service's name and an "Update" button beside that
/// when a newer release is confirmed. The answer is the Core's (`provider_update.rs`): which package owns the
/// installed binary, whether the registry proves a newer plain release, and whether an exact rollback exists.
/// Nothing here decides any of that.
///
/// No clock of its own. The inspection asks the package registry over the network, and this repository bans
/// polling loops outright, so it is asked exactly when something happened: the window reaches a Core (fresh
/// activation or a reconnect after an update), an update just ran, or the person invokes the check command.
export class ProviderUpdateWatch {
  private lines = new Map<string, ProviderUpdateLine>();
  private inFlight: Promise<void> | null = null;
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(private readonly inspect: () => Promise<readonly ProviderUpdateLine[]>) {}

  /// The Core's line for this service, or undefined before the first answer.
  get(providerId: string): ProviderUpdateLine | undefined {
    return this.lines.get(providerId);
  }

  /// The release this service can be updated to, or null when it is current, unconfirmed, or unknown.
  updateTargetFor(providerId: string): string | null {
    const line = this.lines.get(providerId);
    return line?.state === "available" && line.target && line.rollback ? line.target : null;
  }

  /// The installed release as the Core saw it, or null.
  installedFor(providerId: string): string | null {
    return this.lines.get(providerId)?.installed ?? null;
  }

  /// One inspection. A failure keeps the previous answer: a registry that did not answer is not a release.
  check(): Promise<void> {
    if (this.inFlight) return this.inFlight;
    this.inFlight = this.inspect()
      .then((lines) => {
        if (this.disposed) return;
        const next = new Map(lines.map((line) => [line.provider, line] as const));
        const changed = !sameLines(this.lines, next);
        this.lines = next;
        if (changed) for (const listener of this.listeners) listener();
      })
      .catch(() => undefined)
      .finally(() => {
        this.inFlight = null;
      });
    return this.inFlight;
  }

  onDidChange(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return { dispose: () => this.listeners.delete(listener) };
  }

  dispose(): void {
    this.disposed = true;
    this.listeners.clear();
  }
}

function sameLines(
  a: ReadonlyMap<string, ProviderUpdateLine>,
  b: ReadonlyMap<string, ProviderUpdateLine>,
): boolean {
  if (a.size !== b.size) return false;
  for (const [provider, line] of a) {
    const other = b.get(provider);
    if (!other) return false;
    if (
      other.state !== line.state || other.installed !== line.installed
      || other.target !== line.target || other.rollback !== line.rollback
    ) return false;
  }
  return true;
}
