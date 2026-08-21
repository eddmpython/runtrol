export type AmbiguousSubmissionWriter = (taskIds: readonly string[]) => PromiseLike<void>;

/// A small durable safety marker for the gap between a committed Mission Send intent and public Runtime delivery.
/// It stores Task identities only. Updates serialize so parallel provider acknowledgements cannot lose a marker.
export class AmbiguousSubmissions {
  private readonly taskIds: Set<string>;
  private persistence: Promise<void> = Promise.resolve();

  constructor(
    initial: readonly string[],
    private readonly write: AmbiguousSubmissionWriter,
  ) {
    this.taskIds = new Set(initial);
  }

  current(): ReadonlySet<string> {
    return this.taskIds;
  }

  mark(taskId: string): Promise<void> {
    return this.persist(taskId, true);
  }

  clear(taskId: string): Promise<void> {
    return this.persist(taskId, false);
  }

  private persist(taskId: string, ambiguous: boolean): Promise<void> {
    const update = this.persistence.then(async () => {
      if (this.taskIds.has(taskId) === ambiguous) return;
      const next = new Set(this.taskIds);
      if (ambiguous) next.add(taskId);
      else next.delete(taskId);
      await this.write([...next].sort());
      this.taskIds.clear();
      for (const id of next) this.taskIds.add(id);
    });
    this.persistence = update.then(
      () => undefined,
      () => undefined,
    );
    return update;
  }
}
