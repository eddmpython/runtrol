import { spawnSync } from "node:child_process";
import { readFile, stat, writeFile } from "node:fs/promises";
import net from "node:net";
import path from "node:path";

const MAX_FRAME_BYTES = 1024 * 1024;

export async function approveNextTestIntegration(core, environment, timeoutMs = 180_000, signal = null) {
  const marker = environment.RUNTROL_TEST_EXTERNAL_INTEGRATION_APPROVAL;
  if (typeof marker !== "string" || !path.isAbsolute(marker)) {
    throw new Error("the external integration approval marker must be absolute");
  }
  const wire = await privateWireVersion();
  const deadline = Date.now() + timeoutMs;
  let lastFailure = "the Core executable is not ready";
  let enrollmentObserved = false;
  while (Date.now() < deadline) {
    if (signal?.aborted) return null;
    try {
      await stat(core);
      const located = spawnSync(core, ["endpoint"], {
        env: environment,
        encoding: "utf8",
        timeout: 5_000,
        windowsHide: true,
      });
      if (located.status !== 0 || !located.stdout.trim()) {
        lastFailure = located.stderr || located.stdout || "Core endpoint discovery failed";
      } else {
        const connection = await FramedConnection.connect(located.stdout.trim());
        try {
          await expect(connection, { ask: "hello", with: { wire } }, "welcome");
          const listed = await expect(connection, { ask: "integrationEnrollments" }, "integrationEnrollments");
          if (!Array.isArray(listed.with)) {
            throw new Error("the private IPC enrollment list is malformed");
          }
          const enrollment = listed.with.find((candidate) => candidate.client_name === "Runtrol Studio");
          if (enrollment) {
            enrollmentObserved = true;
            if (!Array.isArray(enrollment.scopes) || !Array.isArray(enrollment.roots)) {
              throw new Error("the Studio integration proposal has malformed authority");
            }
            const begun = await expect(
              connection,
              {
                ask: "integrationApprovalBegin",
                with: {
                  pending_id: enrollment.pending_id,
                  scopes: enrollment.scopes,
                  roots: enrollment.roots,
                },
              },
              "integrationApprovalChallenge",
            );
            const answer = challengeAnswer(begun.with.prompt);
            const approved = await expect(
              connection,
              {
                ask: "integrationApprovalFinish",
                with: { challenge_id: begun.with.challenge_id, answer },
              },
              "integrationApproved",
            );
            const integrationId = approved.with.integration_id;
            if (typeof integrationId !== "string" || !/^int_[0-9a-f]{32}$/u.test(integrationId)) {
              throw new Error("the approved integration identity is malformed");
            }
            await writeFile(marker, `${integrationId}\n`, { encoding: "utf8", flag: "wx" });
            return integrationId;
          }
          lastFailure = "the Studio integration proposal is not pending yet";
        } finally {
          connection.close();
        }
      }
    } catch (error) {
      lastFailure = error instanceof Error ? error.message : String(error);
      if (enrollmentObserved) {
        throw new Error(`the isolated Studio integration approval failed: ${lastFailure}`);
      }
    }
    await delay(50);
  }
  if (signal?.aborted) return null;
  throw new Error(`the isolated Studio integration was not approved: ${lastFailure}`);
}

async function privateWireVersion() {
  const source = await readFile(new URL("../src/protocol.ts", import.meta.url), "utf8");
  const match = /^export const WIRE_VERSION = (\d+);$/mu.exec(source);
  if (!match) {
    throw new Error("the private IPC wire version is not readable from its source of truth");
  }
  return Number(match[1]);
}

async function expect(connection, request, expected) {
  await connection.send(request);
  const response = await connection.receive();
  if (!response || typeof response !== "object" || Array.isArray(response)) {
    throw new Error("the private IPC response is not an object");
  }
  if (response.say === "failed") {
    throw new Error(`the private IPC request failed: ${String(response.with?.message)}`);
  }
  if (response.say !== expected) {
    throw new Error(`the private IPC answered ${String(response.say)}, expected ${expected}`);
  }
  return response;
}

function challengeAnswer(prompt) {
  if (typeof prompt !== "string") {
    throw new Error("the integration approval challenge has no prompt");
  }
  const marker = "type: ";
  const offset = prompt.lastIndexOf(marker);
  const answer = offset < 0 ? "" : prompt.slice(offset + marker.length).trim();
  if (!/^[a-z]+(?:-[a-z]+){2}$/u.test(answer)) {
    throw new Error("the integration approval challenge has no exact three-word answer");
  }
  return answer;
}

class FramedConnection {
  constructor(socket) {
    this.socket = socket;
    this.iterator = socket[Symbol.asyncIterator]();
    this.buffer = Buffer.alloc(0);
  }

  static async connect(endpoint) {
    const socket = net.createConnection(endpoint);
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        socket.destroy();
        reject(new Error("the private IPC connection timed out"));
      }, 5_000);
      const connected = () => {
        clearTimeout(timeout);
        socket.off("error", failed);
        resolve();
      };
      const failed = (error) => {
        clearTimeout(timeout);
        socket.off("connect", connected);
        reject(error);
      };
      socket.once("connect", connected);
      socket.once("error", failed);
    });
    return new FramedConnection(socket);
  }

  async send(value) {
    const payload = Buffer.from(JSON.stringify(value));
    if (payload.length > MAX_FRAME_BYTES) {
      throw new Error("the private IPC request exceeds its test bound");
    }
    const frame = Buffer.allocUnsafe(4 + payload.length);
    frame.writeUInt32BE(payload.length, 0);
    payload.copy(frame, 4);
    await new Promise((resolve, reject) => {
      this.socket.write(frame, (error) => error ? reject(error) : resolve());
    });
  }

  async receive() {
    await this.fill(4);
    const length = this.buffer.readUInt32BE(0);
    if (length > MAX_FRAME_BYTES) {
      throw new Error("the private IPC response exceeds its test bound");
    }
    await this.fill(4 + length);
    const payload = this.buffer.subarray(4, 4 + length);
    this.buffer = this.buffer.subarray(4 + length);
    return JSON.parse(payload.toString("utf8"));
  }

  close() {
    this.socket.destroy();
  }

  async fill(length) {
    while (this.buffer.length < length) {
      const next = await this.iterator.next();
      if (next.done) {
        throw new Error("the private IPC connection closed before its response completed");
      }
      this.buffer = Buffer.concat([this.buffer, next.value]);
      if (this.buffer.length > MAX_FRAME_BYTES + 4) {
        throw new Error("the private IPC receive buffer exceeds its test bound");
      }
    }
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
