/// A fixed-capacity set that admits each value once until the oldest value is evicted.
///
/// JavaScript Set iteration follows insertion order, so eviction stays O(1) without a second queue
/// that would duplicate every retained value. Seeing a duplicate does not make it new again.
export class BoundedDedupe<Value> {
  private readonly values = new Set<Value>();

  constructor(private readonly capacity: number) {
    if (!Number.isSafeInteger(capacity) || capacity < 1) {
      throw new RangeError("BoundedDedupe capacity must be a positive safe integer");
    }
  }

  /// Remember a value and report whether it was newly admitted.
  remember(value: Value): boolean {
    if (this.values.has(value)) return false;
    if (this.values.size === this.capacity) {
      const oldest = this.values.values().next();
      if (!oldest.done) this.values.delete(oldest.value);
    }
    this.values.add(value);
    return true;
  }
}
