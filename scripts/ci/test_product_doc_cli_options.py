"""Regression coverage for CLI examples checked against generated Clap help."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "product_docs", Path(__file__).resolve().parents[1] / "check-product-docs.py"
)
PRODUCT_DOCS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PRODUCT_DOCS)


class ShippedCliOptionsTests(unittest.TestCase):
    def test_aliases_and_repeated_flags_are_accepted_but_unknown_flags_are_not(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            help_path = root / "docs/generated/cli-help.md"
            help_path.parent.mkdir(parents=True)
            help_path.write_text(
                "## `labby`\n\n```text\nOptions:\n"
                "  -h, --help\n          Print help\n"
                "  -v, --verbose...\n          Repeat for detail\n"
                "      --json\n          JSON output\n```\n",
                encoding="utf-8",
            )
            skill = root / "plugins/labby/skills/using-labby/SKILL.md"
            skill.parent.mkdir(parents=True)
            skill.write_text(
                "```bash\nlabby -h\nlabby --help\nlabby -v\n"
                "labby --verbose\nlabby --json\nlabby --unknown\n```\n",
                encoding="utf-8",
            )
            failures = []
            with patch.object(PRODUCT_DOCS, "ROOT", root):
                PRODUCT_DOCS.validate_shipped_skill_cli_examples(failures)
            self.assertEqual(len(failures), 1, failures)
            self.assertIn("--unknown is not accepted by labby", failures[0])


if __name__ == "__main__":
    unittest.main()
