#!/usr/bin/env python3
"""Linux process identity and pidfd-safe shutdown for release qualification."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import signal
import sys
import time


def inspect(proc_root: Path, pid: int) -> tuple[str, str, str] | None:
    proc = proc_root / str(pid)
    try:
        text = (proc / "stat").read_text()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise RuntimeError(f"cannot read process stat: {error}") from error
    tail = text[text.rfind(") ") + 2 :].split()
    if len(tail) < 20:
        raise RuntimeError("malformed process stat")
    if tail[0] == "Z":
        return None
    start = tail[19]
    if not start.isdecimal():
        raise RuntimeError("malformed process start time")
    try:
        executable = os.readlink(proc / "exe").removesuffix(" (deleted)")
    except OSError as error:
        # /proc tears down piecemeal. Recheck stat to distinguish normal exit
        # and zombies from an inspection failure that must remain fail-closed.
        try:
            retry = (proc / "stat").read_text()
            retry_tail = retry[retry.rfind(") ") + 2 :].split()
        except FileNotFoundError:
            return None
        if retry_tail and retry_tail[0] == "Z":
            return None
        raise RuntimeError(f"cannot read process executable: {error}") from error
    return start, executable, tail[0]


def identity(args: argparse.Namespace) -> int:
    observed = inspect(Path(args.proc_root), args.pid)
    if observed is None:
        return 1
    start, executable, _ = observed
    if executable != args.executable:
        return 1
    print(f"{start}\t{executable}")
    return 0


def stop(args: argparse.Namespace) -> int:
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("pidfd signaling is unavailable")
    proc_root = Path(args.proc_root)
    if inspect(proc_root, args.pid) is None:
        return 0
    try:
        pidfd = os.pidfd_open(args.pid)
    except ProcessLookupError:
        return 0
    try:
        observed = inspect(proc_root, args.pid)
        if observed is None:
            return 0
        if observed[:2] != (args.start, args.executable):
            raise RuntimeError("process identity changed; refusing to signal it")
        signal.pidfd_send_signal(pidfd, signal.SIGTERM)
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            observed = inspect(proc_root, args.pid)
            if observed is None or observed[:2] != (args.start, args.executable):
                return 0
            time.sleep(0.1)
        raise RuntimeError("verified process did not stop")
    finally:
        os.close(pidfd)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("identity", "stop"))
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--proc-root", default="/proc")
    parser.add_argument("--executable", required=True)
    parser.add_argument("--start")
    parser.add_argument("--timeout", type=float, default=10)
    args = parser.parse_args()
    try:
        return identity(args) if args.command == "identity" else stop(args)
    except (OSError, RuntimeError) as error:
        print(error, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
