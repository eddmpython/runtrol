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
        "runtimeNotInstalled": RuntimeNotInstalledError,
        "outcomeUnknown": OutcomeUnknownError,
    }.get(failure["code"], RuntimeError)
    raise error_type(**failure) from error
