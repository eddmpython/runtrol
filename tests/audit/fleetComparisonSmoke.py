"""Gate: two installed provider CLIs compete in isolated worktrees and one exact result wins.

The daemon, provider processes, Git worktrees, Mission scheduler, Receipts, Gates, and private framed IPC are real.
Provider requests terminate at the same loopback model fixtures used by the established live Mission gate.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import claudeApprovalSmoke as claude_gate
import externalAcpSmoke as external
import genericAcpSmoke as process
import missionLiveJourney as live


class Failed(Exception):
    """The fleet comparison journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Bounded product facts and cleanup facts."""

    tasks: int
    distinct_worktrees: int
    provider_requests: int
    receipts: int
    artifact_paths_visible: bool
    wrong_result_rejected: bool
    selected_result_completed: bool
    archived: bool
    cleanup_complete: bool


def verifyEvidence(evidence: Evidence) -> None:
    checks = (
        (evidence.tasks == 2, f"observed {evidence.tasks} Tasks, not two"),
        (evidence.distinct_worktrees == 2, "the attempts did not receive distinct linked worktrees"),
        (evidence.provider_requests >= 2, "both installed providers did not receive a request"),
        (evidence.receipts == 2, "both attempts did not seal passing Receipts"),
        (evidence.artifact_paths_visible, "the comparison surface did not receive sealed Artifact paths"),
        (evidence.wrong_result_rejected, "a non-applied passing result completed integration"),
        (evidence.selected_result_completed, "the applied selected result did not complete integration"),
        (evidence.archived, "the completed comparison Mission was not archived"),
        (evidence.cleanup_complete, "an owned provider session or daemon survived cleanup"),
    )
    for held, message in checks:
        if not held:
            raise Failed(message)


def selftest() -> int:
    valid = Evidence(2, 2, 2, 2, True, True, True, True, True)
    defects = (
        replace(valid, tasks=1),
        replace(valid, distinct_worktrees=1),
        replace(valid, provider_requests=1),
        replace(valid, receipts=1),
        replace(valid, artifact_paths_visible=False),
        replace(valid, wrong_result_rejected=False),
        replace(valid, selected_result_completed=False),
        replace(valid, archived=False),
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
        print(f"[fleetComparisonSmoke:selftest] FAIL: {error}", file=sys.stderr)
        return 2
    print(f"[fleetComparisonSmoke:selftest] OK. all {len(defects)} evidence mutations made the gate red.")
    return 0


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def writeProject(project: Path) -> None:
    instructions = project / "instructions"
    outputs = project / "outputs"
    instructions.mkdir()
    outputs.mkdir()
    instruction = instructions / "compare.md"
    instruction.write_text("Inspect the reviewed fixture and reply with exactly: done\n", encoding="utf-8")
    (outputs / "result.txt").write_text("base\n", encoding="utf-8")
    mission = f'''schema = "runtrol.dev/mission/v1alpha1"
name = "live fleet comparison"
project_id = "fleet-comparison-fixture"
base_ref = "main"
require_clean_base = true
completion_policy = "choose_one"

[limits]
max_parallel_tasks = 2
max_hot_providers = 2
max_runs_per_task = 1
max_repair_cycles = 0
stop_on_critical_failure = false

[[tasks]]
id = "attempt-one"
instruction_ref = "instructions/compare.md"
instruction_sha256 = "{sha256(instruction)}"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:claude"
output_roots = ["outputs/result.txt"]
gate_refs = ["fleet-live-check"]

[[tasks]]
id = "attempt-two"
instruction_ref = "instructions/compare.md"
instruction_sha256 = "{sha256(instruction)}"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:{external.PROVIDER}"
output_roots = ["outputs/result.txt"]
gate_refs = ["fleet-live-check"]
'''
    (project / "fleet.toml").write_text(mission, encoding="utf-8", newline="")


def exercise(claude: str, opencode: str) -> None:
    binary = external.buildBinary()
    with (
        tempfile.TemporaryDirectory(prefix="runtrol-fleet-live-") as raw_root,
        live.RunningClaudeModel() as claude_model,
        external.RunningModel() as external_model,
    ):
        root = Path(raw_root)
        home = root / "runtrol-home"
        project = root / "project"
        project.mkdir()
        live.run(["git", "init", "--initial-branch=main"], project)
        live.run(["git", "config", "user.email", "fixture@runtrol.invalid"], project)
        live.run(["git", "config", "user.name", "Runtrol Fixture"], project)
        writeProject(project)
        live.run(["git", "add", "--", "fleet.toml", "instructions", "outputs"], project)
        live.run(["git", "commit", "-m", "fixture"], project)
        env = live.configureEnvironment(root, home, claude, opencode, claude_model, external_model)
        daemon = process.startDaemon(binary, env, home)
        sessions: list[str] = []
        stage = "register Gate"
        try:
            live.response(
                live.ipc(
                    binary,
                    env,
                    {
                        "ask": "missionRegisterGate",
                        "with": {
                            "gate_id": "fleet-live-check",
                            "program": "git",
                            "arguments": ["diff", "--check", "HEAD"],
                            "timeout_ms": 30_000,
                        },
                    },
                ),
                "done",
            )
            validated = live.response(
                live.ipc(
                    binary,
                    env,
                    {"ask": "missionValidate", "with": {"project": str(project), "mission_ref": "fleet.toml"}},
                ),
                "mission",
            )
            if validated["mission"]["completion_policy"] != "chooseOne":
                raise Failed("the reviewed completion policy was not projected to the product surface")
            mission_id = validated["mission"]["mission_id"]
            snapshot = live.response(
                live.ipc(
                    binary,
                    env,
                    {
                        "ask": "missionStart",
                        "with": {"mission_id": mission_id, "mission_sha256": validated["mission_sha256"]},
                    },
                ),
                "mission",
            )
            providers = {"attempt-one": "claude", "attempt-two": external.PROVIDER}
            workspaces: dict[str, Path] = {}
            submitted: list[tuple[str, dict[str, Any]]] = []
            request_counts = {"attempt-one": claude_model.requests, "attempt-two": external_model.requests}
            models = {"attempt-one": claude_model, "attempt-two": external_model}

            for index, key in enumerate(("attempt-one", "attempt-two"), start=1):
                stage = f"prepare {key}"
                row = live.taskRows(snapshot)[key]
                if row["state"] != "reserved":
                    raise Failed(f"{key} was not reserved in parallel")
                workspace = live.response(
                    live.ipc(
                        binary,
                        env,
                        {
                            "ask": "missionPrepareTask",
                            "with": {"mission_id": mission_id, "task_id": row["task_id"]},
                        },
                    ),
                    "missionWorkspace",
                )
                worktree = Path(workspace["workspace"])
                workspaces[key] = worktree
                (worktree / "outputs" / "result.txt").write_text(f"attempt {index}\n", encoding="utf-8")
                provider = providers[key]
                session = process.command(binary, env, ["start", provider, str(worktree)])
                if process.SESSION_RE.fullmatch(session) is None:
                    raise Failed(f"provider {provider} returned no session identity")
                sessions.append(session)
                native = live.sessionLine(binary, env, session)["native"]
                snapshot = live.response(
                    live.ipc(
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
                                "workspace": str(worktree),
                            },
                        },
                    ),
                    "mission",
                )
                instruction = live.response(
                    live.ipc(
                        binary,
                        env,
                        {
                            "ask": "missionSendTaskInstruction",
                            "with": {
                                "mission_id": mission_id,
                                "task_id": row["task_id"],
                                "instruction_sha256": live.taskRows(snapshot)[key]["instruction_sha256"],
                            },
                        },
                    ),
                    "missionInstruction",
                )
                submitted.append((session, instruction))

            for session, instruction in submitted:
                process.command(binary, env, ["say", session, instruction["instruction"]])
            for session, _instruction in submitted:
                live.waitIdle(binary, env, session)

            for key in ("attempt-one", "attempt-two"):
                stage = f"verify {key}"
                row = live.taskRows(snapshot)[key]
                snapshot = live.response(
                    live.ipc(
                        binary,
                        env,
                        {"ask": "missionVerifyTask", "with": {"mission_id": mission_id, "task_id": row["task_id"]}},
                    ),
                    "mission",
                )
            rows = live.taskRows(snapshot)
            receipts = sum(1 for row in rows.values() if row["state"] == "passed" and row["receipt_id"])
            artifact_paths_visible = all(row["artifact_paths"] == ["outputs/result.txt"] for row in rows.values())
            provider_requests = sum(
                int(models[key].requests > request_counts[key]) for key in ("attempt-one", "attempt-two")
            )
            (project / "outputs" / "result.txt").write_text("attempt 2\n", encoding="utf-8")
            wrong_result_rejected = False
            try:
                live.ipc(
                    binary,
                    env,
                    {
                        "ask": "missionCompleteIntegration",
                        "with": {"mission_id": mission_id, "task_id": rows["attempt-one"]["task_id"]},
                    },
                )
            except live.Failed:
                wrong_result_rejected = True
            completed = live.response(
                live.ipc(
                    binary,
                    env,
                    {
                        "ask": "missionCompleteIntegration",
                        "with": {"mission_id": mission_id, "task_id": rows["attempt-two"]["task_id"]},
                    },
                ),
                "mission",
            )
            archived = live.response(
                live.ipc(binary, env, {"ask": "missionArchive", "with": {"mission_id": mission_id}}),
                "mission",
            )
            for session in list(sessions):
                process.command(binary, env, ["close", session, "--now"])
                sessions.remove(session)
            cleanup_complete = len(live.response(live.ipc(binary, env, {"ask": "list"}), "sessions")["sessions"]) == 0
            verifyEvidence(
                Evidence(
                    tasks=len(rows),
                    distinct_worktrees=len(set(workspaces.values())),
                    provider_requests=provider_requests,
                    receipts=receipts,
                    artifact_paths_visible=artifact_paths_visible,
                    wrong_result_rejected=wrong_result_rejected,
                    selected_result_completed=completed["mission"]["state"] == "completed",
                    archived=archived["mission"]["state"] == "archived",
                    cleanup_complete=cleanup_complete,
                )
            )
            print("[fleetComparisonSmoke] OK. two real CLIs competed and only the applied selected Receipt completed.")
        except Exception as error:
            stdout, stderr = process.stopDaemon(daemon)
            detail = (stderr or stdout or "the daemon exited without diagnostics").strip()
            raise Failed(f"{stage}: {error}; isolated daemon: {detail[-4000:]}") from error
        finally:
            for session in reversed(sessions):
                try:
                    process.command(binary, env, ["close", session, "--now"])
                except (process.Failed, subprocess.SubprocessError):
                    # ok: an attempt session may already be closed; the isolated daemon is stopped next.
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
        message = "[fleetComparisonSmoke] installed Claude Code and OpenCode CLIs are both required"
        print(message, file=sys.stderr if required else sys.stdout)
        return 2 if required else 0
    try:
        exercise(claude, opencode)
        return 0
    except (Failed, live.Failed, process.Failed, external.Failed, OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"[fleetComparisonSmoke] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
