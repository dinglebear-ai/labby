#!/usr/bin/env python3
"""Capture bounded Lane C checks without modifying another worktree's sources."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import signal
import socket
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def main():
    if len(sys.argv) < 4 or sys.argv[2] != "--" or not re.fullmatch(r"[a-z0-9-]+", sys.argv[1]):
        raise SystemExit("usage: run_checks.py CHECK-NAME -- COMMAND [ARG ...]")
    name, command = sys.argv[1], sys.argv[3:]
    log_path = HERE / "evidence" / (name + ".log")
    receipt_path = log_path.with_suffix(".json")
    if log_path.exists() or receipt_path.exists():
        raise SystemExit("evidence already exists; use a new check name")
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "2"
    # Cargo locks this shared build cache; no source checkout or cache is cleaned.
    env["CARGO_TARGET_DIR"] = "/Users/jmagar/workspace/labby/target"
    started = datetime.datetime.now(datetime.timezone.utc).isoformat()
    start = time.monotonic()
    timed_out = False
    with log_path.open("x") as log:
        log.write("cwd: " + str(ROOT) + "\ncommand: " + shlex.join(command) + "\n")
        log.flush()
        try:
            proc = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log,
                                    stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = proc.wait(timeout=1200)
            except subprocess.TimeoutExpired:
                timed_out = True
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
                code = 124
        except OSError as exc:
            log.write(str(exc) + "\n")
            code = 127
    git = lambda *args: subprocess.check_output(["git", "-C", str(ROOT), *args], text=True).strip()
    files = sorted((ROOT / "crates/labby-gateway/tests").glob("issue_771_server_lifecycle*.rs"))
    files += sorted((ROOT / "crates/labby-gateway/tests/issue_771_server_lifecycle").glob("*.rs"))
    receipt = {"check": name, "started_utc": started, "host": socket.gethostname(),
               "cwd": str(ROOT), "head": git("rev-parse", "HEAD"),
               "branch": git("branch", "--show-current"), "command": command,
               "environment": {key: env[key] for key in ("CARGO_BUILD_JOBS", "CARGO_TARGET_DIR")},
               "exit_code": code, "timed_out": timed_out,
               "elapsed_seconds": round(time.monotonic() - start, 3),
               "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest(),
               "test_source_sha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                      for path in files}}
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({"check": name, "exit_code": code, "receipt": str(receipt_path)}))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
