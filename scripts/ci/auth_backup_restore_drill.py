#!/usr/bin/env python3
"""Run Labby's authentic offline OAuth recovery-set drill."""

import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

def main() -> None:
    subprocess.run(
        [
            "cargo",
            "test",
            "-p",
            "labby-auth",
            "--all-features",
            "sqlite::tests::offline_auth_recovery_set_restores_real_schema_ciphertext_and_signing_key",
            "--",
            "--exact",
        ],
        cwd=ROOT,
        check=True,
    )
    print("auth backup restore drill passed")


if __name__ == "__main__":
    main()
