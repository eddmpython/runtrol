import type { ProviderUpdateLine } from "./protocol";

/// How often the Core is asked again whether a newer release of each service exists.
///
/// The inspection asks the package registry over the network (up to thirty seconds a service), so it is not on
/// the render path and not on any short clock. A release lands a few times a week; once every few hours finds it
/// the same day, and a person who wants it now has the command.
const RECHECK_EVERY_MS = 6 * 60 * 60 * 1000;

/// The Core's last answer about each service's release, and when to ask again.
///
/// The sidebar reads it to put the installed version beside a service's name and an "Update" button beside that
/// when a newer release is confirmed. The answer is the Core's (`provider_update.rs`): which package owns the
/// installed binary, whether the registry proves a newer plain release, and whether an exact rollback exists.
/// Nothing here decides any of that.
export class ProviderUpdateWatch {
  private lines = new Map<string, ProviderUpdateLine>();
  private timer: NodeJS.Timeout | null = null;
  private inFlight: Promise<void> | null = null;
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly inspect: () => Promise<readonly ProviderUpdateLine[]>,
    private readonly recheckEveryMs = RECHECK_EVERY_MS,
  ) {}

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

  /// Ask now, and keep asking on the long clock from here. Safe to call again: one inspection at a time.
  start(): Promise<void> {
    if (this.timer === null && !this.disposed) {
      this.timer = setInterval(() => void this.check(), this.recheckEveryMs);
    }
    return this.check();
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
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
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
