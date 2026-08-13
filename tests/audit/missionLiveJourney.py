"""Gate: two installed provider CLIs complete one reviewed Mission through production IPC.

The provider processes, Runtrol daemon, Git worktrees, Mission scheduler, evidence ledger, and Gate runner are real.
Each provider talks only to a loopback deterministic model endpoint that discards request bodies without retaining or
parsing them. The gate opens no graphical application and owns every process and temporary path it creates.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import claudeApprovalSmoke as claude_gate
import externalAcpSmoke as external
import genericAcpSmoke as process

ROOT = Path(__file__).resolve().parents[2]
IPC_HELPER = ROOT / "tests" / "audit" / "missionIpc.mjs"
TURN_TIMEOUT_S = 90.0


class Failed(Exception):
    """The real Mission journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Bounded facts from the two-provider Mission and cleanup."""

    providers: int
    tasks: int
    passed_tasks: int
    receipts: int
    task_submissions: int
    provider_requests: int
    capability_reused: bool
    tamper_detected: bool
    rollback_restored: bool
    completed: bool
    archived: bool
    project_artifacts_survived: bool
    cleanup_complete: bool


def verifyEvidence(evidence: Evidence) -> None:
    """Reject a journey that skipped a provider, Task, Receipt, integration, or cleanup boundary."""
    checks = (
        (evidence.providers == 2, f"observed {evidence.providers} provider kinds, not two"),
        (evidence.tasks == 5, f"observed {evidence.tasks} Tasks, not five"),
        (evidence.passed_tasks == 5, f"only {evidence.passed_tasks} Tasks passed"),
        (evidence.receipts == 5, f"observed {evidence.receipts} passing Receipts, not five"),
        (evidence.task_submissions == 5, f"observed {evidence.task_submissions} Task submissions, not five"),
        (evidence.provider_requests >= 5, f"observed only {evidence.provider_requests} provider requests"),
        (evidence.capability_reused, "a later Mission did not select the exact approved capability"),
        (evidence.tamper_detected, "changed active capability bytes remained selectable"),
        (evidence.rollback_restored, "rollback did not restore the prior approved capability bytes"),
        (evidence.completed, "integrated-tree verification did not complete the Mission"),
        (evidence.archived, "the completed Mission was not archived"),
        (evidence.project_artifacts_survived, "removing Runtrol metadata removed a project Artifact"),
        (evidence.cleanup_complete, "an owned daemon or provider session survived cleanup"),
    )
    for held, message in checks:
        if not held:
            raise Failed(message)


def selftest() -> int:
    """Prove each evidence field can independently turn the gate red."""
    valid = Evidence(2, 5, 5, 5, 5, 6, True, True, True, True, True, True, True)
    defects = (
        replace(valid, providers=1),
        replace(valid, tasks=2),
        replace(valid, passed_tasks=2),
        replace(valid, receipts=2),
        replace(valid, task_submissions=2),
        replace(valid, task_submissions=4),
        replace(valid, provider_requests=2),
        replace(valid, capability_reused=False),
        replace(valid, tamper_detected=False),
        replace(valid, rollback_restored=False),
        replace(valid, completed=False),
        replace(valid, archived=False),
        replace(valid, project_artifacts_survived=False),
        replace(valid, cleanup_complete=False),
    )
    try:
        verifyEvidence(valid)
        for defect in defects:
            try:
                verifyEvidence(defect)
            except Failed:
                # ok: rejection is the assertion, and the next independent mutation must also fail.
                continue
            raise Failed(f"injected defect escaped: {defect}")
    except Failed as error:
        print(f"[missionLiveJourney:selftest] FAIL: {error}", file=sys.stderr)
        return 2
    print(f"[missionLiveJourney:selftest] OK. all {len(defects)} evidence mutations made the gate red.")
    return 0


class ClaudeModelServer(ThreadingHTTPServer):
    """Loopback Anthropic-compatible endpoint that retains only a request count."""

    daemon_threads = True
    block_on_close = False

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), ClaudeModelHandler)
        self.request_count = 0
        self.lock = threading.Lock()
        self.sentinel_auth = True
        self.endpoint_contract = True

    def observed(self) -> None:
        with self.lock:
            self.request_count += 1


class ClaudeModelHandler(BaseHTTPRequestHandler):
    """Discard one provider request and return one fixed terminal stream."""

    protocol_version = "HTTP/1.1"

    def do_POST(self) -> None:  # noqa: N802
        server = self.server
        if not isinstance(server, ClaudeModelServer):
            self.send_error(500)
            return
        if self.headers.get("Authorization") != f"Bearer {claude_gate.TOKEN}":
            server.sentinel_auth = False
            self.send_error(401)
            return
        length = self.headers.get("Content-Length")
        if length is None or not length.isdecimal():
            self.send_error(411)
            return
        remaining = int(length)
        while remaining:
            chunk = self.rfile.read(min(remaining, 64 * 1024))
            if not chunk:
                self.send_error(400)
                return
            remaining -= len(chunk)
        if claude_gate.validCountTarget(self.path):
            body = b'{"input_tokens":1}'
            self._send("application/json", body)
            return
        if not claude_gate.validModelTarget(self.path):
            server.endpoint_contract = False
            self.send_error(404)
            return
        server.observed()
        body = b"".join(
            f"event: {kind}\r\ndata: {json.dumps(payload, separators=(',', ':'))}\r\n\r\n".encode()
            for kind, payload in claude_gate.completionEvents()
        )
        self._send("text/event-stream", body)

    def _send(self, content_type: str, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, _format: str, *args: object) -> None:
        """Keep HTTP diagnostics from exposing request metadata."""


class RunningClaudeModel:
    """Own and stop the loopback Claude endpoint."""

    def __init__(self) -> None:
        self.server = ClaudeModelServer()
        self.thread = threading.Thread(target=self.server.serve_forever, name="mission-claude-model", daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}"

    @property
    def requests(self) -> int:
        with self.server.lock:
            return self.server.request_count

    def __enter__(self) -> RunningClaudeModel:
        self.thread.start()
        return self

    def __exit__(self, *_error: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5.0)
        if self.thread.is_alive():
            raise Failed("the Claude loopback endpoint did not stop")


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> str:
    """Run one bounded setup command and return its standard output."""
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120.0,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "command failed without output").strip()
        raise Failed(detail[-4000:])
    return (result.stdout or "").strip()


def wireVersion() -> str:
    """Read the extension projection so the helper greets with the current product wire."""
    source = (ROOT / "extensions" / "runtrol-vscode" / "src" / "protocol.ts").read_text(encoding="utf-8")
    matched = re.search(r"export const WIRE_VERSION = (\d+);", source)
    if matched is None:
        raise Failed("the TypeScript wire version is unavailable")
    return matched.group(1)


def ipc(binary: Path, env: dict[str, str], request: dict[str, Any]) -> Any:
    """Send one private local request over the production framed endpoint."""
    endpoint = process.command(binary, env, ["endpoint"])
    node = shutil.which("node")
    if node is None:
        raise Failed("Node.js is required for the raw local IPC verifier")
    result = run(
        [node, str(IPC_HELPER), endpoint, wireVersion(), json.dumps(request, separators=(",", ":"))],
        ROOT,
        env,
    )
    response = json.loads(result)
    if response.get("say") == "failed":
        detail = response.get("with", {}).get("message", "the daemon refused the Mission request")
        raise Failed(str(detail))
    return response


def response(response: Any, kind: str) -> Any:
    """Require one exact tagged response."""
    if not isinstance(response, dict) or response.get("say") != kind:
        raise Failed(f"expected {kind}, received {response!r}")
    return response.get("with")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def writeProject(project: Path) -> None:
    """Create the frozen reviewed Mission and predeclared deterministic Artifacts."""
    instructions = project / "instructions"
    handoffs = project / ".runtrol" / "handoffs"
    candidate = project / ".runtrol" / "capabilities" / "candidates" / "reuse-v1"
    outputs = project / "outputs"
    instructions.mkdir(parents=True)
    handoffs.mkdir(parents=True)
    candidate.mkdir(parents=True)
    outputs.mkdir()
    bodies = {
        "investigate": "Inspect the frozen fixture and report completion without changing files.\n",
        "implement": "Inspect the frozen implementation fixture and report completion without changing files.\n",
        "review": "Review the frozen fixture and report completion without changing files.\n",
    }
    for name, body in bodies.items():
        (instructions / f"{name}.md").write_text(body, encoding="utf-8", newline="")
    (handoffs / "investigation.txt").write_text("reviewed investigation artifact\n", encoding="utf-8")
    (handoffs / "review.txt").write_text("reviewed independent review artifact\n", encoding="utf-8")
    (candidate / "SKILL.md").write_text("# Review procedure\n\nCheck fixed Gates before completion.\n", encoding="utf-8")
    (outputs / "implementation.txt").write_text("reviewed implementation artifact\n", encoding="utf-8")
    mission = f'''schema = "runtrol.dev/mission/v1alpha1"
name = "two provider live fixture"
project_id = "mission-live-fixture"
base_ref = "main"
require_clean_base = true

[limits]
max_parallel_tasks = 1
max_hot_providers = 1
max_runs_per_task = 2
max_repair_cycles = 1
stop_on_critical_failure = true

[[tasks]]
id = "investigate"
instruction_ref = "instructions/investigate.md"
instruction_sha256 = "{sha256(instructions / 'investigate.md')}"
workspace_mode = "read_only_base"
provider_selector = "runtime:claude"
output_roots = [".runtrol/handoffs/investigation.txt"]
gate_refs = ["mission-live-check"]

[[tasks]]
id = "implement"
depends_on = ["investigate"]
instruction_ref = "instructions/implement.md"
instruction_sha256 = "{sha256(instructions / 'implement.md')}"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:{external.PROVIDER}"
output_roots = ["outputs/implementation.txt", ".runtrol/capabilities/candidates/reuse-v1/SKILL.md"]
gate_refs = ["mission-live-check"]

[[tasks]]
id = "review"
depends_on = ["implement"]
instruction_ref = "instructions/review.md"
instruction_sha256 = "{sha256(instructions / 'review.md')}"
workspace_mode = "read_only_base"
provider_selector = "runtime:claude"
output_roots = [".runtrol/handoffs/review.txt"]
gate_refs = ["mission-live-check"]
'''
    (project / "mission.toml").write_text(mission, encoding="utf-8", newline="")


def treeDigest(root: Path, relative_files: tuple[str, ...]) -> str:
    """Match the capability crate's stable sorted tree digest."""
    digest = hashlib.sha256()
    for relative in sorted(relative_files):
        path = root / relative
        encoded = relative.encode("utf-8")
        body = path.read_bytes()
        digest.update(struct.pack(">Q", len(encoded)))
        digest.update(encoded)
        digest.update(hashlib.sha256(body).digest())
        digest.update(struct.pack(">Q", len(body)))
    return digest.hexdigest()


def writeCandidateMetadata(
    project: Path,
    candidate_ref: str,
    mission: dict[str, Any],
    author_key: str,
    verifier_key: str,
    parent_version: str | None,
) -> None:
    """Write closed provenance after both exact passing Runs are visible."""
    rows = taskRows(mission)
    author = rows[author_key]
    verifier = rows[verifier_key]
    required = (
        author.get("task_id"),
        author.get("run_id"),
        author.get("receipt_id"),
        verifier.get("run_id"),
    )
    if not all(required):
        raise Failed("capability provenance is incomplete in the Mission snapshot")
    candidate = project / candidate_ref
    payload_digest = treeDigest(candidate, ("SKILL.md",))
    parent = f'parent_version = "{parent_version}"\n' if parent_version else ""
    manifest = f'''schema = "runtrol.dev/capability/v1alpha1"
capability_id = "review-procedure"
kind = "skill"
scope = "project"
content_sha256 = "{payload_digest}"
source_mission_id = "{mission['mission']['mission_id']}"
source_task_id = "{author['task_id']}"
source_run_id = "{author['run_id']}"
source_receipt_id = "{author['receipt_id']}"
policy_sha256 = "{mission['policy_sha256']}"
{parent}license = "MIT"
'''
    verification = f'''schema = "runtrol.dev/capability-verification/v1alpha1"
author_run_id = "{author['run_id']}"
verifier_run_id = "{verifier['run_id']}"
replay_instruction_ref = "{verifier['instruction_ref']}"
fixture_ref = "outputs/implementation.txt"
gate_refs = ["mission-live-check"]
'''
    (candidate / "capability.toml").write_text(manifest, encoding="utf-8", newline="")
    (candidate / "verify.toml").write_text(verification, encoding="utf-8", newline="")


def capabilityLine(lines: list[dict[str, Any]], capability_id: str) -> dict[str, Any]:
    for line in lines:
        if line["capability_id"] == capability_id:
            return line
    raise Failed(f"capability {capability_id} is absent from the trust index")


def approveCandidate(binary: Path, env: dict[str, str], project: Path, candidate_ref: str) -> dict[str, Any]:
    """Propose, independently verify, and locally approve one exact candidate."""
    proposed = response(
        ipc(
            binary,
            env,
            {"ask": "capabilityPropose", "with": {"project": str(project), "candidate_ref": candidate_ref}},
        ),
        "capabilities",
    )
    candidate = capabilityLine(proposed, "review-procedure")
    identity = {
        "project": str(project),
        "capability_id": candidate["capability_id"],
        "version_sha256": candidate["version_sha256"],
    }
    verified = capabilityLine(
        response(ipc(binary, env, {"ask": "capabilityVerify", "with": identity}), "capabilities"),
        "review-procedure",
    )
    if verified["state"] != "verified" or not verified["verification_receipt_id"]:
        raise Failed("candidate verification produced no passing Receipt")
    approved = capabilityLine(
        response(ipc(binary, env, {"ask": "capabilityApprove", "with": identity}), "capabilities"),
        "review-procedure",
    )
    if approved["state"] != "active" or approved["active_version_sha256"] != candidate["version_sha256"]:
        raise Failed("exact capability approval did not activate its digest")
    return approved


def writeReuseMission(project: Path, approved_version: str) -> None:
    """Create a later Mission that explicitly selects v1 and produces v2."""
    candidate = project / ".runtrol" / "capabilities" / "candidates" / "reuse-v2"
    candidate.mkdir(parents=True)
    (candidate / "SKILL.md").write_text(
        "# Review procedure\n\nCheck fixed Gates and exact Artifacts before completion.\n",
        encoding="utf-8",
    )
    instructions = project / "instructions"
    (instructions / "reuse.md").write_text("Apply the explicitly selected project procedure.\n", encoding="utf-8")
    (instructions / "reuse-review.md").write_text("Independently verify the reused procedure.\n", encoding="utf-8")
    handoff = project / ".runtrol" / "handoffs" / "reuse-review.txt"
    handoff.write_text("reviewed capability reuse artifact\n", encoding="utf-8")
    mission = f'''schema = "runtrol.dev/mission/v1alpha1"
name = "explicit capability reuse fixture"
project_id = "mission-live-fixture"
base_ref = "main"
require_clean_base = true

[limits]
max_parallel_tasks = 1
max_hot_providers = 1
max_runs_per_task = 2
max_repair_cycles = 1
stop_on_critical_failure = true

[[tasks]]
id = "reuse"
instruction_ref = "instructions/reuse.md"
instruction_sha256 = "{sha256(instructions / 'reuse.md')}"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:claude"
output_roots = [".runtrol/capabilities/candidates/reuse-v2/SKILL.md"]
gate_refs = ["mission-live-check"]
capability_versions = [{{ capability_id = "review-procedure", version_sha256 = "{approved_version}" }}]

[[tasks]]
id = "reuse-review"
depends_on = ["reuse"]
instruction_ref = "instructions/reuse-review.md"
instruction_sha256 = "{sha256(instructions / 'reuse-review.md')}"
workspace_mode = "read_only_base"
provider_selector = "runtime:{external.PROVIDER}"
output_roots = [".runtrol/handoffs/reuse-review.txt"]
gate_refs = ["mission-live-check"]
'''
    (project / "mission-reuse.toml").write_text(mission, encoding="utf-8", newline="")


def configureEnvironment(
    root: Path,
    home: Path,
    claude: str,
    opencode: str,
    claude_model: RunningClaudeModel,
    external_model: external.RunningModel,
) -> dict[str, str]:
    """Combine both provider isolation contracts under one daemon environment."""
    claude_config = root / "claude"
    claude_config.mkdir()
    env = claude_gate.environment(root, home, claude_config, claude_model, claude)
    opencode_root = root / "opencode"
    opencode_root.mkdir()
    config = external.writeWorkspaceConfig(opencode_root, external_model.base_url)
    external.writeManifest(home, opencode)
    external_env = external.environment(home, opencode_root, config)
    for name, value in external_env.items():
        if name.startswith(("XDG_", "OPENCODE_")):
            env[name] = value
    for name in (
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
    ):
        env.pop(name, None)
    return env


def waitIdle(binary: Path, env: dict[str, str], session: str) -> None:
    """Wait for one exact provider turn to return to idle."""
    deadline = time.monotonic() + TURN_TIMEOUT_S
    while time.monotonic() < deadline:
        row = sessionLine(binary, env, session)
        if row["doing"] == "idle":
            return
        time.sleep(0.25)
    raise Failed(f"provider session {session} did not return to idle")


def sessionLine(binary: Path, env: dict[str, str], session: str) -> dict[str, Any]:
    """Read one session from the same private Core snapshot used by Studio."""
    listing = response(ipc(binary, env, {"ask": "list"}), "sessions")
    for row in listing["sessions"]:
        if row["session"] == session:
            return row
    raise Failed(f"provider session {session} is absent from the Core snapshot")


def taskRows(snapshot: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {str(task["key"]): task for task in snapshot["tasks"]}


def exercise(claude: str, opencode: str) -> None:
    """Run the production two-provider Mission without a graphical surface."""
    binary = external.buildBinary()
    with (
        tempfile.TemporaryDirectory(prefix="runtrol-mission-live-") as raw_root,
        RunningClaudeModel() as claude_model,
        external.RunningModel() as external_model,
    ):
        root = Path(raw_root)
        home = root / "runtrol-home"
        project = root / "project"
        project.mkdir()
        run(["git", "init", "--initial-branch=main"], project)
        run(["git", "config", "user.email", "fixture@runtrol.invalid"], project)
        run(["git", "config", "user.name", "Runtrol Fixture"], project)
        writeProject(project)
        run(["git", "add", "--", ".runtrol", "instructions", "outputs", "mission.toml"], project)
        run(["git", "commit", "-m", "fixture"], project)
        env = configureEnvironment(root, home, claude, opencode, claude_model, external_model)
        daemon = process.startDaemon(binary, env, home)
        sessions: list[str] = []
        snapshot: dict[str, Any] | None = None
        cleanup_complete = False
        task_submissions = 0
        stage = "register Gate"
        reusable_sessions: dict[str, str] = {}
        try:
            response(
                ipc(
                    binary,
                    env,
                    {
                        "ask": "missionRegisterGate",
                        "with": {
                            "gate_id": "mission-live-check",
                            "program": "git",
                            "arguments": ["diff", "--check", "HEAD"],
                            "timeout_ms": 30_000,
                        },
                    },
                ),
                "done",
            )
            validated = response(
                ipc(
                    binary,
                    env,
                    {"ask": "missionValidate", "with": {"project": str(project), "mission_ref": "mission.toml"}},
                ),
                "mission",
            )
            mission_id = validated["mission"]["mission_id"]
            stage = "start Mission"
            snapshot = response(
                ipc(
                    binary,
                    env,
                    {
                        "ask": "missionStart",
                        "with": {"mission_id": mission_id, "mission_sha256": validated["mission_sha256"]},
                    },
                ),
                "mission",
            )
            providers = {"investigate": "claude", "implement": external.PROVIDER, "review": "claude"}
            model_for = {
                "investigate": claude_model,
                "implement": external_model,
                "review": claude_model,
            }
            for key in ("investigate", "implement", "review"):
                stage = f"prepare Task {key}"
                row = taskRows(snapshot)[key]
                if row["state"] != "reserved":
                    raise Failed(f"Task {key} was not reserved after its dependencies passed")
                workspace = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionPrepareTask",
                            "with": {"mission_id": mission_id, "task_id": row["task_id"]},
                        },
                    ),
                    "missionWorkspace",
                )
                provider = providers[key]
                session = reusable_sessions.get(provider)
                if session is None:
                    stage = f"start provider for Task {key}"
                    session = process.command(binary, env, ["start", provider, workspace["workspace"]])
                    if process.SESSION_RE.fullmatch(session) is None:
                        raise Failed(f"provider {provider} returned no session identity")
                    sessions.append(session)
                native = sessionLine(binary, env, session)["native"]
                stage = f"bind Task {key}"
                snapshot = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionBindSession",
                            "with": {
                                "mission_id": mission_id,
                                "task_id": row["task_id"],
                                "session_id": session,
                                "provider_runtime_id": provider,
                                "native_session_id": native,
                                "workspace": workspace["workspace"],
                            },
                        },
                    ),
                    "mission",
                )
                instruction = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionSendTaskInstruction",
                            "with": {
                                "mission_id": mission_id,
                                "task_id": row["task_id"],
                                "instruction_sha256": taskRows(snapshot)[key]["instruction_sha256"],
                            },
                        },
                    ),
                    "missionInstruction",
                )
                expected = (project / taskRows(snapshot)[key]["instruction_ref"]).read_bytes().decode("utf-8")
                if instruction["instruction"] != expected or instruction["session_id"] != session:
                    raise Failed(f"Task {key} did not return its exact reviewed instruction and session")
                model = model_for[key]
                requests_before = model.requests
                stage = f"submit Task {key}"
                process.command(binary, env, ["say", session, instruction["instruction"]])
                task_submissions += 1
                stage = f"wait for Task {key}"
                waitIdle(binary, env, session)
                if model.requests <= requests_before:
                    raise Failed(f"Task {key} made no provider model request")
                stage = f"verify Task {key}"
                snapshot = response(
                    ipc(
                        binary,
                        env,
                        {"ask": "missionVerifyTask", "with": {"mission_id": mission_id, "task_id": row["task_id"]}},
                    ),
                    "mission",
                )
                passed = taskRows(snapshot)[key]
                if passed["state"] != "passed" or not passed["receipt_id"]:
                    raise Failed(f"Task {key} did not seal a passing Receipt")
                if key == "investigate":
                    reusable_sessions[provider] = session
                else:
                    process.command(binary, env, ["close", session, "--now"])
                    sessions.remove(session)
                    reusable_sessions.pop(provider, None)

            stage = "complete integration"
            completed = response(
                ipc(
                    binary,
                    env,
                    {"ask": "missionCompleteIntegration", "with": {"mission_id": mission_id}},
                ),
                "mission",
            )
            stage = "archive Mission"
            archived = response(
                ipc(binary, env, {"ask": "missionArchive", "with": {"mission_id": mission_id}}),
                "mission",
            )
            stage = "approve capability v1"
            writeCandidateMetadata(
                project,
                ".runtrol/capabilities/candidates/reuse-v1",
                archived,
                "implement",
                "review",
                None,
            )
            approved_v1 = approveCandidate(
                binary,
                env,
                project,
                ".runtrol/capabilities/candidates/reuse-v1",
            )
            version_v1 = approved_v1["version_sha256"]

            stage = "prepare capability reuse Mission"
            writeReuseMission(project, version_v1)
            run(
                [
                    "git",
                    "add",
                    "--",
                    ".runtrol/capabilities/active",
                    ".runtrol/capabilities/candidates/reuse-v1",
                    ".runtrol/capabilities/candidates/reuse-v2",
                    ".runtrol/handoffs/reuse-review.txt",
                    "instructions/reuse.md",
                    "instructions/reuse-review.md",
                    "mission-reuse.toml",
                ],
                project,
            )
            run(["git", "commit", "-m", "reuse fixture"], project)
            validated_reuse = response(
                ipc(
                    binary,
                    env,
                    {
                        "ask": "missionValidate",
                        "with": {"project": str(project), "mission_ref": "mission-reuse.toml"},
                    },
                ),
                "mission",
            )
            reuse_mission_id = validated_reuse["mission"]["mission_id"]
            snapshot_reuse = response(
                ipc(
                    binary,
                    env,
                    {
                        "ask": "missionStart",
                        "with": {
                            "mission_id": reuse_mission_id,
                            "mission_sha256": validated_reuse["mission_sha256"],
                        },
                    },
                ),
                "mission",
            )
            reuse_providers = {"reuse": "claude", "reuse-review": external.PROVIDER}
            reuse_models = {"reuse": claude_model, "reuse-review": external_model}
            for key in ("reuse", "reuse-review"):
                stage = f"prepare Task {key}"
                row = taskRows(snapshot_reuse)[key]
                if row["state"] != "reserved":
                    raise Failed(f"Task {key} was not reserved after its dependencies passed")
                workspace = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionPrepareTask",
                            "with": {"mission_id": reuse_mission_id, "task_id": row["task_id"]},
                        },
                    ),
                    "missionWorkspace",
                )
                provider = reuse_providers[key]
                stage = f"start provider for Task {key}"
                session = process.command(binary, env, ["start", provider, workspace["workspace"]])
                if process.SESSION_RE.fullmatch(session) is None:
                    raise Failed(f"provider {provider} returned no session identity")
                sessions.append(session)
                native = sessionLine(binary, env, session)["native"]
                snapshot_reuse = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionBindSession",
                            "with": {
                                "mission_id": reuse_mission_id,
                                "task_id": row["task_id"],
                                "session_id": session,
                                "provider_runtime_id": provider,
                                "native_session_id": native,
                                "workspace": workspace["workspace"],
                            },
                        },
                    ),
                    "mission",
                )
                instruction = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionSendTaskInstruction",
                            "with": {
                                "mission_id": reuse_mission_id,
                                "task_id": row["task_id"],
                                "instruction_sha256": taskRows(snapshot_reuse)[key]["instruction_sha256"],
                            },
                        },
                    ),
                    "missionInstruction",
                )
                expected = (project / taskRows(snapshot_reuse)[key]["instruction_ref"]).read_bytes().decode("utf-8")
                if instruction["instruction"] != expected or instruction["session_id"] != session:
                    raise Failed(
                        f"Task {key} local Send mismatch: "
                        f"instruction={instruction['instruction'] == expected}, "
                        f"session={instruction['session_id'] == session}"
                    )
                model = reuse_models[key]
                requests_before = model.requests
                stage = f"submit Task {key}"
                process.command(binary, env, ["say", session, instruction["instruction"]])
                task_submissions += 1
                stage = f"wait for Task {key}"
                waitIdle(binary, env, session)
                if model.requests <= requests_before:
                    raise Failed(f"Task {key} made no provider model request")
                stage = f"verify Task {key}"
                snapshot_reuse = response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "missionVerifyTask",
                            "with": {"mission_id": reuse_mission_id, "task_id": row["task_id"]},
                        },
                    ),
                    "mission",
                )
                passed = taskRows(snapshot_reuse)[key]
                if passed["state"] != "passed" or not passed["receipt_id"]:
                    raise Failed(f"Task {key} did not seal a passing Receipt")
                process.command(binary, env, ["close", session, "--now"])
                sessions.remove(session)

            completed_reuse = response(
                ipc(
                    binary,
                    env,
                    {"ask": "missionCompleteIntegration", "with": {"mission_id": reuse_mission_id}},
                ),
                "mission",
            )
            archived_reuse = response(
                ipc(binary, env, {"ask": "missionArchive", "with": {"mission_id": reuse_mission_id}}),
                "mission",
            )
            selected_versions = taskRows(archived_reuse)["reuse"]["capability_versions"]
            capability_reused = selected_versions == [
                {"capability_id": "review-procedure", "version_sha256": version_v1}
            ]

            stage = "approve capability v2"
            writeCandidateMetadata(
                project,
                ".runtrol/capabilities/candidates/reuse-v2",
                archived_reuse,
                "reuse",
                "reuse-review",
                version_v1,
            )
            approved_v2 = approveCandidate(
                binary,
                env,
                project,
                ".runtrol/capabilities/candidates/reuse-v2",
            )
            active_skill = project / ".runtrol" / "capabilities" / "active" / "review-procedure" / "SKILL.md"
            active_skill.write_text("tampered\n", encoding="utf-8")
            tampered = capabilityLine(
                response(ipc(binary, env, {"ask": "capabilityList"}), "capabilities"),
                "review-procedure",
            )
            tamper_detected = (
                tampered["state"] == "tampered"
                and tampered["active_version_sha256"] == approved_v2["version_sha256"]
            )
            rolled_back = capabilityLine(
                response(
                    ipc(
                        binary,
                        env,
                        {
                            "ask": "capabilityRollback",
                            "with": {
                                "project": str(project),
                                "capability_id": "review-procedure",
                                "version_sha256": version_v1,
                            },
                        },
                    ),
                    "capabilities",
                ),
                "review-procedure",
            )
            rollback_restored = (
                rolled_back["state"] == "rolledBack"
                and rolled_back["active_version_sha256"] == version_v1
                and active_skill.read_text(encoding="utf-8")
                == "# Review procedure\n\nCheck fixed Gates before completion.\n"
                and approved_v2["version_sha256"] != version_v1
            )
            process.stopDaemon(daemon)
            cleanup_complete = daemon.poll() is not None
            shutil.rmtree(home)
            artifacts_survived = all(
                path.is_file()
                for path in (
                    project / ".runtrol/handoffs/investigation.txt",
                    project / "outputs/implementation.txt",
                    project / ".runtrol/handoffs/review.txt",
                    project / "mission.toml",
                    project / "mission-reuse.toml",
                    active_skill,
                )
            )
            all_tasks = [*archived["tasks"], *archived_reuse["tasks"]]
            verifyEvidence(
                Evidence(
                    providers=len(set(providers.values())),
                    tasks=len(all_tasks),
                    passed_tasks=sum(task["state"] == "passed" for task in all_tasks),
                    receipts=sum(bool(task["receipt_id"]) for task in all_tasks),
                    task_submissions=task_submissions,
                    provider_requests=claude_model.requests + external_model.requests,
                    capability_reused=capability_reused,
                    tamper_detected=tamper_detected,
                    rollback_restored=rollback_restored,
                    completed=(
                        completed["mission"]["state"] == "completed"
                        and completed_reuse["mission"]["state"] == "completed"
                    ),
                    archived=(
                        archived["mission"]["state"] == "archived"
                        and archived_reuse["mission"]["state"] == "archived"
                    ),
                    project_artifacts_survived=artifacts_survived,
                    cleanup_complete=cleanup_complete,
                )
            )
            print(
                "[missionLiveJourney] OK. two installed provider CLIs completed five reviewed Tasks, "
                "explicit capability reuse, tamper detection, rollback, integration, archive, and exact cleanup."
            )
        except (Failed, process.Failed, OSError, subprocess.SubprocessError) as error:
            sessions.clear()
            if daemon.poll() is None:
                process.stopDaemon(daemon)
            stdout, stderr = daemon.communicate()
            detail = (stderr or stdout or "the daemon exited without diagnostics").strip()
            raise Failed(f"{stage}: {error}; isolated daemon: {detail[-4000:]}") from error
        finally:
            for session in reversed(sessions):
                try:
                    process.command(binary, env, ["close", session, "--now"])
                except (process.Failed, subprocess.SubprocessError):
                    # ok: the isolated daemon is stopped next, then TemporaryDirectory removes fixture state.
                    pass
            if daemon.poll() is None:
                process.stopDaemon(daemon)


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    claude = claude_gate.claudeProgram()
    opencode = external.externalProgram()
    required = "--require-external" in argv
    if claude is None or opencode is None:
        message = "[missionLiveJourney] installed Claude Code and OpenCode CLIs are both required"
        print(message, file=sys.stderr if required else sys.stdout)
        return 2 if required else 0
    try:
        exercise(claude, opencode)
        return 0
    except (Failed, process.Failed, external.Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[missionLiveJourney] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
