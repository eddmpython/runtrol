"""Generate Python type aliases and the packaged schema from the Rust-derived Runtime schema."""

from __future__ import annotations

import hashlib
import json
import keyword
import shutil
import sys
from pathlib import Path
from typing import Any

PACKAGE = Path(__file__).resolve().parents[1]
ROOT = PACKAGE.parents[1]
SOURCE = ROOT / "crates" / "runtrol-runtime-protocol" / "schema" / "runtime.schema.json"
SCHEMA = PACKAGE / "schema" / "runtime.schema.json"
GENERATED = PACKAGE / "python" / "runtrol_runtime" / "generated.py"


def reference(value: str) -> str:
    prefix = "#/$defs/"
    return f"ForwardRef({value[len(prefix):]!r})" if value.startswith(prefix) else "JsonValue"


def literal(value: object) -> str:
    return repr(value)


def expression(schema: dict[str, Any]) -> str:
    if "$ref" in schema:
        return reference(str(schema["$ref"]))
    if "const" in schema:
        return f"Literal[{literal(schema['const'])}]"
    if isinstance(schema.get("enum"), list):
        values = ", ".join(literal(value) for value in schema["enum"])
        return f"Literal[{values}]"
    variants = schema.get("oneOf") or schema.get("anyOf")
    if isinstance(variants, list):
        return " | ".join(expression(value) for value in variants if isinstance(value, dict)) or "JsonValue"
    kind = schema.get("type")
    if isinstance(kind, list):
        return " | ".join(expression({**schema, "type": value}) for value in kind)
    if kind == "string":
        return "str"
    if kind == "integer":
        return "int"
    if kind == "number":
        return "float"
    if kind == "boolean":
        return "bool"
    if kind == "null":
        return "None"
    if kind == "array":
        items = schema.get("items")
        return f"list[{expression(items) if isinstance(items, dict) else 'JsonValue'}]"
    if kind == "object":
        return "JsonObject"
    return "JsonValue"


def definition(name: str, schema: dict[str, Any]) -> str:
    properties = schema.get("properties")
    if schema.get("type") == "object" and isinstance(properties, dict):
        required = set(schema.get("required", []))
        fields = []
        for field, shape in properties.items():
            value = expression(shape) if isinstance(shape, dict) else "JsonValue"
            wrapper = "Required" if field in required else "NotRequired"
            fields.append(f"    {field!r}: {wrapper}[{value}],")
        body = "\n".join(fields)
        return f"{name} = TypedDict({name!r}, {{\n{body}\n}})"
    return f"{name}: TypeAlias = {expression(schema)}"


def render(schema: dict[str, Any], digest: str) -> str:
    definitions = schema.get("$defs", {})
    rendered = []
    for name, shape in sorted(definitions.items()):
        if not isinstance(name, str) or not name.isidentifier() or keyword.iskeyword(name):
            continue
        if isinstance(shape, dict):
            rendered.append(definition(name, shape))
    return "\n".join(
        [
            '"""Generated from the checked Rust Runtime schema. Do not edit by hand."""',
            "",
            "from __future__ import annotations",
            "",
            "from typing import ForwardRef, Literal, NotRequired, Required, TypeAlias, TypedDict",
            "",
            "JsonValue: TypeAlias = None | bool | int | float | str | list[\"JsonValue\"] | dict[str, \"JsonValue\"]",
            "JsonObject: TypeAlias = dict[str, JsonValue]",
            f"SCHEMA_SHA256 = {digest!r}",
            "",
            *rendered,
            "",
        ]
    )


def main() -> int:
    raw = SOURCE.read_bytes()
    schema = json.loads(raw)
    generated = render(schema, hashlib.sha256(raw).hexdigest())
    check = "--check" in sys.argv
    if check:
        if not SCHEMA.is_file() or SCHEMA.read_bytes() != raw:
            print("packaged Python schema is stale")
            return 2
        if not GENERATED.is_file() or GENERATED.read_text(encoding="utf-8") != generated:
            print("generated Python Runtime types are stale")
            return 2
        return 0
    SCHEMA.parent.mkdir(parents=True, exist_ok=True)
    GENERATED.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SOURCE, SCHEMA)
    GENERATED.write_text(generated, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
