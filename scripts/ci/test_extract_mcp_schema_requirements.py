"""Tests for deterministic MCP machine-schema constraint extraction."""

from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci.extract_mcp_schema_requirements import (
    SOURCE_PATH,
    extract,
    extract_schema,
)


ROOT = Path(__file__).resolve().parents[2]
PINNED_SOURCE = Path("/tmp/labby-mcp-spec-2026-07-28")


class McpSchemaExtractionTests(unittest.TestCase):
    @staticmethod
    def synthetic(schema: dict) -> dict:
        encoded = json.dumps(schema, sort_keys=True).encode()
        return extract_schema(schema, encoded)

    def test_committed_inventory_matches_pinned_source(self) -> None:
        self.assertEqual(
            extract(PINNED_SOURCE),
            json.loads((ROOT / "conformance/mcp-spec-schema.json").read_text()),
        )

    def test_inventory_covers_every_definition_and_has_no_static_outcome(self) -> None:
        source = json.loads((PINNED_SOURCE / SOURCE_PATH).read_text())
        document = extract(PINNED_SOURCE)
        self.assertEqual(
            [row["name"] for row in document["definitions"]],
            sorted(source["$defs"]),
        )
        self.assertEqual(len(document["definitions"]), 155)
        self.assertTrue(document["constraints"])

        def schema_ref_count(node: object) -> int:
            if isinstance(node, bool):
                return 0
            if not isinstance(node, dict):
                return 0
            count = int(isinstance(node.get("$ref"), str))
            for keyword in (
                "$defs",
                "properties",
                "patternProperties",
                "dependentSchemas",
            ):
                children = node.get(keyword, {})
                if isinstance(children, dict):
                    count += sum(schema_ref_count(child) for child in children.values())
            for keyword in (
                "items",
                "contains",
                "additionalProperties",
                "propertyNames",
                "if",
                "then",
                "else",
                "not",
                "unevaluatedItems",
                "unevaluatedProperties",
                "contentSchema",
            ):
                if keyword in node:
                    count += schema_ref_count(node[keyword])
            for keyword in ("prefixItems", "allOf", "anyOf", "oneOf"):
                children = node.get(keyword, [])
                if isinstance(children, list):
                    count += sum(schema_ref_count(child) for child in children)
            return count

        self.assertEqual(len(document["refs"]), schema_ref_count(source))
        self.assertTrue(
            all(row["applicability"] == "unreviewed" for row in document["constraints"])
        )
        encoded = json.dumps(document)
        self.assertNotIn('"status"', encoded)
        self.assertIn("separate denominator from BCP 14 prose", document["coverage_note"])

    def test_instance_payload_keywords_are_not_walked_as_schemas(self) -> None:
        document = self.synthetic(
            {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": {
                    "Example": {
                        "type": "object",
                        "default": {"type": "not-a-schema", "$ref": "not-a-ref"},
                        "properties": {"value": {"const": {"required": ["instance"]}}},
                    }
                },
            }
        )
        pointers = {row["pointer"] for row in document["constraints"]}
        self.assertNotIn("#/$defs/Example/default/type", pointers)
        self.assertNotIn("#/$defs/Example/default/$ref", pointers)
        self.assertNotIn("#/$defs/Example/properties/value/const/required", pointers)

    def test_boolean_schemas_are_recorded_only_in_schema_positions(self) -> None:
        document = self.synthetic(
            {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "$defs": {
                    "BooleanPositions": {
                        "properties": {"allowed": True, "denied": False},
                        "items": False,
                        "allOf": [True, False],
                        "const": {"properties": {"fake": False}},
                        "default": {"items": True},
                    }
                }
            },
        )
        booleans = {
            row["pointer"]: row["value"]
            for row in document["constraints"]
            if row["keyword"] == "$boolean"
        }
        self.assertEqual(
            booleans,
            {
                "#/$defs/BooleanPositions/allOf/0/$boolean": True,
                "#/$defs/BooleanPositions/allOf/1/$boolean": False,
                "#/$defs/BooleanPositions/items/$boolean": False,
                "#/$defs/BooleanPositions/properties/allowed/$boolean": True,
                "#/$defs/BooleanPositions/properties/denied/$boolean": False,
            },
        )

    def test_dangling_or_malformed_local_refs_are_rejected(self) -> None:
        base = {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {"Target": {"properties": {"value": {"type": "string"}}}},
        }
        for target in (
            "#/$defs/Missing",
            "#/$defs/Target/properties/missing",
            "#/$defs/Target/~2invalid",
            "#/$defs/Target/properties/01",
            "#/$defs/Target/%GG",
            "#not-a-pointer",
        ):
            schema = deepcopy(base)
            schema["$defs"]["Source"] = {"$ref": target}
            with self.subTest(target=target), self.assertRaises(ValueError):
                self.synthetic(schema)
        valid = deepcopy(base)
        valid["$defs"]["Source"] = {"$ref": "#/$defs/Target/properties/value"}
        self.assertEqual(
            self.synthetic(valid)["refs"][0]["target"],
            "#/$defs/Target/properties/value",
        )
        escaped = {
            "$defs": {
                "Target/name~": {"type": "string"},
                "Source": {"$ref": "#/$defs/Target~1name~0"},
            }
        }
        self.assertEqual(
            self.synthetic(escaped)["refs"][0]["target_definition"],
            "Target/name~",
        )

    def test_pinned_extraction_rejects_wrong_revision_or_dirty_schema(self) -> None:
        schema = {"$defs": {"Example": {"type": "string"}}}
        source_bytes = json.dumps(schema).encode()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / SOURCE_PATH
            source.parent.mkdir(parents=True)
            source.write_bytes(source_bytes)

            with patch(
                "scripts.ci.extract_mcp_schema_requirements.subprocess.check_output",
                return_value="wrong-revision\n",
            ), self.assertRaisesRegex(ValueError, "not pinned"):
                extract(root)

            with patch(
                "scripts.ci.extract_mcp_schema_requirements.subprocess.check_output",
                side_effect=[
                    "5f5440bb26a62e2cf3440b92da5a667efa03b267\n",
                    b'{"$defs":{"Different":true}}',
                ],
            ), self.assertRaisesRegex(ValueError, "differs from the pinned Git blob"):
                extract(root)

    def test_missing_definition_or_altered_constraint_changes_inventory(self) -> None:
        original = json.loads((PINNED_SOURCE / SOURCE_PATH).read_text())
        baseline = extract(PINNED_SOURCE)
        missing = deepcopy(original)
        missing["$defs"].pop(next(iter(missing["$defs"])))
        with self.assertRaisesRegex(ValueError, "dangling local schema reference"):
            self.synthetic(missing)

        altered = deepcopy(original)
        definition = next(
            value for value in altered["$defs"].values()
            if isinstance(value, dict) and "type" in value
        )
        definition["type"] = "null"
        self.assertNotEqual(self.synthetic(altered), baseline)


if __name__ == "__main__":
    unittest.main()
