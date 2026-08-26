import { openDeviceStore } from "./identityStore.js";
import { utf8 } from "./bytes.js";
import { Terminal } from "./vendor/xterm/xterm.mjs";
import { FitAddon } from "./vendor/xterm/addon-fit.mjs";
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
import { disablePush, enablePush, pushAvailable, synchronizePush } from "./push.js";
import { safeVisibleText } from "./presentation.js";

const state = {
  store: null,
  identity: null,
  connection: null,
  pairing: null,
  sessions: [],
  usage: [],
  providers: [],
  selected: null,
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
const sessionList = element("session-list");
const usageStrip = element("usage-strip");
const sessionDetail = element("session-detail");
const terminalHost = element("terminal");
const terminalNote = element("terminal-note");
const refresh = element("refresh");
const panic = element("panic");
const forget = element("forget-device");
const notifications = element("notifications");
const nextAttention = element("next-attention");
/// The open terminal view: the xterm instance, its fit addon, and the Core channel it rides.
let terminalView = null;
/// Bumped whenever the session index watch must restart; a stale loop sees a new value and ends.
let indexWatchGeneration = 0;

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
  // Interrupt is the terminal's own word for it: Ctrl+C into the CLI, exactly as a keyboard would send it.
  element("interrupt").addEventListener("click", () => runAction(async () => {
    if (!terminalView) throw new Error("No conversation is open.");
    await terminalView.client.sendTerminalInput(base64Of(utf8("\u0003")));
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
  indexWatchGeneration += 1;
  state.usage = [];
  setup.hidden = false;
  sessionsView.hidden = true;
  panic.disabled = true;
  forget.hidden = true;
  notifications.hidden = true;
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
      state.usage = Array.isArray(response.with.usage) ? response.with.usage : [];
    } finally {
      client.close();
    }
    populateProviders();
    await synchronizeNotifications();
    renderSessions();
    renderUsage();
    followSessionIndex();
    const selected = preferredSession(
      state.sessions,
      requestedSession,
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

/// Every account's position against its limits, icon plus progress, from the index the PC pushes.
function renderUsage() {
  usageStrip.replaceChildren();
  usageStrip.hidden = state.usage.length === 0;
  for (const line of state.usage) {
    const row = document.createElement("div");
    row.className = "usage-row";
    const window = line.primary ?? line.secondary ?? null;
    const percent = typeof window?.used_percent === "number" ? Math.max(0, Math.min(100, window.used_percent)) : null;
    const detail = [];
    if (line.reached) detail.push("limit reached");
    if (percent !== null) detail.push(`${percent}%`);
    if (typeof line.tokens_today === "number") detail.push(`${formatTokens(line.tokens_today)} today`);
    if (detail.length === 0) detail.push("no limit reported");
    row.innerHTML = `<span class="usage-name"></span><span class="usage-meter"><span></span></span><span class="usage-detail"></span>`;
    row.querySelector(".usage-name").textContent = safeVisibleText(line.provider);
    const meter = row.querySelector(".usage-meter");
    meter.classList.toggle("reached", line.reached === true);
    meter.firstElementChild.style.width = `${percent ?? 0}%`;
    meter.hidden = percent === null;
    row.querySelector(".usage-detail").textContent = detail.join(" · ");
    usageStrip.append(row);
  }
}

function formatTokens(tokens) {
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(1)}B tokens`;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M tokens`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k tokens`;
  return `${tokens} tokens`;
}

/// Keep the rows and the usage strip current from the PC's own push: one index watch on its own
/// connection, replaced whenever the surface reconnects, never a clock.
function followSessionIndex() {
  indexWatchGeneration += 1;
  const generation = indexWatchGeneration;
  (async () => {
    while (generation === indexWatchGeneration && state.connection) {
      let client = null;
      try {
        client = await CoreClient.connect(state.connection, state.identity);
        await client.beginSessionWatch();
        while (generation === indexWatchGeneration) {
          const listing = await client.nextSessions();
          if (generation !== indexWatchGeneration) break;
          state.sessions = listing.sessions;
          state.usage = Array.isArray(listing.usage) ? listing.usage : [];
          if (state.selected) {
            state.selected = state.sessions.find((session) => session.session === state.selected.session) ?? state.selected;
          }
          renderSessions();
          renderUsage();
        }
      } catch (error) {
        if (generation !== indexWatchGeneration) break;
        setStatus(failureMessage(error, "PC offline"), "offline");
        await new Promise((resolve) => setTimeout(resolve, 2_000));
      } finally {
        client?.close();
      }
    }
  })();
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
  selectSession(next);
}

function selectSession(session) {
  state.selected = session;
  state.watchGeneration += 1;
  closeTerminalView();
  sessionDetail.hidden = false;
  element("selected-title").textContent = safeVisibleText(session.label || workspaceName(session.workspace));
  element("selected-provider").textContent = safeVisibleText(session.provider);
  element("selected-workspace").textContent = safeVisibleText(session.workspace);
  element("delete-session").disabled = !hasScope("session.delete");
  renderSessions();
  // The conversation is the service's own terminal interface, hosted by the Core (`docs/terminalSurface.md`).
  // Opening a stored conversation is a resume; this phone needs that scope, the service, and the folder.
  const reason = !session.native
    ? "This conversation has no identity its service can reopen."
    : !hasScope("session.resume")
      ? "This phone may not reopen conversations."
      : !hasScope("session.input.write")
        ? "This phone may not type into conversations."
        : !state.connection.providers.includes(session.provider)
          ? "This phone is not approved for that service."
          : !isWorkspaceApproved(session.workspace)
            ? "This phone is not approved for that folder."
            : null;
  element("interrupt").disabled = reason !== null;
  terminalNote.hidden = reason === null;
  terminalNote.textContent = reason ?? "";
  if (reason === null) void openTerminalView(session, state.watchGeneration);
}

/// Everything the phone needs to show the conversation: xterm draws, the Core hosts, the channel carries.
async function openTerminalView(session, generation) {
  const terminal = new Terminal({
    cursorBlink: true,
    fontSize: 13,
    fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, monospace',
    scrollback: 0,
    allowProposedApi: false,
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);
  terminal.open(terminalHost);
  fit.fit();
  let client;
  try {
    client = await CoreClient.connect(state.connection, state.identity);
    if (generation !== state.watchGeneration) {
      client.close();
      terminal.dispose();
      return;
    }
    terminalView = { terminal, fit, client };
    await client.beginTerminal(session, terminal.cols, terminal.rows);
    terminal.onData((data) => {
      void client.sendTerminalInput(base64Of(utf8(data))).catch((error) => setStatus(failureMessage(error, "PC offline"), "offline"));
    });
    terminal.onResize(({ cols, rows }) => {
      void client.sendTerminalResize(cols, rows).catch((error) => setStatus(failureMessage(error, "PC offline"), "offline"));
    });
    terminalView.resize = () => fit.fit();
    window.addEventListener("resize", terminalView.resize);
    terminal.focus();
    setStatus("PC online", "online");
    while (generation === state.watchGeneration) {
      const response = await client.nextTerminal();
      if (response.say === "terminalOutput") {
        terminal.write(bytesOfBase64(response.with.bytes));
      } else if (response.say === "terminalLagged") {
        // The Core sends the whole screen next; a clean page for it.
        terminal.reset();
      } else {
        terminal.write(`\r\n[the service ended with code ${Number(response.with.code)}]\r\n`);
        break;
      }
    }
  } catch (error) {
    if (generation === state.watchGeneration) setStatus(failureMessage(error, "PC offline"), "offline");
  } finally {
    if (generation === state.watchGeneration) {
      client?.close();
    }
  }
}

function closeTerminalView() {
  const view = terminalView;
  terminalView = null;
  if (!view) return;
  if (view.resize) window.removeEventListener("resize", view.resize);
  view.client.close();
  view.terminal.dispose();
  terminalHost.replaceChildren();
}

function base64Of(bytes) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function bytesOfBase64(text) {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function clearSelection() {
  state.selected = null;
  state.watchGeneration += 1;
  closeTerminalView();
  sessionDetail.hidden = true;
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
