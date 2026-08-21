"""Gate: Agent Tools is one local action, one narrow root, and a complete revocation.

This drives the built product against the real installed provider CLIs without starting a model turn. It uses
isolated Runtrol, Claude, and Codex homes, then proves all of the following through public product surfaces:

- `tools enable` creates exactly one root-bound Runtime integration and uses each provider CLI's official MCP
  registration commands.
- Every registration contains only this exact executable plus the `mcp` argument, with no environment, secret,
  prompt, or project authority copied into provider configuration.
- Modern discovery, legacy initialization, the seven-tool catalogue, provider inventory, and session inventory all
  work over raw UTF-8 stdio. No approval or deletion tool exists.
- The same globally registered server default-denies a process started outside the approved root.
- A pre-existing or externally replaced `runtrolTools` entry is never overwritten or removed, and a failed
  first enable rolls its new Runtime authority back completely.
- `tools disable` removes provider registrations, revokes Runtime authority, deletes both the public grant and the
  OS-protected identity envelope, and leaves tool calls default-denied.

Provider inventory and session inventory are read-only. No provider session is started, no prompt is sent, and no
model token or rate limit is consumed.

Hosted CI does not have the operator's installed provider CLIs. This gate and its selftest are therefore declared
local-only in `gateCoverage.py` and run in every operator preflight.

Usage::

    python -X utf8 tests/audit/agentToolsSmoke.py --selftest
    python -X utf8 tests/audit/agentToolsSmoke.py
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NAME = "runtrolTools"
TOOLS = [
    "runtrol_providers",
    "runtrol_models",
    "runtrol_sessions",
    "runtrol_start",
    "runtrol_send",
    "runtrol_next_event",
    "runtrol_stop",
]
SCOPES = [
    "provider.read",
    "model.read",
    "session.list",
    "session.output.read",
    "session.start",
    "session.input.write",
    "session.stop",
]
COMMAND_TIMEOUT_S = 120.0
MCP_TIMEOUT_S = 30.0
SLOT = re.compile(r"^[0-9a-f]{64}$")


class Failed(Exception):
    """One product assertion did not hold."""


def buildBinary() -> Path:
    """Build the exact executable registered and driven by this gate."""
    proc = subprocess.run(
        ["cargo", "build", "-p", "runtrol"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=COMMAND_TIMEOUT_S,
        check=False,
    )
    if proc.returncode != 0:
        raise Failed(f"cargo could not build runtrol: {(proc.stderr or proc.stdout).strip()}")
    name = "runtrol.exe" if sys.platform == "win32" else "runtrol"
    binary = ROOT / "target" / "debug" / name
    if not binary.is_file():
        raise Failed(f"cargo succeeded without producing {binary.relative_to(ROOT)}")
    return binary.resolve()


def run(
    environment: dict[str, str],
    words: list[str],
    *,
    cwd: Path = ROOT,
    succeeds: bool = True,
) -> str:
    """Run one bounded command and require the requested exit class."""
    try:
        proc = subprocess.run(
            words,
            cwd=cwd,
            env=environment,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=COMMAND_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        raise Failed(f"`{' '.join(words)}` did not answer in {COMMAND_TIMEOUT_S:.0f} s") from expired
    output = "\n".join(part.strip() for part in (proc.stdout, proc.stderr) if part and part.strip())
    if succeeds and proc.returncode != 0:
        raise Failed(f"`{' '.join(words)}` failed: {output}")
    if not succeeds and proc.returncode == 0:
        raise Failed(f"`{' '.join(words)}` succeeded when it had to refuse: {output}")
    return output


def isolatedEnvironment(scratch: Path, claudeHome: Path, codexHome: Path) -> tuple[dict[str, str], Path]:
    """Align private Runtrol home selection with the public Runtime locator on every platform."""
    environment = dict(os.environ)
    state = scratch / "state"
    state.mkdir()
    if sys.platform == "win32":
        environment["LOCALAPPDATA"] = str(state)
        home = state / "runtrol"
    elif sys.platform == "darwin":
        environment["HOME"] = str(state)
        home = state / "Library" / "Application Support" / "runtrol"
    else:
        environment["XDG_STATE_HOME"] = str(state)
        environment["HOME"] = str(state)
        home = state / "runtrol"
    environment["RUNTROL_HOME"] = str(home)
    environment["CLAUDE_CONFIG_DIR"] = str(claudeHome)
    environment["CODEX_HOME"] = str(codexHome)
    return environment, home


def mcpExchange(
    binary: Path,
    environment: dict[str, str],
    requests: list[dict[str, object]],
    *,
    cwd: Path,
) -> dict[int, dict[str, object]]:
    """Send exact UTF-8 JSON lines, close input, and index every answer by numeric id."""
    payload = "".join(json.dumps(request, separators=(",", ":")) + "\n" for request in requests)
    try:
        proc = subprocess.run(
            [str(binary), "mcp"],
            cwd=cwd,
            env=environment,
            input=payload,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="strict",
            timeout=MCP_TIMEOUT_S,
            check=False,
        )
    except subprocess.TimeoutExpired as expired:
        raise Failed(f"Agent Tools MCP did not answer and exit in {MCP_TIMEOUT_S:.0f} s") from expired
    if proc.returncode != 0:
        raise Failed(f"Agent Tools MCP failed: {(proc.stderr or proc.stdout).strip()}")
    answers: dict[int, dict[str, object]] = {}
    for line in proc.stdout.splitlines():
        try:
            answer = json.loads(line)
        except json.JSONDecodeError as error:
            raise Failed(f"MCP stdout was not one JSON object per line: {line!r}: {error}") from error
        if not isinstance(answer, dict) or not isinstance(answer.get("id"), int):
            raise Failed(f"MCP emitted an answer without a numeric id: {answer!r}")
        identifier = answer["id"]
        if identifier in answers:
            raise Failed(f"MCP answered id {identifier} more than once")
        answers[identifier] = answer
    expected = {request["id"] for request in requests if isinstance(request.get("id"), int)}
    if set(answers) != expected:
        raise Failed(f"MCP answered ids {sorted(answers)} instead of {sorted(expected)}")
    return answers


def resultOf(answers: dict[int, dict[str, object]], identifier: int) -> dict[str, object]:
    """Require one successful JSON-RPC result object."""
    answer = answers.get(identifier)
    if not isinstance(answer, dict) or answer.get("jsonrpc") != "2.0" or "error" in answer:
        raise Failed(f"MCP id {identifier} was not a successful JSON-RPC answer: {answer!r}")
    result = answer.get("result")
    if not isinstance(result, dict):
        raise Failed(f"MCP id {identifier} result was not an object: {result!r}")
    return result


def assertServerMeta(result: dict[str, object]) -> None:
    """Require the finalized modern server identity metadata."""
    meta = result.get("_meta")
    info = meta.get("io.modelcontextprotocol/serverInfo") if isinstance(meta, dict) else None
    if not isinstance(info, dict) or info.get("name") != "runtrol-agent-tools":
        raise Failed(f"modern server metadata is absent or wrong: {meta!r}")
    if not isinstance(info.get("version"), str) or not info["version"]:
        raise Failed(f"modern server metadata has no version: {info!r}")


def assertCatalogue(result: dict[str, object]) -> None:
    """Require the fixed seven-tool catalogue and the absence of privileged tools."""
    tools = result.get("tools")
    if not isinstance(tools, list):
        raise Failed(f"tools/list did not return a list: {tools!r}")
    names = [tool.get("name") for tool in tools if isinstance(tool, dict)]
    if names != TOOLS:
        raise Failed(f"Agent Tools catalogue is {names!r}, expected {TOOLS!r}")
    if any("approval" in name or "delete" in name for name in names if isinstance(name, str)):
        raise Failed(f"Agent Tools exposed an approval or deletion capability: {names!r}")
    if result.get("resultType") != "complete":
        raise Failed(f"tools/list is not a complete modern result: {result!r}")
    assertServerMeta(result)


def assertToolResult(result: dict[str, object], *, error: bool) -> None:
    """Require dual text and structured content with the exact error state."""
    if result.get("isError") is not error:
        raise Failed(f"tool result error state is {result.get('isError')!r}, expected {error}")
    if not isinstance(result.get("structuredContent"), dict):
        raise Failed(f"tool result has no structured content: {result!r}")
    content = result.get("content")
    if not isinstance(content, list) or not content or not isinstance(content[0], dict):
        raise Failed(f"tool result has no MCP text content: {content!r}")
    if content[0].get("type") != "text" or not isinstance(content[0].get("text"), str):
        raise Failed(f"tool result text content is malformed: {content!r}")
    assertServerMeta(result)


def assertProtocolJourney(answers: dict[int, dict[str, object]]) -> None:
    """Require modern discovery, legacy compatibility, catalogue, and two real Runtime reads."""
    discover = resultOf(answers, 1)
    if discover.get("supportedVersions") != ["2026-07-28"] or discover.get("resultType") != "complete":
        raise Failed(f"server/discover did not select the finalized modern revision: {discover!r}")
    assertServerMeta(discover)

    initialized = resultOf(answers, 2)
    if initialized.get("protocolVersion") != "2024-11-05":
        raise Failed(f"legacy initialize did not preserve its supported revision: {initialized!r}")
    info = initialized.get("serverInfo")
    if not isinstance(info, dict) or info.get("name") != "runtrol-agent-tools":
        raise Failed(f"legacy initialize has no stable serverInfo: {initialized!r}")

    assertCatalogue(resultOf(answers, 3))
    assertToolResult(resultOf(answers, 4), error=False)
    assertToolResult(resultOf(answers, 5), error=False)


def assertGrant(record: object, root: Path) -> None:
    """Require the public grant to contain only one root and the fixed narrow scopes."""
    if not isinstance(record, dict) or set(record) != {"schema", "root", "grant"}:
        raise Failed(f"grant record does not have its closed top-level shape: {record!r}")
    if record.get("schema") != 1 or Path(str(record.get("root"))).resolve() != root.resolve():
        raise Failed(f"grant record is not bound to the enabled root: {record!r}")
    grant = record.get("grant")
    expected = {"integrationId", "scopes", "roots", "keyGeneration", "grantGeneration"}
    if not isinstance(grant, dict) or set(grant) != expected:
        raise Failed(f"Runtime grant does not have its closed public shape: {grant!r}")
    if grant.get("scopes") != SCOPES:
        raise Failed(f"Runtime grant scopes are {grant.get('scopes')!r}, expected {SCOPES!r}")
    roots = grant.get("roots")
    if not isinstance(roots, list) or len(roots) != 1 or Path(str(roots[0])).resolve() != root.resolve():
        raise Failed(f"Runtime grant is not bound to exactly one project root: {roots!r}")


def assertCredentialSlot(home: Path, root: Path) -> None:
    """Require one bounded slot with one protected envelope and one closed public grant."""
    directory = home / "agent-tools"
    slots = list(directory.iterdir()) if directory.is_dir() else []
    if len(slots) != 1 or not slots[0].is_dir() or SLOT.fullmatch(slots[0].name) is None:
        raise Failed(f"Agent Tools credential area is not one digest-named directory: {slots!r}")
    children = {child.name for child in slots[0].iterdir()}
    if children != {"identity.vault", "grant.json"}:
        raise Failed(f"credential slot contains files beyond identity and grant: {sorted(children)!r}")
    identity = slots[0] / "identity.vault"
    if not 9 < identity.stat().st_size <= 64 * 1024:
        raise Failed(f"protected identity envelope has an unsafe size: {identity.stat().st_size}")
    grantPath = slots[0] / "grant.json"
    if grantPath.stat().st_size > 64 * 1024:
        raise Failed(f"public grant exceeds its 64 KiB bound: {grantPath.stat().st_size}")
    assertGrant(json.loads(grantPath.read_text(encoding="utf-8")), root)


def providerRegistration(provider: str, claudeHome: Path, codexHome: Path) -> dict[str, object] | None:
    """Read the isolated provider configuration as structured data, never by interpreting prose."""
    if provider == "claude":
        config = claudeHome / ".claude.json"
        if not config.is_file():
            return None
        data = json.loads(config.read_text(encoding="utf-8"))
        servers = data.get("mcpServers") if isinstance(data, dict) else None
    else:
        config = codexHome / "config.toml"
        if not config.is_file():
            return None
        data = tomllib.loads(config.read_text(encoding="utf-8"))
        servers = data.get("mcp_servers") if isinstance(data, dict) else None
    if not isinstance(servers, dict):
        return None
    entry = servers.get(NAME)
    return entry if isinstance(entry, dict) else None


def assertRegistration(entry: object, binary: Path, provider: str) -> None:
    """Require an exact local stdio command with no ambient authority copied into the provider."""
    if not isinstance(entry, dict):
        raise Failed(f"{provider} has no structured {NAME} registration: {entry!r}")
    allowed = {"type", "command", "args", "env"} if provider == "claude" else {"command", "args"}
    if set(entry) - allowed:
        raise Failed(f"{provider} registration carries unexpected fields: {sorted(set(entry) - allowed)!r}")
    command = entry.get("command")
    if not isinstance(command, str) or Path(command).resolve() != binary.resolve():
        raise Failed(f"{provider} registration names {command!r}, expected {str(binary)!r}")
    if entry.get("args") != ["mcp"]:
        raise Failed(f"{provider} registration arguments are not exactly ['mcp']: {entry.get('args')!r}")
    if entry.get("env") not in (None, {}):
        raise Failed(f"{provider} registration copied environment authority: {entry.get('env')!r}")


def providerGetWords(provider: str, executable: str) -> list[str]:
    """The provider's own official readback command."""
    suffix = ["mcp", "get", NAME]
    if provider == "codex":
        suffix.append("--json")
    return [executable, *suffix]


def providerAddWords(
    provider: str, executable: str, command: str, args: list[str]
) -> list[str]:
    """The provider's own official global or user-scoped registration command."""
    if provider == "claude":
        prefix = [executable, "mcp", "add", "--scope", "user", NAME, "--"]
    else:
        prefix = [executable, "mcp", "add", NAME, "--"]
    return [*prefix, command, *args]


def providerRemoveWords(provider: str, executable: str) -> list[str]:
    """The provider's own official removal command for this gate's isolated home."""
    if provider == "claude":
        return [executable, "mcp", "remove", "--scope", "user", NAME]
    return [executable, "mcp", "remove", NAME]


def assertNoCredentials(home: Path) -> None:
    """Require no root-bound Agent Tools slot to remain."""
    directory = home / "agent-tools"
    if directory.is_dir() and any(directory.iterdir()):
        raise Failed(f"Agent Tools credentials remain: {list(directory.iterdir())!r}")


def assertForeignRegistration(
    provider: str,
    command: str,
    args: list[str],
    claudeHome: Path,
    codexHome: Path,
) -> None:
    """Require the exact colliding entry to remain untouched."""
    entry = providerRegistration(provider, claudeHome, codexHome)
    if not isinstance(entry, dict):
        raise Failed(f"{provider} colliding {NAME} entry disappeared: {entry!r}")
    if entry.get("command") != command or entry.get("args") != args:
        raise Failed(f"{provider} colliding {NAME} entry was changed: {entry!r}")


def assertProviderRegistrations(
    providers: dict[str, str],
    environment: dict[str, str],
    binary: Path,
    claudeHome: Path,
    codexHome: Path,
) -> None:
    """Require every installed registrar to read back the exact entry it wrote."""
    for provider, executable in providers.items():
        run(environment, providerGetWords(provider, executable))
        assertRegistration(providerRegistration(provider, claudeHome, codexHome), binary, provider)


def assertProviderRegistrationsGone(
    providers: dict[str, str],
    environment: dict[str, str],
    claudeHome: Path,
    codexHome: Path,
) -> None:
    """Require each provider's own readback to refuse and its structured configuration to be empty."""
    for provider, executable in providers.items():
        run(environment, providerGetWords(provider, executable), succeeds=False)
        if providerRegistration(provider, claudeHome, codexHome) is not None:
            raise Failed(f"{provider} still contains {NAME} after disable")


def assertCollisionRefused(
    binary: Path,
    providers: dict[str, str],
    environment: dict[str, str],
    home: Path,
    claudeHome: Path,
    codexHome: Path,
) -> None:
    """Plant a foreign name collision and prove first enable is atomic and default-deny."""
    provider = sorted(providers)[-1]
    executable = providers[provider]
    foreignCommand = sys.executable
    foreignArgs = ["-c", "pass"]
    run(
        environment,
        providerAddWords(provider, executable, foreignCommand, foreignArgs),
    )
    refused = run(
        environment,
        [str(binary), "tools", "enable", str(ROOT)],
        succeeds=False,
    )
    if "will not overwrite" not in refused:
        raise Failed(f"name collision was not refused by ownership: {refused!r}")
    assertForeignRegistration(provider, foreignCommand, foreignArgs, claudeHome, codexHome)
    for other in set(providers) - {provider}:
        if providerRegistration(other, claudeHome, codexHome) is not None:
            raise Failed(f"{other} was partially registered before collision refusal")
    assertNoCredentials(home)
    if run(environment, [str(binary), "tools", "list"]) != "no projects enabled":
        raise Failed("a refused first enable retained project authority")
    run(environment, providerRemoveWords(provider, executable))
    print("  collision: foreign entry preserved and new Runtime authority rolled back")


def protocolRequests() -> list[dict[str, object]]:
    """The cost-free dual-era discovery and real read-only Runtime journey."""
    return [
        {"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {}},
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "runtrol-gate", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}},
        {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "runtrol_providers", "arguments": {}},
        },
        {
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {"name": "runtrol_sessions", "arguments": {}},
        },
    ]


def deniedRequest() -> list[dict[str, object]]:
    """One tool call that requires a project credential and must return a tool-level error."""
    return [
        {
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {"name": "runtrol_providers", "arguments": {}},
        }
    ]


def assertDenied(answers: dict[int, dict[str, object]]) -> None:
    """Require default deny to be an MCP tool error rather than a process or protocol failure."""
    result = resultOf(answers, 9)
    assertToolResult(result, error=True)
    structured = result["structuredContent"]
    error = structured.get("error") if isinstance(structured, dict) else None
    if not isinstance(error, str) or "not enabled" not in error:
        raise Failed(f"default deny did not explain the missing project authority: {error!r}")


def exercise(
    binary: Path,
    providers: dict[str, str],
    environment: dict[str, str],
    home: Path,
    claudeHome: Path,
    codexHome: Path,
    outside: Path,
) -> None:
    """Drive enable, bounded use, root isolation, disable, and post-revocation denial."""
    enabled = run(environment, [str(binary), "tools", "enable", str(ROOT)])
    if "Agent Tools enabled" not in enabled or "approvals still require a person" not in enabled:
        raise Failed(f"enable did not state its authority boundary: {enabled!r}")
    run(environment, [str(binary), "tools", "status"], cwd=ROOT)
    listed = run(environment, [str(binary), "tools", "list"], cwd=ROOT)
    prefix = "enabled  "
    if not listed.startswith(prefix) or "\n" in listed or Path(listed[len(prefix):]).resolve() != ROOT.resolve():
        raise Failed(f"tools list did not expose exactly the enabled root: {listed!r}")
    assertCredentialSlot(home, ROOT)
    assertProviderRegistrations(providers, environment, binary, claudeHome, codexHome)
    print("  enable: one root-bound grant and exact official provider registrations")

    answers = mcpExchange(binary, environment, protocolRequests(), cwd=ROOT)
    assertProtocolJourney(answers)
    print("  MCP: modern and legacy discovery plus two real read-only Runtime calls")

    assertDenied(mcpExchange(binary, environment, deniedRequest(), cwd=outside))
    print("  root boundary: the globally registered server default-denied an unapproved working directory")

    disabled = run(environment, [str(binary), "tools", "disable", str(ROOT)])
    if "disabled and Runtime authority revoked" not in disabled:
        raise Failed(f"disable did not state complete authority removal: {disabled!r}")
    assertProviderRegistrationsGone(providers, environment, claudeHome, codexHome)
    assertNoCredentials(home)
    run(environment, [str(binary), "tools", "status"], cwd=ROOT, succeeds=False)
    if run(environment, [str(binary), "tools", "list"], cwd=ROOT) != "no projects enabled":
        raise Failed("tools list retained project authority after disable")
    assertDenied(mcpExchange(binary, environment, deniedRequest(), cwd=ROOT))
    print("  disable: registrations gone, credential slot gone, Runtime tools default-denied")

    again = run(environment, [str(binary), "tools", "disable", str(ROOT)])
    if "already disabled" not in again:
        raise Failed(f"idempotent disable did not report the settled state: {again!r}")


def exerciseReplacementSafety(
    binary: Path,
    providers: dict[str, str],
    environment: dict[str, str],
    home: Path,
    claudeHome: Path,
    codexHome: Path,
) -> None:
    """Replace one owned entry and prove disable revokes authority without deleting any entry."""
    run(environment, [str(binary), "tools", "enable", str(ROOT)])
    assertProviderRegistrations(providers, environment, binary, claudeHome, codexHome)

    provider = sorted(providers)[-1]
    executable = providers[provider]
    foreignCommand = sys.executable
    foreignArgs = ["-c", "pass"]
    run(environment, providerRemoveWords(provider, executable))
    run(
        environment,
        providerAddWords(provider, executable, foreignCommand, foreignArgs),
    )

    disabled = run(environment, [str(binary), "tools", "disable", str(ROOT)])
    if "disabled and Runtime authority revoked" not in disabled or "left it untouched" not in disabled:
        raise Failed(f"disable did not separate revocation from a foreign entry: {disabled!r}")
    assertForeignRegistration(provider, foreignCommand, foreignArgs, claudeHome, codexHome)
    for other in set(providers) - {provider}:
        assertRegistration(
            providerRegistration(other, claudeHome, codexHome), binary, other
        )
    assertNoCredentials(home)
    if run(environment, [str(binary), "tools", "list"]) != "no projects enabled":
        raise Failed("disable retained project authority after replacement refusal")
    assertDenied(mcpExchange(binary, environment, deniedRequest(), cwd=ROOT))

    for name, providerExecutable in providers.items():
        if providerRegistration(name, claudeHome, codexHome) is not None:
            run(environment, providerRemoveWords(name, providerExecutable))
    assertProviderRegistrationsGone(providers, environment, claudeHome, codexHome)
    print("  replacement: foreign entry preserved while Runtime authority was fully revoked")


def main(argv: list[str]) -> int:
    """Run the product journey, or prove the gate's own assertions with mutations."""
    if "--selftest" in argv:
        return selftest()
    if shutil.which("cargo") is None:
        print("[agentToolsSmoke] SKIP: cargo is unavailable, so the product cannot be built.")
        return 0
    providers = {
        name: executable
        for name in ("claude", "codex")
        if (executable := shutil.which(name)) is not None
    }
    if not providers:
        print(
            "[agentToolsSmoke] SKIP: no supported provider CLI is installed. Agent Tools is unverified here."
        )
        return 0

    scratch = Path(tempfile.mkdtemp(prefix="runtrolAgentTools"))
    claudeHome = scratch / "claude"
    codexHome = scratch / "codex"
    outside = scratch / "outside"
    for directory in (claudeHome, codexHome, outside):
        directory.mkdir()
    environment, home = isolatedEnvironment(scratch, claudeHome, codexHome)
    binary: Path | None = None
    outcome = 0
    daemonStopped = True
    print(f"[agentToolsSmoke] driving {', '.join(sorted(providers))} under {scratch}")
    try:
        binary = buildBinary()
        assertCollisionRefused(binary, providers, environment, home, claudeHome, codexHome)
        exercise(binary, providers, environment, home, claudeHome, codexHome, outside)
        exerciseReplacementSafety(
            binary, providers, environment, home, claudeHome, codexHome
        )
    except Failed as failure:
        print(f"[agentToolsSmoke] product journey failed: {failure}", file=sys.stderr)
        crash = home / "daemon-crash.log"
        if crash.is_file():
            print(crash.read_text(encoding="utf-8", errors="replace")[-4096:], file=sys.stderr)
        outcome = 2
    finally:
        if binary is not None:
            try:
                run(environment, [str(binary), "panic"])
            except Failed as stopping:
                print(f"[agentToolsSmoke] could not stop its isolated daemon: {stopping}", file=sys.stderr)
                daemonStopped = False
                outcome = 2
        if daemonStopped:
            shutil.rmtree(scratch, ignore_errors=True)

    if outcome != 0:
        if not daemonStopped:
            print(f"[agentToolsSmoke] retained isolated state for exact-PID recovery: {scratch}", file=sys.stderr)
        return outcome

    print(
        "[agentToolsSmoke] OK. one action enabled seven bounded tools, one root stayed isolated, "
        "and collision or replacement never granted ownership of another entry."
    )
    return 0


def selftest() -> int:
    """Inject failures into each high-value judgement before trusting the live journey."""
    problems: list[str] = []

    def refused(name: str, work) -> None:
        try:
            work()
        except Failed:
            return
        problems.append(f"{name} was accepted")

    meta = {
        "io.modelcontextprotocol/serverInfo": {"name": "runtrol-agent-tools", "version": "1"}
    }
    goodCatalogue = {
        "resultType": "complete",
        "tools": [{"name": name} for name in TOOLS],
        "_meta": meta,
    }
    refused(
        "a catalogue missing one tool",
        lambda: assertCatalogue({**goodCatalogue, "tools": goodCatalogue["tools"][:-1]}),
    )
    refused(
        "a catalogue with an approval tool",
        lambda: assertCatalogue(
            {**goodCatalogue, "tools": [{"name": name} for name in [*TOOLS[:-1], "runtrol_approval"]]}
        ),
    )
    refused(
        "modern output without server metadata",
        lambda: assertServerMeta({}),
    )
    refused(
        "a successful tool call marked as an error",
        lambda: assertToolResult(
            {
                "isError": True,
                "structuredContent": {},
                "content": [{"type": "text", "text": "{}"}],
                "_meta": meta,
            },
            error=False,
        ),
    )
    binary = Path.cwd() / ("runtrol.exe" if sys.platform == "win32" else "runtrol")
    refused(
        "a provider registration carrying an environment secret",
        lambda: assertRegistration(
            {"command": str(binary), "args": ["mcp"], "env": {"TOKEN": "secret"}},
            binary,
            "claude",
        ),
    )
    refused(
        "a provider registration naming another executable",
        lambda: assertRegistration(
            {"command": str(binary.with_name("other")), "args": ["mcp"]}, binary, "codex"
        ),
    )
    refused(
        "a provider registration carrying extra arguments",
        lambda: assertRegistration(
            {"command": str(binary), "args": ["mcp", "--root", "anywhere"]}, binary, "codex"
        ),
    )
    root = Path.cwd()
    grant = {
        "schema": 1,
        "root": str(root),
        "grant": {
            "integrationId": "int_test",
            "scopes": SCOPES,
            "roots": [str(root)],
            "keyGeneration": 0,
            "grantGeneration": 0,
        },
    }
    refused(
        "a grant with approval authority",
        lambda: assertGrant(
            {**grant, "grant": {**grant["grant"], "scopes": [*SCOPES, "approval.respond.low"]}}, root
        ),
    )
    refused(
        "a grant spanning a second root",
        lambda: assertGrant(
            {**grant, "grant": {**grant["grant"], "roots": [str(root), str(root.parent)]}}, root
        ),
    )
    refused(
        "a default-deny result without an error",
        lambda: assertDenied(
            {
                9: {
                    "jsonrpc": "2.0",
                    "id": 9,
                    "result": {
                        "isError": False,
                        "structuredContent": {},
                        "content": [{"type": "text", "text": "{}"}],
                        "_meta": meta,
                    },
                }
            }
        ),
    )

    try:
        assertCatalogue(goodCatalogue)
        assertRegistration({"command": str(binary), "args": ["mcp"]}, binary, "codex")
        assertGrant(grant, root)
    except Failed as wrong:
        problems.append(f"the valid fixture was refused: {wrong}")

    if problems:
        print("[agentToolsSmoke --selftest] the gate missed its own mutations.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print("[agentToolsSmoke --selftest] OK. 10 injected defects all caught.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
