// The window's behaviour. It asks the daemon through the commands this crate exposes and draws the answers.
//
// Two rules run through all of it.
//
// **Nothing here interprets a conversation.** This file deals in session identifiers, provider names and one
// word of state. What an agent said is not routed through here.
//
// **There is no spinner.** The list is drawn from whatever the last answer held, and a refresh replaces rows
// in place. A surface that blanks itself while it re-asks is a surface that shows loading on every tick, and
// loading that could have been invisible is the thing this product refuses to show.

const invoke = window.__TAURI__.core.invoke;

// How often the list is re-asked.
//
// The wire has no "the list changed" notification, only a per-session watch, so a list is polled. That is a
// real answer rather than a placeholder: the question travels over a local pipe, the daemon answers it out of
// state it already holds, and no transcript is touched. What it is not is the end state. When the daemon
// grows a push for this, the interval goes and nothing else here changes.
const REFRESH_MS = 1000;

const el = {
  sessions: document.getElementById("sessions"),
  empty: document.getElementById("empty"),
  nothing: document.getElementById("nothing"),
  detail: document.getElementById("detail"),
  detailProvider: document.getElementById("detailProvider"),
  detailDot: document.getElementById("detailDot"),
  detailDoing: document.getElementById("detailDoing"),
  detailSession: document.getElementById("detailSession"),
  detailNative: document.getElementById("detailNative"),
  closeSession: document.getElementById("closeSession"),
  newButton: document.getElementById("new"),
  starter: document.getElementById("starter"),
  provider: document.getElementById("provider"),
  workspace: document.getElementById("workspace"),
  confirmStart: document.getElementById("confirmStart"),
  said: document.getElementById("said"),
};

let rows = [];
let selected = null;

// Measurement, off unless the window was started to be measured.
//
// Two of this window's completion criteria are numbers only this page can produce: how long after asking
// something is on the screen, and whether frames hold up while output pours in. Nothing outside the process
// can see either, so the page reports and a gate reads it off the process's output.
let traceOn = false;
const startedAt = performance.now();
let firstDraw = true;

function trace(line) {
  if (traceOn) invoke("trace", { line });
}

/// Show what the daemon said, in its own words, until something replaces it.
function say(message, kind) {
  el.said.replaceChildren();
  el.said.append(document.createTextNode(message));
  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.textContent = "닫기";
  dismiss.addEventListener("click", () => {
    el.said.hidden = true;
  });
  el.said.append(dismiss);
  el.said.dataset.kind = kind;
  el.said.hidden = false;
}

/// Unwrap one answer, putting a refusal or a breakage on the screen and handing back nothing.
//
// The three outcomes are kept apart all the way to the surface. A refusal carries the daemon's own words and
// sometimes says the operator has to be at their own machine, which is the only honest answer to a provider
// asking for authentication.
function unwrap(answer) {
  if (answer.outcome === "ok") return answer.value;
  if (answer.outcome === "refused") {
    const where = answer.needsTheOperator ? " (이 기계 앞에서 해결해야 한다)" : "";
    const again = answer.retryable ? " (다시 해 보면 될 수 있다)" : "";
    say(answer.message + where + again, "refused");
    return undefined;
  }
  say(answer.message, "broken");
  return undefined;
}

/// Draw the list.
//
// Rows are reconciled by identifier rather than rebuilt, so a refresh does not throw away the selection, the
// scroll position, or the focus. Rebuilding is what makes a list flicker once a second.
function drawList() {
  el.empty.hidden = rows.length > 0;

  const wanted = new Set(rows.map((row) => row.session));
  for (const node of [...el.sessions.children]) {
    if (!wanted.has(node.dataset.session)) node.remove();
  }

  for (const [index, row] of rows.entries()) {
    let node = el.sessions.querySelector(`[data-session="${row.session}"]`);
    if (!node) {
      node = document.createElement("li");
      node.dataset.session = row.session;
      node.tabIndex = 0;
      const dot = document.createElement("span");
      dot.className = "dot";
      const badge = document.createElement("span");
      badge.className = "badge";
      const name = document.createElement("span");
      name.className = "rowName";
      node.append(dot, badge, name);
      node.addEventListener("click", () => select(row.session));
      node.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") select(row.session);
      });
    }
    // Order can change between answers, and putting each row where it belongs keeps the list stable without
    // rebuilding it.
    if (el.sessions.children[index] !== node) {
      el.sessions.insertBefore(node, el.sessions.children[index] ?? null);
    }

    const [dot, badge, name] = node.children;
    dot.dataset.doing = row.doing;
    dot.dataset.stuck = String(row.looksStuck);
    badge.textContent = row.provider;
    // The conversation's own name when it has one, and runtrol's until then. The short form is enough to tell
    // rows apart and the whole value is on the detail panel.
    name.textContent = (row.native ?? row.session).slice(0, 8);
    node.title = row.looksStuck ? `${row.doing} (응답이 없다)` : row.doing;
    node.setAttribute("aria-selected", String(row.session === selected));
  }

  drawDetail();
}

/// Draw the panel for whatever is selected.
function drawDetail() {
  const row = rows.find((one) => one.session === selected);
  el.detail.hidden = !row;
  el.nothing.hidden = Boolean(row);
  if (!row) return;

  el.detailProvider.textContent = row.provider;
  el.detailDot.dataset.doing = row.doing;
  el.detailDot.dataset.stuck = String(row.looksStuck);
  el.detailDoing.textContent = row.looksStuck ? `${row.doing} (응답이 없다)` : row.doing;
  el.detailSession.textContent = row.session;
  // Said plainly rather than left blank. One of the two CLIs has no conversation until its first turn, and a
  // blank field would read as something missing rather than as something that does not exist yet.
  el.detailNative.textContent = row.native ?? "아직 이름 없음 (첫 턴 전)";
}

function select(session) {
  selected = session;
  drawList();
}

/// Ask for the list and draw it.
async function refresh() {
  const asked = performance.now();
  const answer = await invoke("sessions");
  const value = unwrap(answer);
  if (value === undefined) return;
  rows = value;
  if (selected && !rows.some((row) => row.session === selected)) selected = null;
  drawList();

  const drawn = performance.now();
  if (firstDraw) {
    firstDraw = false;
    // The one number that says the window works rather than merely opened: how long after launch the list was
    // on the screen, and how many rows it had.
    trace(`first list at ${(drawn - startedAt).toFixed(0)} ms with ${rows.length} rows`);
    for (const row of rows) {
      trace(`row ${row.provider} ${row.session} native=${row.native ?? "-"} doing=${row.doing}`);
    }
  }
  trace(`list refreshed in ${(drawn - asked).toFixed(1)} ms with ${rows.length} rows`);
}

async function openStarter() {
  const answer = await invoke("providers");
  const offered = unwrap(answer);
  if (offered === undefined) return;

  el.provider.replaceChildren();
  for (const one of offered) {
    const option = document.createElement("option");
    option.value = one.id;
    // A provider this build cannot serve is shown and marked, never hidden. An operator whose provider
    // vanished from the list has no way to find out why.
    option.textContent = one.usable ? one.displayName : `${one.displayName} (${one.whyNot ?? "사용 불가"})`;
    option.disabled = !one.usable;
    el.provider.append(option);
  }
  el.starter.showModal();
}

el.newButton.addEventListener("click", openStarter);

el.confirmStart.addEventListener("click", async () => {
  const provider = el.provider.value;
  const workspace = el.workspace.value.trim();
  if (!provider || !workspace) {
    say("공급자와 작업 폴더가 모두 필요하다.", "broken");
    return;
  }
  const answer = await invoke("start", { provider, workspace });
  const session = unwrap(answer);
  if (session === undefined) return;
  selected = session;
  await refresh();
});

el.closeSession.addEventListener("click", async () => {
  if (!selected) return;
  // Not a delete. The conversation stays with its provider, which is what makes removing everything runtrol
  // holds lose nothing.
  const answer = await invoke("close", { session: selected, now: false });
  if (unwrap(answer) === undefined) return;
  selected = null;
  await refresh();
});

async function begin() {
  traceOn = await invoke("tracing");
  await refresh();
  setInterval(refresh, REFRESH_MS);
}

begin();
