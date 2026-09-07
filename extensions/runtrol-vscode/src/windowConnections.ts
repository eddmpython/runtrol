import type { RuntimeClient, WindowInputSubscription, WindowRevealSubscription } from "@runtrol/runtime-client";
import type { ValidatedLocator } from "@runtrol/runtime-client";
import type {
  WindowMirrorOpenParams, WindowRegistration, WindowRegisterParams, WindowUpdateParams,
} from "@runtrol/runtime-client";

export type WindowRoute = { readonly locator: ValidatedLocator; readonly currentRevision: string };
export type WindowState = { readonly register: WindowRegisterParams; readonly update: WindowUpdateParams };
export type WindowConnection = Pick<RuntimeClient, "windows" | "close" | "initialization">;
type Lane = "registration" | "mirror" | "input" | "reveal";
type MirrorRequest = Omit<WindowMirrorOpenParams, "registrationGeneration" | "ownerToken">;

export interface WindowConnectionHooks<C extends WindowConnection> {
  /** Exact routes from the caller's latest validated locator snapshot. No selection is performed here. */
  listed(): readonly ValidatedLocator[];
  connect(route: WindowRoute, lane: Exclude<Lane, "registration">, signal: AbortSignal): Promise<C>;
  /** Equality stamp from the authenticated integration identity and key/grant generations. */
  authorityRevision(connection: C): string;
  connectionFailure(error: unknown): boolean;
  reveal(group: WindowOwnershipGroup<C>, terminalKey: string, signal: AbortSignal): void | Promise<void>;
  input?: (group: WindowOwnershipGroup<C>, subscription: WindowInputSubscription, signal: AbortSignal) => Promise<void>;
  failed(group: WindowOwnershipGroup<C>, lane: Lane, error: unknown): void;
}

export interface WindowGroupCandidate<C extends WindowConnection> {
  update(state: WindowState): Promise<void>;
  commit(): WindowOwnershipGroup<C>;
  abort(): void;
}

/** Window ownership only. The caller owns primary selection, command arbitration and locator observation. */
export class WindowConnections<C extends WindowConnection> {
  private readonly groups = new Map<string, WindowOwnershipGroup<C>>();
  private closed = false;
  private readonly abort: () => void;

  constructor(private readonly hooks: WindowConnectionHooks<C>, private readonly lifetime: AbortSignal) {
    this.abort = () => this.close();
    lifetime.addEventListener("abort", this.abort, { once: true });
    if (lifetime.aborted) this.close();
  }

  lookup(currentRevision: string): WindowOwnershipGroup<C> | undefined {
    const group = this.groups.get(currentRevision);
    return group?.committed && !group.closed ? group : undefined;
  }

  /** Membership cleanup only; the caller still selects and commits the primary route. */
  pruneMembership(): void {
    const listed = this.hooks.listed();
    for (const locator of listed) locator.assertSdkValidated();
    for (const group of [...this.groups.values()]) {
      if (!listed.some(locator => sameGeneration(locator, group.route.locator)
        && locator.revision === group.route.locator.revision)) {
        group.close("window owner incarnation left the validated generation snapshot");
      }
    }
  }

  /** Ownership of registrationConnection transfers at entry, including failed or cancelled preparation. */
  async prepare(route: WindowRoute, registrationConnection: C, state: WindowState, signal?: AbortSignal): Promise<WindowGroupCandidate<C>> {
    const existing = this.lookup(route.currentRevision);
    if (existing?.connection === registrationConnection) {
      signal?.throwIfAborted();
      return { update: state => existing.update(state), commit: () => existing, abort: () => undefined };
    }
    let group: WindowOwnershipGroup<C> | null = null;
    try {
      if (this.closed) throw new Error("window connection lifetime ended");
      signal?.throwIfAborted();
      route.locator.assertSdkValidated();
      if (route.currentRevision !== route.locator.revision) throw new Error("window route revision does not match its validated locator");
      const listed = this.hooks.listed();
      for (const candidate of listed) candidate.assertSdkValidated();
      if (!listed.some(candidate => sameGeneration(candidate, route.locator) && candidate.revision === route.locator.revision)) {
        throw new Error("window route is not in the validated generation snapshot");
      }
      if ([...this.groups.values()].some(candidate => sameGeneration(candidate.route.locator, route.locator))) {
        throw new Error("retire the prior ownership group before replacing its generation incarnation");
      }
      // One group per member of the validated locator. Its existing schema ceiling is the bound.
      const represented = new Set(listed.map(generationKey));
      if (this.groups.size >= represented.size) throw new Error("window ownership groups exceed the validated generation membership");
      group = new WindowOwnershipGroup(route, registrationConnection, state, this.hooks, ended => {
        if (this.groups.get(route.currentRevision) === ended) this.groups.delete(route.currentRevision);
      });
      this.groups.set(route.currentRevision, group);
      const preparing = group;
      const abort = () => preparing.close("window group preparation cancelled");
      signal?.addEventListener("abort", abort, { once: true });
      try {
        await group.prepare();
        signal?.throwIfAborted();
      } catch (error) {
        signal?.removeEventListener("abort", abort);
        throw error;
      }
      let settled = false;
      return {
        update: state => {
          if (settled || preparing.closed) return Promise.reject(new Error("window group candidate is no longer available"));
          return preparing.update(state);
        },
        commit: () => {
          if (settled || preparing.closed) throw new Error("window group candidate is no longer available");
          settled = true;
          signal?.removeEventListener("abort", abort);
          preparing.activate();
          return preparing;
        },
        abort: () => {
          if (settled) return;
          settled = true;
          signal?.removeEventListener("abort", abort);
          preparing.close("window group candidate aborted");
        },
      };
    } catch (error) {
      if (group) group.close("window group preparation failed"); else registrationConnection.close();
      throw error;
    }
  }

  /** The latest shell state reaches retained generations as well as the caller's primary group. */
  async update(state: WindowState): Promise<readonly PromiseSettledResult<void>[]> {
    return Promise.allSettled([...this.groups.values()].map(group => group.update(state)));
  }

  /** Retire current authority without ending the owner lifetime. Existing mirrors never rebind. */
  clear(reason: string): void {
    for (const group of [...this.groups.values()]) group.close(reason);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.lifetime.removeEventListener("abort", this.abort);
    this.clear("window connection lifetime ended");
  }
}

export class WindowOwnershipGroup<C extends WindowConnection> {
  private state: WindowState;
  private updateEpoch = 1;
  private publishedEpoch = 0;
  private publishing: Promise<void> | null = null;
  private proof: WindowRegistration | null = null;
  private readonly authority: string;
  private readonly lifetime = new AbortController();
  private readonly connections = new Set<C>();
  private revealSubscription: WindowRevealSubscription | null = null;
  private inputSubscription: WindowInputSubscription | null = null;
  private feeder: { connection: C; tail: Promise<void> } | null = null;
  private openingFeeder: Promise<{ connection: C; tail: Promise<void> }> | null = null;
  private readonly mirrors = new Set<BoundMirror<C>>();
  private active = false;
  private ended = false;

  constructor(
    readonly route: WindowRoute,
    readonly connection: C,
    state: WindowState,
    private readonly hooks: WindowConnectionHooks<C>,
    private readonly retired: (group: WindowOwnershipGroup<C>) => void,
  ) {
    this.state = state;
    this.authority = hooks.authorityRevision(connection);
    this.connections.add(connection);
  }

  get committed(): boolean { return this.active; }
  get closed(): boolean { return this.ended; }
  get signal(): AbortSignal { return this.lifetime.signal; }
  get registration(): WindowRegistration {
    if (!this.proof) throw new Error("window registration is not prepared");
    return this.proof;
  }

  async prepare(): Promise<void> {
    this.requireOpen();
    this.proof = await this.connection.windows().register(this.state.register);
    this.requireOpen();
    await this.flushUpdate();
    const reveal = await this.connect("reveal");
    const reveals = await reveal.windows().watchReveals({ windowSessionId: this.state.register.windowSessionId });
    if (this.closed) { reveals.close(); this.requireOpen(); }
    this.revealSubscription = reveals;
    this.requireOpen();
    if (this.proof.ownerToken && this.hooks.input) {
      const input = await this.connect("input");
      const inputs = await input.windows().watchInput({
        windowSessionId: this.state.register.windowSessionId,
        registrationGeneration: this.proof.registrationGeneration,
        ownerToken: this.proof.ownerToken,
      });
      if (this.closed) { inputs.close(); this.requireOpen(); }
      this.inputSubscription = inputs;
      this.requireOpen();
    }
    // An update arriving while subscriptions were prepared must reach this candidate before commit.
    await this.flushUpdate();
  }

  activate(): void {
    this.requireOpen();
    if (this.active) return;
    this.active = true;
    const reveal = this.revealSubscription;
    if (reveal) void this.followReveals(reveal);
    const input = this.inputSubscription;
    if (input && this.hooks.input) {
      void this.hooks.input(this, input, this.signal).then(
        () => { if (!this.closed) this.subscriptionEnded("input", new Error("owner input ended; existing mirror authority was not rebound")); },
        error => this.subscriptionEnded("input", error),
      );
    }
  }

  async update(state: WindowState): Promise<void> {
    this.requireOpen();
    if (state.register.windowSessionId !== this.state.register.windowSessionId
      || state.register.hostGeneration !== this.state.register.hostGeneration) {
      throw new Error("window identity changed; existing mirror registration cannot be rebound");
    }
    this.state = state;
    this.updateEpoch += 1;
    if (this.proof) await this.flushUpdate();
  }

  private flushUpdate(): Promise<void> {
    if (this.publishing) return this.publishing;
    const publishing = (async () => {
      while (!this.closed && this.publishedEpoch !== this.updateEpoch) {
        const epoch = this.updateEpoch;
        const params: WindowUpdateParams = this.connection.initialization.serverCapabilities.windowWorkspaceFoldersUpdate === true
          ? { ...this.state.update, workspaceFolders: this.state.register.workspaceFolders }
          : { terminals: this.state.update.terminals };
        try { await this.connection.windows().update(params); }
        catch (error) {
          this.registrationFailed(error);
          throw error;
        }
        this.requireOpen();
        this.publishedEpoch = epoch;
      }
    })();
    this.publishing = publishing;
    void publishing.then(
      () => { if (this.publishing === publishing) this.publishing = null; },
      () => { if (this.publishing === publishing) this.publishing = null; },
    );
    return publishing;
  }

  /** A failure on this exact dedicated registration connection, never the generic command or primary pointer. */
  registrationFailed(error: unknown): void {
    if (this.closed) return;
    if (this.hooks.connectionFailure(error)) this.close("window registration connection was lost");
    this.hooks.failed(this, "registration", error);
  }

  async openMirror(params: MirrorRequest): Promise<BoundMirror<C>> {
    this.requireActive();
    if (!this.registration.ownerToken) throw new Error("Runtime returned no mirror owner proof");
    if (params.windowSessionId !== this.state.register.windowSessionId) throw new Error("mirror window identity does not match its route");
    await this.flushUpdate();
    const feeder = await this.mirrorFeeder();
    this.requireActive();
    const opened = await this.feed(feeder, connection => connection.windows().mirrorOpen({
      ...params, registrationGeneration: this.registration.registrationGeneration, ownerToken: this.registration.ownerToken!,
    }));
    this.requireActive();
    const mirror = new BoundMirror(this, feeder, opened.terminalId);
    this.mirrors.add(mirror);
    return mirror;
  }

  /** Exact feeder captured by the handle. A failed mutation is reported once and is never replayed. */
  feed<T>(feeder: { connection: C; tail: Promise<void> }, run: (connection: C) => Promise<T>): Promise<T> {
    const result = feeder.tail.then(async () => {
      this.requireActive();
      if (this.feeder !== feeder) throw new Error("the original mirror feeder ended");
      try { return await run(feeder.connection); }
      catch (error) {
        if (this.feeder === feeder) {
          this.feeder = null;
          feeder.connection.close();
          this.connections.delete(feeder.connection);
          for (const mirror of this.mirrors) if (mirror.fedBy(feeder)) mirror.invalidate();
        }
        this.hooks.failed(this, "mirror", error);
        throw error;
      }
    });
    feeder.tail = result.then(() => undefined, () => undefined);
    return result;
  }

  forgetMirror(mirror: BoundMirror<C>): void { this.mirrors.delete(mirror); }

  private async mirrorFeeder(): Promise<{ connection: C; tail: Promise<void> }> {
    if (this.feeder) return this.feeder;
    if (this.openingFeeder) return this.openingFeeder;
    const opening = this.connect("mirror").then(connection => {
      const feeder = { connection, tail: Promise.resolve() };
      this.feeder = feeder;
      return feeder;
    });
    this.openingFeeder = opening;
    try { return await opening; }
    finally { if (this.openingFeeder === opening) this.openingFeeder = null; }
  }

  private async connect(lane: Exclude<Lane, "registration">): Promise<C> {
    this.requireOpen();
    const connection = await this.hooks.connect(this.route, lane, this.signal);
    try {
      this.requireOpen();
      if (this.hooks.authorityRevision(connection) !== this.authority) {
        throw new Error("window registration authority changed; existing mirror ownership cannot be rebound");
      }
      this.connections.add(connection);
      return connection;
    } catch (error) { connection.close(); throw error; }
  }

  private async followReveals(subscription: WindowRevealSubscription): Promise<void> {
    try {
      while (!this.closed) {
        const notification = await subscription.next();
        if (this.closed) return;
        if (notification.kind === "ended") throw new Error("window reveal subscription ended");
        await this.hooks.reveal(this, notification.requested.terminalKey, this.signal);
      }
    } catch (error) { this.subscriptionEnded("reveal", error); }
  }

  private subscriptionEnded(lane: "input" | "reveal", error: unknown): void {
    if (this.closed) return;
    if (lane === "input") { this.inputSubscription?.close(); this.inputSubscription = null; }
    else { this.revealSubscription?.close(); this.revealSubscription = null; }
    // No automatic re-registration, owner-token refresh, mutation replay, or silent migration.
    this.hooks.failed(this, lane, error);
  }

  close(_reason: string): void {
    if (this.ended) return;
    this.ended = true;
    this.lifetime.abort();
    this.inputSubscription?.close();
    this.revealSubscription?.close();
    this.inputSubscription = null;
    this.revealSubscription = null;
    for (const mirror of [...this.mirrors]) mirror.invalidate();
    this.feeder = null;
    for (const connection of this.connections) connection.close();
    this.connections.clear();
    this.retired(this);
  }

  private requireOpen(): void { if (this.closed) throw new Error("window ownership group ended"); }
  private requireActive(): void { this.requireOpen(); if (!this.active) throw new Error("window ownership group was not committed"); }
}

/** One real mirror on one feeder. No output is copied, queued independently, or sent to another generation. */
export class BoundMirror<C extends WindowConnection> {
  private ended = false;
  constructor(
    private readonly group: WindowOwnershipGroup<C>,
    private readonly feeder: { connection: C; tail: Promise<void> },
    readonly terminalId: string,
  ) {}
  get route(): WindowRoute { return this.group.route; }
  get closed(): boolean { return this.ended; }
  fedBy(feeder: object): boolean { return this.feeder === feeder; }
  output(bytesBase64: string): Promise<void> {
    if (this.ended) return Promise.reject(new Error("mirror feeder is no longer available"));
    return this.group.feed(this.feeder, connection => connection.windows().mirrorOutput({ terminalId: this.terminalId, bytesBase64 }));
  }
  async end(exitCode?: number): Promise<void> {
    if (this.ended) return;
    this.ended = true;
    try { await this.group.feed(this.feeder, connection => connection.windows().mirrorEnd({ terminalId: this.terminalId, ...(exitCode === undefined ? {} : { exitCode }) })); }
    finally { this.group.forgetMirror(this); }
  }
  invalidate(): void { this.ended = true; this.group.forgetMirror(this); }
}

function generationKey(locator: ValidatedLocator): string {
  return JSON.stringify([locator.instanceId, locator.digest, locator.endpoint]);
}
function sameGeneration(left: ValidatedLocator, right: ValidatedLocator): boolean {
  return generationKey(left) === generationKey(right);
}
