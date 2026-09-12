#!/usr/bin/env python3
"""Hermetic npm pointer transaction tests; never contacts a registry."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NpmPointerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.tags = self.root / "tags.json"
        self.tags.write_text('{"latest":"1.0.0"}')
        self.fail = self.root / "fail-after-write"
        fake = self.root / "npm"
        fake.write_text(f'#!{sys.executable}\n' + '''import json, os, pathlib, sys
p = pathlib.Path(os.environ["TEST_TAGS"])
f = pathlib.Path(os.environ["TEST_FAIL"])
a = sys.argv[1:]
if a[:2] == ["dist-tag", "ls"]:
    if f.exists() and f.read_text() == "read-fail":
        f.unlink()
        raise SystemExit(1)
    projection = os.environ.get("TEST_STALE_READ")
    print(projection if projection is not None else p.read_text())
elif a[:2] == ["dist-tag", "add"]:
    tags = json.loads(p.read_text()); tags["latest"] = a[2].rsplit("@",1)[1]
    p.write_text(json.dumps(tags))
    if f.exists(): f.write_text("read-fail")
elif a[:2] == ["dist-tag", "rm"]:
    tags = json.loads(p.read_text()); tags.pop("latest", None); p.write_text(json.dumps(tags))
else: raise SystemExit(2)
''')
        fake.chmod(0o755)
        self.env = {**os.environ, "NPM_BIN": str(fake), "TEST_TAGS": str(self.tags), "TEST_FAIL": str(self.fail)}

    def command(self, mode, version="2.0.0"):
        return subprocess.run([sys.executable, str(ROOT / "scripts/ci/promote-npm-pointer.py"), mode, "--package", "@test/pkg", "--version", version, "--receipt", str(self.root / "receipt.json")], env=self.env, capture_output=True, text=True)

    def test_successful_write_failed_verification_restores_previous(self):
        self.assertEqual(self.command("prepare").returncode, 0)
        self.fail.write_text("armed")
        self.assertNotEqual(self.command("promote").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "2.0.0")
        self.assertEqual(self.command("rollback").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "1.0.0")

    def test_stale_reads_do_not_skip_compensation_after_attempted_write(self):
        self.env["TEST_STALE_READ"] = '{"latest":"1.0.0"}'
        self.assertEqual(self.command("prepare").returncode, 0)
        self.assertNotEqual(self.command("promote").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "2.0.0")
        self.assertEqual(self.command("rollback").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "1.0.0")

    def test_stale_run_cannot_overwrite_newer_version(self):
        self.tags.write_text('{"latest":"3.0.0"}')
        self.assertNotEqual(self.command("prepare").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "3.0.0")
        self.assertFalse((self.root / "receipt.json").exists())

    def test_rollback_never_overwrites_another_promotion(self):
        self.assertEqual(self.command("prepare").returncode, 0)
        self.assertEqual(self.command("promote").returncode, 0)
        self.tags.write_text('{"latest":"3.0.0"}')
        self.assertNotEqual(self.command("rollback").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "3.0.0")

    def test_absent_previous_pointer_is_removed_on_rollback(self):
        self.tags.write_text('{}')
        self.assertEqual(self.command("prepare").returncode, 0)
        self.assertEqual(self.command("promote").returncode, 0)
        self.assertEqual(self.command("rollback").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text()), {})

    def test_already_stable_candidate_cannot_be_withdrawn_by_rollback(self):
        self.tags.write_text('{"latest":"2.0.0"}')
        self.assertEqual(self.command("prepare").returncode, 0)
        self.assertNotEqual(self.command("rollback").returncode, 0)
        self.assertEqual(json.loads(self.tags.read_text())["latest"], "2.0.0")

    def test_registry_failure_is_not_treated_as_absent_pointer(self):
        self.fail.write_text("read-fail")
        self.assertNotEqual(self.command("prepare").returncode, 0)
        self.assertFalse((self.root / "receipt.json").exists())


if __name__ == "__main__":
    unittest.main()
