export const WIRE_VERSION = 6;

export type WatchCursor = {
  stream: string;
  epoch: number;
  seq: number;
};

export type WatchGap = {
  requested: WatchCursor;
  live_at: WatchCursor;
};

export type ProviderLine = {
  id: string;
  display_name: string;
  usable: boolean;
  why_not: string | null;
};

export type SessionLine = {
  session: string;
  provider: string;
  native: string | null;
  workspace: string;
  hot: boolean;
  doing: string;
  looks_stuck: boolean;
};

export type SessionListing = {
  sessions: SessionLine[];
  warnings: string[];
};

export type ModelChoice = {
  id: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  reasoningEfforts: Array<{ id: string; description: string }>;
};

export type ModelCatalog =
  | { kind: "known"; models: ModelChoice[] }
  | { kind: "aliases"; aliases: string[]; why: string }
  | { kind: "partial"; aliases: string[]; models: ModelChoice[]; why: string }
  | { kind: "unknown"; why: string };

export type WireError = {
  message: string;
  retryable: boolean;
  needs_the_operator: boolean;
};

export type Request =
  | { ask: "hello"; with: { wire: number } }
  | { ask: "list" }
  | { ask: "models"; with: { provider: string } }
  | {
      ask: "start";
      with: {
        provider: string;
        workspace: string;
        model: string | null;
        permission: string | null;
      };
    }
  | { ask: "prompt"; with: { session: string; text: string } }
  | {
      ask: "answerApproval";
      with: { session: string; approval: string; option: number; subject_digest: number[] };
    }
  | { ask: "interrupt"; with: { session: string } }
  | { ask: "watch"; with: { session: string; after: WatchCursor | null } }
  | { ask: "close"; with: { session: string; now: boolean } };

export type Response =
  | { say: "welcome"; with: { wire: number; providers: ProviderLine[] } }
  | { say: "sessions"; with: SessionListing }
  | { say: "models"; with: ModelCatalog }
  | { say: "started"; with: { session: string } }
  | { say: "done" }
  | {
      say: "watching";
      with: { starts_at: WatchCursor; live_at: WatchCursor; gap: WatchGap | null };
    }
  | { say: "event"; with: { payload: unknown; next_expected: WatchCursor } }
  | { say: "lagged"; with: { next_expected: WatchCursor } }
  | { say: "failed"; with: WireError };

export function requestHello(): Request {
  return { ask: "hello", with: { wire: WIRE_VERSION } };
}

export function readResponse(value: unknown): Response {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("the daemon response is not an object");
  }
  const say = (value as { say?: unknown }).say;
  if (typeof say !== "string") {
    throw new Error("the daemon response has no string discriminator");
  }
  return value as Response;
}

export function failureMessage(response: Response): string | null {
  return response.say === "failed" ? response.with.message : null;
}
