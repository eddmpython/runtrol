import { openDeviceStore } from "./identityStore.js";
import {
  attentionCount,
  consumeAttentionRequest,
  isAttentionMessage,
  needsAttention,
  nextAttentionSession,
  preferredSession,
} from "./attention.js";
import { consumePairingFragment } from "./pairing.js";
import { CoreClient, CoreFailure, readDeviceAuthority, withCore } from "./core.js";
import { keyFingerprint, pairThroughRelay } from "./relay.js";
import { missionActions, readMissionCatalogue, readMissionSnapshot } from "./missions.js";
import {
  missionFlightDestination,
  missionFlightBadge,
  missionFlightLabel,
  readMissionFlightSignals,
} from "./missionSignals.js";
import { disablePush, enablePush, pushAvailable, synchronizePush } from "./push.js";
import {
  approvalOptions,
  contentText,
  eventBody,
  exactSubject,
  safeVisibleText,
} from "./presentation.js";

const MAX_VISIBLE_EVENTS = 400;
const MAX_VISIBLE_CHARACTERS = 256 * 1024;
const MAX_VISIBLE_MISSION_TASKS = 200;
const state = {
  store: null,
  identity: null,
  connection: null,
  pairing: null,
  sessions: [],
  missions: [],
  flightSignals: [],
  providers: [],
  selected: null,
  selectedMission: null,
  watchGeneration: 0,
  cursor: null,
  presentation: null,
  pushPublicKey: null,
  pushSynchronized: false,
  serviceWorker: null,
  attentionRequested: false,
};

const status = element("connection-status");
const setup = element("setup");
const sessionsView = element("sessions-view");
const sessionBrowser = element("session-browser");
const missionBrowser = element("mission-browser");
const sessionList = element("session-list");
const missionList = element("mission-list");
const sessionDetail = element("session-detail");
const missionDetail = element("mission-detail");
const output = element("output");
const composer = element("composer");
const prompt = element("prompt");
const refresh = element("refresh");
const refreshMissions = element("refresh-missions");
const sessionsTab = element("show-sessions");
const missionsTab = element("show-missions");
const panic = element("panic");
const forget = element("forget-device");
const notifications = element("notifications");
const nextAttention = element("next-attention");
let visibleCharacters = 0;

await boot();

async function boot() {
  try {
    state.pairing = consumePairingFragment(location, history);
    state.attentionRequested = consumeAttentionRequest(location, history);
    state.presentation = await fetch("assets/event-presentation.json", { cache: "no-cache" }).then((response) => {
      if (!response.ok) throw new Error("event presentation contract is unavailable");
      return response.json();
    });
    state.store = await openDeviceStore();
    state.identity = await state.store.identity();
    state.connection = await state.store.connection();
    state.serviceWorker = registerServiceWorker();
    bindActions();
    if (state.pairing) {
      await showPairing();
    } else if (state.connection) {
      await showSessions();
    } else {
      showUnpaired();
    }
  } catch (error) {
    showFatal(error);
  }
}

function bindActions() {
  refresh.addEventListener("click", () => refreshSessions());
  nextAttention.addEventListener("click", () => focusNextAttention());
  refreshMissions.addEventListener("click", () => runAction(async () => refreshMissionCatalogue()));
  sessionsTab.addEventListener("click", () => activateSurface("sessions"));
  missionsTab.addEventListener("click", () => runAction(async () => {
    activateSurface("missions");
    await refreshMissionCatalogue();
  }));
  panic.addEventListener("click", () => runAction(async () => {
    if (!state.connection) throw new Error("Pair this phone before using panic stop.");
    if (!confirm("Stop every supervised session on the PC now?")) return;
    await withCore(state.connection, state.identity, (client) => client.stopEverything());
    setStatus("Panic stop sent", "warning");
    await refreshSessions();
  }));
  forget.addEventListener("click", () => runAction(async () => {
    if (!confirm("Forget this PC and remove the phone identity from this device?")) return;
    state.watchGeneration += 1;
    await forgetPushSubscription();
    await state.store.forget();
    state.connection = null;
    state.identity = await state.store.identity();
    showUnpaired();
  }));
  notifications.addEventListener("click", () => runAction(async () => {
    if (!state.connection || !state.pushPublicKey || !state.serviceWorker) return;
    const registration = await state.serviceWorker;
    if (!registration) throw new Error("Install support is unavailable.");
    if (notifications.dataset.enabled === "true") {
      await withCore(state.connection, state.identity, (client) => disablePush(client, registration));
      renderNotificationState(false, "Notifications are off.");
      return;
    }
    await withCore(state.connection, state.identity, (client) => enablePush(
      client,
      state.pushPublicKey,
      registration,
    ));
    renderNotificationState(true, "Notifications are on.");
  }));
  composer.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = prompt.value;
    if (!state.selected || !value.trim()) return;
    prompt.value = "";
    runAction(async () => {
      await withCore(state.connection, state.identity, (client) => client.prompt(state.selected.session, value));
      markSessionWaiting(state.selected.session, null);
      setStatus("Prompt delivered unchanged", "online");
    });
  });
  element("interrupt").addEventListener("click", () => runAction(async () => {
    await withCore(state.connection, state.identity, (client) => client.interrupt(state.selected.session));
  }));
  element("delete-session").addEventListener("click", () => runAction(async () => {
    if (!confirm("Remove this runtrol session pointer? The provider-owned conversation remains with its CLI.")) return;
    await withCore(state.connection, state.identity, (client) => client.closeSession(state.selected.session, false));
    state.selected = null;
    await refreshSessions();
  }));
  element("back-to-sessions").addEventListener("click", () => {
    clearSelection();
    renderSessions();
  });
  element("back-to-missions").addEventListener("click", () => {
    missionDetail.hidden = true;
  });
  element("resume-session").addEventListener("click", () => runAction(async () => {
    if (!state.selected?.native) throw new Error("This session has no provider resume identity.");
    await withCore(state.connection, state.identity, (client) => client.resume(state.selected));
    await refreshSessions();
  }));
  element("new-session").addEventListener("submit", (event) => {
    event.preventDefault();
    runAction(async () => {
      const provider = element("new-provider").value;
      const workspace = element("new-workspace").value;
      if (!provider || !workspace) throw new Error("Choose an approved provider and workspace.");
      const response = await withCore(state.connection, state.identity, (client) => client.start(provider, workspace));
      if (response.say === "started") await refreshSessions(response.with.session);
    });
  });
  element("pause-mission").addEventListener("click", () => runAction(async () => {
    await changeMission((client, mission) => client.pauseMission(mission));
  }));
  element("resume-mission").addEventListener("click", () => runAction(async () => {
    await changeMission((client, mission) => client.resumeMission(mission));
  }));
  element("cancel-mission").addEventListener("click", () => runAction(async () => {
    const mission = state.selectedMission?.mission;
    if (!mission) return;
    if (!confirm(`Cancel ${safeVisibleText(mission.name)} and release its exact reservations?`)) return;
    await changeMission((client, selected) => client.cancelMission(selected));
  }));
}

async function showPairing() {
  setup.hidden = false;
  sessionsView.hidden = true;
  panic.disabled = true;
  forget.hidden = true;
  notifications.hidden = true;
  setStatus("Ready to pair", "connecting");
  const pcFingerprint = await keyFingerprint(state.pairing.pcPublicKey);
  setup.innerHTML = `
    <div class="setup-card">
      <p class="eyebrow">ONE-TIME PAIRING</p>
      <h1>Connect this phone</h1>
      <p>The PC must approve this exact phone before any session data can cross.</p>
      <dl><dt>PC key</dt><dd id="pc-fingerprint"></dd><dt>Expires</dt><dd id="pairing-expiry"></dd></dl>
      <form id="pairing-form">
        <label>Phone name<input id="phone-name" required maxlength="64" autocomplete="off"></label>
        <label>Platform<input id="phone-platform" required maxlength="32" autocomplete="off"></label>
        <button class="primary" type="submit">Ask the PC to pair</button>
      </form>
      <p class="quiet" id="pairing-state" aria-live="polite"></p>
    </div>`;
  element("pc-fingerprint").textContent = pcFingerprint;
  element("pairing-expiry").textContent = new Date(state.pairing.expiresAtMs).toLocaleTimeString();
  element("phone-name").value = "My phone";
  element("phone-platform").value = navigator.userAgentData?.platform || navigator.platform || "Phone";
  element("pairing-form").addEventListener("submit", (event) => {
    event.preventDefault();
    runAction(async () => {
      const pairingState = element("pairing-state");
      pairingState.textContent = "Waiting for the exact approval in Runtrol Studio on the PC.";
      const result = await pairThroughRelay(state.pairing, state.identity, {
        name: element("phone-name").value.trim(),
        platform: element("phone-platform").value.trim(),
      });
      result.channel.close();
      await state.store.saveConnection(result.connection);
      state.connection = result.connection;
      state.pairing = null;
      await showSessions();
    });
  });
}

function showUnpaired() {
  state.watchGeneration += 1;
  state.missions = [];
  state.flightSignals = [];
  state.selectedMission = null;
  setup.hidden = false;
  sessionsView.hidden = true;
  panic.disabled = true;
  forget.hidden = true;
  notifications.hidden = true;
  missionsTab.hidden = true;
  setup.innerHTML = `
    <div class="setup-card">
      <p class="eyebrow">PHONE CONTROL SURFACE</p>
      <h1>Pair from Runtrol Studio</h1>
      <p>In VS Code, run <strong>Runtrol: Pair a Phone</strong>, then scan the one-use QR shown there.</p>
      <p class="quiet">The phone stores only its protected device identity and connection secrets. Conversation content stays with the provider CLI.</p>
    </div>`;
  setStatus("Not paired", "offline");
}

async function showSessions() {
  setup.hidden = true;
  sessionsView.hidden = false;
  panic.disabled = false;
  forget.hidden = false;
  notifications.hidden = !pushAvailable() || state.serviceWorker === null;
  activateSurface("sessions");
  const attentionRequested = state.attentionRequested;
  state.attentionRequested = false;
  await refreshSessions(null, attentionRequested);
}

async function refreshSessions(requestedSession = null, attentionRequested = false) {
  if (!state.connection) return;
  setStatus("Connecting to PC", "connecting");
  try {
    const client = await CoreClient.connect(state.connection, state.identity);
    try {
      state.providers = (client.welcome.providers ?? []).filter((provider) => provider.usable);
      state.pushPublicKey = client.welcome.push_public_key ?? null;
      const authority = readDeviceAuthority(client.welcome.device);
      state.connection = Object.freeze({ ...state.connection, ...authority });
      await state.store.saveConnection(state.connection);
      const response = await client.list();
      if (response.say !== "sessions") throw new Error("Core returned no session list");
      state.sessions = response.with.sessions;
      if (attentionRequested && hasScope("mission.read")) {
        const signalResponse = await client.listMissionFlightSignals(state.connection.missionSignalCursor);
        if (signalResponse.say !== "missionFlightSignals") {
          throw new Error("Core returned no Mission Flight Signals");
        }
        const page = readMissionFlightSignals(signalResponse.with);
        state.flightSignals = page.signals;
        state.connection = Object.freeze({
          ...state.connection,
          missionSignalCursor: page.next_cursor,
        });
        await state.store.saveConnection(state.connection);
      } else if (attentionRequested) {
        state.flightSignals = [];
      }
    } finally {
      client.close();
    }
    populateProviders();
    configureSurfaceTabs();
    await synchronizeNotifications();
    renderSessions();
    renderFlightSignals();
    const destination = attentionRequested
      ? missionFlightDestination(state.flightSignals, state.sessions)
      : null;
    if (destination?.surface === "mission") {
      activateSurface("missions");
      await refreshMissionCatalogue(destination.missionId);
      return;
    }
    const selected = preferredSession(
      state.sessions,
      destination?.session?.session ?? requestedSession,
      state.selected?.session ?? null,
      attentionRequested,
      isNarrowViewport(),
    );
    if (selected) selectSession(selected);
    else clearSelection();
    setStatus("PC online", "online");
  } catch (error) {
    setStatus(failureMessage(error, "PC offline"), "offline");
  }
}

async function synchronizeNotifications() {
  if (!pushAvailable() || !state.serviceWorker || !state.pushPublicKey) {
    notifications.hidden = true;
    return;
  }
  notifications.hidden = false;
  if (state.pushSynchronized) return;
  state.pushSynchronized = true;
  const registration = await state.serviceWorker;
  if (!registration) {
    notifications.hidden = true;
    return;
  }
  const result = await withCore(state.connection, state.identity, (client) => synchronizePush(
    client,
    state.pushPublicKey,
    registration,
  ));
  renderNotificationState(result.enabled, result.reason);
}

function renderNotificationState(enabled, reason) {
  notifications.dataset.enabled = String(enabled);
  notifications.textContent = enabled ? "Disable notifications" : "Enable notifications";
  notifications.title = reason;
}

async function forgetPushSubscription() {
  if (!pushAvailable() || !state.serviceWorker) return;
  const registration = await state.serviceWorker;
  if (!registration) return;
  const subscription = await registration.pushManager.getSubscription();
  if (subscription) await subscription.unsubscribe();
  if (state.connection) {
    try {
      await withCore(state.connection, state.identity, (client) => client.setPushSubscription(null));
    } catch {
      // Browser unsubscribe invalidates the bearer capability. An unreachable PC cannot deliver through it, and
      // forgetting the local identity must still finish while that PC is offline.
    }
  }
  state.pushSynchronized = false;
  state.pushPublicKey = null;
}

function renderSessions() {
  sessionList.replaceChildren();
  element("session-count").textContent = String(state.sessions.length);
  const waiting = attentionCount(state.sessions);
  element("attention-count").textContent = String(waiting);
  nextAttention.hidden = waiting === 0;
  nextAttention.setAttribute("aria-label", `${waiting} sessions need you. Open the next one.`);
  for (const session of state.sessions) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `session-row${session.session === state.selected?.session ? " selected" : ""}`;
    const title = safeVisibleText(session.label || workspaceName(session.workspace));
    const activity = needsAttention(session)
      ? "Needs you"
      : session.waiting_on === "quota"
        ? "Waiting on limit"
        : session.doing;
    const detail = safeVisibleText(`${session.provider}  ${activity}`);
    const dot = needsAttention(session) ? "needs-you" : session.waiting_on === "quota" ? "quota" : session.hot ? "hot" : "";
    button.classList.toggle("needs-you", needsAttention(session));
    button.innerHTML = `<span class="state-dot ${dot}"></span><span><strong></strong><small></small></span><b></b>`;
    button.querySelector("strong").textContent = title;
    button.querySelector("small").textContent = detail;
    button.querySelector("b").textContent = session.hot ? "open" : "cold";
    button.addEventListener("click", () => selectSession(session));
    sessionList.append(button);
  }
}

function focusNextAttention() {
  const next = nextAttentionSession(state.sessions, state.selected?.session ?? null);
  if (!next) return;
  activateSurface("sessions");
  selectSession(next);
}

function markSessionWaiting(sessionId, waitingOn) {
  let selected = null;
  state.sessions = state.sessions.map((session) => {
    if (session.session !== sessionId) return session;
    const updated = { ...session, waiting_on: waitingOn };
    if (state.selected?.session === sessionId) selected = updated;
    return updated;
  });
  if (selected) state.selected = selected;
  renderSessions();
}

function configureSurfaceTabs() {
  const missionsAllowed = hasScope("mission.read");
  missionsTab.hidden = !missionsAllowed;
  if (!missionsAllowed && !missionBrowser.hidden) activateSurface("sessions");
}

function activateSurface(surface) {
  const missions = surface === "missions";
  sessionBrowser.hidden = missions;
  missionBrowser.hidden = !missions;
  sessionsTab.setAttribute("aria-pressed", String(!missions));
  missionsTab.setAttribute("aria-pressed", String(missions));
  if (missions) {
    state.watchGeneration += 1;
    sessionDetail.hidden = true;
  } else {
    missionDetail.hidden = true;
    sessionDetail.hidden = state.selected === null;
  }
}

async function refreshMissionCatalogue(preferredMission = state.selectedMission?.mission?.mission_id ?? null) {
  if (!hasScope("mission.read")) throw new Error("This phone cannot read Mission status.");
  setStatus("Connecting to PC", "connecting");
  const response = await withCore(state.connection, state.identity, (client) => client.listMissions());
  if (response.say !== "missions") {
    throw new Error("Core returned no Mission list");
  }
  state.missions = readMissionCatalogue(response.with);
  renderMissions();
  const selected = state.missions.find((mission) => mission.mission_id === preferredMission)
    ?? state.missions[0];
  if (selected) await selectMission(selected);
  else {
    state.selectedMission = null;
    missionDetail.hidden = true;
  }
  setStatus("PC online", "online");
}

function renderMissions() {
  missionList.replaceChildren();
  element("mission-count").textContent = String(state.missions.length);
  for (const mission of state.missions) {
    const signal = latestMissionFlightSignal(mission.mission_id);
    const button = document.createElement("button");
    button.type = "button";
    button.className = `mission-row${mission.mission_id === state.selectedMission?.mission?.mission_id ? " selected" : ""}`;
    button.classList.toggle("flight-signal", signal !== null);
    const dot = document.createElement("span");
    dot.className = `state-dot ${safeVisibleText(mission.state)}`;
    const labels = document.createElement("span");
    const title = document.createElement("strong");
    title.textContent = safeVisibleText(mission.name);
    const project = document.createElement("small");
    project.textContent = safeVisibleText(mission.project);
    labels.append(title, project);
    const progress = document.createElement("b");
    progress.textContent = signal
      ? missionFlightBadge(signal.kind)
      : `${Number(mission.passed_tasks) || 0}/${Number(mission.total_tasks) || 0}`;
    button.append(dot, labels, progress);
    button.addEventListener("click", () => runAction(async () => selectMission(mission)));
    missionList.append(button);
  }
}

async function selectMission(mission) {
  state.watchGeneration += 1;
  sessionDetail.hidden = true;
  const response = await withCore(
    state.connection,
    state.identity,
    (client) => client.getMission(mission.mission_id),
  );
  if (response.say !== "mission") throw new Error("Core returned no Mission snapshot");
  renderMissionSnapshot(readMissionSnapshot(response.with));
}

function renderMissionSnapshot(snapshot) {
  state.selectedMission = snapshot;
  missionDetail.hidden = false;
  element("selected-mission-state").textContent = safeVisibleText(snapshot.mission.state).toUpperCase();
  element("selected-mission-title").textContent = safeVisibleText(snapshot.mission.name);
  element("selected-mission-project").textContent = safeVisibleText(snapshot.mission.project);
  element("mission-progress").textContent = `${Number(snapshot.mission.passed_tasks) || 0} of ${Number(snapshot.mission.total_tasks) || 0}`;
  element("mission-awaiting").textContent = String(Number(snapshot.mission.awaiting_input) || 0);
  element("mission-source").textContent = safeVisibleText(snapshot.mission_ref);
  element("mission-policy").textContent = safeVisibleText(snapshot.policy_sha256);
  const signal = latestMissionFlightSignal(snapshot.mission.mission_id);
  const signalBanner = element("mission-flight-signal");
  signalBanner.hidden = signal === null;
  signalBanner.textContent = signal ? missionFlightLabel(signal.kind) : "";

  const actions = missionActions(snapshot.mission, state.connection.scopes);
  element("pause-mission").hidden = !actions.pause;
  element("resume-mission").hidden = !actions.resume;
  element("cancel-mission").hidden = !actions.cancel;

  const tasks = element("mission-tasks");
  tasks.replaceChildren();
  for (const row of snapshot.tasks.slice(0, MAX_VISIBLE_MISSION_TASKS)) {
    const card = document.createElement("article");
    card.className = "mission-task";
    const title = document.createElement("h3");
    title.textContent = safeVisibleText(row.key);
    const stateLine = document.createElement("p");
    stateLine.textContent = `${safeVisibleText(row.state)}  ${safeVisibleText(row.workspace_mode)}  ${safeVisibleText(row.provider_selector)}`;
    const source = document.createElement("p");
    source.textContent = safeVisibleText(row.instruction_ref);
    const gates = document.createElement("p");
    gates.textContent = `${Number(row.passed_gates) || 0} gates passed, ${Number(row.failed_gates) || 0} failed`;
    card.append(title, stateLine, source, gates);
    if (row.receipt_id) {
      const receipt = document.createElement("p");
      receipt.textContent = `Receipt ${safeVisibleText(row.receipt_id)}`;
      card.append(receipt);
    }
    tasks.append(card);
  }
  if (snapshot.tasks.length > MAX_VISIBLE_MISSION_TASKS) {
    const bounded = document.createElement("p");
    bounded.className = "quiet";
    bounded.textContent = `${snapshot.tasks.length - MAX_VISIBLE_MISSION_TASKS} more Tasks remain in the bounded Core snapshot.`;
    tasks.append(bounded);
  }
  renderMissions();
}

function renderFlightSignals() {
  const count = new Set(state.flightSignals.map((signal) => signal.mission_id)).size;
  const badge = element("mission-signal-count");
  badge.textContent = String(count);
  badge.hidden = count === 0;
  renderMissions();
}

function latestMissionFlightSignal(missionId) {
  return state.flightSignals.findLast((signal) => signal.mission_id === missionId) ?? null;
}

async function changeMission(request) {
  const mission = state.selectedMission?.mission;
  if (!mission) throw new Error("Choose a Mission first.");
  const response = await withCore(
    state.connection,
    state.identity,
    (client) => request(client, mission.mission_id),
  );
  if (response.say !== "mission") throw new Error("Core returned no Mission snapshot");
  const snapshot = readMissionSnapshot(response.with);
  const current = snapshot.mission;
  state.missions = state.missions.map((row) => (
    row.mission_id === current.mission_id ? current : row
  ));
  renderMissionSnapshot(snapshot);
  setStatus("PC online", "online");
}

function selectSession(session) {
  state.selected = session;
  state.cursor = null;
  state.watchGeneration += 1;
  output.replaceChildren();
  visibleCharacters = 0;
  missionDetail.hidden = true;
  sessionDetail.hidden = false;
  element("selected-title").textContent = safeVisibleText(session.label || workspaceName(session.workspace));
  element("selected-provider").textContent = safeVisibleText(session.provider);
  element("selected-workspace").textContent = safeVisibleText(session.workspace);
  prompt.disabled = !session.hot || !hasScope("session.input.write");
  element("send").disabled = prompt.disabled;
  element("interrupt").disabled = !session.hot || !hasScope("session.stop");
  element("delete-session").disabled = !hasScope("session.delete");
  element("resume-session").hidden = session.hot;
  element("resume-session").disabled = !hasScope("session.resume")
    || !session.native
    || !state.connection.providers.includes(session.provider)
    || !isWorkspaceApproved(session.workspace);
  renderSessions();
  if (session.hot && hasScope("session.output.read")) void watchSession(session, state.watchGeneration);
  else appendOutput("meta", session.hot ? "This phone cannot read session output." : "Resume this provider-owned session to continue.");
}

function clearSelection() {
  state.selected = null;
  state.watchGeneration += 1;
  sessionDetail.hidden = true;
}

async function watchSession(session, generation) {
  let delay = 200;
  while (generation === state.watchGeneration) {
    let client;
    try {
      client = await CoreClient.connect(state.connection, state.identity);
      const started = await client.beginWatch(session.session, state.cursor);
      if (started.gap) appendOutput("warning", "Some older output is no longer in the bounded reconnect window.");
      setStatus("PC online", "online");
      delay = 200;
      while (generation === state.watchGeneration) {
        const response = await client.nextWatch();
        if (response.say === "lagged") {
          state.cursor = response.with.next_expected;
          appendOutput("warning", "Live output fell behind. Reconnecting at the next available event.");
          break;
        }
        state.cursor = response.with.next_expected;
        presentEvent(response.with.payload, session);
      }
    } catch (error) {
      if (generation === state.watchGeneration) setStatus(failureMessage(error, "PC offline"), "offline");
    } finally {
      client?.close();
    }
    if (generation !== state.watchGeneration) return;
    await wait(delay);
    delay = Math.min(delay * 2, 2_000);
  }
}

function presentEvent(payload, session) {
  const body = eventBody(payload);
  if (!body) {
    appendOutput("warning", "An unreadable provider event arrived.");
    return;
  }
  const contract = state.presentation.events?.[body.event];
  if (contract?.kind === "message") {
    appendOutput(contract.side, contentText(body));
  } else if (contract?.kind === "approval") {
    markSessionWaiting(session.session, "person");
    appendApproval(body, session);
  } else if (contract?.kind === "tool") {
    appendOutput("meta", toolLine(body));
  } else if (contract?.kind === "status" || contract?.kind === "turn" || contract?.kind === "notice") {
    appendOutput("meta", safeVisibleText(contract.textKey || body.event));
  }
}

// The provider's own classification and label, mirroring the PC surface so one vocabulary is learned once. Only
// the label is read out of the payload; raw input, raw output and diffs stay untouched because they are the
// conversation, and a phone is the last place to spill them.
const TOOL_VERBS = {
  read: "Read",
  edit: "Edit",
  delete: "Delete",
  move: "Move",
  search: "Search",
  execute: "Run",
  think: "Think",
  fetch: "Fetch",
  switchMode: "Switch mode",
  other: "Tool",
};

function toolLine(body) {
  const verb = TOOL_VERBS[body.kind] || "Tool";
  const title = safeVisibleText(typeof body.payload?.title === "string" ? body.payload.title : "");
  const head = title ? `${verb} ${title}` : verb;
  if (body.status === "failed") return `${head} failed`;
  if (body.status === "cancelled") return `${head} cancelled`;
  if (body.status === "inProgress") return `${head} running`;
  return head;
}

function appendApproval(body, session) {
  const card = document.createElement("article");
  card.className = `event approval ${body.risk === "high" ? "high" : ""}`;
  const title = document.createElement("strong");
  title.textContent = body.subject_incomplete ? "Approval blocked" : `${safeVisibleText(body.kind)} approval`;
  const context = document.createElement("dl");
  context.innerHTML = "<dt>Session</dt><dd></dd><dt>Workspace</dt><dd></dd>";
  context.children[1].textContent = safeVisibleText(session.label || session.session);
  context.children[3].textContent = safeVisibleText(session.workspace);
  const subject = document.createElement("pre");
  subject.textContent = exactSubject(body.subject);
  const actions = document.createElement("div");
  actions.className = "approval-actions";
  for (const option of approvalOptions(body, state.connection.scopes)) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = option.label;
    button.disabled = option.unavailable !== null;
    if (option.unavailable) button.title = option.unavailable;
    button.addEventListener("click", () => runAction(async () => {
      await withCore(state.connection, state.identity, (client) => client.answerApproval(
        session.session,
        body.id,
        option.id,
        body.subject_digest,
      ));
      markSessionWaiting(session.session, null);
      for (const choice of actions.querySelectorAll("button")) choice.disabled = true;
    }));
    actions.append(button);
  }
  card.append(title, context, subject, actions);
  const characters = card.textContent?.length ?? 0;
  card.dataset.characters = String(characters);
  visibleCharacters += characters;
  output.append(card);
  trimOutput();
}

function appendOutput(kind, value) {
  const text = safeVisibleText(value);
  if (!text) return;
  const item = document.createElement("article");
  item.className = `event ${kind}`;
  item.textContent = text;
  item.dataset.characters = String(text.length);
  visibleCharacters += text.length;
  output.append(item);
  trimOutput();
}

function trimOutput() {
  while (output.childElementCount > MAX_VISIBLE_EVENTS || visibleCharacters > MAX_VISIBLE_CHARACTERS) {
    const oldest = output.firstElementChild;
    if (!oldest) break;
    visibleCharacters -= Number(oldest.dataset.characters || oldest.textContent?.length || 0);
    oldest.remove();
  }
  output.scrollTop = output.scrollHeight;
}

function populateProviders() {
  const select = element("new-provider");
  select.replaceChildren();
  for (const provider of state.providers.filter((provider) => state.connection.providers.includes(provider.id))) {
    const option = document.createElement("option");
    option.value = provider.id;
    option.textContent = safeVisibleText(provider.display_name);
    select.append(option);
  }
  const workspace = element("new-workspace");
  workspace.replaceChildren();
  for (const root of state.connection.roots) {
    const option = document.createElement("option");
    option.value = root;
    option.textContent = safeVisibleText(root);
    workspace.append(option);
  }
  const startAllowed = hasScope("session.start")
    && state.connection.roots.length > 0
    && state.connection.providers.length > 0;
  workspace.disabled = !startAllowed;
  select.disabled = !startAllowed;
  element("start-session").disabled = !startAllowed;
  element("new-session-help").textContent = startAllowed
    ? "Use an exact workspace and provider approved on the PC."
    : "Starting sessions stays locked until the PC grants an exact workspace and provider.";
}

function isWorkspaceApproved(workspace) {
  const normalized = workspace.replaceAll("\\", "/").replace(/\/+$/u, "");
  return state.connection.roots.some((root) => {
    const approved = root.replaceAll("\\", "/").replace(/\/+$/u, "");
    const insensitive = /^[A-Za-z]:\//u.test(approved);
    const candidate = insensitive ? normalized.toLowerCase() : normalized;
    const boundary = insensitive ? approved.toLowerCase() : approved;
    return candidate === boundary || candidate.startsWith(`${boundary}/`);
  });
}

function hasScope(scope) {
  return state.connection?.scopes.includes(scope) === true;
}

async function runAction(action) {
  try {
    await action();
  } catch (error) {
    setStatus(failureMessage(error, "Action failed"), "offline");
  }
}

function failureMessage(error, fallback) {
  if (error instanceof CoreFailure && error.needsOperator) return `${error.message} Go to the PC.`;
  return error instanceof Error ? error.message : fallback;
}

function setStatus(message, kind) {
  status.textContent = message;
  status.dataset.state = kind;
}

function showFatal(error) {
  setup.hidden = false;
  sessionsView.hidden = true;
  setup.textContent = failureMessage(error, "The phone app could not start.");
  setStatus("Setup failed", "offline");
}

function workspaceName(workspace) {
  return workspace.split(/[\\/]/u).filter(Boolean).at(-1) || workspace;
}

function wait(milliseconds) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function registerServiceWorker() {
  if ("serviceWorker" in navigator) {
    navigator.serviceWorker.addEventListener("message", (event) => {
      if (!isAttentionMessage(event.data)) return;
      state.attentionRequested = true;
      if (!state.connection) return;
      runAction(async () => {
        activateSurface("sessions");
        state.attentionRequested = false;
        await refreshSessions(null, true);
      });
    });
    return navigator.serviceWorker.register("service-worker.js").catch((error) => {
      setStatus(`Install support failed: ${error instanceof Error ? error.message : String(error)}`, "offline");
      return null;
    });
  }
  return null;
}

function isNarrowViewport() {
  return typeof window.matchMedia === "function"
    ? window.matchMedia("(max-width: 760px)").matches
    : window.innerWidth <= 760;
}

function element(id) {
  const found = document.getElementById(id);
  if (!found) throw new Error(`missing phone app element ${id}`);
  return found;
}
