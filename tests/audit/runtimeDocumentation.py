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
            "runtrol runtime-locator",
        ),
        "runtimeSecurity.md": (
            "same OS user",
            "never holds or forwards a model API key",
            "never parses for meaning, rewrites, or stores a conversation copy",
            "providerArchitecture.md#session-ownership",
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
        # Citing the initiative layer is asserted for every tracked file by `publicReferences.py`, so
        # repeating the rule here would be a second reading of it.
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
    # These are documentation anchors, not a proof of driver behavior. The driver contracts and
    # structural-read tests own that evidence; this gate keeps the public provenance boundary visible.
    for token in ("provider-owned source", "coverage", "bounded native-store metadata adapter"):
        if token not in providerArchitecture:
            found.append(f"providerArchitecture.md omits catalogue provenance boundary `{token}`")
    for token in ("source and coverage", "providerArchitecture.md#session-ownership"):
        if token not in protocol:
            found.append(f"runtimeProtocol.md omits catalogue provenance boundary `{token}`")
    if "Runtime never scans provider storage" in " ".join(protocol.split()):
        found.append("runtimeProtocol.md contradicts the permitted provider-owned metadata source")
    if "never scans, parses for meaning" in docs.get("runtimeSecurity.md", ""):
        found.append("runtimeSecurity.md contradicts the permitted provider-owned metadata source")
    return found


def selftest() -> int:
    """Prove missing docs, vocabulary, and boundary text make the gate red."""
    docs = {
        "runtimeProtocol.md": (
            "2026-08-13 runtime.locator.json 32-bit big-endian UUIDv7 Store rollback floor method scope error "
            "source and coverage providerArchitecture.md#session-ownership"
        ),
        "runtimeIntegration.md": (
            "runtimeNotInstalled Review Integration Requests outcomeUnknown presenceRequired "
            "Consumer private keys stay outside Runtime runtrol runtime-locator"
        ),
        "runtimeSecurity.md": (
            "same OS user never holds or forwards a model API key "
            "never parses for meaning, rewrites, or stores a conversation copy "
            "providerArchitecture.md#session-ownership control lease Hosted companions"
        ),
        "runtimeOperations.md": (
            "Windows, macOS, and Linux Sigstore runtrol endpoint RuntrolRuntime runtrol panic runtimeNotInstalled"
        ),
    }
    packages = {"README": "2026-08-13 0.1.1", "CHANGELOG": "2026-08-13 0.1.1"}
    catalogue = "provider-owned source coverage bounded native-store metadata adapter"
    arguments = (docs, {"method"}, {"scope"}, {"error"}, packages, catalogue)
    if documentationProblems(*arguments):
        print("[runtimeDocumentation --selftest] FAIL. the green fixture was rejected.", file=sys.stderr)
        return 2
    mutations = [
        ({key: value for key, value in docs.items() if key != "runtimeSecurity.md"}, {"method"}, {"scope"}, {"error"}, packages, catalogue),
        (docs, {"missingMethod"}, {"scope"}, {"error"}, packages, catalogue),
        (docs, {"method"}, {"missingScope"}, {"error"}, packages, catalogue),
        (docs, {"method"}, {"scope"}, {"missingError"}, packages, catalogue),
        (docs, {"method"}, {"scope"}, {"error"}, {"README": "empty"}, catalogue),
        (docs, {"method"}, {"scope"}, {"error"}, packages, "provider files"),
    ]
    for name, token in (
        ("runtimeSecurity.md", "never parses for meaning, rewrites, or stores a conversation copy"),
        ("runtimeSecurity.md", "providerArchitecture.md#session-ownership"),
        ("runtimeProtocol.md", "source and coverage"),
        ("runtimeProtocol.md", "providerArchitecture.md#session-ownership"),
    ):
        changed = {**docs, name: docs[name].replace(token, "")}
        mutations.append((changed, {"method"}, {"scope"}, {"error"}, packages, catalogue))
    for token in ("provider-owned source", "coverage", "bounded native-store metadata adapter"):
        mutations.append((docs, {"method"}, {"scope"}, {"error"}, packages, catalogue.replace(token, "")))
    for name, contradiction in (
        ("runtimeProtocol.md", "Runtime never scans provider storage"),
        ("runtimeSecurity.md", "Runtime never scans, parses for meaning"),
    ):
        changed = {**docs, name: f"{docs[name]} {contradiction}"}
        mutations.append((changed, {"method"}, {"scope"}, {"error"}, packages, catalogue))
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
