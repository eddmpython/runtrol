import net from "node:net";

const MAX_FRAME = 16 * 1024 * 1024 + 64 * 1024;
const [endpoint, wireText, requestText] = process.argv.slice(2);

if (!endpoint || !wireText || !requestText) {
  throw new Error("usage: node missionIpc.mjs <endpoint> <wire> <request-json>");
}

function frame(value) {
  const payload = Buffer.from(JSON.stringify(value), "utf8");
  if (payload.length > MAX_FRAME) throw new Error("request exceeds the wire frame bound");
  const encoded = Buffer.allocUnsafe(payload.length + 4);
  encoded.writeUInt32BE(payload.length, 0);
  payload.copy(encoded, 4);
  return encoded;
}

function receiver(socket) {
  let buffered = Buffer.alloc(0);
  const waiting = [];
  let failure = null;

  function drain() {
    while (buffered.length >= 4) {
      const length = buffered.readUInt32BE(0);
      if (length > MAX_FRAME) {
        fail(new Error("response exceeds the wire frame bound"));
        return;
      }
      if (buffered.length < length + 4) return;
      const payload = buffered.subarray(4, length + 4);
      buffered = buffered.subarray(length + 4);
      const next = waiting.shift();
      if (!next) {
        fail(new Error("the daemon sent an unsolicited response"));
        return;
      }
      next.resolve(payload);
    }
  }

  function fail(error) {
    if (failure) return;
    failure = error;
    for (const next of waiting.splice(0)) next.reject(error);
  }

  socket.on("data", (chunk) => {
    buffered = Buffer.concat([buffered, chunk]);
    drain();
  });
  socket.once("error", fail);
  socket.once("close", () => fail(new Error("the daemon connection closed")));

  return () => {
    if (failure) return Promise.reject(failure);
    return new Promise((resolve, reject) => waiting.push({ resolve, reject }));
  };
}

const socket = net.createConnection(endpoint);
socket.setTimeout(60_000, () => socket.destroy(new Error("the daemon response timed out")));
await new Promise((resolve, reject) => {
  socket.once("connect", resolve);
  socket.once("error", reject);
});
const receive = receiver(socket);

let succeeded = false;
try {
  socket.write(frame({ ask: "hello", with: { wire: Number(wireText) } }));
  const welcome = JSON.parse((await receive()).toString("utf8"));
  if (welcome.say !== "welcome") throw new Error(JSON.stringify(welcome));
  socket.write(frame(JSON.parse(requestText)));
  const response = JSON.parse((await receive()).toString("utf8"));
  process.stdout.write(`${JSON.stringify(response)}\n`);
  succeeded = true;
} finally {
  if (succeeded) socket.end();
  else socket.destroy();
}
