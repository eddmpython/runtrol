export type RenderSchedule = {
  requestFrame(callback: () => void): void;
  setDelay(callback: () => void, milliseconds: number): number;
  clearDelay(handle: number): void;
};

export function afterFrameOrDelay(
  schedule: RenderSchedule,
  maximumWaitMs: number,
  action: () => void,
): void {
  let completed = false;
  const complete = () => {
    if (completed) {
      return;
    }
    completed = true;
    action();
  };
  const fallback = schedule.setDelay(complete, maximumWaitMs);
  schedule.requestFrame(() => {
    schedule.clearDelay(fallback);
    complete();
  });
}
