import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("protected_doc_guard.py")
SPEC = importlib.util.spec_from_file_location("protected_doc_guard", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(MODULE)


class ProtectedDocGuardTests(unittest.TestCase):
    def test_protects_sessions_and_superpowers(self) -> None:
        self.assertEqual(
            MODULE.protected_paths(
                [
                    "docs/README.md",
                    "docs/sessions/2026-08-19.md",
                    "docs/superpowers/plans/example.md",
                ]
            ),
            [
                "docs/sessions/2026-08-19.md",
                "docs/superpowers/plans/example.md",
            ],
        )

    def test_does_not_protect_similar_or_archived_paths(self) -> None:
        self.assertEqual(
            MODULE.protected_paths(
                [
                    "docs/archive/sessions/example.md",
                    "docs/session/example.md",
                    "docs/superpowers-old/example.md",
                ]
            ),
            [],
        )


if __name__ == "__main__":
    unittest.main()
