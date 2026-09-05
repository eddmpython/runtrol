// A managed process for the courier admission journey. Birth secrets cross only the test's transient pipe.
import net from "node:net";
import { spawn, spawnSync } from "node:child_process";

const birthNames = ["RUNTROL_COURIER_EXE", "RUNTROL_COURIER_ENDPOINT", "RUNTROL_COURIER_TOKEN", "RUNTROL_MANAGED_SESSION"];
const birth = Object.fromEntries(birthNames.map((name) => [name, process.env[name]]));
const peer = net.createConnection(process.env.RUNTROL_COURIER_PROBE);
const held = [];
const waits = new Map();
let pending = "";
peer.setEncoding("utf8");
peer.on("error", (error) => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
peer.on("connect", async () => {
  try {
    const first = await hello();
    peer.write(JSON.stringify({ birth, first }) + "\n");
  } catch (error) {
    peer.write(JSON.stringify({ failure: error.message }) + "\n");
  }
});
peer.on("data", (chunk) => {
  pending += chunk;
  while (pending.includes("\n")) {
    const at = pending.indexOf("\n");
    const request = JSON.parse(pending.slice(0, at));
    pending = pending.slice(at + 1);
    run(request).then((result) => peer.write(JSON.stringify({ result }) + "\n"),
      (error) => peer.write(JSON.stringify({ failure: error.message }) + "\n"));
  }
});

async function run(request) {
  if (request.kind === "malformed") return malformed(request);
  if (request.kind === "request") return hello({ request: request.command }, 2);
  if (request.kind === "hold") {
    return hello({ request: request.command }, 1, request.key);
  }
  if (request.kind === "release") {
    const socket = waits.get(request.key);
    if (!socket) throw new Error("no held wait");
    socket.destroy(); waits.delete(request.key);
    return true;
  }
  if (request.kind === "cli") {
    return new Promise((resolve, reject) => {
      const child = spawn(birth.RUNTROL_COURIER_EXE, ["courier", ...request.words], {
        windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
      });
      let stdout = ""; let stderr = "";
      const timer = setTimeout(() => { child.kill(); reject(new Error("courier child exceeded test deadline")); }, 15_000);
      child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
      child.stdout.on("data", (chunk) => { stdout += chunk; });
      child.stderr.on("data", (chunk) => { stderr += chunk; });
      child.on("error", (error) => { clearTimeout(timer); reject(error); });
      child.on("close", (status) => { clearTimeout(timer); resolve({ status, stdout, stderr }); });
      child.stdin.end(request.body ?? "");
    });
  }
  if (request.kind === "many") {
    const answers = [];
    for (let index = 0; index < 12; index += 1) answers.push(await hello());
    return answers;
  }
  if (request.kind === "wrongToken") return hello({ token: "wrong" });
  if (request.kind === "hello") return hello();
  if (request.kind === "command") {
    const command = spawnSync(birth.RUNTROL_COURIER_EXE, ["courier"], {
      encoding: "utf8", windowsHide: true, timeout: 10_000,
    });
    return { status: command.status, welcome: command.stdout.trim() === "courier: welcome" };
  }
  throw new Error("unknown test request");
}

function malformed({ shape, marker }) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(birth.RUNTROL_COURIER_ENDPOINT);
    const received = [];
    let receivedBytes = 0;
    const timer = setTimeout(() => { socket.destroy(); reject(new Error("malformed courier probe timed out")); }, 8_000);
    socket.on("error", (error) => {
      if (error.code !== "ECONNRESET" && error.code !== "EPIPE") {
        clearTimeout(timer); reject(new Error("malformed courier probe transport failed"));
      }
    });
    socket.on("data", (chunk) => {
      receivedBytes += chunk.length;
      if (receivedBytes > 4096) {
        clearTimeout(timer); socket.destroy(); reject(new Error("malformed courier refusal exceeded its bound"));
      } else received.push(chunk);
    });
    socket.on("close", () => {
      clearTimeout(timer);
      const bytes = Buffer.concat(received);
      resolve({ closed: true, refused: bytes.includes(Buffer.from('"answer":"refused"')),
        bodyAbsent: !bytes.includes(Buffer.from(marker)) });
    });
    socket.on("connect", () => {
      const invalid = { protocol_version: 1, session: birth.RUNTROL_MANAGED_SESSION,
        token: birth.RUNTROL_COURIER_TOKEN, request: { command: "room_ask", room: birth.RUNTROL_MANAGED_SESSION,
          target: birth.RUNTROL_MANAGED_SESSION, message_id: birth.RUNTROL_MANAGED_SESSION,
          body: { marker }, timeout_ms: 1_000 } };
      const bytes = Buffer.from(shape === "invalidJson" ? `{${marker}` : JSON.stringify(invalid));
      const prefix = Buffer.alloc(4);
      prefix.writeUInt32BE(shape === "oversizedPrefix" ? 16 * 1024 * 6 + 4097 : bytes.length);
      socket.write(shape === "oversizedPrefix" ? prefix : Buffer.concat([prefix, bytes]));
    });
  });
}

function hello(overrides = {}, expected = 1, holdKey = null) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(birth.RUNTROL_COURIER_ENDPOINT);
    held.push(socket);
    if (holdKey !== null) waits.set(holdKey, socket);
    let received = Buffer.alloc(0);
    let frames = 0;
    let settled = false;
    const timer = setTimeout(() => { socket.destroy(); reject(new Error("courier hello timed out")); }, 8_000);
    socket.on("error", (error) => { clearTimeout(timer); reject(error); });
    socket.on("connect", () => {
      const bytes = Buffer.from(JSON.stringify({ protocol_version: 1, session: birth.RUNTROL_MANAGED_SESSION,
        token: birth.RUNTROL_COURIER_TOKEN, ...overrides }));
      const prefix = Buffer.alloc(4);
      prefix.writeUInt32BE(bytes.length);
      socket.write(Buffer.concat([prefix, bytes]));
    });
    socket.on("data", (chunk) => {
      received = Buffer.concat([received, chunk]);
      while (received.length >= 4 && received.length >= 4 + received.readUInt32BE()) {
        const length = received.readUInt32BE();
        const answer = JSON.parse(received.subarray(4, 4 + length).toString());
        received = received.subarray(4 + length);
        frames += 1;
        if (frames === expected || answer.answer === "refused") {
          clearTimeout(timer); settled = true; resolve(answer);
          if (expected === 2 && holdKey === null) socket.end();
        }
      }
    });
    socket.on("end", () => {
      clearTimeout(timer);
      if (!settled) reject(new Error("courier ended before the expected answer"));
      socket.end();
    });
  });
}
