#!/usr/bin/env python3
"""Opt-in daily Labby updates for macOS, using the verified release installer."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import urllib.request

LABEL = "net.labby.auto-update"
REPO = "dinglebear-ai/labby"
ASSET = "lab-aarch64-apple-darwin.tar.gz"


def version(text):
    match = re.fullmatch(r"(?:labby |v)?(\d+)\.(\d+)\.(\d+)", text.strip())
    if not match:
        raise ValueError(f"Expected a stable Labby version, got {text!r}")
    return tuple(map(int, match.groups()))


def select_release(releases, current):
    candidates = []
    for release in releases:
        if release.get("draft") or release.get("prerelease"):
            continue
        try:
            candidate = version(release["tag_name"])
        except (ValueError, KeyError):
            continue
        names = {asset["name"] for asset in release.get("assets", [])}
        if {ASSET, ASSET + ".sha256"} <= names and candidate > current:
            candidates.append((candidate, release["tag_name"]))
    return max(candidates)[1] if candidates else None


def run(binary, installer):
    # Serialize manual and scheduled invocations of this updater.
    with (binary.parent / ".labby-auto-update.lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        current = version(
            subprocess.check_output([str(binary), "--version"], text=True, timeout=30)
        )
        request = urllib.request.Request(
            f"https://api.github.com/repos/{REPO}/releases?per_page=100",
            headers={
                "Accept": "application/vnd.github+json",
                "User-Agent": "labby-auto-update",
            },
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            releases = json.load(response)
        tag = select_release(releases, current)
        if tag is None:
            print("No newer stable Labby binary release.", flush=True)
            return
        print(f"Updating {binary} to {tag}", flush=True)
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("LABBY_INSTALL_")
        }
        env.update(
            LABBY_INSTALL_VERSION=tag,
            LABBY_INSTALL_DIR=str(binary.parent),
            LABBY_INSTALL_REPO=REPO,
            LABBY_ALLOW_SOURCE_FALLBACK="0",
        )
        subprocess.run(["sh", str(installer)], env=env, check=True, timeout=900)
        installed = version(
            subprocess.check_output([str(binary), "--version"], text=True, timeout=30)
        )
        if installed != version(tag):
            raise RuntimeError("Installed binary does not match the selected release")


def job(python, runner, binary, installer, log, path):
    return {
        "Label": LABEL,
        "ProgramArguments": [
            python,
            str(runner),
            "run",
            "--binary",
            str(binary),
            "--installer",
            str(installer),
        ],
        "StartInterval": 86400,
        "RunAtLoad": True,
        "StandardOutPath": str(log),
        "StandardErrorPath": str(log),
        "EnvironmentVariables": {"PATH": path},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["enable", "disable", "status", "run"])
    parser.add_argument("--binary", type=Path, default=Path.home() / ".local/bin/labby")
    parser.add_argument(
        "--installer", type=Path, default=Path(__file__).with_name("install.sh")
    )
    args = parser.parse_args()
    if (platform.system(), platform.machine()) != ("Darwin", "arm64"):
        parser.error("Automatic updates currently support macOS Apple Silicon only")
    binary = args.binary.expanduser().absolute()
    state = Path.home() / "Library/Application Support/Labby/auto-update"
    plist = Path.home() / "Library/LaunchAgents" / (LABEL + ".plist")
    domain = f"gui/{os.getuid()}"
    if args.action == "run":
        run(binary, args.installer.resolve())
    elif args.action == "status":
        subprocess.run(["launchctl", "print", f"{domain}/{LABEL}"], check=True)
    elif args.action == "disable":
        if plist.exists():
            subprocess.run(["launchctl", "bootout", domain, str(plist)], check=True)
            plist.unlink()
        print("Automatic Labby updates disabled.")
    else:
        version(
            subprocess.check_output([str(binary), "--version"], text=True, timeout=30)
        )
        if not shutil.which("gh"):
            parser.error("Install GitHub CLI (gh) for release attestation verification")
        state.mkdir(parents=True, exist_ok=True)
        plist.parent.mkdir(parents=True, exist_ok=True)
        runner = state / "auto-update.py"
        installer = state / "install.sh"
        for source, destination in [
            (Path(__file__).resolve(), runner),
            (args.installer.resolve(), installer),
        ]:
            if source != destination:
                shutil.copyfile(source, destination)
        if plist.exists():
            subprocess.run(["launchctl", "bootout", domain, str(plist)], check=True)
        log = state / "update.log"
        plist.write_bytes(
            plistlib.dumps(
                job(sys.executable, runner, binary, installer, log, os.environ["PATH"])
            )
        )
        print(f"Enabling daily updates for {binary}. Log: {log}", flush=True)
        subprocess.run(["launchctl", "bootstrap", domain, str(plist)], check=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(f"labby auto-update: {error}")
