"""Gate: an independently installed ACP CLI works through an external TOML manifest.

The external process is OpenCode's real ``opencode acp`` command. Its temporary workspace selects one custom
OpenAI-compatible provider whose only endpoint is a loopback server owned by this gate. The server discards request
bodies without parsing or retaining prompts and returns one fixed SSE response. No provider credential, model API,
or account is involved.

The journey crosses the product's manifest loader, binary discovery, generic ACP driver, daemon, command surface,
stream mapping, provider-declared completion, daemon restart, and native session load.

Usage::

    python -X utf8 tests/audit/externalAcpSmoke.py
    python -X utf8 tests/audit/externalAcpSmoke.py --require-external
    python -X utf8 tests/audit/externalAcpSmoke.py --selftest

Exit codes:
    0 the external journey completed, the selftest caught every defect, or an optional external CLI was absent
    2 the journey failed, or ``--require-external`` was used and the external CLI was absent
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import IO

import genericAcpSmoke as acp

ROOT = Path(__file__).resolve().parents[2]
PROVIDER = "external-acp"
EXTERNAL_NAMES = ("opencode", "opencode.exe", "opencode.cmd")
EXPECTED = "RUNTROL_EXTERNAL_ACP_FIXED_RESPONSE"
COMMAND_TIMEOUT_S = 180.0


class Failed(Exception):
    """The external ACP journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Facts observed from the independent process across the daemon restart."""

    external_requests: int
    expected_responses: int
    completed_turns: int
    native_before: str
    native_after: str


def verifyEvidence(evidence: Evidence) -> None:
    """Reject a journey that did not exercise the external process or preserve its session."""
    if evidence.external_requests < 2:
        raise Failed(
            f"the external ACP process made {evidence.external_requests} loopback model request(s), not two"
        )
    if evidence.expected_responses < 2:
        raise Failed(
            f"the fixed external model response reached {evidence.expected_responses} command watcher(s), not two"
        )
    if evidence.completed_turns < 2:
        raise Failed(f"the external ACP process declared {evidence.completed_turns} completed turn(s), not two")
    if evidence.native_after != evidence.native_before:
        raise Failed(
            f"the loaded native session changed from {evidence.native_before!r} to "
            f"{evidence.native_after!r}"
        )


def selftest() -> int:
    """Prove external execution, response, identity, and completion defects each make the gate red."""
    valid = Evidence(
        external_requests=2,
        expected_responses=2,
        completed_turns=2,
        native_before="external-session",
        native_after="external-session",
    )
    defects = {
        "external process was not executed": replace(valid, external_requests=0),
        "expected response was missing": replace(valid, expected_responses=1),
        "native session changed": replace(valid, native_after="different-session"),
        "provider completion was missing": replace(valid, completed_turns=1),
    }
    problems: list[str] = []
    try:
        verifyEvidence(valid)
    except Failed as error:
        problems.append(f"the valid journey was rejected: {error}")

    for name, evidence in defects.items():
        rejected = False
        try:
            verifyEvidence(evidence)
        except Failed:
            rejected = True
        if not rejected:
            problems.append(f"{name} was accepted")

    if problems:
        print("[externalAcpSmoke --selftest] the gate cannot detect what it claims to.", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2
    print(f"[externalAcpSmoke --selftest] OK. {len(defects)} injected defects all made the gate red.")
    return 0


class FixedModelServer(ThreadingHTTPServer):
    """A loopback-only deterministic endpoint that remembers only a request count."""

    daemon_threads = True
    block_on_close = False

    def __init__(self) -> None:
        super().__init__(("127.0.0.1", 0), FixedModelHandler)
        self.request_count = 0
        self.request_lock = threading.Lock()

    def observedRequest(self) -> int:
        """Count one request without retaining any part of it and return its sequence."""
        with self.request_lock:
            self.request_count += 1
            return self.request_count


class FixedModelHandler(BaseHTTPRequestHandler):
    """Discard an OpenAI-compatible request and emit one fixed streaming answer."""

    protocol_version = "HTTP/1.1"

    def do_POST(self) -> None:  # noqa: N802  (BaseHTTPRequestHandler owns this spelling)
        """Consume bytes without interpreting them, then return deterministic SSE."""
        self.connection.settimeout(10.0)
        length = self.headers.get("Content-Length")
        if length is None or not length.isdecimal():
            self.send_error(411)
            return
        remaining = int(length)
        while remaining > 0:
            chunk = self.rfile.read(min(remaining, 64 * 1024))
            if not chunk:
                self.send_error(400)
                return
            remaining -= len(chunk)

        server = self.server
        if not isinstance(server, FixedModelServer):
            self.send_error(500)
            return
        request_number = server.observedRequest()
        response_marker = f"{EXPECTED}_{request_number}"

        chunks = (
            {
                "id": "chatcmpl-runtrol-fixture",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "fixed",
                "choices": [
                    {
                        "index": 0,
                        "delta": {"role": "assistant", "content": response_marker},
                        "finish_reason": None,
                    }
                ],
            },
            {
                "id": "chatcmpl-runtrol-fixture",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "fixed",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        )
        body = b"".join(
            f"data: {json.dumps(chunk, separators=(',', ':'))}\n\n".encode() for chunk in chunks
        ) + b"data: [DONE]\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, _format: str, *args: object) -> None:
        """Keep HTTP diagnostics from ever including a request path or body."""


class RunningModel:
    """Own the local HTTP thread and stop it on every exit path."""

    def __init__(self) -> None:
        self.server = FixedModelServer()
        self.thread = threading.Thread(
            target=self.server.serve_forever,
            name="external-acp-model",
            daemon=True,
        )

    @property
    def base_url(self) -> str:
        """The OpenAI-compatible v1 endpoint in this process."""
        host, port = self.server.server_address
        return f"http://{host}:{port}/v1"

    @property
    def requests(self) -> int:
        """How many request bodies were discarded and answered."""
        with self.server.request_lock:
            return self.server.request_count

    def __enter__(self) -> RunningModel:
        self.thread.start()
        return self

    def __exit__(self, *_error: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5.0)
        if self.thread.is_alive():
            raise Failed("the loopback model server thread did not stop")


def externalProgram() -> str | None:
    """The first independent ACP executable the operating system resolves."""
    for name in EXTERNAL_NAMES:
        if shutil.which(name) is not None:
            return name
    return None


def buildBinary() -> Path:
    """Build only the product binary, never the in-repository ACP fixture."""
    built = subprocess.run(
        ["cargo", "build", "-p", "runtrol", "--bin", "runtrol"],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        timeout=COMMAND_TIMEOUT_S,
    )
    if built.returncode != 0:
        detail = (built.stderr or built.stdout or "cargo build failed without output").strip()
        raise Failed(detail)
    binary = ROOT / "target" / "debug" / ("runtrol.exe" if sys.platform == "win32" else "runtrol")
    if not binary.is_file():
        raise Failed(f"cargo succeeded but {binary.relative_to(ROOT)} is absent")
    return binary


def writeWorkspaceConfig(workspace: Path, base_url: str) -> Path:
    """Select one fixed custom model through OpenCode's project configuration."""
    config = {
        "$schema": "https://opencode.ai/config.json",
        "model": "fixture/fixed",
        "provider": {
            "fixture": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "runtrol local fixture",
                "options": {"baseURL": base_url, "apiKey": "local-fixture-not-a-secret"},
                "models": {"fixed": {"name": "Fixed deterministic model"}},
            }
        },
    }
    path = workspace / "opencode.json"
    path.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    return path


def writeManifest(home: Path, external: str) -> None:
    """Register OpenCode using only an operator-style external TOML file."""
    providers = home / "providers"
    providers.mkdir(parents=True)
    text = f'''schema = 1
id = "{PROVIDER}"
display_name = "External OpenCode ACP"
kind = "acp"

[bin]
names = [{json.dumps(external)}]

[probe]
version = {{ args = ["--version"], parse = "semver-anywhere" }}

[transport]
argv = ["acp"]
listen = "stdio"
'''
    (providers / f"{PROVIDER}.toml").write_text(text, encoding="utf-8")


def environment(home: Path, isolation: Path, config: Path) -> dict[str, str]:
    """Keep OpenCode state and configuration inside the gate without replacing the user's home."""
    result = dict(os.environ)
    result["RUNTROL_HOME"] = str(home)
    result["XDG_CONFIG_HOME"] = str(isolation / "config")
    result["XDG_DATA_HOME"] = str(isolation / "data")
    result["XDG_CACHE_HOME"] = str(isolation / "cache")
    result["XDG_STATE_HOME"] = str(isolation / "state")
    result["OPENCODE_CONFIG"] = str(config)
    result["OPENCODE_CONFIG_DIR"] = str(isolation / "config" / "opencode")
    result["OPENCODE_DISABLE_AUTOUPDATE"] = "true"
    result["OPENCODE_DISABLE_DEFAULT_PLUGINS"] = "true"
    result["OPENCODE_DISABLE_LSP_DOWNLOAD"] = "true"
    result["OPENCODE_DISABLE_CLAUDE_CODE"] = "true"
    result["OPENCODE_DISABLE_MODELS_FETCH"] = "true"
    result["OPENCODE_DISABLE_AUTOCOMPACT"] = "true"
    for name in (
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
    ):
        result.pop(name, None)
    return result


def nativeFor(listing: str, session: str) -> str:
    """Read the external provider's native identifier from one product listing row."""
    for line in listing.splitlines():
        fields = line.split()
        if fields and fields[0] == session:
            if len(fields) < 5:
                raise Failed(f"the session listing is missing its native identifier: {line!r}")
            return fields[4]
    raise Failed(f"session {session} is absent from the listing")


def completeTurn(
    binary: Path,
    env: dict[str, str],
    session: str,
    model: RunningModel,
) -> tuple[bool, bool, str]:
    """Drive one turn and report fixed response and provider completion separately."""
    watcher = subprocess.Popen(
        [str(binary), "watch", session],
        cwd=ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    # Both pipes are drained while the turn runs. An undrained pipe fills at the platform's buffer size
    # (measured: the resumed session's replayed history exceeds it on Windows), which blocks the watcher
    # mid-event and makes the terminal declaration vanish with the terminate below.
    watched_lines: list[str] = []
    turn_ended = threading.Event()

    def drain(stream: IO[str] | None) -> None:
        for line in stream or ():
            watched_lines.append(line)
            if '"step":"ended"' in line and '"stop":"endTurn"' in line:
                turn_ended.set()

    readers = [
        threading.Thread(target=drain, args=(watcher.stdout,), daemon=True),
        threading.Thread(target=drain, args=(watcher.stderr,), daemon=True),
    ]
    for reader in readers:
        reader.start()
    try:
        time.sleep(0.25)
        requests_before = model.requests
        acp.command(binary, env, ["say", session, "opaque external ACP gate prompt"])
        deadline = time.monotonic() + 60.0
        while time.monotonic() < deadline:
            row = next(
                (
                    line
                    for line in acp.command(binary, env, ["list"]).splitlines()
                    if line.startswith(session)
                ),
                "",
            )
            if "  idle  " in row:
                break
            time.sleep(0.1)
        else:
            raise Failed("the external ACP turn did not return to idle")

        # The list can say idle before the watch stream carries the provider's own declaration, so the
        # watcher is stopped only after that declaration arrives or its bounded wait names the absence.
        turn_ended.wait(timeout=10.0)
        watcher.terminate()
        watcher.wait(timeout=5.0)
        for reader in readers:
            reader.join(timeout=5.0)
        watched = "".join(watched_lines)
        requests_after = model.requests
        if requests_after <= requests_before:
            raise Failed(
                "the external turn made no new model request; the count changed "
                f"from {requests_before} to {requests_after}"
            )
        current_markers = (
            f"{EXPECTED}_{request_number}"
            for request_number in range(requests_before + 1, requests_after + 1)
        )
        diagnostic = next(
            (
                line
                for line in watched.splitlines()
                if "cannot serialize" in line or "protocol violation" in line
            ),
            "no protocol diagnostic was emitted",
        )
        completed = '"step":"ended"' in watched and '"stop":"endTurn"' in watched
        return any(marker in watched for marker in current_markers), completed, diagnostic
    finally:
        if watcher.poll() is None:
            watcher.terminate()
            try:
                watcher.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                watcher.kill()
                watcher.wait(timeout=2.0)


def restartDaemon(
    binary: Path,
    env: dict[str, str],
    home: Path,
    first: subprocess.Popen[str],
    session: str,
) -> subprocess.Popen[str]:
    """Close the external child, stop exactly one daemon, and start a distinct serving process."""
    acp.command(binary, env, ["close", session, "--now"])
    acp.stopDaemon(first)
    if first.poll() is None:
        raise Failed("the first daemon did not stop before restart")
    second = acp.startDaemon(binary, env, home)
    if second.poll() is not None:
        raise Failed("the restarted daemon exited before native load")
    return second


def exercise(external: str) -> None:
    """Run two external turns around a daemon restart and native session load."""
    binary = buildBinary()
    with tempfile.TemporaryDirectory(prefix="runtrol-external-acp-") as raw_root, RunningModel() as model:
        root = Path(raw_root)
        home = root / "runtrol-home"
        workspace = root / "workspace"
        isolation = root / "opencode"
        workspace.mkdir()
        isolation.mkdir()
        config = writeWorkspaceConfig(workspace, model.base_url)
        writeManifest(home, external)
        env = environment(home, isolation, config)

        first = acp.startDaemon(binary, env, home)
        second: subprocess.Popen[str] | None = None
        try:
            started = acp.command(binary, env, ["start", PROVIDER, str(workspace)])
            if acp.SESSION_RE.fullmatch(started) is None:
                raise Failed(f"session/new returned no runtrol session identifier: {started!r}")
            native_before = nativeFor(acp.command(binary, env, ["list"]), started)
            first_response, first_completion, first_diagnostic = completeTurn(
                binary, env, started, model
            )

            second = restartDaemon(binary, env, home, first, started)
            resumed = acp.command(binary, env, ["resume", PROVIDER, native_before, str(workspace)])
            if acp.SESSION_RE.fullmatch(resumed) is None or resumed == started:
                raise Failed(f"session/load returned no fresh runtrol session identifier: {resumed!r}")
            native_after = nativeFor(acp.command(binary, env, ["list"]), resumed)
            second_response, second_completion, second_diagnostic = completeTurn(
                binary, env, resumed, model
            )

            if not first_response or not second_response:
                raise Failed(
                    "the fixed response was absent; watcher diagnostics: "
                    f"{first_diagnostic}; {second_diagnostic}"
                )

            verifyEvidence(
                Evidence(
                    external_requests=model.requests,
                    expected_responses=int(first_response) + int(second_response),
                    completed_turns=int(first_completion) + int(second_completion),
                    native_before=native_before,
                    native_after=native_after,
                )
            )
            acp.command(binary, env, ["close", resumed, "--now"])
            print(
                "[externalAcpSmoke] OK. external OpenCode ACP completed a fixed streamed turn, "
                "survived daemon restart, loaded the same native session, and completed another turn."
            )
        finally:
            if first.poll() is None:
                acp.stopDaemon(first)
            if second is not None:
                acp.stopDaemon(second)


def main(argv: list[str]) -> int:
    """Run the selftest or the external ACP journey."""
    if "--selftest" in argv:
        return selftest()

    external = externalProgram()
    required = "--require-external" in argv
    if external is None:
        message = (
            "[externalAcpSmoke] external OpenCode ACP is not installed; "
            "install `opencode` or run without --require-external for an explicit optional skip."
        )
        print(message, file=sys.stderr if required else sys.stdout)
        return 2 if required else 0

    try:
        exercise(external)
        return 0
    except (Failed, acp.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[externalAcpSmoke] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
