#!/usr/bin/env python3
"""Recoverable channel-bound npm pointer promotion. Caller must hold the release promotion lock."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time


try:
    from release_channel import identity, order
except ModuleNotFoundError:
    from scripts.ci.release_channel import identity, order


def version(value):
    return order(value)


def run(*args):
    return subprocess.check_output([os.environ.get("NPM_BIN", "npm"), *args], text=True, timeout=30).strip()


def latest(package, tag="latest"):
    # A failed registry read is not evidence that the pointer is absent.
    # `npm dist-tag ls` ignores --json and prints `tag: version` lines; only
    # `npm view <pkg> dist-tags --json` returns the tag map as JSON.
    tags = json.loads(run("view", package, "dist-tags", "--json", "--prefer-online", "--fetch-retries=0", "--fetch-timeout=20000"))
    return tags.get(tag)



def verify_latest(package, expected, tag="latest"):
    """Allow bounded registry propagation after one pointer mutation."""
    for attempt in range(20):
        try:
            observed = latest(package) if tag == "latest" else latest(package, tag)
            if observed == expected:
                return
            detail = f"observed {observed!r}"
        except (subprocess.SubprocessError, json.JSONDecodeError):
            detail = "registry lookup failed"
        print(f"npm {tag} verification {attempt + 1}/20: {detail}", flush=True)
        if attempt < 19:
            time.sleep(5)
    raise ValueError("npm pointer verification timed out")


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
    release = identity(args.version)
    tag = release["npm_tag"]
    read = lambda: latest(args.package) if tag == "latest" else latest(args.package, tag)
    if args.mode == "prepare":
        previous = read()
        if previous is not None and identity(previous)["channel"] != release["channel"]:
            raise ValueError("npm pointer contains a different channel")
        if previous is not None and version(args.version) < version(previous):
            raise ValueError("refusing to replace a newer npm stable release")
        version(args.version)
        save(args.receipt, dict(package=args.package, candidate=args.version, previous=previous, state="prepared", channel=release["channel"], npm_tag=tag))
        return
    record = json.loads(args.receipt.read_text())
    if (record["package"], record["candidate"]) != (args.package, args.version):
        raise ValueError("receipt does not belong to this promotion")
    if record.get("channel", "stable") != release["channel"] or record.get("npm_tag", "latest") != tag:
        raise ValueError("receipt channel does not belong to this promotion")
    current = read()
    if args.mode == "promote":
        if current != record["previous"]:
            raise ValueError("npm stable pointer changed since preparation")
        # Persist intent BEFORE the mutation: a timeout can still mean it applied.
        record["state"] = "promoting"
        save(args.receipt, record)
        run("dist-tag", "add", f"{args.package}@{args.version}", tag)
        verify_latest(args.package, args.version, tag)
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
                run("dist-tag", "rm", args.package, tag)
            else:
                run("dist-tag", "add", f'{args.package}@{record["previous"]}', tag)
            verify_latest(args.package, record["previous"], tag)
        elif current != record["previous"]:
            raise ValueError("npm stable pointer changed; refusing stale rollback")
        record["state"] = "rolled-back"
    save(args.receipt, record)


if __name__ == "__main__":
    main()
