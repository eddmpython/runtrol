"""Typed loader for the Runtrol Studio performance budget SSOT."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import TypedDict

root = Path(__file__).resolve().parents[2]
budgetPath = root / "extensions" / "runtrol-vscode" / "performance-budget.json"

hostFields = (
    "activationMs",
    "openViewMs",
    "refreshP95Ms",
    "rssGrowthBytes",
    "coldResumeMs",
    "sessionSwitchP95Ms",
    "reloadRestoreMs",
    "followArrivalMs",
)
multiWindowFields = (
    "firstUseDeliveryMs",
    "warmDeliveryP95Ms",
    "latencySampleCount",
)
realProviderFields = (
    "runtimeClientDeliveryMs",
)
hostLoadFields = (
    "managedSessionCount",
    "hotSessionCount",
)


class PerformanceBudget(TypedDict):
    """Validated Studio budgets grouped by the journey that owns each metric."""

    host: dict[str, float]
    hostLoad: dict[str, int]
    multiWindowTerminal: dict[str, float]
    realProviderTerminal: dict[str, float]


def loadPerformanceBudget() -> PerformanceBudget:
    """Read the exact schema and reject unknown, missing, or non-positive values."""
    raw = json.loads(
        budgetPath.read_text(encoding="utf-8"),
        object_pairs_hook=uniqueObject,
        parse_constant=rejectConstant,
    )
    sections = {"host", "hostLoad", "multiWindowTerminal", "realProviderTerminal"}
    if not isinstance(raw, dict) or set(raw) != sections:
        raise ValueError(
            f"{budgetPath.relative_to(root)} must contain exactly {', '.join(sorted(sections))}"
        )
    host = numericSection(raw["host"], "host", hostFields)
    hostLoad = integerSection(raw["hostLoad"], "hostLoad", hostLoadFields)
    multiWindow = numericSection(
        raw["multiWindowTerminal"],
        "multiWindowTerminal",
        multiWindowFields,
    )
    realProvider = numericSection(
        raw["realProviderTerminal"],
        "realProviderTerminal",
        realProviderFields,
    )
    sampleCount = multiWindow["latencySampleCount"]
    if not sampleCount.is_integer():
        raise ValueError("budget multiWindowTerminal.latencySampleCount must be a positive integer")
    if sampleCount < 2:
        raise ValueError("budget multiWindowTerminal.latencySampleCount must include first-use and warm samples")
    return {
        "host": host,
        "hostLoad": hostLoad,
        "multiWindowTerminal": multiWindow,
        "realProviderTerminal": realProvider,
    }


def uniqueObject(pairs: list[tuple[str, object]]) -> dict[str, object]:
    """Reject duplicate JSON names so every consumer sees one budget value."""
    value: dict[str, object] = {}
    for name, item in pairs:
        if name in value:
            raise ValueError(f"performance budget repeats JSON name {name!r}")
        value[name] = item
    return value


def rejectConstant(value: str) -> object:
    """Reject the non-standard NaN and Infinity constants accepted by Python's JSON parser."""
    raise ValueError(f"performance budget contains non-standard number {value}")


def numericSection(value: object, name: str, fields: tuple[str, ...]) -> dict[str, float]:
    """Validate one exact positive-number section."""
    if not isinstance(value, dict) or set(value) != set(fields):
        raise ValueError(
            f"budget {name} must contain exactly {', '.join(fields)}"
        )
    section: dict[str, float] = {}
    for field in fields:
        measured = value[field]
        if (
            isinstance(measured, bool)
            or not isinstance(measured, (int, float))
            or not math.isfinite(measured)
            or measured <= 0
        ):
            raise ValueError(f"budget {name}.{field} must be a finite positive number")
        section[field] = float(measured)
    return section


def integerSection(value: object, name: str, fields: tuple[str, ...]) -> dict[str, int]:
    """Validate one exact positive-integer section."""
    numeric = numericSection(value, name, fields)
    if any(not measured.is_integer() for measured in numeric.values()):
        raise ValueError(f"budget {name} values must be positive integers")
    return {field: int(measured) for field, measured in numeric.items()}
