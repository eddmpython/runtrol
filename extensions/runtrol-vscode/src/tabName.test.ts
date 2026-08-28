import assert from "node:assert/strict";
import test from "node:test";

import { tabName } from "./tabName";

test("a tab is called what fits on a tab", () => {
  assert.equal(tabName("현재 이 프로젝트 수준은?"), "현재 이 프로젝트 수준은?");
  // Measured 2026-08-28 from the operator's window: one tab held a whole first prompt and the tab bar held
  // nothing else.
  const long = tabName("그리고 사용량에 하나의 프로바이더의 프로그래스들은 갭을 줄인다. 그리고 행하나하나에 아이콘을 다는게");
  assert.equal(long.length, 24);
  assert.ok(long.endsWith("…"));
  // A first prompt with newlines would otherwise break the tab into something unreadable.
  assert.equal(tabName("한 줄\n그리고 다음 줄"), "한 줄 그리고 다음 줄");
  assert.equal(tabName("   여백   "), "여백");
});
