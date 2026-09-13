#!/usr/bin/env python3
"""Exercise fallback with owned descendants and without coreutils timeout."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]


class TimeoutTests(unittest.TestCase):
    def test_descendants_are_killed_and_pipes_close(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for command in ("date", "sleep"):
                (root / command).symlink_to(shutil.which(command))
            marker = root / "survived"
            child = root / "child.py"
            child.write_text('import pathlib,sys,time; time.sleep(4); pathlib.Path(sys.argv[1]).write_text("survived")')
            parent = root / "parent.py"
            parent.write_text('import subprocess,sys,time; subprocess.Popen([sys.executable,sys.argv[1],sys.argv[2]]); time.sleep(10)')
            started = time.monotonic()
            result = subprocess.run(["/bin/bash", str(ROOT / "scripts/with_timeout.sh"), "1", "--", sys.executable, str(parent), str(child), str(marker)], env={**os.environ, "PATH": str(root)}, capture_output=True, timeout=6)
            self.assertEqual(result.returncode, 124)
            self.assertLess(time.monotonic() - started, 3.5)
            time.sleep(4)
            self.assertFalse(marker.exists())

    def test_normal_exit_code_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for command in ("date", "sleep"):
                (root / command).symlink_to(shutil.which(command))
            result = subprocess.run(["/bin/bash", str(ROOT / "scripts/with_timeout.sh"), "5", "--", "/bin/sh", "-c", "exit 7"], env={**os.environ, "PATH": str(root)}, capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 7)


if __name__ == "__main__":
    unittest.main()
