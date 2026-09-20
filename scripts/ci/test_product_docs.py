#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-product-docs.py"
SPEC = importlib.util.spec_from_file_location("check_product_docs", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProductDocsServiceIndexTests(unittest.TestCase):
    def test_only_first_table_column_counts_as_service_index_coverage(self) -> None:
        text = (
            "`proxy` mentioned in prose must not count.\n\n"
            "| Service | Documentation |\n"
            "| --- | --- |\n"
            "| `access`, `projects` | [Access](./ACCESS.md) |\n"
            "| gateway runtime | Mentions `gateway` only in the second column |\n"
        )
        self.assertEqual(MODULE.service_names_from_table(text), {"access", "projects"})

    def test_named_section_selects_its_own_service_table(self) -> None:
        text = (
            "| `wrong` | Earlier table |\n"
            "| --- | --- |\n\n"
            "## Current Product Services\n\n"
            "| Service | Product doc |\n"
            "| --- | --- |\n"
            "| `gateway`, `proxy` | Docs |\n"
        )
        self.assertEqual(
            MODULE.service_names_from_table(text, "Current Product Services"),
            {"gateway", "proxy"},
        )
        self.assertEqual(MODULE.service_names_from_table(text, "Missing"), set())


if __name__ == "__main__":
    unittest.main()
