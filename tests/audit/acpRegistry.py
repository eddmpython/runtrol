"""Gate: official ACP Registry adapters stay local-only, generated, and bounded.

Usage::

    python -X utf8 tests/audit/acpRegistry.py --selftest
    python -X utf8 tests/audit/acpRegistry.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "crates" / "runtrol-drivers" / "tooling" / "sync-acp-registry.mjs"
GENERATED = ROOT / "crates" / "runtrol-drivers" / "src" / "generated_acp_registry.rs"
OFFICIAL = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"


def violations(generator: str, generated: str, runtime_sources: str) -> list[str]:
    """Return violations of the catalogue supply and runtime network boundary."""
    found: list[str] = []
    required_generator = (
        OFFICIAL,
        'const REGISTRY_SCHEMA = "1.0.0"',
        "MAX_REGISTRY_BYTES",
        "MAX_AGENTS",
        'redirect: "error"',
        "metadata.name !== coordinate.name",
        "metadata.version !== coordinate.version",
        "Object.keys(npx.env ?? {}).length > 0",
        "Object.keys(target.env ?? {}).length > 0",
        "writeFile(OUTPUT",
    )
    for token in required_generator:
        if token not in generator:
            found.append(f"registry synchronizer lost `{token}`")

    def count(name: str) -> int | None:
        match = re.search(rf"pub const {name}: usize = ([0-9]+);", generated)
        return int(match.group(1)) if match else None

    agents = count("ACP_REGISTRY_AGENT_COUNT")
    adapters = count("ACP_REGISTRY_ADAPTER_COUNT")
    replaced = count("ACP_REGISTRY_REPLACED_COUNT")
    skipped = count("ACP_REGISTRY_SKIPPED_COUNT")
    if agents is None or adapters is None or replaced is None or skipped is None:
        found.append("generated registry counts are absent")
    elif adapters < 20 or replaced < 1 or agents != adapters + skipped + replaced:
        found.append(
            "generated registry coverage is inconsistent: "
            f"agents={agents}, adapters={adapters}, replaced={replaced}, skipped={skipped}"
        )
    if generated.count('r#"') != adapters:
        found.append("generated manifest count differs from its declared adapter count")
    digest = re.search(r'ACP_REGISTRY_SHA256: &str =\s*"([0-9a-f]+)"', generated)
    if not digest or len(digest.group(1)) != 64:
        found.append("generated registry digest is not one SHA-256")
    for token in (
        'include_str!("../manifests/claude.toml")',
        'include_str!("../manifests/codex.toml")',
        'include_str!("../manifests/grok.toml")',
        'id = "glm-acp-agent"',
        'id = "qwen-code"',
        'id = "gemini"',
    ):
        if token not in generated:
            found.append(f"generated adapter set lost `{token}`")
    if 'names = ["npx"' in generated or 'names = ["uvx"' in generated:
        found.append("a generated provider can invoke a downloader instead of an installed CLI")
    for absent in ('id = "cline"', 'id = "opencode"'):
        if absent in generated:
            found.append(f"the generated adapter set carries a provider this product does not ship: {absent}")
    if 'id = "grok-build"' in generated:
        found.append("the official Grok launch duplicates the richer handwritten Grok provider")
    if "[update]" in generated:
        found.append("registry data claimed executable update authority")
    if OFFICIAL in runtime_sources or "registry.npmjs.org" in runtime_sources:
        found.append("product runtime can contact a package registry")
    return found


def product_sources() -> str:
    """Read product source while excluding the maintainer synchronizer and its generated snapshot."""
    files = [
        *list((ROOT / "crates").rglob("src/**/*.rs")),
        *list((ROOT / "extensions" / "runtrol-vscode" / "src").rglob("*.ts")),
    ]
    chunks = [
        file.read_text(encoding="utf-8")
        for file in files
        if file != GENERATED
    ]
    return "\n".join(chunks)


def selftest() -> int:
    """Prove each core defect makes the gate red."""
    generator = GENERATOR.read_text(encoding="utf-8")
    generated = GENERATED.read_text(encoding="utf-8")
    cases = [
        (generator.replace(OFFICIAL, "https://example.invalid/registry.json"), generated, ""),
        (generator.replace('redirect: "error"', 'redirect: "follow"'), generated, ""),
        (generator.replace("MAX_REGISTRY_BYTES", "UNBOUNDED_REGISTRY_BYTES"), generated, ""),
        (generator, generated.replace('names = ["glm-acp-agent"', 'names = ["npx"'), ""),
        (generator, generated.replace('id = "glm-acp-agent"', 'id = "removed-agent"'), ""),
        (generator, generated.replace('id = "goose"', 'id = "grok-build"', 1), ""),
        (generator, re.sub(r'(ACP_REGISTRY_SHA256: &str =\s*\")', r"\1bad", generated, count=1), ""),
        (generator, generated + "\n[update]\nhint = \"npm\"\n", ""),
        (generator, generated, OFFICIAL),
    ]
    failed = [index for index, case in enumerate(cases, 1) if not violations(*case)]
    if failed:
        print(f"[acpRegistry --selftest] defects not detected: {failed}", file=sys.stderr)
        return 1
    print(f"[acpRegistry --selftest] OK. {len(cases)} injected defects all made the gate red.")
    return 0


def main() -> int:
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: acpRegistry.py [--selftest]", file=sys.stderr)
        return 2
    found = violations(
        GENERATOR.read_text(encoding="utf-8"),
        GENERATED.read_text(encoding="utf-8"),
        product_sources(),
    )
    if found:
        for defect in found:
            print(f"[acpRegistry] FAIL: {defect}", file=sys.stderr)
        return 1
    print("[acpRegistry] OK. official catalogue is bounded, generated, local-only, and never auto-installs.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
