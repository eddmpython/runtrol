"""Gate: public Runtime documentation stays complete and derived from shipped vocabulary."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PUBLIC_DOCS = (
    "runtimeProtocol.md",
    "runtimeIntegration.md",
    "runtimeSecurity.md",
    "runtimeOperations.md",
)


def vocabulary(source: str) -> set[str]:
    """Extract stable wire strings from one exhaustive Rust `as_str` match."""
    return set(re.findall(r'Self::[A-Za-z0-9_]+\s*=>\s*"([^"]+)"', source))


def documentationProblems(
    docs: dict[str, str],
    methods: set[str],
    scopes: set[str],
    errors: set[str],
    packageDocs: dict[str, str],
    providerArchitecture: str,
) -> list[str]:
    """Return missing public-contract and package-documentation details."""
    found: list[str] = []
    required = {
        "runtimeProtocol.md": (
            "2026-08-13",
            "runtime.locator.json",
            "32-bit big-endian",
            "UUIDv7",
            "Store rollback floor",
        ),
        "runtimeIntegration.md": (
            "runtimeNotInstalled",
            "Review Integration Requests",
            "outcomeUnknown",
            "presenceRequired",
            "Consumer private keys stay outside Runtime",
        ),
        "runtimeSecurity.md": (
            "same OS user",
            "never holds or forwards a model API key",
            "never scans, parses for meaning, rewrites, or stores a copy",
            "control lease",
            "Hosted companions",
        ),
        "runtimeOperations.md": (
            "Windows, macOS, and Linux",
            "Sigstore",
            "runtrol endpoint",
            "RuntrolRuntime",
            "runtrol panic",
            "runtimeNotInstalled",
        ),
    }
    for name, tokens in required.items():
        body = docs.get(name)
        if body is None:
            found.append(f"public Runtime document {name} is missing")
            continue
        if "mainPlan/" in body:
            found.append(f"{name} cites a provisional initiative")
        if "\u2014" in body or "\u2013" in body:
            found.append(f"{name} contains a forbidden dash character")
        for token in tokens:
            if token not in body:
                found.append(f"{name} is missing `{token}`")

    protocol = docs.get("runtimeProtocol.md", "")
    for label, values in (("method", methods), ("scope", scopes), ("error", errors)):
        for value in sorted(values):
            if value not in protocol:
                found.append(f"runtimeProtocol.md omits public {label} `{value}`")

    for name, body in packageDocs.items():
        for token in ("2026-08-13", "0.1.1"):
            if token not in body:
                found.append(f"{name} omits compatibility value `{token}`")
    if "official" not in providerArchitecture or "never scan" not in providerArchitecture:
        found.append("providerArchitecture.md does not freeze official catalogue and no-scan boundaries")
    return found


def selftest() -> int:
    """Prove missing docs, vocabulary, and boundary text make the gate red."""
    docs = {
        "runtimeProtocol.md": "2026-08-13 runtime.locator.json 32-bit big-endian UUIDv7 Store rollback floor method scope error",
        "runtimeIntegration.md": (
            "runtimeNotInstalled Review Integration Requests outcomeUnknown presenceRequired "
            "Consumer private keys stay outside Runtime"
        ),
        "runtimeSecurity.md": (
            "same OS user never holds or forwards a model API key never scans, parses for meaning, rewrites, or stores a copy "
            "control lease Hosted companions"
        ),
        "runtimeOperations.md": (
            "Windows, macOS, and Linux Sigstore runtrol endpoint RuntrolRuntime runtrol panic runtimeNotInstalled"
        ),
    }
    packages = {"README": "2026-08-13 0.1.1", "CHANGELOG": "2026-08-13 0.1.1"}
    arguments = (docs, {"method"}, {"scope"}, {"error"}, packages, "official catalogue never scan")
    if documentationProblems(*arguments):
        print("[runtimeDocumentation --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = (
        ({key: value for key, value in docs.items() if key != "runtimeSecurity.md"}, {"method"}, {"scope"}, {"error"}, packages, "official catalogue never scan"),
        (docs, {"missingMethod"}, {"scope"}, {"error"}, packages, "official catalogue never scan"),
        (docs, {"method"}, {"missingScope"}, {"error"}, packages, "official catalogue never scan"),
        (docs, {"method"}, {"scope"}, {"missingError"}, packages, "official catalogue never scan"),
        (docs, {"method"}, {"scope"}, {"error"}, {"README": "empty"}, "official catalogue never scan"),
        (docs, {"method"}, {"scope"}, {"error"}, packages, "provider files"),
    )
    for index, mutation in enumerate(mutations, start=1):
        if not documentationProblems(*mutation):
            print(f"[runtimeDocumentation --selftest] FAIL. mutation {index} escaped.", file=sys.stderr)
            return 2
    print(f"[runtimeDocumentation --selftest] OK. all {len(mutations)} defects make the gate red.")
    return 0


def run() -> int:
    """Compare public documentation with current Rust wire authorities."""
    docs = {
        name: (ROOT / "docs" / name).read_text(encoding="utf-8")
        for name in PUBLIC_DOCS
        if (ROOT / "docs" / name).is_file()
    }
    protocol = ROOT / "crates" / "runtrol-runtime-protocol"
    packageDocs = {
        path.relative_to(ROOT).as_posix(): path.read_text(encoding="utf-8")
        for path in (
            protocol / "README.md",
            protocol / "CHANGELOG.md",
            ROOT / "crates" / "runtrol-runtime-client" / "README.md",
            ROOT / "crates" / "runtrol-runtime-client" / "CHANGELOG.md",
            ROOT / "clients" / "typescript" / "README.md",
            ROOT / "clients" / "typescript" / "CHANGELOG.md",
        )
    }
    found = documentationProblems(
        docs,
        vocabulary((protocol / "src" / "method.rs").read_text(encoding="utf-8")),
        vocabulary((protocol / "src" / "integration.rs").read_text(encoding="utf-8")),
        vocabulary((protocol / "src" / "error.rs").read_text(encoding="utf-8")),
        packageDocs,
        (ROOT / "docs" / "providerArchitecture.md").read_text(encoding="utf-8"),
    )
    if found:
        print("[runtimeDocumentation] FAIL. public Runtime documentation drift:", file=sys.stderr)
        for problem in found:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[runtimeDocumentation] OK. protocol, integration, security, operations, and package docs match the wire.")
    return 0


def main() -> int:
    """Select selftest or the real gate."""
    if sys.argv[1:] == ["--selftest"]:
        return selftest()
    if sys.argv[1:]:
        print("usage: runtimeDocumentation.py [--selftest]", file=sys.stderr)
        return 1
    return run()


if __name__ == "__main__":
    raise SystemExit(main())
