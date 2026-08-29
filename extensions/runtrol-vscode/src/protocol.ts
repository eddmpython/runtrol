export const WIRE_VERSION = 29;

export type PrivateProviderLine = {
  id: string;
  display_name: string;
  usable: boolean;
  why_not: string | null;
  terminal_commands?: string[];
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
  manifest_digest: string;
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
  key_generation: number;
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

/// A public caller asking to write in a working tree somebody is already writing in; the public Runtime
/// holds it until the person at this machine says yes.
export type RuntimeSharedOpenLine = {
  confirmation_id: string;
  integration_id: string;
  integration_label: string;
  operation: string;
  provider_id: string;
  workspace: string;
  expires_at_ms: number;
};

export type MissionLine = {
  mission_id: string;
  name: string;
  project: string;
  state: string;
  completion_policy: "allTasks" | "chooseOne" | "unavailableAfterRestart";
  passed_tasks: number;
  total_tasks: number;
  awaiting_input: number;
  schedule?: MissionScheduleLine | null;
};

export type MissionScheduleProviderLine = {
  task_id: string;
  provider_runtime_id: string;
};

export type MissionScheduleLine = {
  schedule_id: string;
  due_unix_ms: number;
  state: "pending" | "launching" | "started" | "cancelled" | "refused" | "attention";
  providers: MissionScheduleProviderLine[];
  failure: string | null;
};

export type GateLine = {
  gate_id: string;
  program: string;
  timeout_ms: number;
};

export type MissionTaskLine = {
  task_id: string;
  key: string;
  state: string;
  instruction_ref: string;
  instruction_sha256: string;
  workspace_mode: "readOnlyBase" | "isolatedWorktree" | "unavailableAfterRestart";
  provider_selector: string;
  output_roots: string[];
  artifact_paths: string[];
  artifacts?: Array<{ path: string; size: number; sha256: string }>;
  gate_refs: string[];
  capability_versions: Array<{ capability_id: string; version_sha256: string }>;
  session_id: string | null;
  workspace: string | null;
  base_commit: string | null;
  receipt_id: string | null;
  run_id: string | null;
  passed_gates: number;
  failed_gates: number;
};

export type MissionSnapshot = {
  mission: MissionLine;
  mission_sha256: string;
  mission_ref: string;
  policy_sha256: string;
  approval_expires_unix_ms: number;
  integration?: {
    selected_task_id: string | null;
    selected_receipt_id: string | null;
  } | null;
  tasks: MissionTaskLine[];
};

export type MissionFlightSignalKind = "person" | "stopped" | "landing";

export type MissionFlightSignalLine = {
  signal_id: string;
  mission_id: string;
  mission_sha256: string;
  kind: MissionFlightSignalKind;
  session_id: string | null;
};

export type MissionFlightSignalPage = {
  signals: MissionFlightSignalLine[];
  next_cursor: string | null;
  gap: boolean;
};

export type MissionWorkspace = {
  mission_id: string;
  task_id: string;
  workspace: string;
  base_commit: string;
};

export type IsolatedWorkspaceLine = {
  workspace_id: string;
  project: string;
  workspace: string;
  base_commit: string;
  state: "creating" | "ready" | "bound" | "preservedDirty" | "released";
  session_id: string | null;
};

export type IsolatedWorkspaceReleaseLine = {
  workspace_id: string;
  workspace: string;
  outcome: "removed" | "preservedDirty" | "alreadyRemoved";
};

export type MissionInstruction = {
  mission_id: string;
  task_id: string;
  session_id: string;
  instruction: string;
  instruction_sha256: string;
};

export type CapabilityLine = {
  project: string;
  capability_id: string;
  kind: "skill" | "gateRecipe" | "playbook";
  state: string;
  version_sha256: string;
  source_ref: string;
  source_receipt_id: string;
  verification_receipt_id: string | null;
  verification_gates: Array<{
    gate_id: string;
    definition_sha256: string;
    outcome: string;
    duration_ms: number;
  }>;
  active_version_sha256: string | null;
  approved_versions: string[];
};

export type WireError = {
  message: string;
  retryable: boolean;
  needs_the_operator: boolean;
};

export type RemoteConnection = {
  relay_origin: string | null;
  state: "disabled" | "connecting" | "online" | "offline";
  stage: "discovery" | "registration" | "connection" | "exchange" | null;
};

export type PairingInvitationLine = {
  pairing_url: string;
  expires_at_ms: number;
  pc_key_fingerprint: string;
};

export type PairingProposalLine = {
  proposal_id: string;
  name: string;
  platform: string;
  key_fingerprint: string;
  available_scopes: string[];
};

export type DeviceLine = {
  device_id: string;
  name: string;
  platform: string;
  key_fingerprint: string;
  scopes: string[];
  available_scopes: string[];
  roots: string[];
  providers: string[];
  paired_at_ms: number;
};

export type Request =
  | { ask: "hello"; with: { wire: number } }
  | { ask: "providerUpdates" }
  | { ask: "providerUpdateStatus" }
  | { ask: "providerUpdate"; with: { provider: string } }
  | { ask: "remoteConnection" }
  | { ask: "remoteConfigure"; with: { relay_origin: string | null } }
  | { ask: "pairingBegin" }
  | { ask: "pairingProposals" }
  | { ask: "pairingApprovalBegin"; with: { proposal_id: string; scopes: string[] } }
  | { ask: "pairingApprovalFinish"; with: { challenge_id: string; answer: string } }
  | { ask: "pairingDeny"; with: { proposal_id: string } }
  | { ask: "devices" }
  | { ask: "deviceRevoke"; with: { device_id: string } }
  | {
      ask: "deviceAuthorityBegin";
      with: { device_id: string; scopes: string[]; roots: string[]; providers: string[] };
    }
  | { ask: "deviceAuthorityFinish"; with: { challenge_id: string; answer: string } }
  | { ask: "integrationEnrollments" }
  | {
      ask: "integrationApprovalBegin";
      with: { pending_id: string; scopes: string[]; roots: string[] };
    }
  | { ask: "integrationApprovalFinish"; with: { challenge_id: string; answer: string } }
  | { ask: "integrationSelfApprove"; with: { pending_id: string; signature: string } }
  | { ask: "integrationEnrollmentDeny"; with: { pending_id: string } }
  | { ask: "integrations" }
  | { ask: "providerHelp"; with: { provider_id: string } }
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
  | { ask: "runtimeSharedOpenRequests" }
  | { ask: "runtimeSharedOpenConfirm"; with: { confirmation_id: string } }
  | {
      ask: "missionRegisterGate";
      with: { gate_id: string; program: string; arguments: string[]; timeout_ms: number };
    }
  | { ask: "missionListGates" }
  | { ask: "missionValidate"; with: { project: string; mission_ref: string } }
  | { ask: "missionList" }
  | { ask: "missionGet"; with: { mission_id: string } }
  | { ask: "missionFlightSignals"; with: { after: string | null } }
  | {
      ask: "missionFlightSignal";
      with: { signal_id: string; mission_id: string; mission_sha256: string; kind: "stopped" | "landing" };
    }
  | { ask: "missionFlightSignalClear"; with: { mission_id: string; mission_sha256: string } }
  | { ask: "missionStart"; with: { mission_id: string; mission_sha256: string } }
  | {
    ask: "missionSchedule";
    with: {
      schedule_id: string;
      replaces_schedule_id: string | null;
      mission_id: string;
      mission_sha256: string;
      due_unix_ms: number;
      providers: MissionScheduleProviderLine[];
    };
  }
  | {
    ask: "missionScheduleCancel";
    with: { mission_id: string; mission_sha256: string; schedule_id: string };
  }
  | { ask: "missionPrepareTask"; with: { mission_id: string; task_id: string } }
  | { ask: "missionPause"; with: { mission_id: string } }
  | { ask: "missionResumeSafe"; with: { mission_id: string } }
  | { ask: "missionCancel"; with: { mission_id: string } }
  | {
      ask: "missionBindSession";
      with: {
        mission_id: string;
        task_id: string;
        session_id: string;
        provider_runtime_id: string;
        native_session_id: string | null;
        workspace: string;
      };
    }
  | {
      ask: "missionSendTaskInstruction";
      with: { mission_id: string; task_id: string; instruction_sha256: string };
    }
  | { ask: "missionVerifyTask"; with: { mission_id: string; task_id: string } }
  | { ask: "missionRetryTask"; with: { mission_id: string; task_id: string } }
  | { ask: "missionCompleteIntegration"; with: { mission_id: string; task_id: string | null } }
  | { ask: "missionArchive"; with: { mission_id: string } }
  | { ask: "capabilityPropose"; with: { project: string; candidate_ref: string } }
  | { ask: "capabilityList" }
  | {
      ask: "capabilityVerify";
      with: { project: string; capability_id: string; version_sha256: string };
    }
  | {
      ask: "capabilityApprove";
      with: { project: string; capability_id: string; version_sha256: string };
    }
  | { ask: "capabilityReject"; with: { project: string; capability_id: string } }
  | { ask: "capabilityQuarantine"; with: { project: string; capability_id: string } }
  | {
      ask: "capabilityRollback";
      with: { project: string; capability_id: string; version_sha256: string };
    }
  | { ask: "capabilityArchive"; with: { project: string; capability_id: string } }
  | { ask: "workspaceIsolatePrepare"; with: { request_id: string; project: string } }
  | { ask: "workspaceIsolateList" }
  | {
      ask: "workspaceIsolateBind";
      with: { workspace_id: string; session_id: string; workspace: string };
    }
  | {
      ask: "workspaceIsolateRelease";
      with: {
        workspace_id: string | null;
        session_id: string | null;
        workspace: string;
      };
    }
  | { ask: "rename"; with: { session: string; label: string | null } };

export type Response =
  | {
      say: "welcome";
      with: {
        wire: number;
        providers: PrivateProviderLine[];
        device: { scopes: string[]; roots: string[]; providers: string[] } | null;
        push_public_key: string | null;
        // SHA-256 of the daemon's own executable: its generation. Compared with the installed Core
        // to prove the daemon that answered is this build. Absent only on daemons older than 2026-08-20.
        build_digest?: string | null;
      };
    }
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
  | { say: "remoteConnection"; with: RemoteConnection }
  | { say: "pairingInvitation"; with: PairingInvitationLine }
  | { say: "pairingProposals"; with: PairingProposalLine[] }
  | {
      say: "pairingApprovalChallenge";
      with: { challenge_id: string; prompt: string };
    }
  | { say: "devices"; with: DeviceLine[] }
  | { say: "deviceAuthorityChallenge"; with: { challenge_id: string; prompt: string } }
  | { say: "integrationEnrollments"; with: IntegrationEnrollmentLine[] }
  | {
      say: "integrationApprovalChallenge";
      with: { challenge_id: string; prompt: string };
    }
  | { say: "integrationApproved"; with: { integration_id: string } }
  | { say: "integrations"; with: IntegrationLine[] }
  | {
      say: "providerHelp";
      with: {
        provider_id: string;
        display_name: string;
        installation_state: string;
        version: string | null;
        why: string | null;
        sign_in: string | null;
        /// Additive (2026-08-29): absent from an older Core, which an older reader never asked for.
        sign_out?: string | null;
        diagnose: string | null;
        install: string | null;
      };
    }
  | { say: "runtimeForgetRequests"; with: RuntimeForgetLine[] }
  | { say: "runtimeKeyRotationRequests"; with: RuntimeKeyRotationLine[] }
  | { say: "runtimeSharedOpenRequests"; with: RuntimeSharedOpenLine[] }
  | { say: "missions"; with: MissionLine[] }
  | { say: "missionGates"; with: GateLine[] }
  | { say: "mission"; with: MissionSnapshot }
  | { say: "missionFlightSignals"; with: MissionFlightSignalPage }
  | { say: "missionFlightSignalRecorded"; with: { inserted: boolean } }
  | { say: "missionWorkspace"; with: MissionWorkspace }
  | { say: "isolatedWorkspace"; with: IsolatedWorkspaceLine }
  | { say: "isolatedWorkspaces"; with: IsolatedWorkspaceLine[] }
  | { say: "isolatedWorkspaceReleased"; with: IsolatedWorkspaceReleaseLine }
  | { say: "missionInstruction"; with: MissionInstruction }
  | { say: "capabilities"; with: CapabilityLine[] }
  | { say: "done" }
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
