"""Source-level package contracts that do not need a running Runtime."""

from __future__ import annotations

from pathlib import Path

PACKAGE = Path(__file__).resolve().parents[1]


def test_native_binding_links_only_public_workspace_crates() -> None:
    manifest = (PACKAGE / "Cargo.toml").read_text(encoding="utf-8")
    workspace_edges = {
        line.split("=")[0].strip()
        for line in manifest.splitlines()
        if "workspace = true" in line and line.lstrip().startswith("runtrol-")
    }
    assert workspace_edges == {"runtrol-runtime-client", "runtrol-runtime-protocol"}


def test_binding_has_no_provider_or_installation_policy() -> None:
    source = "\n".join(path.read_text(encoding="utf-8") for path in (PACKAGE / "src").glob("*.rs"))
    for forbidden in ("claude", "codex", "grok", "npm install", "transcript"):
        assert forbidden not in source.lower()
