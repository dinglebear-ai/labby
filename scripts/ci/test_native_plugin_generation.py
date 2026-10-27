#!/usr/bin/env python3
"""Keep local Python caches out of portable Skill packages."""

import importlib.util
import pathlib
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "native_plugins", ROOT / "scripts/generate-native-plugins.py"
)
assert SPEC and SPEC.loader
generator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(generator)


class NativePluginGenerationTest(unittest.TestCase):
    def test_packages_preserve_skill_sources_without_local_python_caches(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            source = root / "source"
            scripts = source / "example/scripts"
            scripts.mkdir(parents=True)
            (source / "example/SKILL.md").write_text("# Example\n")
            (scripts / "helper.py").write_text("print('example')\n")
            (scripts / "__pycache__").mkdir()
            (scripts / "__pycache__/helper.cpython-314.pyc").write_bytes(b"cache")
            (scripts / "legacy.pyc").write_bytes(b"cache")
            with patch.object(generator, "SOURCE", source):
                generator.build("openclaw", root / "output")
            for package in [root / "output/skills", root / "output/.agents/skills"]:
                self.assertEqual((package / "example/SKILL.md").read_text(), "# Example\n")
                self.assertEqual((package / "example/scripts/helper.py").read_text(), "print('example')\n")
                self.assertFalse((package / "example/scripts/__pycache__").exists())
                self.assertFalse((package / "example/scripts/legacy.pyc").exists())


if __name__ == "__main__":
    unittest.main()
