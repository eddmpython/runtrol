/// What a service's private help line says, kept per service so the sidebar can draw from it.
///
/// The public inventory every window reads is validated against a closed schema by every shipped client, so a
/// new fact cannot ride on it without breaking the windows still running an older build (measured
/// 2026-08-29 against the 0.1.36 client). The admin `providerHelp` answer is additive by contract, and this is
/// its reader: asked once per set of usable services, and again when that set changes or an ask failed.
export type ProviderHelpFacts = {
  /// The service's own sign-out command, or null when it declares none.
  readonly signOut: string | null;
};

export class ProviderHelpCache {
  private facts = new Map<string, ProviderHelpFacts>();
  /// The set of services last asked about, so the same set is not asked twice.
  private asked = "";
  private inFlight: Promise<void> | null = null;
  private readonly listeners = new Set<() => void>();
  private disposed = false;

  constructor(private readonly ask: (providerId: string) => Promise<ProviderHelpFacts | null>) {}

  signOutFor(providerId: string): string | null {
    return this.facts.get(providerId)?.signOut ?? null;
  }

  /// Ask about these services, unless they are the set already asked about and answered.
  refresh(providerIds: readonly string[]): Promise<void> {
    if (this.disposed) return Promise.resolve();
    const key = [...providerIds].sort().join("\n");
    if (key === this.asked) return this.inFlight ?? Promise.resolve();
    if (this.inFlight) return this.inFlight;
    this.asked = key;
    this.inFlight = this.askAll(providerIds).finally(() => {
      this.inFlight = null;
    });
    return this.inFlight;
  }

  private async askAll(providerIds: readonly string[]): Promise<void> {
    let changed = false;
    let failed = false;
    for (const providerId of providerIds) {
      let answer: ProviderHelpFacts | null;
      try {
        answer = await this.ask(providerId);
      } catch {
        // The Core did not answer this ask (a reconnect in flight, a generation handing over). The sign-out
        // line is a convenience that simply stays absent, and forgetting the asked set below makes the next
        // refresh ask again rather than believe this one.
        failed = true;
        continue;
      }
      if (!answer) continue;
      if (this.facts.get(providerId)?.signOut !== answer.signOut) {
        this.facts.set(providerId, answer);
        changed = true;
      }
    }
    if (failed) this.asked = "";
    if (changed && !this.disposed) for (const listener of this.listeners) listener();
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
