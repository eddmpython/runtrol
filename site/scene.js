// The trademark scene: a Runtrol sidebar where a cursor opens conversations as editor tabs, the running
// agent icon spins, replies stream in, usage ticks, an approval asks for the user, the phone toast arrives,
// and the cursor allows it. Keyframes are a flat list of [ms, action] applied from a single clock, so the
// sequence reads like a storyboard and reruns deterministically. Reduced motion or no IntersectionObserver
// shows the end state.

const LOOP_MS = 16_000;
const STREAM_MS = 900;

function on(root, name, value = true) {
  for (const node of root.querySelectorAll(`[data-scene="${name}"]`)) {
    node.classList.toggle("is-on", value);
  }
}

function rowState(root, conv, state) {
  const row = root.querySelector(`.row[data-conv="${conv}"]`);
  if (!row) {
    return;
  }
  row.classList.toggle("is-running", state === "running");
  row.classList.toggle("is-waiting", state === "waiting");
}

function chatState(root, name, state) {
  const dot = root.querySelector(`[data-scene="${name}"] .chat-state`);
  if (dot) {
    dot.dataset.state = state;
  }
}

function usage(root, provider, percent) {
  const meter = root.querySelector(`#meter-${provider}`);
  const label = root.querySelector(`#usage-${provider}`);
  if (meter) {
    meter.style.setProperty("--v", String(percent));
  }
  if (label) {
    label.textContent = `${percent}%`;
  }
}

function select(root, conv) {
  for (const row of root.querySelectorAll(".row")) {
    row.classList.toggle("is-selected", row.dataset.conv === conv);
  }
}

// Cursor -----------------------------------------------------------------------------------------------

function cursorTo(root, selector, anchorX = 0.5, anchorY = 0.5) {
  const cursor = root.querySelector("#scene-cursor");
  const target = root.querySelector(selector);
  if (!cursor || !target) {
    return;
  }
  const base = root.getBoundingClientRect();
  const box = target.getBoundingClientRect();
  const x = box.left - base.left + box.width * anchorX;
  const y = box.top - base.top + box.height * anchorY;
  cursor.style.setProperty("--x", `${Math.round(x)}px`);
  cursor.style.setProperty("--y", `${Math.round(y)}px`);
  cursor.classList.add("is-on");
}

function cursorPress(root, pressed) {
  root.querySelector("#scene-cursor")?.classList.toggle("is-pressed", pressed);
}

function cursorHide(root) {
  root.querySelector("#scene-cursor")?.classList.remove("is-on", "is-pressed");
}

// Streaming --------------------------------------------------------------------------------------------

// Reveals a message's text nodes left to right over STREAM_MS, keeping inline <code> intact.
function stream(root, name) {
  const node = root.querySelector(`[data-scene="${name}"]`);
  if (!node) {
    return;
  }
  if (!node.dataset.full) {
    node.dataset.full = node.innerHTML;
  }
  const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
  const texts = [];
  let total = 0;
  for (let text = walker.nextNode(); text; text = walker.nextNode()) {
    texts.push({ text, full: text.data });
    total += text.data.length;
  }
  for (const entry of texts) {
    entry.text.data = "";
  }
  node.classList.add("is-on", "is-streaming");
  const started = performance.now();
  function step(now) {
    const shown = Math.min(total, Math.round(((now - started) / STREAM_MS) * total));
    let remaining = shown;
    for (const entry of texts) {
      const take = Math.max(0, Math.min(entry.full.length, remaining));
      entry.text.data = entry.full.slice(0, take);
      remaining -= take;
    }
    if (shown < total) {
      requestAnimationFrame(step);
    } else {
      node.classList.remove("is-streaming");
    }
  }
  requestAnimationFrame(step);
}

function restore(root) {
  for (const node of root.querySelectorAll("[data-full]")) {
    node.innerHTML = node.dataset.full;
    node.classList.remove("is-streaming");
  }
}

// Storyboard -------------------------------------------------------------------------------------------

function keyframes(root) {
  return [
    [0, () => reset(root)],
    [100, () => on(root, "p1")],
    [220, () => on(root, "r1")],
    [340, () => on(root, "r2")],
    [460, () => on(root, "r3")],
    [600, () => on(root, "p2")],
    [700, () => on(root, "r4")],
    [820, () => on(root, "p3")],
    [920, () => on(root, "r5")],
    [1100, () => on(root, "usage")],

    [1200, () => cursorTo(root, ".tree", 0.7, 0.9)],
    [1500, () => cursorTo(root, '.row[data-conv="a"]', 0.45, 0.5)],
    [2100, () => cursorPress(root, true)],
    [2200, () => { cursorPress(root, false); select(root, "a"); on(root, "tab-a"); on(root, "chat-a"); }],
    [2600, () => on(root, "a-u1")],
    [3000, () => { on(root, "a-typing"); rowState(root, "a", "running"); chatState(root, "chat-a", "running"); }],
    [3600, () => usage(root, "claude", 60)],

    [3900, () => cursorTo(root, '.row[data-conv="b"]', 0.5, 0.5)],
    [4500, () => cursorPress(root, true)],
    [4600, () => { cursorPress(root, false); select(root, "b"); on(root, "tab-b"); on(root, "chat-b"); }],
    [5000, () => on(root, "b-u1")],
    [5300, () => cursorTo(root, ".studio-title", 0.5, 2.2)],
    [5400, () => { on(root, "b-typing"); rowState(root, "b", "running"); chatState(root, "chat-b", "running"); }],

    [5800, () => { on(root, "a-typing", false); stream(root, "a-a1"); }],
    [6300, () => usage(root, "claude", 62)],
    [7000, () => { on(root, "b-typing", false); stream(root, "b-a1"); usage(root, "codex", 33); }],
    [7800, () => stream(root, "a-a2")],
    [8100, () => usage(root, "claude", 64)],
    [8900, () => { rowState(root, "a", "done"); chatState(root, "chat-a", "done"); on(root, "a-done"); }],

    [9300, () => { on(root, "b-approve"); rowState(root, "b", "waiting"); chatState(root, "chat-b", "waiting"); }],
    [9800, () => on(root, "toast")],
    [11000, () => cursorTo(root, '[data-scene="b-allow"]', 0.5, 0.5)],
    [11700, () => cursorPress(root, true)],
    [11800, () => { cursorPress(root, false); on(root, "b-allow"); }],
    [12100, () => { on(root, "toast", false); rowState(root, "b", "running"); chatState(root, "chat-b", "running"); on(root, "b-typing"); }],
    [12400, () => cursorTo(root, ".composer", 0.85, 0.5)],
    [13100, () => { on(root, "b-typing", false); stream(root, "b-a2"); usage(root, "codex", 35); }],
    [14000, () => { rowState(root, "b", "done"); chatState(root, "chat-b", "done"); }],
    [15200, () => cursorHide(root)],
  ];
}

function reset(root) {
  restore(root);
  for (const node of root.querySelectorAll("[data-scene]")) {
    node.classList.remove("is-on");
  }
  for (const row of root.querySelectorAll(".row")) {
    row.classList.remove("is-running", "is-waiting", "is-selected");
  }
  for (const dot of root.querySelectorAll(".chat-state")) {
    dot.dataset.state = "idle";
  }
  cursorHide(root);
  usage(root, "claude", 58);
  usage(root, "codex", 31);
}

function finalState(root) {
  root.classList.add("is-static");
  for (const [, action] of keyframes(root)) {
    action();
  }
  restore(root);
  on(root, "toast", false);
  cursorHide(root);
}

function tickClock(root, elapsedMs) {
  const clock = root.querySelector("#studio-clock");
  if (clock) {
    const minute = 2 + Math.floor(elapsedMs / 5000);
    clock.textContent = `14:${String(minute).padStart(2, "0")}`;
  }
}

export function startScene(root) {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (reduced || typeof IntersectionObserver !== "function") {
    finalState(root);
    return () => {};
  }

  const frames = keyframes(root);
  let start = 0;
  let cursor = 0;
  let handle = 0;
  let visible = false;

  function frame(now) {
    if (!visible || document.hidden) {
      handle = 0;
      return;
    }
    if (start === 0) {
      start = now;
    }
    const elapsed = now - start;
    while (cursor < frames.length && frames[cursor][0] <= elapsed) {
      frames[cursor][1]();
      cursor += 1;
    }
    tickClock(root, elapsed);
    if (elapsed >= LOOP_MS) {
      start = now;
      cursor = 0;
    }
    handle = requestAnimationFrame(frame);
  }

  function resume() {
    if (handle === 0 && visible && !document.hidden) {
      start = 0;
      cursor = 0;
      handle = requestAnimationFrame(frame);
    }
  }

  const observer = new IntersectionObserver((entries) => {
    visible = entries.some((entry) => entry.isIntersecting);
    resume();
  }, { threshold: 0.25 });
  observer.observe(root);
  document.addEventListener("visibilitychange", resume);

  return () => {
    observer.disconnect();
    document.removeEventListener("visibilitychange", resume);
    if (handle !== 0) {
      cancelAnimationFrame(handle);
    }
  };
}
