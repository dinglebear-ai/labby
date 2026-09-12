#!/usr/bin/env python3
"""Recoverable npm latest promotion. Caller must hold the release promotion lock."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess


def version(value):
    match = re.fullmatch(r"v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value)
    if not match:
        raise ValueError("stable pointer must name a stable SemVer")
    return tuple(map(int, match.groups()))


def run(*args):
    return subprocess.check_output([os.environ.get("NPM_BIN", "npm"), *args], text=True).strip()


def latest(package):
    # A failed registry read is not evidence that the pointer is absent.
    tags = json.loads(run("dist-tag", "ls", package, "--json"))
    return tags.get("latest")


def save(path, record):
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(record) + "\n")
    temp.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare", "promote", "rollback"])
    parser.add_argument("--package", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    if args.mode == "prepare":
        previous = latest(args.package)
        if previous is not None and version(args.version) < version(previous):
            raise ValueError("refusing to replace a newer npm stable release")
        version(args.version)
        save(args.receipt, dict(package=args.package, candidate=args.version, previous=previous, state="prepared"))
        return
    record = json.loads(args.receipt.read_text())
    if (record["package"], record["candidate"]) != (args.package, args.version):
        raise ValueError("receipt does not belong to this promotion")
    current = latest(args.package)
    if args.mode == "promote":
        if current != record["previous"]:
            raise ValueError("npm stable pointer changed since preparation")
        # Persist intent BEFORE the mutation: a timeout can still mean it applied.
        record["state"] = "promoting"
        save(args.receipt, record)
        run("dist-tag", "add", f"{args.package}@{args.version}", "latest")
        if latest(args.package) != args.version:
            raise ValueError("npm stable promotion verification failed")
        record["state"] = "promoted"
    else:
        if current == record["previous"] == args.version:
            raise ValueError("candidate was already the stable release; its assets must remain public")
        if current not in (record["previous"], args.version):
            raise ValueError("npm stable pointer changed; refusing stale rollback")
        if record["state"] in ("promoting", "promoted"):
            # Registry reads may lag a successful write. Always compensate an
            # attempted promotion, even if the read still shows the old value.
            if record["previous"] is None:
                run("dist-tag", "rm", args.package, "latest")
            else:
                run("dist-tag", "add", f'{args.package}@{record["previous"]}', "latest")
            if latest(args.package) != record["previous"]:
                raise ValueError("npm stable rollback verification failed")
        elif current != record["previous"]:
            raise ValueError("npm stable pointer changed; refusing stale rollback")
        record["state"] = "rolled-back"
    save(args.receipt, record)


if __name__ == "__main__":
    main()
