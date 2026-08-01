// The window's behaviour. It asks the daemon through the commands this crate exposes and draws the answers.
//
// Three rules run through all of it.
//
// **Nothing here interprets a conversation.** This file deals in session identifiers, provider names, folders
// and one word of state. What an agent said is laid out by which kind of frame it is and its text is passed
// through untouched.
//
// **There is no spinner.** The list is drawn from whatever the last answer held and a refresh replaces rows in
// place. A surface that blanks itself while it re-asks shows loading on every tick, and loading that could
// have been invisible is the thing this product refuses to show.
//
// **Sessions are grouped by the folder they work in.** Which session is touching which folder is a question
// the operator has to answer at a glance, and it is the shape of the desktop app this one is modelled on.

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

// How often the list is re-asked.
//
// The wire has no "the list changed" notification, only a per-session watch, so a list is polled. That is a
// real answer rather than a placeholder: the question travels over a local pipe, the daemon answers it out of
// state it already holds, and no transcript is touched. When the daemon grows a push for this, the interval
// goes and nothing else here changes.
const REFRESH_MS = 1000;

// The two event names the window pushes on. They match `runtrol_gui::FRAME_EVENT` and `OVER_EVENT`; a second
// spelling of either would be a conversation that arrives and is never shown.
const FRAME_EVENT = "session-frame";
const OVER_EVENT = "session-over";

const el = {
  groups: document.getElementById("groups"),
  empty: document.getElementById("empty"),
  nothing: document.getElementById("nothing"),
  detail: document.getElementById("detail"),
  talk: document.getElementById("talk"),
  composer: document.getElementById("composer"),
  composerFolder: document.getElementById("composerFolder"),
  composerProvider: document.getElementById("composerProvider"),
  composerState: document.getElementById("composerState"),
  composerNative: document.getElementById("composerNative"),
  toSay: document.getElementById("toSay"),
  send: document.getElementById("send"),
  closeSession: document.getElementById("closeSession"),
  newButton: document.getElementById("new"),
  starter: document.getElementById("starter"),
  provider: document.getElementById("provider"),
  workspace: document.getElementById("workspace"),
  confirmStart: document.getElementById("confirmStart"),
  said: document.getElementById("said"),
  daemonDot: document.getElementById("daemonDot"),
  daemonWord: document.getElementById("daemonWord"),
};

let rows = [];
let selected = null;

// Measurement, off unless the window was started to be measured. Two of this window's completion criteria are
// numbers only this page can produce, and nothing outside the process can see them.
let traceOn = false;
const startedAt = performance.now();
let firstDraw = true;

function trace(line) {
  if (traceOn) invoke("trace", { line });
}

/// Show what the daemon said, in its own words, until something replaces it.
function say(message, kind) {
  el.said.replaceChildren(document.createTextNode(message));
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
// The three outcomes stay apart all the way to the surface. A refusal carries the daemon's own words and
// sometimes says the operator has to be at their own machine, which is the only honest answer to a provider
// asking for authentication.
function unwrap(answer) {
  if (answer.outcome === "ok") {
    setDaemon(true);
    return answer.value;
  }
  if (answer.outcome === "refused") {
    setDaemon(true);
    const where = answer.needsTheOperator ? " (이 기계 앞에서 해결해야 한다)" : "";
    const again = answer.retryable ? " (다시 해 보면 될 수 있다)" : "";
    say(answer.message + where + again, "refused");
    return undefined;
  }
  setDaemon(false);
  say(answer.message, "broken");
  return undefined;
}

function setDaemon(reachable) {
  el.daemonDot.dataset.stuck = String(!reachable);
  el.daemonWord.textContent = reachable ? "데몬 연결됨" : "데몬에 닿지 않는다";
}

/// Draw the rail: one heading per folder, its sessions under it.
//
// Rebuilt only when the shape changed. Redrawing every second would throw away the selection highlight and
// make the rail flicker, which is exactly the stutter this product treats as a defect.
function drawGroups() {
  el.empty.hidden = rows.length > 0;

  const byFolder = new Map();
  for (const row of rows) {
    if (!byFolder.has(row.workspace)) byFolder.set(row.workspace, []);
    byFolder.get(row.workspace).push(row);
  }

  const shape = JSON.stringify(
    [...byFolder].map(([path, group]) => [path, group.map((row) => `${row.session}${row.doing}${row.looksStuck}${row.native ?? ""}`)]),
  );
  if (shape === el.groups.dataset.shape) {
    markSelected();
    return;
  }
  el.groups.dataset.shape = shape;
  el.groups.replaceChildren();

  for (const [path, group] of byFolder) {
    const heading = document.createElement("div");
    heading.className = "groupName";
    const icon = document.createElement("span");
    icon.className = "ico";
    icon.textContent = "▸";
    const name = document.createElement("span");
    name.className = "path";
    name.textContent = group[0].folder;
    name.title = path;
    heading.append(icon, name);
    el.groups.append(heading);

    for (const row of group) {
      const node = document.createElement("div");
      node.className = "row";
      node.dataset.session = row.session;
      node.tabIndex = 0;

      const dot = document.createElement("span");
      dot.className = "dot";
      dot.dataset.doing = row.doing;
      dot.dataset.stuck = String(row.looksStuck);

      const label = document.createElement("span");
      label.className = "name";
      // The conversation's own name when it has one, and runtrol's until then. A short form tells rows apart;
      // the whole value sits on the composer.
      label.textContent = (row.native ?? row.session).slice(0, 8);

      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = row.provider;

      node.title = row.looksStuck ? `${row.doing} (응답이 없다)` : row.doing;
      node.append(dot, label, badge);
      node.addEventListener("click", () => select(row.session));
      node.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          select(row.session);
        }
      });
      el.groups.append(node);
    }
  }
  markSelected();
}

function markSelected() {
  for (const node of el.groups.querySelectorAll(".row")) {
    node.setAttribute("aria-selected", String(node.dataset.session === selected));
  }
  drawComposer();
}

/// Draw what the composer says about the session it belongs to.
function drawComposer() {
  const row = rows.find((one) => one.session === selected);
  el.detail.hidden = !row;
  el.composer.hidden = !row;
  el.nothing.hidden = Boolean(row);
  if (!row) return;

  el.composerFolder.textContent = row.folder;
  el.composerFolder.title = row.workspace;
  el.composerProvider.textContent = row.provider;
  el.composerState.textContent = row.looksStuck ? `${row.doing} (응답이 없다)` : row.doing;
  // Said plainly rather than left blank. One of the two CLIs has no conversation until its first turn, and an
  // empty field would read as something missing rather than as something that does not exist yet.
  el.composerNative.textContent = row.native ?? "첫 턴 전 (아직 이름 없음)";
  el.composerNative.title = row.native ?? "";
}

// What each kind of frame is, for the page to lay out. The set is small on purpose: everything runtrol has no
// opinion about is still shown, tagged with whatever the provider called it. A conversation is never hidden
// because this build has not heard of its frame.
const SAID = {
  userMessageChunk: { who: "", side: "mine" },
  agentMessageChunk: { who: "", side: "theirs" },
  agentThoughtChunk: { who: "생각", side: "thought" },
};

/// Pull the readable text out of a frame's payload, without deciding what it means.
//
// The payload is the provider's own object and its shape is theirs. This looks in the few places text is
// actually written and otherwise shows nothing rather than guessing: a frame whose text cannot be found is
// still rendered, with its kind, so nothing disappears.
function textOf(payload) {
  if (typeof payload === "string") return payload;
  if (!payload || typeof payload !== "object") return "";
  for (const key of ["delta", "text"]) {
    if (typeof payload[key] === "string") return payload[key];
  }
  if (payload.item && typeof payload.item.text === "string") return payload.item.text;
  if (payload.message && Array.isArray(payload.message.content)) {
    return payload.message.content.map((one) => (typeof one?.text === "string" ? one.text : "")).join("");
  }
  if (Array.isArray(payload.content)) {
    return payload.content.map((one) => (typeof one?.text === "string" ? one.text : "")).join("");
  }
  return "";
}

/// Put one frame on the screen.
function showFrame(event) {
  const body = event?.body ?? {};
  const kind = body.event;

  if (kind === "turn") {
    // Only the ending carries an outcome, and only the provider's own word means it is known. runtrol saying
    // it stopped waiting is a different sentence and reads as one.
    const said =
      body.step === "ended"
        ? `턴 끝 · ${body.stop}${body.declared_by?.by === "provider" ? "" : " (공급자가 말한 것이 아님)"}`
        : body.step === "accepted"
          ? "접수됨"
          : body.step === "started"
            ? "시작됨"
            : body.step;
    append("meta", "", said);
    return;
  }

  if (kind === "notice") {
    append("meta", "", `알림 · ${body.code}`);
    return;
  }

  const known = SAID[kind];
  const text = textOf(body.content ?? body.payload);

  if (!known) {
    // Shown rather than dropped. A vendor shipping something new is a line the operator can read, not a gap.
    append("meta", "", `${kind}${text ? ` · ${text}` : ""}`);
    return;
  }

  // Fragments of one message join the block already there, which is what makes a streaming answer read as one
  // paragraph instead of a column of single words.
  const last = el.talk.lastElementChild;
  const id = body.message_id ?? null;
  if (body.delta && last && id && last.dataset.messageId === String(id)) {
    last.querySelector(".text").append(document.createTextNode(text));
    el.talk.scrollTop = el.talk.scrollHeight;
    return;
  }
  append(known.side, known.who, text, id);
}

function append(side, who, text, messageId) {
  const block = document.createElement("div");
  block.className = `said ${side}`;
  if (messageId) block.dataset.messageId = String(messageId);
  if (who) {
    const label = document.createElement("span");
    label.className = "who";
    label.textContent = who;
    block.append(label);
  }
  const body = document.createElement("span");
  body.className = "text";
  body.textContent = text;
  block.append(body);
  el.talk.append(block);
  el.talk.scrollTop = el.talk.scrollHeight;
}

async function select(session) {
  if (selected === session) return;
  selected = session;
  // Cleared and then filled by whatever arrives. The conversation lives with the provider, so what is on
  // screen is a view of the stream from here on rather than a copy runtrol was keeping.
  el.talk.replaceChildren();
  markSelected();
  if (!session) return;
  const answer = await invoke("watch", { session });
  const watched = unwrap(answer);
  trace(`watching ${session} ${watched === undefined ? "refused" : "ok"}`);
}

listen(FRAME_EVENT, (message) => {
  const { session, frame } = message.payload;
  // A frame from the session that was open a moment ago. Ignored rather than shown under the wrong heading.
  if (session !== selected) return;
  let event;
  try {
    event = JSON.parse(frame);
  } catch {
    // ok: a frame this build cannot read is still worth showing as one. Dropping it would make a vendor's new
    // shape look like a session that went quiet.
    append("meta", "", "읽을 수 없는 프레임이 왔다");
    return;
  }
  showFrame(event);
  // What a gate reads: that a frame arrived, and what kind. Never its text.
  trace(`frame ${event?.body?.event ?? "?"}`);
});

listen(OVER_EVENT, (message) => {
  if (message.payload !== selected) return;
  append("meta", "", "이 세션의 흐름이 끝났다");
  trace("stream over");
});

/// Ask for the list and draw it.
async function refresh() {
  const asked = performance.now();
  const answer = await invoke("sessions");
  const value = unwrap(answer);
  if (value === undefined) return;
  rows = value;
  if (selected && !rows.some((row) => row.session === selected)) selected = null;
  drawGroups();

  const drawn = performance.now();
  if (firstDraw) {
    trace(`first list at ${(drawn - startedAt).toFixed(0)} ms with ${rows.length} rows`);
    for (const row of rows) {
      trace(`row ${row.provider} ${row.session} folder=${row.folder} native=${row.native ?? "-"}`);
    }
  }
  trace(`list refreshed in ${(drawn - asked).toFixed(1)} ms with ${rows.length} rows`);

  // Opening to an empty panel when there are sessions is a step the operator has to take for no reason.
  const opening = firstDraw;
  firstDraw = false;
  if (opening && !selected && rows.length > 0) await select(rows[0].session);
}

/// Send what was typed, exactly as it was typed.
async function send() {
  const text = el.toSay.value;
  if (!selected || text.trim() === "") return;
  // Cleared before the answer comes back, because a box that stays full while the agent works reads as though
  // nothing was sent. The text is already on its way.
  el.toSay.value = "";
  grow();
  const answer = await invoke("prompt", { session: selected, text });
  unwrap(answer);
}

/// Let the box grow with what is in it, up to the height the stylesheet allows.
function grow() {
  el.toSay.style.height = "auto";
  el.toSay.style.height = `${el.toSay.scrollHeight}px`;
}

el.composer.addEventListener("submit", (event) => {
  event.preventDefault();
  send();
});

el.toSay.addEventListener("input", grow);

el.toSay.addEventListener("keydown", (event) => {
  // Enter sends, as it does in the app this one is modelled on. A new line is deliberate.
  if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    send();
  }
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

el.newButton.addEventListener("click", async () => {
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
  // The folder of whatever is open, because the next session is usually in the same place.
  const here = rows.find((row) => row.session === selected);
  if (here && !el.workspace.value) el.workspace.value = here.workspace;
  el.starter.showModal();
});

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
  await refresh();
  await select(session);
});

async function begin() {
  traceOn = await invoke("tracing");
  await refresh();
  setInterval(refresh, REFRESH_MS);
}

begin();
