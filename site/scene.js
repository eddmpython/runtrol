// The trademark scene: a Runtrol sidebar where conversations pop open as editor tabs, the running
// agent icon spins, usage ticks, an approval asks for the user, and the phone toast arrives.
// Keyframes are a flat list of [ms, action] applied from a single clock, so the sequence reads like
// a storyboard and reruns deterministically. Reduced motion or no IntersectionObserver shows the end state.

const LOOP_MS = 15_000;

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

    [1500, () => { select(root, "a"); on(root, "tab-a"); on(root, "chat-a"); }],
    [1900, () => on(root, "a-u1")],
    [2300, () => { on(root, "a-typing"); rowState(root, "a", "running"); chatState(root, "chat-a", "running"); }],
    [3200, () => usage(root, "claude", 60)],

    [3600, () => { select(root, "b"); on(root, "tab-b"); on(root, "chat-b"); }],
    [4000, () => on(root, "b-u1")],
    [4400, () => { on(root, "b-typing"); rowState(root, "b", "running"); chatState(root, "chat-b", "running"); }],

    [5000, () => { on(root, "a-typing", false); on(root, "a-a1"); }],
    [5400, () => usage(root, "claude", 62)],
    [6200, () => { on(root, "b-typing", false); on(root, "b-a1"); usage(root, "codex", 33); }],
    [7000, () => on(root, "a-a2")],
    [7200, () => usage(root, "claude", 64)],
    [7800, () => { rowState(root, "a", "done"); chatState(root, "chat-a", "done"); on(root, "a-done"); }],

    [8400, () => { on(root, "b-approve"); rowState(root, "b", "waiting"); chatState(root, "chat-b", "waiting"); }],
    [9000, () => on(root, "toast")],
    [11000, () => { on(root, "b-allow"); }],
    [11400, () => { on(root, "toast", false); rowState(root, "b", "running"); chatState(root, "chat-b", "running"); on(root, "b-typing"); }],
    [12400, () => { on(root, "b-typing", false); on(root, "b-a2"); usage(root, "codex", 35); }],
    [13000, () => { rowState(root, "b", "done"); chatState(root, "chat-b", "done"); }],
  ];
}

function reset(root) {
  for (const node of root.querySelectorAll("[data-scene]")) {
    node.classList.remove("is-on");
  }
  for (const row of root.querySelectorAll(".row")) {
    row.classList.remove("is-running", "is-waiting", "is-selected");
  }
  for (const dot of root.querySelectorAll(".chat-state")) {
    dot.dataset.state = "idle";
  }
  usage(root, "claude", 58);
  usage(root, "codex", 31);
}

function finalState(root) {
  root.classList.add("is-static");
  for (const [, action] of keyframes(root)) {
    action();
  }
  on(root, "toast", false);
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
