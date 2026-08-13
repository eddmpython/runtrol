export const WIRE_VERSION = 13;

export type WorkspaceAccess = "exclusive" | "shared";

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

export type ProviderUpdateState = "current" | "available" | "observeOnly" | "notInstalled" | "unconfirmed";

export type ProviderUpdateLine = {
  provider: string;
  state: ProviderUpdateState;
  package: string | null;
  installed: string | null;
  target: string | null;
  rollback: string | null;
  why: string | null;
};

export type IntegrationEnrollmentLine = {
  pending_id: string;
  client_name: string;
  client_version: string;
  client_instance_id: string;
  key_fingerprint: string;
  scopes: string[];
  roots: string[];
  expires_at_ms: number;
};

export type IntegrationLine = {
  integration_id: string;
  label: string;
  client_instance_id: string;
  scopes: string[];
  available_scopes: string[];
  roots: string[];
  grant_generation: number;
  revoked: boolean;
};

export type RuntimeForgetLine = {
  confirmation_id: string;
  integration_id: string;
  integration_label: string;
  session_id: string;
  expires_at_ms: number;
};

export type RuntimeKeyRotationLine = {
  confirmation_id: string;
  integration_id: string;
  integration_label: string;
  current_key_generation: number;
  new_key_fingerprint: string;
  expires_at_ms: number;
};

export type SessionLine = {
  session: string;
  provider: string;
  native: string | null;
  label?: string | null;
  workspace: string;
  hot: boolean;
  doing: string;
  looks_stuck: boolean;
  runtime_lifecycle?: "hotIdle" | "hotRunning" | "cold" | "failed";
  session_generation?: number;
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
  | { ask: "watchSessions" }
  | { ask: "models"; with: { provider: string } }
  | { ask: "providerUpdates" }
  | { ask: "providerUpdate"; with: { provider: string } }
  | { ask: "integrationEnrollments" }
  | {
      ask: "integrationApprovalBegin";
      with: { pending_id: string; scopes: string[]; roots: string[] };
    }
  | { ask: "integrationApprovalFinish"; with: { challenge_id: string; answer: string } }
  | { ask: "integrationEnrollmentDeny"; with: { pending_id: string } }
  | { ask: "integrations" }
  | { ask: "integrationRevoke"; with: { integration_id: string } }
  | {
      ask: "integrationGrantChange";
      with: {
        integration_id: string;
        expected_grant_generation: number;
        scopes: string[];
        roots: string[];
      };
    }
  | { ask: "runtimeForgetRequests" }
  | { ask: "runtimeForgetConfirm"; with: { confirmation_id: string } }
  | { ask: "runtimeKeyRotationRequests" }
  | { ask: "runtimeKeyRotationConfirm"; with: { confirmation_id: string } }
  | {
      ask: "start";
      with: {
        provider: string;
        workspace: string;
        workspace_access: WorkspaceAccess;
        model: string | null;
        permission: string | null;
      };
    }
  | {
      ask: "resume";
      with: {
        provider: string;
        native: string;
        workspace: string;
        workspace_access: WorkspaceAccess;
      };
    }
  | { ask: "prompt"; with: { session: string; text: string } }
  | { ask: "rename"; with: { session: string; label: string | null } }
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
  | { say: "providerUpdates"; with: ProviderUpdateLine[] }
  | {
      say: "providerUpdated";
      with: {
        provider: string;
        outcome: "alreadyCurrent" | "updated" | "rolledBack";
        from: string;
        to: string;
        why: string | null;
      };
    }
  | { say: "integrationEnrollments"; with: IntegrationEnrollmentLine[] }
  | {
      say: "integrationApprovalChallenge";
      with: { challenge_id: string; prompt: string };
    }
  | { say: "integrationApproved"; with: { integration_id: string } }
  | { say: "integrations"; with: IntegrationLine[] }
  | { say: "runtimeForgetRequests"; with: RuntimeForgetLine[] }
  | { say: "runtimeKeyRotationRequests"; with: RuntimeKeyRotationLine[] }
  | { say: "started"; with: { session: string } }
  | { say: "done" }
  | {
      say: "watching";
      with: { starts_at: WatchCursor; live_at: WatchCursor; gap: WatchGap | null };
    }
  | { say: "watchingSessions" }
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
