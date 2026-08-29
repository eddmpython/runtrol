import type { TerminalDescriptor, TerminalIndexSnapshot } from "./runtimeTypes";

/// The hosted terminals of every Runtime generation, read as one list.
///
/// A conversation's terminal lives in the exact generation that opened it, and an update leaves that generation
/// draining beside the new one for as long as its conversations run. Each generation
/// publishes only its own terminals. A window that followed one generation therefore saw none of the others,
/// and the provider's own process roster still showed those conversations alive, so the sidebar took them for
/// terminals somebody else owned and refused to open them (measured 2026-08-29: five draining generations held
/// eight idle conversations, and every one of their rows was a dead end). One snapshot per generation, merged,
/// lets a row find its terminal whichever generation owns it and attach there instead of resuming a copy.
export class TerminalFleet {
  private readonly byGeneration = new Map<string, TerminalIndexSnapshot>();
  private readonly unreachable = new Map<string, string>();

  /// The latest snapshot one generation pushed.
  set(generation: string, snapshot: TerminalIndexSnapshot): void {
    this.byGeneration.set(generation, snapshot);
    this.unreachable.delete(generation);
  }

  /// A generation that ended, or is no longer listed, contributes nothing.
  delete(generation: string): void {
    this.byGeneration.delete(generation);
    this.unreachable.delete(generation);
  }

  /// A listed generation this window could not follow. Its terminals are unknown rather than absent, and the
  /// merged snapshot says so instead of quietly listing fewer conversations than the machine runs.
  markUnreachable(generation: string, why: string): void {
    this.byGeneration.delete(generation);
    this.unreachable.set(generation, why);
  }

  /// One snapshot for the sidebar. Generations are laid out in digest order, so the same fleet always reads the
  /// same way whichever generation happened to answer first.
  merged(): TerminalIndexSnapshot {
    const terminals: TerminalDescriptor[] = [];
    const warnings: string[] = [];
    for (const generation of [...this.byGeneration.keys()].sort(compare)) {
      const snapshot = this.byGeneration.get(generation);
      if (!snapshot) continue;
      terminals.push(...snapshot.terminals);
      warnings.push(...snapshot.warnings);
    }
    for (const generation of [...this.unreachable.keys()].sort(compare)) {
      warnings.push(`Runtime generation ${generation} could not be followed: ${this.unreachable.get(generation) ?? ""}`);
    }
    return { terminals, warnings };
  }
}

function compare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
