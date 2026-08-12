import assert from "node:assert/strict";
import test from "node:test";

import { afterFrameOrDelay, type RenderSchedule } from "./renderReady";

test("the first animation frame acknowledges the render once", () => {
  const fixture = scheduleFixture();
  let acknowledgements = 0;
  afterFrameOrDelay(fixture.schedule, 250, () => {
    acknowledgements += 1;
  });

  fixture.frame();
  fixture.delay();

  assert.equal(acknowledgements, 1);
  assert.equal(fixture.cleared(), true);
});

test("a bounded delay acknowledges a render when animation frames stop", () => {
  const fixture = scheduleFixture();
  let acknowledgements = 0;
  afterFrameOrDelay(fixture.schedule, 250, () => {
    acknowledgements += 1;
  });

  fixture.delay();
  fixture.frame();

  assert.equal(acknowledgements, 1);
});

function scheduleFixture(): {
  schedule: RenderSchedule;
  frame(): void;
  delay(): void;
  cleared(): boolean;
} {
  let frameCallback = () => {};
  let delayCallback = () => {};
  let delayCleared = false;
  return {
    schedule: {
      requestFrame(callback) {
        frameCallback = callback;
      },
      setDelay(callback, milliseconds) {
        assert.equal(milliseconds, 250);
        delayCallback = callback;
        return 7;
      },
      clearDelay(handle) {
        assert.equal(handle, 7);
        delayCleared = true;
      },
    },
    frame: () => frameCallback(),
    delay: () => delayCallback(),
    cleared: () => delayCleared,
  };
}
