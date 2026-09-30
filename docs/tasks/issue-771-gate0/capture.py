#!/usr/bin/env python3
"""Capture reproducible Gate 0 commands; never edit product code or other worktrees."""
from __future__ import annotations
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("label")
    parser.add_argument("command")
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--min-free-gib", type=float, default=3.0)
    args = parser.parse_args()
    if not args.label.replace("-", "").isalnum():
        parser.error("label must be alphanumeric with optional hyphens")
    evidence = HERE / "evidence"
    evidence.mkdir(exist_ok=True)
    log = evidence / (args.label + ".log")
    receipt = evidence / (args.label + ".json")
    if log.exists() or receipt.exists():
        parser.error("evidence label already exists; choose a new label")
    overrides = {"CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
                 "CARGO_TARGET_DIR": str(ROOT / "target"),
                 "CARGO_TERM_COLOR": "never", "NO_COLOR": "1"}
    env = dict(os.environ, **overrides)
    started = dt.datetime.now(dt.timezone.utc).isoformat()
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    start = time.monotonic()
    stopped = None
    with log.open("xb") as output:
        proc = subprocess.Popen(["/bin/bash", "-o", "pipefail", "-c", args.command],
                                cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT,
                                start_new_session=True)
        while proc.poll() is None:
            if time.monotonic() - start > args.timeout:
                stopped = "wall_clock_budget"
            elif shutil.disk_usage(ROOT).free < args.min_free_gib * 1024**3:
                stopped = f"disk_free_below_{args.min_free_gib:g}_GiB"
            elif log.stat().st_size > 8 * 1024**2:
                stopped = "output_over_8_MiB"
            if stopped:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
                break
            time.sleep(0.25)
    result = {"label": args.label, "command": args.command, "cwd": str(ROOT),
              "head": head, "environment_overrides": overrides, "started_at": started,
              "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(),
              "elapsed_seconds": round(time.monotonic() - start, 3),
              "timeout_seconds": args.timeout, "minimum_free_gib": args.min_free_gib,
              "capture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "exit_code": proc.returncode,
              "stopped_by_capture": stopped, "log": log.name,
              "log_sha256": hashlib.sha256(log.read_bytes()).hexdigest(),
              "log_bytes": log.stat().st_size}
    receipt.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result), flush=True)
    print(log.read_text(errors="replace")[-6500:], flush=True)
    return 0 if proc.returncode == 0 and stopped is None else 1

if __name__ == "__main__":
    raise SystemExit(main())
