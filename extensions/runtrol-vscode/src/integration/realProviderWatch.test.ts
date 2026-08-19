import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import readline from "node:readline";

type ApprovalFact = {
  kind: "approval";
  approval: string;
  option: number;
  subjectDigest: number[];
  target: string;
};

export type EndFact = {
  kind: "end";
  stop: string;
  declaredBy: string;
};

export type ModelFact = {
  kind: "model";
  model: string;
};

export type ModeFact = {
  kind: "mode";
  mode: string;
};

type Fact = ApprovalFact | EndFact | ModelFact | ModeFact;

export class Watcher {
  readonly ready: Promise<void>;
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly facts: Fact[] = [];
  private readonly waiters: Array<{
    kind: Fact["kind"];
    resolve(fact: Fact): void;
    reject(error: Error): void;
  }> = [];
  private readyResolve: (() => void) | null = null;
  private readyReject: ((error: Error) => void) | null = null;
  private stopped = false;

  constructor(core: string, private readonly session: string) {
    this.ready = new Promise<void>((resolve, reject) => {
      this.readyResolve = resolve;
      this.readyReject = reject;
    });
    this.child = spawn(core, ["watch", session], {
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    this.child.stdin.end();
    this.child.stderr.resume();
    readline.createInterface({ input: this.child.stdout }).on("line", (line) => this.accept(line));
    this.child.once("error", (error) => this.fail(error));
    this.child.once("exit", (code) => {
      if (!this.stopped) {
        this.fail(new Error(`the session watcher exited early with ${String(code)}`));
      }
    });
  }

  async next<K extends Fact["kind"]>(kind: K, timeoutMs: number): Promise<Extract<Fact, { kind: K }>> {
    const found = this.facts.findIndex((fact) => fact.kind === kind);
    if (found >= 0) {
      return this.facts.splice(found, 1)[0] as Extract<Fact, { kind: K }>;
    }
    let timer: NodeJS.Timeout | undefined;
    let waiter: typeof this.waiters[number] | undefined;
    try {
      return await new Promise<Extract<Fact, { kind: K }>>((resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`waiting for ${kind} exceeded ${timeoutMs} ms`)), timeoutMs);
        waiter = {
          kind,
          resolve: (fact) => resolve(fact as Extract<Fact, { kind: K }>),
          reject,
        };
        this.waiters.push(waiter);
      });
    } finally {
      if (timer) {
        clearTimeout(timer);
      }
      const index = waiter ? this.waiters.indexOf(waiter) : -1;
      if (index >= 0) {
        this.waiters.splice(index, 1);
      }
    }
  }

  async stop(): Promise<void> {
    this.stopped = true;
    if (this.child.exitCode !== null) {
      return;
    }
    const exited = new Promise<void>((resolve) => this.child.once("exit", () => resolve()));
    this.child.kill();
    await within(exited, 5_000, "stopping the exact session watcher");
  }

  private accept(line: string): void {
    if (line.startsWith("watching  ")) {
      this.readyResolve?.();
      this.readyResolve = null;
      this.readyReject = null;
      return;
    }
    if (line.startsWith("watch event  next ")) {
      return;
    }
    let event: unknown;
    try {
      event = JSON.parse(line);
    } catch {
      this.fail(new Error("the session watcher emitted a malformed line"));
      return;
    }
    try {
      const fact = watchFact(event, this.session);
      if (fact) {
        this.publish(fact);
      }
    } catch (error) {
      this.fail(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private publish(fact: Fact): void {
    const waiting = this.waiters.findIndex((waiter) => waiter.kind === fact.kind);
    if (waiting >= 0) {
      this.waiters.splice(waiting, 1)[0]?.resolve(fact);
      return;
    }
    if (this.facts.length >= 16) {
      this.fail(new Error("the bounded watcher fact queue overflowed"));
      return;
    }
    this.facts.push(fact);
  }

  private fail(error: Error): void {
    this.readyReject?.(error);
    this.readyResolve = null;
    this.readyReject = null;
    for (const waiter of this.waiters.splice(0)) {
      waiter.reject(error);
    }
  }
}

function watchFact(value: unknown, session: string): Fact | null {
  if (!record(value) || value.session !== session || !record(value.body)) {
    throw new Error("the watcher delivered an event outside the selected session boundary");
  }
  const body = value.body;
  if (body.event === "approvalRequested") {
    const options = Array.isArray(body.options) ? body.options : [];
    const rejection = options.filter(
      (option) => record(option) && option.kind === "rejectOnce" && Number.isInteger(option.id),
    );
    const digest = Array.isArray(body.subject_digest) ? body.subject_digest : [];
    const subject = record(body.subject) ? body.subject : null;
    const input = subject && record(subject.input) ? subject.input : null;
    if (
      typeof body.id !== "string"
      || rejection.length !== 1
      || digest.length !== 32
      || !digest.every((byte) => Number.isInteger(byte) && Number(byte) >= 0 && Number(byte) <= 255)
      || !input
      || typeof input.file_path !== "string"
    ) {
      throw new Error("the provider approval omitted its exact authorization boundary");
    }
    return {
      kind: "approval",
      approval: body.id,
      option: Number(rejection[0]?.id),
      subjectDigest: digest.map(Number),
      target: input.file_path,
    };
  }
  if (body.event === "turn" && body.step === "ended") {
    const declared = record(body.declared_by) ? body.declared_by.by : null;
    if (typeof body.stop !== "string" || typeof declared !== "string") {
      throw new Error("the terminal event omitted outcome provenance");
    }
    return { kind: "end", stop: body.stop, declaredBy: declared };
  }
  if (body.event === "currentModelUpdate") {
    if (typeof body.model_id !== "string" || !body.model_id) {
      throw new Error("the model update omitted which model answers");
    }
    return { kind: "model", model: body.model_id };
  }
  if (body.event === "currentModeUpdate") {
    if (typeof body.mode_id !== "string" || !body.mode_id) {
      throw new Error("the mode update omitted which mode governs");
    }
    return { kind: "mode", mode: body.mode_id };
  }
  if (body.event === "notice" && body.code === "protocolViolation") {
    throw new Error("the installed provider journey hit a protocol violation");
  }
  return null;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

async function within<T>(work: Thenable<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      Promise.resolve(work),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      }),
    ]);
  } finally {
    if (timer) {
      clearTimeout(timer);
    }
  }
}
