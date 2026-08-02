"""Gate: a real Claude Code process consumes a denied hidden stdio approval.

The provider CLI and runtrol's production ``claude-stream-json`` driver are real. Only the Anthropic Messages
endpoint is a loopback deterministic fixture. It discards request bodies without parsing or retaining prompts,
returns one harmless Write tool request, then ends the turn after Claude consumes runtrol's native denial.

Usage::

    python -X utf8 tests/audit/claudeApprovalSmoke.py
    python -X utf8 tests/audit/claudeApprovalSmoke.py --require-external
    python -X utf8 tests/audit/claudeApprovalSmoke.py --selftest

Exit codes:
    0 the live CLI journey completed, every injected defect was rejected, or an optional CLI was absent
    2 the journey failed, or ``--require-external`` was used and Claude Code was absent
"""

from __future__ import annotations

import json
import os
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, replace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

import genericAcpSmoke as process

ROOT = Path(__file__).resolve().parents[2]
PROVIDER = "claude"
MODEL = "runtrol-loopback-model"
TOKEN = "runtrol-loopback-sentinel-not-a-secret"
CLAUDE_NAMES = ("claude", "claude.exe", "claude.cmd")
COMMAND_TIMEOUT_S = 180.0
TURN_TIMEOUT_S = 60.0
WATCH_ACK_RE = re.compile(
    r"^watching  [0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}:"
    r"(?P<epoch>0|[1-9][0-9]*):(?P<seq>0|[1-9][0-9]*)$"
)
# The runtrol-owned reconnect boundary printed before every event payload. Transport metadata, not a
# provider line, so it carries no supervision fact; a malformed one is a protocol fault like any other.
WATCH_EVENT_RE = re.compile(
    r"^watch event  next [0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}:"
    r"(?P<epoch>0|[1-9][0-9]*):(?P<seq>0|[1-9][0-9]*)$"
)


class Failed(Exception):
    """The live Claude approval journey did not hold."""


@dataclass(frozen=True)
class Evidence:
    """Facts that must all come from one real process journey."""

    cli_probed: bool
    model_requests: int
    sentinel_auth: bool
    endpoint_contract: bool
    approval_received: bool
    subject_complete: bool
    rejection_selected: bool
    denial_sent: bool
    second_request_after_answer: bool
    provider_ended: bool
    turn_correlated: bool
    target_absent: bool
    cleanup_complete: bool


def verifyEvidence(evidence: Evidence) -> None:
    """Reject any journey that skipped one side of the approval round trip."""
    if not evidence.cli_probed:
        raise Failed("the installed Claude parser and version were not probed")
    if evidence.model_requests != 2:
        raise Failed(f"Claude made {evidence.model_requests} Messages requests, not exactly two")
    if not evidence.sentinel_auth:
        raise Failed("the loopback endpoint received a credential other than the sentinel")
    if not evidence.endpoint_contract:
        raise Failed("Claude used an unexpected Messages endpoint target")
    if not evidence.approval_received:
        raise Failed("the real Claude process emitted no hidden can_use_tool approval")
    if not evidence.subject_complete:
        raise Failed("the approval did not name the exact target file")
    if not evidence.rejection_selected:
        raise Failed("the approval did not offer exactly one rejectOnce option")
    if not evidence.denial_sent:
        raise Failed("runtrol did not carry an explicit denial to the pending approval")
    if not evidence.second_request_after_answer:
        raise Failed("the second Messages request began before runtrol answered the approval")
    if not evidence.provider_ended:
        raise Failed("Claude did not declare the turn ended after consuming the denial")
    if not evidence.turn_correlated:
        raise Failed("the terminal event did not belong to the approval's session and turn")
    if not evidence.target_absent:
        raise Failed("the denied Write created its target file")
    if not evidence.cleanup_complete:
        raise Failed("a process or thread started by the gate survived cleanup")


def selftest() -> int:
    """Prove every acceptance fact can turn the gate red."""
    valid = Evidence(True, 2, True, True, True, True, True, True, True, True, True, True, True)
    try:
        verifyEvidence(valid)
    except Failed as error:
        print(f"[claudeApprovalSmoke:selftest] FAIL. the valid journey was rejected: {error}", file=sys.stderr)
        return 2

    defects = (
        replace(valid, cli_probed=False),
        replace(valid, model_requests=1),
        replace(valid, model_requests=3),
        replace(valid, sentinel_auth=False),
        replace(valid, endpoint_contract=False),
        replace(valid, approval_received=False),
        replace(valid, subject_complete=False),
        replace(valid, rejection_selected=False),
        replace(valid, denial_sent=False),
        replace(valid, second_request_after_answer=False),
        replace(valid, provider_ended=False),
        replace(valid, turn_correlated=False),
        replace(valid, target_absent=False),
        replace(valid, cleanup_complete=False),
    )
    for defect in defects:
        try:
            verifyEvidence(defect)
        except Failed:
            # ok: rejection is the assertion, and the next injected defect is independent.
            continue
        print(f"[claudeApprovalSmoke:selftest] FAIL. an injected defect passed: {defect}", file=sys.stderr)
        return 2

    session = "019b0000-0000-7000-8000-000000000001"
    turn = {"epoch": 0, "index": 0}
    target = Path("/isolated/must-not-exist.txt")
    approval = json.dumps(
        {
            "session": session,
            "body": {
                "event": "approvalRequested",
                "id": "019b0000-0000-7000-8000-000000000002",
                "turn": turn,
                "kind": "fileChange",
                "options": [{"id": 1, "kind": "rejectOnce"}],
                "subject": {"input": {"file_path": str(target)}},
                "subject_incomplete": False,
                "subject_digest": list(range(32)),
            },
        }
    )
    boundary = approvalFrom(approval, target, session)
    if boundary is None or boundary.turn != turn:
        print("[claudeApprovalSmoke:selftest] FAIL. a valid approval event was rejected.", file=sys.stderr)
        return 2
    ended = json.dumps(
        {
            "session": session,
            "body": {
                "event": "turn",
                "step": "ended",
                "turn": turn,
                "stop": "endTurn",
                "declared_by": {"by": "provider"},
            },
        }
    )
    wrong_declarant = ended.replace('"provider"', '"processExit"')
    wrong_stop = ended.replace('"endTurn"', '"unknown"')
    if not providerEnded(ended, session, turn) or providerEnded(wrong_declarant, session, turn) or providerEnded(wrong_stop, session, turn):
        print("[claudeApprovalSmoke:selftest] FAIL. terminal provenance was not enforced.", file=sys.stderr)
        return 2
    if not validModelTarget("/v1/messages?beta=true") or not validModelTarget("/v1/messages"):
        print("[claudeApprovalSmoke:selftest] FAIL. a supported Messages target was rejected.", file=sys.stderr)
        return 2
    if validModelTarget("/v1/messages?beta=false") or validModelTarget("/v1/messages?wrong=true"):
        print("[claudeApprovalSmoke:selftest] FAIL. a wrong Messages query was accepted.", file=sys.stderr)
        return 2
    if secondAfterAnswer((1.0, 2.0), 2.0) or secondAfterAnswer((1.0, 1.5), 2.0):
        print("[claudeApprovalSmoke:selftest] FAIL. a premature second request was accepted.", file=sys.stderr)
        return 2
    if not secondAfterAnswer((1.0, 2.1), 2.0):
        print("[claudeApprovalSmoke:selftest] FAIL. a post-answer second request was rejected.", file=sys.stderr)
        return 2
    if cleanupComplete({10, 11}, {11}) or not cleanupComplete({10, 11}, set()):
        print("[claudeApprovalSmoke:selftest] FAIL. surviving process detection was wrong.", file=sys.stderr)
        return 2
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(30)"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if cleanupComplete({child.pid}, set(processTable())):
            print("[claudeApprovalSmoke:selftest] FAIL. a live subprocess looked cleaned up.", file=sys.stderr)
            return 2
    finally:
        child.terminate()
        child.wait(timeout=5.0)
    if waitGone({child.pid}):
        print("[claudeApprovalSmoke:selftest] FAIL. a terminated subprocess still looked alive.", file=sys.stderr)
        return 2
    arbitrary = json.dumps({"session": session, "body": {"event": "notice", "code": "other"}})
    if watchFact(arbitrary, target, session, False) is not None:
        print("[claudeApprovalSmoke:selftest] FAIL. an arbitrary event became a watch acknowledgement.", file=sys.stderr)
        return 2
    watch_ack = "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:11\n"
    acknowledged = watchFact(watch_ack, target, session, False)
    if acknowledged is None or acknowledged.kind != "ready":
        print("[claudeApprovalSmoke:selftest] FAIL. the watch acknowledgement was not recognized.", file=sys.stderr)
        return 2
    if watchFact(watch_ack, target, session, True) is not None:
        print("[claudeApprovalSmoke:selftest] FAIL. a duplicate acknowledgement produced another fact.", file=sys.stderr)
        return 2
    malformed_acks = (
        "watching\n",
        "watching  not-a-uuid:7:11\n",
        "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:epoch:11\n",
        "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:07:11\n",
        "watching  0198f0cf-22dc-4c11-94d6-f514bf512dd3:7:11\n",
        "watching  0198f0cf-22dc-7c11-74d6-f514bf512dd3:7:11\n",
        "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:4294967296:11\n",
        "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:18446744073709551616\n",
        "watching  0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:11 extra\n",
    )
    if any((fact := watchFact(line, target, session, False)) is None or fact.kind == "ready" for line in malformed_acks):
        print("[claudeApprovalSmoke:selftest] FAIL. a malformed acknowledgement looked ready.", file=sys.stderr)
        return 2
    boundary = "watch event  next 0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:12\n"
    if watchFact(boundary, target, session, True) is not None:
        print("[claudeApprovalSmoke:selftest] FAIL. the runtrol event boundary line became a fact.", file=sys.stderr)
        return 2
    broken_boundaries = (
        "watch event  next not-a-uuid:7:12\n",
        "watch event  next 0198f0cf-22dc-7c11-94d6-f514bf512dd3:4294967296:12\n",
        "watch event  next 0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:12 extra\n",
        "watch gap  requested 0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:2  live 0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:12\n",
        "watch lagged  reconnect after 0198f0cf-22dc-7c11-94d6-f514bf512dd3:7:12\n",
    )
    if any(
        (fact := watchFact(line, target, session, True)) is None or fact.kind != "fault"
        for line in broken_boundaries
    ):
        print("[claudeApprovalSmoke:selftest] FAIL. a broken or gapped boundary line was not a fault.", file=sys.stderr)
        return 2

    print("[claudeApprovalSmoke:selftest] OK. evidence, parsers, ordering, and cleanup defects are red.")
    return 0


def validTarget(target: str, path: str) -> bool:
    """Accept only one path and the two SDK query forms observed across supported releases."""
    parsed = urlsplit(target)
    return parsed.path.rstrip("/") == path and parsed.query in {"", "beta=true"} and not parsed.fragment


def validModelTarget(target: str) -> bool:
    """Whether this is the exact Messages request target the fixture serves."""
    return validTarget(target, "/v1/messages")


def validCountTarget(target: str) -> bool:
    """Whether this is the exact optional token-count request target."""
    return validTarget(target, "/v1/messages/count_tokens")


class ModelServer(ThreadingHTTPServer):
    """A bounded loopback-only Messages endpoint with count-only evidence."""

    daemon_threads = False
    block_on_close = True

    def __init__(self, target: Path) -> None:
        super().__init__(("127.0.0.1", 0), ModelHandler)
        self.target = target
        self.request_count = 0
        self.request_times: list[float] = []
        self.sentinel_auth = True
        self.endpoint_contract = True
        self.request_lock = threading.Lock()

    def observedRequest(self, arrived_at: float) -> int:
        """Record only that a request arrived, never its body or prompt."""
        with self.request_lock:
            self.request_count += 1
            self.request_times.append(arrived_at)
            return self.request_count

    def rejectedCredential(self) -> None:
        """Remember only that a non-sentinel credential arrived."""
        with self.request_lock:
            self.sentinel_auth = False

    def rejectedEndpoint(self) -> None:
        """Remember that the real CLI addressed an endpoint shape the gate does not serve."""
        with self.request_lock:
            self.endpoint_contract = False


class ModelHandler(BaseHTTPRequestHandler):
    """Discard one request and return a deterministic Anthropic SSE response."""

    protocol_version = "HTTP/1.1"

    def setup(self) -> None:
        super().setup()
        self.connection.settimeout(10.0)

    def do_POST(self) -> None:
        arrived_at = time.monotonic()
        server = self.server
        if not isinstance(server, ModelServer):
            self.send_error(500)
            return
        auth = self.headers.get("Authorization")
        api_key = self.headers.get("x-api-key")
        if auth != f"Bearer {TOKEN}" or api_key is not None:
            server.rejectedCredential()
            self.send_error(401)
            return
        length = self.headers.get("Content-Length")
        if length is None:
            self.send_error(411)
            return
        remaining = int(length)
        while remaining > 0:
            chunk = self.rfile.read(min(remaining, 64 * 1024))
            if not chunk:
                self.send_error(400)
                return
            remaining -= len(chunk)

        if validCountTarget(self.path):
            body = b'{"input_tokens":1}'
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.wfile.flush()
            self.close_connection = True
            return
        if not validModelTarget(self.path):
            server.rejectedEndpoint()
            self.send_error(404)
            return

        request_number = server.observedRequest(arrived_at)
        if request_number > 2:
            self.send_error(500)
            return
        events = toolUseEvents(server.target) if request_number == 1 else completionEvents()
        body = b"".join(
            f"event: {kind}\r\ndata: {json.dumps(payload, separators=(',', ':'))}\r\n\r\n".encode()
            for kind, payload in events
        )
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
        """Keep HTTP diagnostics from exposing a request path or header."""


def messageStart() -> tuple[str, dict[str, object]]:
    """The common opening frame for one Anthropic streamed message."""
    return (
        "message_start",
        {
            "type": "message_start",
            "message": {
                "id": "msg_runtrol_loopback",
                "type": "message",
                "role": "assistant",
                "model": MODEL,
                "content": [],
                "stop_reason": None,
                "stop_sequence": None,
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0,
                },
            },
        },
    )


def toolUseEvents(target: Path) -> tuple[tuple[str, dict[str, object]], ...]:
    """Ask the real CLI to write one sentinel file through its own Write tool."""
    tool_input = json.dumps(
        {"file_path": str(target), "content": "this denial must keep the file absent\n"},
        separators=(",", ":"),
    )
    return (
        messageStart(),
        (
            "content_block_start",
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_runtrol_write", "name": "Write", "input": {}},
            },
        ),
        (
            "content_block_delta",
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": tool_input},
            },
        ),
        ("content_block_stop", {"type": "content_block_stop", "index": 0}),
        (
            "message_delta",
            {
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use", "stop_sequence": None},
                "usage": {"output_tokens": 1},
            },
        ),
        ("message_stop", {"type": "message_stop"}),
    )


def completionEvents() -> tuple[tuple[str, dict[str, object]], ...]:
    """Finish only after the real CLI reports the denied tool result to the model."""
    return (
        messageStart(),
        (
            "content_block_start",
            {
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""},
            },
        ),
        (
            "content_block_delta",
            {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "denial consumed"},
            },
        ),
        ("content_block_stop", {"type": "content_block_stop", "index": 0}),
        (
            "message_delta",
            {
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                "usage": {"output_tokens": 1},
            },
        ),
        ("message_stop", {"type": "message_stop"}),
    )


class RunningModel:
    """Own the local server thread and prove it stops."""

    def __init__(self, target: Path) -> None:
        self.server = ModelServer(target)
        self.thread = threading.Thread(target=self.server.serve_forever, name="claude-model", daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}"

    @property
    def requests(self) -> int:
        with self.server.request_lock:
            return self.server.request_count

    @property
    def request_times(self) -> tuple[float, ...]:
        """Return timing metadata without any request content."""
        with self.server.request_lock:
            return tuple(self.server.request_times)

    @property
    def sentinel_auth(self) -> bool:
        """Whether every credential observed was the fixed non-secret sentinel."""
        with self.server.request_lock:
            return self.server.sentinel_auth

    @property
    def endpoint_contract(self) -> bool:
        """Whether every request target matched the served Messages contract."""
        with self.server.request_lock:
            return self.server.endpoint_contract

    def __enter__(self) -> RunningModel:
        self.thread.start()
        return self

    def __exit__(self, *_error: object) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5.0)
        if self.thread.is_alive():
            raise Failed("the loopback model server thread did not stop")


def claudeProgram() -> str | None:
    """Find the real installed provider through the operating system."""
    for name in CLAUDE_NAMES:
        found = shutil.which(name)
        if found is not None:
            return found
    return None


def buildBinary() -> Path:
    """Build the product binary that owns the production Claude driver."""
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


def environment(root: Path, home: Path, config: Path, model: RunningModel, claude: str) -> dict[str, str]:
    """Keep provider state temporary and make any non-loopback HTTP attempt fail closed."""
    result = dict(os.environ)
    for name in tuple(result):
        if name.startswith(("ANTHROPIC_", "CLAUDE_", "AWS_", "AZURE_", "GOOGLE_")):
            result.pop(name, None)
    user = root / "user"
    user.mkdir()
    for name, relative in (
        ("HOME", "home"),
        ("USERPROFILE", "home"),
        ("APPDATA", "appdata"),
        ("LOCALAPPDATA", "localappdata"),
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_DATA_HOME", "xdg-data"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("XDG_STATE_HOME", "xdg-state"),
    ):
        path = user / relative
        path.mkdir(parents=True, exist_ok=True)
        result[name] = str(path)
    result["RUNTROL_HOME"] = str(home)
    result["CLAUDE_CONFIG_DIR"] = str(config)
    result["ANTHROPIC_CONFIG_DIR"] = str(config)
    result["ANTHROPIC_BASE_URL"] = model.base_url
    result["ANTHROPIC_AUTH_TOKEN"] = TOKEN
    result["ANTHROPIC_MODEL"] = MODEL
    result["ANTHROPIC_SMALL_FAST_MODEL"] = MODEL
    result["CLAUDE_CODE_SUBAGENT_MODEL"] = MODEL
    result["CLAUDE_CODE_USE_GATEWAY"] = "1"
    result["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] = "1"
    result["DISABLE_AUTOUPDATER"] = "1"
    result["DISABLE_TELEMETRY"] = "1"
    result["DISABLE_ERROR_REPORTING"] = "1"
    for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
        result[name] = "http://127.0.0.1:9"
    result["NO_PROXY"] = "127.0.0.1,localhost"
    result["no_proxy"] = "127.0.0.1,localhost"
    result["PATH"] = f"{Path(claude).parent}{os.pathsep}{result.get('PATH', '')}"
    return result


def probeClaude(claude: str, env: dict[str, str]) -> bool:
    """Require a real version answer and the hidden parser surface without starting a turn."""
    version = subprocess.run(
        [claude, "--version"],
        env=env,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30.0,
        check=False,
    )
    hidden = subprocess.run(
        [claude, "-p", "--permission-prompt-tool"],
        env=env,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30.0,
        check=False,
    )
    fake = subprocess.run(
        [claude, "-p", "--runtrol-definitely-not-a-real-flag"],
        env=env,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30.0,
        check=False,
    )
    hidden_said = f"{hidden.stdout}\n{hidden.stderr}".lower()
    fake_said = f"{fake.stdout}\n{fake.stderr}".lower()
    return (
        version.returncode == 0
        and bool((version.stdout or version.stderr).strip())
        and hidden.returncode != 0
        and "unknown option" not in hidden_said
        and fake.returncode != 0
        and "unknown option" in fake_said
    )


def startDaemon(binary: Path, env: dict[str, str], home: Path) -> subprocess.Popen[str]:
    """Start an isolated daemon with diagnostics discarded so no inherited pipe can fill."""
    daemon = subprocess.Popen(
        [str(binary), "daemon"],
        cwd=ROOT,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    ready = home / ("runtrol.redb" if sys.platform == "win32" else "runtrol.sock")
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise Failed("the isolated daemon exited before its endpoint was ready")
        if ready.exists():
            if sys.platform == "win32":
                time.sleep(0.1)
            return daemon
        time.sleep(0.025)
    process.stopDaemon(daemon)
    raise Failed("the isolated daemon did not become ready")


@dataclass(frozen=True)
class ApprovalBoundary:
    """Only the values needed to authorize one answer, with no conversation payload."""

    approval: str
    option: str
    digest: str
    turn: object


@dataclass(frozen=True)
class WatchFact:
    """A bounded supervision fact extracted before the raw watcher line is discarded."""

    kind: str
    approval: ApprovalBoundary | None = None
    turn: object | None = None


def approvalFrom(line: str, target: Path, session: str) -> ApprovalBoundary | None:
    """Extract only the normalized authorization boundary from one event."""
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return None
    if event.get("session") != session:
        raise Failed("the watcher delivered an event for another session")
    body = event.get("body")
    if not isinstance(body, dict) or body.get("event") != "approvalRequested":
        return None
    approval = body.get("id")
    options = body.get("options")
    digest = body.get("subject_digest")
    subject = body.get("subject")
    if not isinstance(approval, str) or not isinstance(options, list) or not isinstance(digest, list):
        raise Failed("the approval event omitted its authorization boundary")
    if body.get("kind") != "fileChange" or body.get("subject_incomplete") is not False:
        raise Failed("the Write approval was not a complete file-change request")
    if not isinstance(subject, dict) or not isinstance(subject.get("input"), dict):
        raise Failed("the approval subject omitted its provider input")
    if subject["input"].get("file_path") != str(target):
        raise Failed("the approval subject named a different target file")
    rejections = [
        option.get("id")
        for option in options
        if isinstance(option, dict) and option.get("kind") == "rejectOnce"
    ]
    if len(rejections) != 1 or not isinstance(rejections[0], int):
        raise Failed("the approval event did not offer exactly one rejectOnce option")
    if len(digest) != 32 or any(not isinstance(byte, int) or byte < 0 or byte > 255 for byte in digest):
        raise Failed("the approval subject digest was not 32 bytes")
    return ApprovalBoundary(
        approval=approval,
        option=str(rejections[0]),
        digest="".join(f"{byte:02x}" for byte in digest),
        turn=body.get("turn"),
    )


def providerEnded(line: str, session: str, turn: object) -> bool:
    """Accept only a normalized end whose declared source is the provider."""
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return False
    body = event.get("body")
    return (
        event.get("session") == session
        and isinstance(body, dict)
        and body.get("event") == "turn"
        and body.get("step") == "ended"
        and body.get("turn") == turn
        and body.get("stop") == "endTurn"
        and body.get("declared_by") == {"by": "provider"}
    )


def watchFact(line: str, target: Path, session: str, ready_sent: bool) -> WatchFact | None:
    """Reduce one raw event immediately to the few supervision facts this gate needs."""
    acknowledgement = WATCH_ACK_RE.fullmatch(line.rstrip("\r\n"))
    if (
        acknowledgement is not None
        and int(acknowledgement.group("epoch")) <= 0xFFFF_FFFF
        and int(acknowledgement.group("seq")) <= 0xFFFF_FFFF_FFFF_FFFF
    ):
        return WatchFact("ready") if not ready_sent else None
    boundary_line = WATCH_EVENT_RE.fullmatch(line.rstrip("\r\n"))
    if boundary_line is not None:
        in_range = (
            int(boundary_line.group("epoch")) <= 0xFFFF_FFFF
            and int(boundary_line.group("seq")) <= 0xFFFF_FFFF_FFFF_FFFF
        )
        return None if in_range else WatchFact("fault")
    if line.rstrip("\r\n").startswith(("watch ", "watching")):
        # A gap, a lag, or a malformed transport line means this watcher can no longer claim it saw
        # every event, and a journey that must observe the exact approval boundary cannot continue.
        return WatchFact("fault")
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return WatchFact("fault")
    if event.get("session") != session:
        return WatchFact("fault")
    body = event.get("body")
    if not isinstance(body, dict):
        return WatchFact("fault")
    if body.get("event") == "approvalRequested":
        try:
            boundary = approvalFrom(line, target, session)
        except Failed:
            return WatchFact("fault")
        return WatchFact("approval", approval=boundary)
    if body.get("event") == "turn" and body.get("step") == "ended":
        return WatchFact("end", turn=body.get("turn")) if (
            body.get("stop") == "endTurn" and body.get("declared_by") == {"by": "provider"}
        ) else WatchFact("fault")
    if body.get("event") == "notice" and body.get("code") == "protocolViolation":
        return WatchFact("fault")
    if body.get("event") == "detached" and body.get("in_turn") is not None:
        return WatchFact("fault")
    return None


def readWatcher(
    watcher: subprocess.Popen[str],
    facts: queue.Queue[WatchFact],
    overflow: threading.Event,
    target: Path,
    session: str,
) -> None:
    """Discard every raw line immediately after reducing it into a bounded fact queue."""
    if watcher.stdout is None:
        overflow.set()
        return
    ready_sent = False
    for line in watcher.stdout:
        fact = watchFact(line, target, session, ready_sent)
        if fact is None:
            continue
        if fact.kind == "ready":
            ready_sent = True
        try:
            facts.put(fact, timeout=0.1)
        except queue.Full:
            overflow.set()
            return


def secondAfterAnswer(request_times: tuple[float, ...], answer_started: float) -> bool:
    """Require exactly one follow-up request whose connection arrived after answer dispatch began."""
    return len(request_times) == 2 and request_times[1] > answer_started


def cleanupComplete(started: set[int], alive: set[int]) -> bool:
    """Whether every daemon descendant observed during the journey is gone."""
    return started.isdisjoint(alive)


def stopProcess(child: subprocess.Popen[str] | None) -> None:
    """Stop one watcher without leaving a pipe reader behind."""
    if child is None or child.poll() is not None:
        return
    child.terminate()
    try:
        child.wait(timeout=2.0)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait(timeout=2.0)


def processTable() -> dict[int, int]:
    """Read only process and parent identifiers, never command lines or environment."""
    if sys.platform == "win32":
        command = (
            "Get-CimInstance Win32_Process | ForEach-Object { "
            "'{0} {1}' -f $_.ProcessId,$_.ParentProcessId }"
        )
        listed = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command],
            capture_output=True,
            text=True,
            timeout=15.0,
            check=False,
        )
    else:
        listed = subprocess.run(
            ["ps", "-axo", "pid=,ppid="],
            capture_output=True,
            text=True,
            timeout=15.0,
            check=False,
        )
    if listed.returncode != 0:
        raise Failed("the gate could not inspect process parent identifiers")
    table: dict[int, int] = {}
    for line in listed.stdout.splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        try:
            child, parent = (int(field) for field in fields)
        except ValueError:
            # ok: an unrelated process row raced its own removal; the remaining complete rows still identify ours.
            continue
        table[child] = parent
    return table


def descendants(parent: int) -> set[int]:
    """Every current descendant of one exact daemon PID."""
    table = processTable()
    found: set[int] = set()
    frontier = {parent}
    while frontier:
        children = {child for child, held_by in table.items() if held_by in frontier and child not in found}
        found.update(children)
        frontier = children
    return found


def waitGone(pids: set[int]) -> set[int]:
    """Wait briefly for process teardown and return the exact survivors."""
    deadline = time.monotonic() + 5.0
    alive = pids
    while alive and time.monotonic() < deadline:
        alive = pids.intersection(processTable())
        if alive:
            time.sleep(0.05)
    return alive


def exercise(claude: str) -> None:
    """Drive one real hidden approval from tool request through explicit denial to provider end."""
    binary = buildBinary()
    with tempfile.TemporaryDirectory(prefix="runtrol-claude-approval-") as raw_root:
        root = Path(raw_root)
        home = root / "runtrol-home"
        workspace = root / "workspace"
        config = root / "claude-config"
        target = workspace / "must-not-exist.txt"
        workspace.mkdir()
        config.mkdir()

        evidence: Evidence | None = None
        with RunningModel(target) as model:
            env = environment(root, home, config, model, claude)
            cli_probed = probeClaude(claude, env)
            daemon = startDaemon(binary, env, home)
            watcher: subprocess.Popen[str] | None = None
            reader: threading.Thread | None = None
            session: str | None = None
            closed = False
            provider_pids: set[int] = set()
            cleanup_complete = False
            try:
                session = process.command(
                    binary,
                    env,
                    ["start", PROVIDER, str(workspace), MODEL],
                )
                if process.SESSION_RE.fullmatch(session) is None:
                    raise Failed(f"start returned no session identifier: {session!r}")
                watcher = subprocess.Popen(
                    [str(binary), "watch", session],
                    cwd=ROOT,
                    env=env,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    bufsize=1,
                )
                facts: queue.Queue[WatchFact] = queue.Queue(maxsize=16)
                overflow = threading.Event()
                reader = threading.Thread(
                    target=readWatcher,
                    args=(watcher, facts, overflow, target, session),
                    name="claude-approval-watcher",
                    daemon=True,
                )
                reader.start()

                ready_deadline = time.monotonic() + 10.0
                while time.monotonic() < ready_deadline:
                    if overflow.is_set():
                        raise Failed("the bounded watcher fact queue overflowed")
                    try:
                        fact = facts.get(timeout=0.25)
                    except queue.Empty:
                        if watcher.poll() is not None:
                            raise Failed("the watcher exited before its replay established the subscription")
                        continue
                    if fact.kind == "ready":
                        break
                    raise Failed("the watcher produced a fault before the prompt was sent")
                else:
                    raise Failed("the watcher did not establish its replay subscription")

                process.command(binary, env, ["say", session, "perform the requested deterministic action"])

                approval_received = False
                subject_complete = False
                rejection_selected = False
                denial_sent = False
                answer_started: float | None = None
                provider_ended = False
                turn_correlated = False
                approval_boundary: ApprovalBoundary | None = None
                deadline = time.monotonic() + TURN_TIMEOUT_S
                while time.monotonic() < deadline and not provider_ended:
                    if overflow.is_set():
                        raise Failed("the bounded watcher fact queue overflowed")
                    try:
                        fact = facts.get(timeout=0.25)
                    except queue.Empty:
                        if watcher.poll() is not None:
                            raise Failed("the watcher exited before provider completion")
                        continue
                    if fact.kind == "fault":
                        raise Failed("the watcher observed a protocol fault or non-provider terminal event")
                    if fact.kind == "approval":
                        if approval_received:
                            raise Failed("the deterministic turn emitted more than one approval")
                        if fact.approval is None or fact.approval.turn is None:
                            raise Failed("the approval did not carry its turn boundary")
                        approval_received = True
                        subject_complete = True
                        rejection_selected = True
                        approval_boundary = fact.approval
                        if target.exists():
                            raise Failed("the target existed before the approval was answered")
                        if model.requests != 1:
                            raise Failed("a second Messages request arrived before the approval answer")
                        provider_pids.update(descendants(daemon.pid))
                        if not provider_pids:
                            raise Failed("the daemon had no real provider descendant during the approval")
                        answer_started = time.monotonic()
                        answered = process.command(
                            binary,
                            env,
                            [
                                "answer",
                                session,
                                approval_boundary.approval,
                                approval_boundary.option,
                                approval_boundary.digest,
                            ],
                        )
                        if answered != "done":
                            raise Failed("the approval answer did not return the product's done response")
                        denial_sent = True
                        if target.exists():
                            raise Failed("the target appeared immediately after the denial")
                    if fact.kind == "end":
                        provider_ended = True
                        turn_correlated = (
                            approval_boundary is not None and fact.turn == approval_boundary.turn
                        )
                if not provider_ended:
                    raise Failed("the real Claude turn did not reach a provider-declared end")

                listing = process.command(binary, env, ["list"])
                row = next((line for line in listing.splitlines() if line.startswith(session)), "")
                if "  idle  " not in row:
                    raise Failed(f"the provider ended but the session did not return to idle: {row!r}")
                evidence = Evidence(
                    cli_probed=cli_probed,
                    model_requests=model.requests,
                    sentinel_auth=model.sentinel_auth,
                    endpoint_contract=model.endpoint_contract,
                    approval_received=approval_received,
                    subject_complete=subject_complete,
                    rejection_selected=rejection_selected,
                    denial_sent=denial_sent,
                    second_request_after_answer=(
                        answer_started is not None and secondAfterAnswer(model.request_times, answer_started)
                    ),
                    provider_ended=provider_ended,
                    turn_correlated=turn_correlated,
                    target_absent=not target.exists(),
                    cleanup_complete=False,
                )
                provider_pids.update(descendants(daemon.pid))
                process.command(binary, env, ["close", session, "--now"])
                closed = True
            finally:
                if daemon.poll() is None:
                    provider_pids.update(descendants(daemon.pid))
                if session is not None and not closed and daemon.poll() is None:
                    try:
                        close = subprocess.run(
                            [str(binary), "close", session, "--now"],
                            cwd=ROOT,
                            env=env,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL,
                            timeout=10.0,
                            check=False,
                        )
                        closed = close.returncode == 0
                    except subprocess.TimeoutExpired:
                        # ok: cleanup continues by terminating this exact daemon and then checks every captured child PID.
                        closed = False
                stopProcess(watcher)
                if reader is not None:
                    reader.join(timeout=2.0)
                process.stopDaemon(daemon)
                survivors = waitGone(provider_pids)
                cleanup_complete = (
                    closed
                    and daemon.poll() is not None
                    and (watcher is None or watcher.poll() is not None)
                    and (reader is None or not reader.is_alive())
                    and cleanupComplete(provider_pids, survivors)
                )
                if evidence is not None:
                    evidence = replace(
                        evidence,
                        model_requests=model.requests,
                        sentinel_auth=model.sentinel_auth,
                        endpoint_contract=model.endpoint_contract,
                        second_request_after_answer=(
                            answer_started is not None
                            and secondAfterAnswer(model.request_times, answer_started)
                        ),
                        target_absent=not target.exists(),
                        cleanup_complete=cleanup_complete,
                    )

        if evidence is None:
            raise Failed("the live journey produced no evidence")
        verifyEvidence(evidence)
        print(
            "[claudeApprovalSmoke] OK. real Claude Code emitted hidden approval, consumed runtrol denial, "
            "declared end_turn, created no target file, and left no child process."
        )


def main(argv: list[str]) -> int:
    """Run the selftest or the live installed-CLI journey."""
    if "--selftest" in argv:
        return selftest()
    claude = claudeProgram()
    required = "--require-external" in argv
    if claude is None:
        message = (
            "[claudeApprovalSmoke] Claude Code is not installed; install `claude` or run without "
            "--require-external for an explicit optional skip."
        )
        print(message, file=sys.stderr if required else sys.stdout)
        return 2 if required else 0
    try:
        exercise(claude)
        return 0
    except (Failed, process.Failed, OSError, subprocess.SubprocessError) as error:
        print(f"[claudeApprovalSmoke] FAIL: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
