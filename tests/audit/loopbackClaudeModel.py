"""One loopback Anthropic-compatible endpoint for gates that need a real CLI to answer without a network.

Lifted out of the Mission live journey when that gate was deleted (2026-08-26): the fixture was never about
Missions. It counts requests and returns one fixed terminal stream, and it retains nothing else.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import claudeApprovalSmoke as claude_gate

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
