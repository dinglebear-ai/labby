#!/usr/bin/env python3
"""Extract the pinned MCP JSON Schema's machine-enforced constraints."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
from urllib.parse import unquote_to_bytes


PROTOCOL_VERSION = "2026-07-28"
SOURCE_REVISION = "5f5440bb26a62e2cf3440b92da5a667efa03b267"
SOURCE_PATH = f"schema/{PROTOCOL_VERSION}/schema.json"

CONSTRAINT_KEYWORDS = {
    "$schema", "$id", "$ref", "$anchor", "$dynamicRef", "$dynamicAnchor",
    "$vocabulary", "type", "const", "enum", "multipleOf", "maximum",
    "exclusiveMaximum", "minimum", "exclusiveMinimum", "maxLength",
    "minLength", "pattern", "maxItems", "minItems", "uniqueItems",
    "maxContains", "minContains", "maxProperties", "minProperties",
    "required", "dependentRequired", "format", "contentEncoding",
    "contentMediaType", "contentSchema", "prefixItems", "items", "contains",
    "additionalProperties", "properties", "patternProperties",
    "dependentSchemas", "propertyNames", "if", "then", "else", "allOf",
    "anyOf", "oneOf", "not", "unevaluatedItems", "unevaluatedProperties",
}

SCHEMA_MAP_KEYWORDS = {"$defs", "properties", "patternProperties", "dependentSchemas"}
SCHEMA_SINGLE_KEYWORDS = {
    "items", "contains", "additionalProperties", "propertyNames", "if", "then",
    "else", "not", "unevaluatedItems", "unevaluatedProperties", "contentSchema",
}
SCHEMA_ARRAY_KEYWORDS = {"prefixItems", "allOf", "anyOf", "oneOf"}


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def pointer_part(value: str) -> str:
    return value.replace("~", "~0").replace("/", "~1")


def row_id(prefix: str, pointer: str) -> str:
    return f"{prefix}-{sha256(pointer.encode())[:20]}"


def local_ref_definition(target: str) -> str | None:
    prefix = "#/$defs/"
    if not target.startswith(prefix):
        return None
    encoded = unquote_to_bytes(target[len(prefix):].split("/", 1)[0]).decode("utf-8")
    return encoded.replace("~1", "/").replace("~0", "~")


def resolve_local_pointer(document: object, target: str) -> object:
    """Resolve a local URI-fragment JSON Pointer, rejecting malformed escapes."""
    if not target.startswith("#"):
        raise ValueError(f"not a local schema reference: {target}")
    raw_fragment = target[1:]
    for index, character in enumerate(raw_fragment):
        if character == "%" and (
            index + 2 >= len(raw_fragment)
            or any(digit not in "0123456789abcdefABCDEF" for digit in raw_fragment[index + 1:index + 3])
        ):
            raise ValueError(f"invalid URI fragment escape: {target}")
    try:
        fragment = unquote_to_bytes(raw_fragment).decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError(f"invalid UTF-8 in URI fragment: {target}") from error
    if fragment == "":
        return document
    if not fragment.startswith("/"):
        raise ValueError(f"invalid local JSON Pointer: {target}")
    current = document
    for encoded in fragment[1:].split("/"):
        index = 0
        while index < len(encoded):
            if encoded[index] == "~" and (
                index + 1 >= len(encoded) or encoded[index + 1] not in "01"
            ):
                raise ValueError(f"invalid JSON Pointer escape: {target}")
            index += 2 if encoded[index] == "~" else 1
        part = encoded.replace("~1", "/").replace("~0", "~")
        if isinstance(current, dict) and part in current:
            current = current[part]
        elif (
            isinstance(current, list)
            and part.isdigit()
            and (part == "0" or not part.startswith("0"))
            and int(part) < len(current)
        ):
            current = current[int(part)]
        else:
            raise ValueError(f"dangling local schema reference: {target}")
    return current


def extract_schema(schema: dict, source_bytes: bytes) -> dict:
    """Pure extraction helper; pinned-checkout verification belongs to `extract`."""
    definitions = schema.get("$defs")
    if not isinstance(definitions, dict) or not definitions:
        raise ValueError("MCP schema must contain a nonempty $defs object")

    definition_rows = []
    for name in sorted(definitions):
        pointer = f"#/$defs/{pointer_part(name)}"
        definition_rows.append({
            "id": row_id("MCP-SCHEMA-DEF", pointer),
            "name": name,
            "pointer": pointer,
            "schema_sha256": sha256(canonical(definitions[name])),
        })

    constraints = []
    refs = []

    def visit(node: object, pointer: str) -> None:
        if isinstance(node, bool):
            constraint_pointer = f"{pointer}/$boolean"
            value_hash = sha256(canonical(node))
            constraints.append({
                "id": row_id("MCP-SCHEMA-CONSTRAINT", constraint_pointer),
                "pointer": constraint_pointer,
                "keyword": "$boolean",
                "value": node,
                "value_sha256": value_hash,
                "constraint_sha256": sha256(canonical([constraint_pointer, "$boolean", value_hash])),
                "applicability": "unreviewed",
                "applicability_reason": "Machine-schema constraint has not been mapped to a Labby product surface.",
            })
            return
        if not isinstance(node, dict):
            raise ValueError(f"schema at {pointer} must be an object or boolean")
        for keyword in sorted(CONSTRAINT_KEYWORDS & node.keys()):
            constraint_pointer = f"{pointer}/{pointer_part(keyword)}"
            value = node[keyword]
            value_hash = sha256(canonical(value))
            constraints.append({
                "id": row_id("MCP-SCHEMA-CONSTRAINT", constraint_pointer),
                "pointer": constraint_pointer,
                "keyword": keyword,
                "value": value,
                "value_sha256": value_hash,
                "constraint_sha256": sha256(canonical([constraint_pointer, keyword, value_hash])),
                "applicability": "unreviewed",
                "applicability_reason": "Machine-schema constraint has not been mapped to a Labby product surface.",
            })
            if keyword == "$ref" and isinstance(value, str):
                if value.startswith("#"):
                    resolve_local_pointer(schema, value)
                refs.append({
                    "source_pointer": constraint_pointer,
                    "target": value,
                    "target_definition": local_ref_definition(value),
                })

        for keyword in sorted(SCHEMA_MAP_KEYWORDS):
            children = node.get(keyword)
            if children is None:
                continue
            if not isinstance(children, dict):
                raise ValueError(f"{keyword} at {pointer} must be an object")
            for name in sorted(children):
                visit(children[name], f"{pointer}/{pointer_part(keyword)}/{pointer_part(name)}")
        for keyword in sorted(SCHEMA_SINGLE_KEYWORDS):
            if keyword in node:
                visit(node[keyword], f"{pointer}/{pointer_part(keyword)}")
        for keyword in sorted(SCHEMA_ARRAY_KEYWORDS):
            children = node.get(keyword)
            if children is None:
                continue
            if not isinstance(children, list):
                raise ValueError(f"{keyword} at {pointer} must be an array")
            for index, child in enumerate(children):
                visit(child, f"{pointer}/{pointer_part(keyword)}/{index}")

    visit(schema, "#")
    constraints.sort(key=lambda row: row["pointer"])
    refs.sort(key=lambda row: (row["source_pointer"], row["target"]))
    ids = [row["id"] for row in definition_rows + constraints]
    if len(ids) != len(set(ids)):
        raise ValueError("generated schema inventory IDs collide")
    return {
        "schema_version": 1,
        "protocol_version": PROTOCOL_VERSION,
        "source_revision": SOURCE_REVISION,
        "source_path": SOURCE_PATH,
        "source_sha256": sha256(source_bytes),
        "definitions": definition_rows,
        "constraints": constraints,
        "refs": refs,
        "coverage_note": (
            "Machine-schema constraints are a separate denominator from BCP 14 prose; "
            "every entry is unreviewed until product applicability and evidence are assigned."
        ),
    }


def extract(source_root: Path) -> dict:
    """Extract only from the clean, immutable pinned specification checkout."""
    source = source_root / SOURCE_PATH
    source_bytes = source.read_bytes()
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source_root, text=True
    ).strip()
    if revision != SOURCE_REVISION:
        raise ValueError(f"schema checkout revision {revision} is not pinned {SOURCE_REVISION}")
    pinned_bytes = subprocess.check_output(
        ["git", "show", f"{SOURCE_REVISION}:{SOURCE_PATH}"], cwd=source_root
    )
    if source_bytes != pinned_bytes:
        raise ValueError("working-tree schema differs from the pinned Git blob")
    return extract_schema(json.loads(source_bytes), source_bytes)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_root", type=Path)
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("tools/verification/conformance/mcp-spec-schema.json"),
    )
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    args = parser.parse_args()
    encoded = json.dumps(extract(args.source_root), indent=2, ensure_ascii=False) + "\n"
    if args.check:
        if not args.output.exists() or args.output.read_text() != encoded:
            parser.exit(1, f"{args.output} differs from deterministic schema extraction\n")
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
