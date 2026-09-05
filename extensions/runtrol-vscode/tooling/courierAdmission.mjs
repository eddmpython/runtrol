// CALL-02: real Runtime admission, retained connections, outside-tree refusal, and generation continuity.
// Usage: node tooling/courierAdmission.mjs --core <built runtrol.exe> --next <different built runtrol.exe>
import assert from "node:assert/strict";
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, open, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { once } from "node:events";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { build } from "esbuild";
import { isolatedRuntimeState, ownedTreeIdentities, terminateCapturedIdentities } from "./isolated-vscode.mjs";
import { processRows, normalizedExecutable } from "./process-identity.mjs";
import { commandJourney } from "./courierCommands.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const core = option("--core");
const nextCore = option("--next");
const probe = path.join(path.dirname(core), "examples", "handoverProbe.exe");
const fixture = fileURLToPath(new URL("./courierProcess.mjs", import.meta.url));
const executionRoot = path.join(process.env.LOCALAPPDATA, "dev-workspace");
const temporary = await mkdtemp(path.join(executionRoot, "runtrolCourier-"));
const workspace = path.join(temporary, "project");
const identity = path.join(temporary, "identity.json");
const { home, environment } = isolatedRuntimeState(temporary);
const pathKey = Object.keys(environment).find((name) => name.toLowerCase() === "path") ?? "PATH";
environment[pathKey] = `${path.dirname(process.execPath)}${path.delimiter}${environment[pathKey] ?? ""}`;
const pipeName = `\\\\.\\pipe\\runtrol-courier-probe-${process.pid}-${Date.now()}`;
const processes = [];
const logs = [];
const peers = new Set();
let firstPeer;
const firstConnected = new Promise((resolve) => { firstPeer = resolve; });
const connected = [];
let pendingPeer;
const server = net.createServer((socket) => {
  peers.add(socket);
  const peer = linePeer(socket);
  if (firstPeer) { firstPeer(peer); firstPeer = null; }
  else if (pendingPeer) { pendingPeer(peer); pendingPeer = null; }
  else connected.push(peer);
});
const extraTerminals = [];
const viewers = [];
let bodyMarker;
try {
  server.listen(pipeName);
  await once(server, "listening");
  await mkdir(workspace);
  await mkdir(path.join(home, "providers"), { recursive: true });
  await writeFile(path.join(home, "providers", "courier-fixture.toml"), [
    'schema = 1', 'id = "courier-fixture"', 'display_name = "Courier Fixture"', 'kind = "acp"',
    '[bin]', `names = [${JSON.stringify(path.basename(process.execPath))}]`,
    '[probe]', 'version = { args = ["--version"], parse = "semver-anywhere" }',
    '[transport]', 'argv = []', 'listen = "stdio"', '[tui]',
    `new = [${JSON.stringify(fixture.replaceAll("\\", "/"))}]`,
    '[tui.env]', `RUNTROL_COURIER_PROBE = ${JSON.stringify(pipeName)}`,
  ].join("\n") + "\n");
  await start(core, "first");
  await waitFor(async () => (await status()).find((generation) => !generation.draining), "first generation");
  const enrolled = command(probe, ["enroll", home, core, identity, workspace]);
  const opened = command(probe, ["open", home, identity, "courier-fixture", workspace]);
  const peer = await deadline(firstConnected, "fixture connection");
  const born = await peer.read();
  assert.equal(born.first?.answer, "welcome", "the first instruction's hello is admitted");
  const birth = born.birth;
  assert.equal((await peer.ask({ kind: "command" })).welcome, true);
  const many = await peer.ask({ kind: "many" });
  assert.equal(many.length, 12);
  assert.ok(many.every((answer) => answer.answer === "welcome"));
  assert.equal((await peer.ask({ kind: "wrongToken" })).answer, "refused");
  const foreign = spawnSync(core, ["courier"], { env: { ...environment, ...birth }, encoding: "utf8",
    windowsHide: true, timeout: 10_000 });
  assert.equal(foreign.status, 1, "an outside process with the right birth values is refused");
  assert.match(foreign.stdout, /courier: refused/);
  const stale = spawnSync(core, ["courier"], { env: { ...environment, ...birth,
    RUNTROL_COURIER_ENDPOINT: birth.RUNTROL_COURIER_ENDPOINT + "-stale" }, encoding: "utf8",
    windowsHide: true, timeout: 10_000 });
  assert.notEqual(stale.status, 0, "a stale generation endpoint is not admitted");
  if (process.argv.includes("--commands")) {
    bodyMarker = await commandJourney(peer, birth.RUNTROL_MANAGED_SESSION, async () => {
      const opened = command(probe, ["open", home, identity, "courier-fixture", workspace]);
      extraTerminals.push(opened);
      const peer = connected.shift() ?? await deadline(new Promise((resolve) => { pendingPeer = resolve; }), "another fixture");
      const born = await peer.read();
      assert.equal(born.first.answer, "welcome");
      return { peer, session: born.birth.RUNTROL_MANAGED_SESSION };
    });
  }
  // This long journey has no Studio window. Hold real SDK views across handover so the existing policy for
  // retiring unwatched, quiet terminals does not legitimately end these fixture processes during the check.
  await watch([opened, ...extraTerminals]);
  await start(nextCore, "next");
  await waitFor(async () => (await status()).some((generation) => generation.digest === opened.generation && generation.draining),
    "first generation draining");
  process.stdout.write("courier handover: checking old managed hello\n");
  assert.equal((await peer.ask({ kind: "hello" })).answer, "welcome", "the draining generation still serves its child");
  assert.equal((await peer.ask({ kind: "command" })).welcome, true);
  process.stdout.write("courier handover: stopping managed terminals\n");
  command(probe, ["stop", home, identity, opened.generation, opened.terminalId]);
  for (const terminal of extraTerminals) command(probe, ["stop", home, identity, terminal.generation, terminal.terminalId]);
  await waitFor(async () => !(await status()).some((generation) => generation.digest === opened.generation), "drained generation exit");
  process.stdout.write(`RUNTROL_COURIER_ADMISSION ${JSON.stringify({ generation: enrolled.generation,
    firstHello: true, twelveHeldClients: true, wrongTokenRefused: true, outsideTreeRefused: true,
    staleEndpointRefused: true, survivedHandover: true, drainedAfterExit: true })}\n`);
} catch (error) {
  for (const label of ["first", "next"]) {
    try {
      const log = await readFile(path.join(temporary, `${label}.log`), "utf8");
      process.stderr.write(`${label} Runtime: ${log.slice(-6000)}\n`);
    } catch (readError) {
      if (readError.code !== "ENOENT") throw readError;
    }
  }
  throw error;
} finally {
  for (const viewer of viewers) viewer.close();
  for (const peer of peers) peer.destroy();
  server.close();
  for (const entry of processes.reverse()) {
    const tree = ownedTreeIdentities(entry.child.pid);
    const current = tree.find((row) => row.pid === entry.child.pid);
    if (current && entry.identity && current.startedAt === entry.identity.startedAt
      && normalizedExecutable(current.executable) === normalizedExecutable(entry.binary)) {
      await terminateCapturedIdentities(tree);
    } else if (current && entry.child.exitCode === null) {
      throw new Error(`cannot prove ownership of cleanup PID ${entry.child.pid}`);
    }
  }
  for (const log of logs) await log.close();
  if (bodyMarker) await noResidue(temporary, bodyMarker);
  await rm(temporary, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}

async function noResidue(directory, marker) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) await noResidue(target, marker);
    else if (entry.isFile()) {
      const bytes = await readFile(target);
      assert.ok(!bytes.includes(Buffer.from(marker)) && !bytes.includes(Buffer.from(marker, "utf16le")),
        `opaque body retained in ${path.basename(target)}`);
    }
  }
}

async function watch(terminals) {
  const bundle = path.join(temporary, "runtimeClient.cjs");
  await build({ entryPoints: [path.join(root, "clients/typescript/src/index.ts")], outfile: bundle,
    bundle: true, platform: "node", format: "cjs", target: "node22", logLevel: "silent" });
  const { RuntimeConnector, RuntimeLocator, IntegrationIdentity, IntegrationCredentials } = createRequire(import.meta.url)(bundle);
  const stored = JSON.parse(await readFile(identity, "utf8"));
  const key = IntegrationIdentity.fromPkcs8(Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(stored.secret),
  ]));
  const credentials = new IntegrationCredentials(key, stored.grant);
  process.env.RUNTROL_HOME = home;
  for (const terminal of terminals) {
    const state = await RuntimeLocator.system({ runtimeExecutable: core, preferDigest: terminal.generation }).inspect();
    assert.equal(state.state, "running");
    const client = await new RuntimeConnector().connect(state.locator, { name: "runtrol-handover-probe", version: "0.0.0", credentials });
    viewers.push(client);
    await client.terminals().attach({ terminalId: terminal.terminalId });
  }
}

function option(name) {
  const index = process.argv.indexOf(name);
  if (index < 0 || !process.argv[index + 1]) throw new Error(`${name} is required`);
  return path.resolve(process.argv[index + 1]);
}

async function start(binary, label) {
  const log = await open(path.join(temporary, `${label}.log`), "w");
  logs.push(log);
  const child = spawn(binary, ["daemon"], { cwd: root, env: environment, windowsHide: true,
    stdio: ["ignore", log.fd, log.fd] });
  const entry = { child, binary, home, identity: null };
  processes.push(entry);
  entry.identity = processRows().find((row) => row.pid === child.pid
    && normalizedExecutable(row.executable) === normalizedExecutable(binary));
  if (!entry.identity) throw new Error(`cannot prove birth identity of daemon ${child.pid}`);
  process.stdout.write(`owned ${label} pid=${child.pid} home=${home}\n`);
  child.on("error", (error) => { process.stderr.write(`${label}: ${error.message}\n`); });
}

function command(binary, words) {
  const result = spawnSync(binary, words, { cwd: root, env: environment, encoding: "utf8", windowsHide: true, timeout: 120_000 });
  if (result.status !== 0) {
    throw new Error(`${words[0]}: ${result.error?.message ?? (result.stderr || result.stdout || `exit ${result.status}`)}`);
  }
  return JSON.parse(result.stdout.trim().split("\n").pop());
}

async function status() {
  try {
    const locator = JSON.parse(await readFile(path.join(home, "runtime.locator.json"), "utf8"));
    return locator.generations ?? [];
  } catch (error) {
    if (error.code === "ENOENT" || error instanceof SyntaxError) return [];
    throw error;
  }
}

async function waitFor(check, description) {
  const end = Date.now() + 60_000;
  while (Date.now() < end) {
    const result = await check();
    if (result) return result;
    for (const entry of processes) {
      if (entry.child.exitCode !== null && entry.child.exitCode !== 0) throw new Error(`daemon exited ${entry.child.exitCode}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`${description} timed out`);
}

function deadline(promise, description) {
  let timer;
  return Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${description} timed out`)), 20_000);
  })]).finally(() => clearTimeout(timer));
}

function linePeer(socket) {
  let buffer = "";
  let ended = null;
  const lines = [];
  const readers = [];
  socket.setEncoding("utf8");
  socket.on("data", (chunk) => {
    buffer += chunk;
    while (buffer.includes("\n")) {
      const end = buffer.indexOf("\n");
      const value = JSON.parse(buffer.slice(0, end));
      buffer = buffer.slice(end + 1);
      const reader = readers.shift();
      if (reader) reader.resolve(value); else lines.push(value);
    }
  });
  const close = (error) => {
    ended = error;
    for (const reader of readers.splice(0)) reader.reject(error);
  };
  socket.on("error", close);
  socket.on("close", () => close(new Error("fixture pipe closed")));
  const read = () => deadline(lines.length ? Promise.resolve(lines.shift()) : ended ? Promise.reject(ended)
    : new Promise((resolve, reject) => readers.push({ resolve, reject })), "fixture reply");
  return { read, async ask(request) {
    socket.write(JSON.stringify(request) + "\n");
    const answer = await read();
    if (answer.failure) throw new Error(answer.failure);
    return answer.result;
  } };
}
