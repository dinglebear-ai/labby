#!/usr/bin/env python3
"""Validate the repository workflow shape on a GitHub-hosted runner."""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

import yaml


SHA = re.compile(r"^[0-9a-f]{40}$")
DOWNLOAD_ARTIFACT_SHA = "d3f86a106a0bac45b974a628896c90dbdf5c8093"
NEEDS_OUTPUT = re.compile(r"needs\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z_][A-Za-z0-9_-]*)")
NEEDS_RESULT = re.compile(r"needs\.([A-Za-z0-9_-]+)\.result")
# Jobs allowed on the organization's privileged self-hosted fleet, as
# (workflow file, job id). Each one is additionally required to be
# non-blocking and to refuse fork pull requests.
FARM_ROUTED_JOBS = {("ci.yml", "test")}
SAME_REPOSITORY_GUARD = "github.event.pull_request.head.repo.full_name == github.repository"


def external_use_errors(path: pathlib.Path, use: str) -> list[str]:
    errors = []
    if use.startswith("./"):
        return errors
    if use.startswith("docker://"):
        if "@sha256:" not in use:
            errors.append(f"{path}: mutable container action {use}")
        return errors
    if "@" not in use or not SHA.fullmatch(use.rsplit("@", 1)[1]):
        errors.append(f"{path}: mutable external action {use}")
    if use.startswith("actions/download-artifact@") and use != f"actions/download-artifact@{DOWNLOAD_ARTIFACT_SHA}":
        errors.append(
            f"{path}: actions/download-artifact must use reviewed revision "
            f"{DOWNLOAD_ARTIFACT_SHA}, found {use}"
        )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-file-regex", required=True)
    parser.add_argument("--allow-arm64", action="store_true")
    args = parser.parse_args()

    release_re = re.compile(args.release_file_regex)
    errors: list[str] = []
    root = pathlib.Path(".github/workflows")

    for path in sorted((*root.glob("*.yml"), *root.glob("*.yaml"))):
        text = path.read_text()
        data = yaml.load(text, Loader=yaml.BaseLoader) or {}
        is_release = bool(release_re.search(path.as_posix()))

        if "permissions" not in data:
            errors.append(f"{path}: missing top-level permissions")
        if not args.allow_arm64 and re.search(
            r"(?i)\b(arm64|aarch64|linux/arm64|setup-qemu)\b", text
        ):
            errors.append(f"{path}: ARM/QEMU contract is forbidden")

        for match in re.finditer(r"(?m)^\s*uses:\s*([^#\s]+)", text):
            use = match.group(1)
            errors.extend(external_use_errors(path, use))

        jobs = data.get("jobs", {})
        for name, job in jobs.items():
            if not isinstance(job, dict) or "uses" in job:
                continue
            if "timeout-minutes" not in job:
                errors.append(f"{path}:{name}: missing timeout-minutes")
            runner = str(job.get("runs-on", ""))
            farm_routed = "self-hosted" in runner or "ci-pool-" in runner
            if farm_routed and (path.name, name) not in FARM_ROUTED_JOBS:
                errors.append(f"{path}:{name}: self-hosted runner selector {runner}")
            elif farm_routed:
                # The farm is privileged (DinD on an Unraid host). A sanctioned
                # job must neither gate a merge nor execute fork-PR code.
                if job.get("continue-on-error") != "true":
                    errors.append(f"{path}:{name}: farm-routed job must set continue-on-error")
                if SAME_REPOSITORY_GUARD not in str(job.get("if", "")):
                    errors.append(f"{path}:{name}: farm-routed job must refuse fork pull requests")
            if is_release and farm_routed:
                errors.append(f"{path}:{name}: heavy release job is farm-routed")

            if "always()" in str(job.get("if", "")) and NEEDS_RESULT.search(str(job)):
                body = str(job)
                inspected = set(NEEDS_RESULT.findall(body))
                consumed = {producer for producer, _ in NEEDS_OUTPUT.findall(body)}
                needs = job.get("needs") or []
                if isinstance(needs, str):
                    needs = [needs]
                for dependency in needs:
                    if dependency not in inspected and dependency not in consumed:
                        errors.append(
                            f"{path}:{name}: aggregate gate needs `{dependency}` but never"
                            f" reads needs.{dependency}.result"
                        )

        for producer, key in sorted(set(NEEDS_OUTPUT.findall(text))):
            job = jobs.get(producer)
            if not isinstance(job, dict) or "uses" in job:
                continue
            if key not in (job.get("outputs") or {}):
                errors.append(
                    f"{path}: needs.{producer}.outputs.{key} is never declared by job"
                    f" `{producer}`"
                )

    if errors:
        print("\n".join(f"::error::{error}" for error in errors))
        return 1
    print(
        "Workflow policy satisfied: every job is GitHub-hosted except the "
        f"{len(FARM_ROUTED_JOBS)} sanctioned farm-routed job(s), which are "
        "non-blocking and refuse fork pull requests."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
