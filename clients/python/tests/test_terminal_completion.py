"""The public Python adapter preserves the native actor's structural completion fact."""

import asyncio
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import patch


def test_terminal_completion_keeps_failure_separate_from_process_code() -> None:
    native = ModuleType("runtrol_runtime._native")
    native.PyIdentity = object
    native.NativeError = RuntimeError
    source = str(Path(__file__).resolve().parents[1] / "python")

    class NativeView:
        opened_json = "{}"

        def __init__(self, failure: str | None) -> None:
            self.failure = failure

        def initial_screen(self) -> bytes:
            return b"last screen"

        async def next(self) -> SimpleNamespace:
            return SimpleNamespace(
                kind="exited", bytes=lambda: b"", sequence=None,
                lost_chunks=None, next_sequence=None, exit_code=0, failure=self.failure,
            )

    # The Rust actor projection has its own executable regression. This seam isolates the public Python
    # wrapper without requiring a built extension or a Runtime process, and restores all import state.
    with patch.dict(sys.modules, {"runtrol_runtime._native": native}), patch.object(sys, "path", [source, *sys.path]):
        from runtrol_runtime.client import AsyncTerminalView

        for failure in [None, "outputReadFailed", "controlStateLost", "inputDeliveryUnknown", "hostInitializationFailed"]:
            view = AsyncTerminalView(NativeView(failure))
            event = asyncio.run(view.next())
            assert view.initial_screen == b"last screen"
            assert event.kind == "exited"
            assert event.exit_code == 0
            assert event.failure == failure
            assert event.bytes == b""
