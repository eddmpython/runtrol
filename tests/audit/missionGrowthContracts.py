"""Gate: Mission execution and project capability growth stay explicit, bounded, and locally approved."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED: dict[str, tuple[str, ...]] = {
    "crates/runtrol-orchestrator/src/validate.rs": (
        "approved_capabilities: &[CapabilitySelection]",
        "FindingCode::Capability",
        "!approved.contains(selection)",
    ),
    "crates/runtrol-orchestrator/src/scheduler.rs": (
        "SchedulerEffect::PrepareSession",
        "SchedulerEffect::PresentIntegration",
        "max_parallel_tasks",
    ),
    "crates/runtrol-daemon/src/mission.rs": (
        "std::fs::read(canonical.as_std_path())",
        "MissionInstruction",
        "restart-ambiguous",
        "run_workspace_preparation",
        "prepare_integration",
        "MissionState::Completed",
        "MISSION_START_APPROVAL_WINDOW",
        "the local Mission start approval expired; validate it again",
    ),
    "crates/runtrol-growth/src/lib.rs": (
        "CapabilityVerification",
        "active_version_sha256",
        "MAX_APPROVED_VERSIONS",
        "parent_version",
    ),
    "crates/runtrol-daemon/src/growth.rs": (
        "verification_receipt_id",
        "verify_provenance",
        "approved_capabilities",
        "active_intact",
        "CapabilityRollback",
    ),
    "crates/runtrol-daemon/src/scope.rs": (
        "LocalScope::MissionSendTaskInstruction",
        "LocalScope::CapabilityPromote",
        "mission_expansion_and_capability_changes_are_never_remote",
    ),
    "extensions/runtrol-vscode/src/mission/controller.ts": (
        "missionSendTaskInstruction",
        "this.sessions.submitResolvedInput",
        "showWarningMessage",
        "missionCompleteIntegration",
    ),
    "extensions/runtrol-vscode/src/capability/controller.ts": (
        "Approve exact digest",
        "Verification Receipt",
        "capabilityRollback",
    ),
}

FORBIDDEN_GROWTH_DEPENDENCIES = (
    "runtrol-daemon",
    "runtrol-drivers",
    "runtrol-ipc",
    "runtrol-ledger",
    "runtrol-orchestrator",
    "runtrol-transport",
)


def violations(sources: dict[str, str]) -> list[str]:
    """Return stable contract violations for already loaded source text."""
    found: list[str] = []
    for relative, tokens in REQUIRED.items():
        source = sources.get(relative, "")
        for token in tokens:
            if token not in source:
                found.append(f"{relative} is missing `{token}`")

    manifest = sources.get("crates/runtrol-growth/Cargo.toml", "")
    for dependency in FORBIDDEN_GROWTH_DEPENDENCIES:
        if f"{dependency} =" in manifest:
            found.append(f"runtrol-growth depends on forbidden `{dependency}`")

    mission_kernel = "\n".join(
        sources.get(path, "")
        for path in (
            "crates/runtrol-orchestrator/src/spec.rs",
            "crates/runtrol-orchestrator/src/validate.rs",
            "crates/runtrol-orchestrator/src/scheduler.rs",
        )
    )
    for token in ("Request::Prompt", "prepend", "semantic_search", "generated_prompt"):
        if token in mission_kernel:
            found.append(f"Mission kernel contains implicit input mechanism `{token}`")
    return found


def load_sources() -> dict[str, str]:
    paths = set(REQUIRED)
    paths.add("crates/runtrol-growth/Cargo.toml")
    paths.update(
        {
            "crates/runtrol-orchestrator/src/spec.rs",
            "crates/runtrol-orchestrator/src/scheduler.rs",
        }
    )
    return {
        relative: (ROOT / relative).read_text(encoding="utf-8")
        for relative in sorted(paths)
    }


def selftest() -> int:
    sources = {path: "\n".join(tokens) for path, tokens in REQUIRED.items()}
    sources["crates/runtrol-growth/Cargo.toml"] = "runtrol-provider = { workspace = true }"
    if violations(sources):
        print("[missionGrowthContracts:selftest] valid fixture was rejected", file=sys.stderr)
        return 2
    broken = dict(sources)
    broken["crates/runtrol-growth/Cargo.toml"] += "\nruntrol-daemon = { workspace = true }"
    if not violations(broken):
        print("[missionGrowthContracts:selftest] forbidden dependency was missed", file=sys.stderr)
        return 2
    broken = dict(sources)
    broken["crates/runtrol-daemon/src/growth.rs"] = ""
    if not violations(broken):
        print("[missionGrowthContracts:selftest] missing lifecycle was missed", file=sys.stderr)
        return 2
    print("[missionGrowthContracts:selftest] OK. dependency and lifecycle mutations are detected.")
    return 0


def main() -> int:
    found = violations(load_sources())
    if found:
        for problem in found:
            print(f"[missionGrowthContracts] {problem}", file=sys.stderr)
        return 2
    result = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "runtrol-ledger",
            "-p",
            "runtrol-orchestrator",
            "-p",
            "runtrol-growth",
            "-p",
            "runtrol-daemon",
        ],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        return result.returncode
    print(
        "[missionGrowthContracts] OK. exact Send, DAG scheduling, workspace isolation, restart, "
        "local scope, verification Receipt, explicit reuse, tamper, and rollback contracts passed."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest() if "--selftest" in sys.argv[1:] else main())
