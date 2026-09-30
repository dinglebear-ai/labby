#!/usr/bin/env python3
"""Capture one bounded Lane A check; never overwrite prior evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[3]
EVIDENCE = Path(__file__).resolve().parent / "evidence"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("label")
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--min-free-gib", type=float, default=3)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not args.label.replace("-", "").isalnum():
        parser.error("provide an alphanumeric/hyphen label and a command after --")
    log_path = EVIDENCE / (args.label + ".log")
    result_path = EVIDENCE / (args.label + ".json")
    if log_path.exists() or result_path.exists():
        parser.error("evidence label already exists; use a new label")
    env = os.environ.copy()
    overrides = {"CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
                 "CARGO_TARGET_DIR": str(ROOT / "target/lane-a/build"),
                 "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0",
                 "CARGO_TERM_COLOR": "never", "RUSTUP_TOOLCHAIN": "1.97.1"}
    env.update(overrides)
    started = datetime.now(timezone.utc).isoformat()
    before = time.monotonic()
    stopped = None
    process = None
    exit_code = None
    try:
        with log_path.open("xb") as log:
            if shutil.disk_usage(ROOT).free < args.min_free_gib * 2**30:
                stopped = "disk_free_below_floor_before_launch"
            else:
                process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                while process.poll() is None:
                    if time.monotonic() - before > args.timeout:
                        stopped = "timeout"
                    elif shutil.disk_usage(ROOT).free < args.min_free_gib * 2**30:
                        stopped = "disk_free_below_floor"
                    if stopped:
                        os.killpg(process.pid, signal.SIGTERM)
                        try:
                            process.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid, signal.SIGKILL)
                        break
                    time.sleep(0.2)
                exit_code = process.wait()
    except (OSError, KeyboardInterrupt) as error:
        stopped = type(error).__name__ + ": " + str(error)
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            exit_code = process.wait()
    payload = log_path.read_bytes() if log_path.exists() else b""
    result = {"label": args.label, "command": command, "cwd": str(ROOT),
              "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "environment_overrides": overrides, "started_at": started,
              "finished_at": datetime.now(timezone.utc).isoformat(),
              "elapsed_seconds": round(time.monotonic() - before, 3),
              "timeout_seconds": args.timeout, "minimum_free_gib": args.min_free_gib,
              "exit_code": exit_code, "stopped_by_capture": stopped,
              "log": log_path.name, "log_sha256": hashlib.sha256(payload).hexdigest()}
    with result_path.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps(result, indent=2))
    print(payload.decode(errors="replace")[-6000:])
    return 0 if exit_code == 0 and stopped is None else 1


if __name__ == "__main__":
    sys.exit(main())
