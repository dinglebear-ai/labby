#!/usr/bin/env python3
"""Run Labby's authentic offline OAuth recovery-set drill."""

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def main() -> None:
    completed = subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "labby-auth",
            "--all-features",
            "--locked",
            "--lib",
            "sqlite::extracted_tests::offline_auth_recovery_set_restores_real_schema_ciphertext_and_signing_key",
            "--",
            "--exact",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )
    print(completed.stdout, end="")
    completed.check_returncode()
    if not re.search(r"^test result: ok\. 1 passed; 0 failed;", completed.stdout, re.MULTILINE):
        raise SystemExit("auth backup restore drill must execute exactly one passing test")
    print("auth backup restore drill passed")


if __name__ == "__main__":
    main()
