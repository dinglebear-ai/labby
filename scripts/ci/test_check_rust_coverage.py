import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("check-rust-coverage.py")
SPEC = importlib.util.spec_from_file_location("check_rust_coverage", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(MODULE)


class CoverageParserTests(unittest.TestCase):
    def test_aggregates_duplicate_line_records_by_source(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "lcov.info"
            report.write_text(
                "SF:/repo/crates/labby-auth/src/lib.rs\nDA:1,1\nDA:2,0\nend_of_record\n"
                "SF:/repo/crates/labby/src/config.rs\nDA:5,2\nend_of_record\n"
            )
            files = MODULE.load_lcov(report)
        self.assertEqual(MODULE.aggregate(files, "crates/labby-auth/src/"), (1, 2))
        self.assertEqual(MODULE.percent((1, 2)), 50.0)


if __name__ == "__main__":
    unittest.main()
