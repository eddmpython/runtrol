// A managed process for the courier admission journey. Birth secrets cross only the test's transient pipe.
import net from "node:net";
import { spawnSync } from "node:child_process";

const birthNames = ["RUNTROL_COURIER_EXE", "RUNTROL_COURIER_ENDPOINT", "RUNTROL_COURIER_TOKEN", "RUNTROL_MANAGED_SESSION"];
const birth = Object.fromEntries(birthNames.map((name) => [name, process.env[name]]));
const peer = net.createConnection(process.env.RUNTROL_COURIER_PROBE);
const held = [];
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

function hello(overrides = {}) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(birth.RUNTROL_COURIER_ENDPOINT);
    held.push(socket);
    let received = Buffer.alloc(0);
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
      if (received.length >= 4 && received.length >= 4 + received.readUInt32BE()) {
        clearTimeout(timer);
        resolve(JSON.parse(received.subarray(4, 4 + received.readUInt32BE()).toString()));
        // Keep the client open deliberately: a completed hello must not retain a server admission slot.
      }
    });
    socket.on("end", () => socket.end());
  });
}
