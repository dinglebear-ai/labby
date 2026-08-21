#!/usr/bin/env python3
"""Fail pull requests that touch protected historical documentation without approval."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

PROTECTED_PREFIXES = ("docs/sessions/", "docs/superpowers/")
DEFAULT_APPROVAL_LABEL = "protected-docs-approved"
MAX_PR_FILES = 3000


def protected_paths(paths: list[str]) -> list[str]:
    return sorted(path for path in paths if path.startswith(PROTECTED_PREFIXES))


def fetch_pull_request_files(repo: str, pr_number: int, token: str) -> list[str]:
    api = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
    repo_path = urllib.parse.quote(repo, safe="/")
    files: list[str] = []
    page = 1
    while True:
        url = f"{api}/repos/{repo_path}/pulls/{pr_number}/files?per_page=100&page={page}"
        request = urllib.request.Request(
            url,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {token}",
                "User-Agent": "labby-protected-doc-guard",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
        if not isinstance(payload, list):
            raise RuntimeError("GitHub pull-request files response was not a list")
        page_files = [item.get("filename") for item in payload if isinstance(item, dict)]
        files.extend(path for path in page_files if isinstance(path, str))
        if len(files) >= MAX_PR_FILES:
            raise RuntimeError(
                f"pull request reaches GitHub's {MAX_PR_FILES}-file API cap; "
                "cannot prove protected paths are absent"
            )
        if len(payload) < 100:
            return files
        page += 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--pr-number", required=True, type=int)
    parser.add_argument("--approved", required=True, choices=("true", "false"))
    parser.add_argument("--approval-label", default=DEFAULT_APPROVAL_LABEL)
    args = parser.parse_args()

    if args.approved == "true":
        print(f"protected-docs: explicit approval present via {args.approval_label}")
        return 0

    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        print("protected-docs: GITHUB_TOKEN is required", file=sys.stderr)
        return 2

    try:
        protected = protected_paths(fetch_pull_request_files(args.repo, args.pr_number, token))
    except (OSError, RuntimeError, urllib.error.URLError) as exc:
        print(f"protected-docs: failed to inspect pull request files: {exc}", file=sys.stderr)
        return 2

    if not protected:
        print("protected-docs: no protected historical/work-product paths changed")
        return 0

    print(
        f"::error title=Protected documentation requires approval::Changes under docs/sessions or docs/superpowers require the maintainer label {args.approval_label}."
    )
    print("protected-docs: blocked paths:", file=sys.stderr)
    for path in protected:
        print(f"  {json.dumps(path)}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
