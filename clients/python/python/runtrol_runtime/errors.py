"""Stable public Python exceptions for Runtime failures."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import NoReturn

from . import _native


@dataclass(slots=True)
class RuntimeError(Exception):
    """A stable public Runtime or client failure."""

    code: str
    message: str
    retryable: bool = False
    action: str | None = None
    correlation_id: str = "python-client"

    def __post_init__(self) -> None:
        Exception.__init__(self, self.message)


class RuntimeNotInstalledError(RuntimeError):
    """No validated shared Runtime installation is present."""


class OutcomeUnknownError(RuntimeError):
    """A mutation may have completed and must be queried or retried with the same request ID."""


class RuntimeUnavailableError(RuntimeError):
    """The installed shared Runtime cannot currently be reached."""


class ProtocolIncompatibleError(RuntimeError):
    """The installed Runtime and this client share no compatible public contract."""


class LegacyGenerationBusyError(RuntimeError):
    """A pre-public draining generation may still own the requested native conversation."""


class NativeConversationBusyError(RuntimeError):
    """A structured process already owns the requested native conversation."""


class TerminalAlreadyLiveError(RuntimeError):
    """A terminal in another Runtime generation already owns the native conversation."""


class TerminalGoneError(RuntimeError):
    """The requested terminal process or view has ended."""


class TerminalGenerationUnavailableError(RuntimeError):
    """The exact Runtime generation recorded on a terminal is no longer available."""


class TerminalWorkspaceConflictError(RuntimeError):
    """The native conversation is live in a different canonical workspace."""


def translate(error: _native.NativeError) -> NoReturn:
    """Raise the typed public equivalent of one native failure payload."""

    try:
        payload = json.loads(str(error))
    except (TypeError, ValueError, json.JSONDecodeError):
        raise RuntimeError("internal", str(error)) from error
    failure = {
        "code": str(payload.get("code", "internal")),
        "message": str(payload.get("message", "Runtime client failed")),
        "retryable": bool(payload.get("retryable", False)),
        "action": payload.get("action") if isinstance(payload.get("action"), str) else None,
        "correlation_id": str(payload.get("correlationId", "python-client")),
    }
    error_type = {
        "legacyGenerationBusy": LegacyGenerationBusyError,
        "nativeConversationBusy": NativeConversationBusyError,
        "protocolIncompatible": ProtocolIncompatibleError,
        "runtimeNotInstalled": RuntimeNotInstalledError,
        "runtimeUnavailable": RuntimeUnavailableError,
        "terminalAlreadyLive": TerminalAlreadyLiveError,
        "terminalGenerationUnavailable": TerminalGenerationUnavailableError,
        "terminalGone": TerminalGoneError,
        "terminalWorkspaceConflict": TerminalWorkspaceConflictError,
        "outcomeUnknown": OutcomeUnknownError,
    }.get(failure["code"], RuntimeError)
    raise error_type(**failure) from error
