"""Repository-owned bounded, read-only Git identity primitives."""
from typing import Any

import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit

class ContextError(Exception):
    def __init__(self, kind: str, message: str) -> None:
        self.kind = kind
        super().__init__(message)

def compact(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))

def digest(value: object) -> str:
    return hashlib.sha256(value if isinstance(value, bytes) else compact(value).encode()).hexdigest()

def clean_string(value: object, name: str, maximum: int = 4096, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not value and not allow_empty) or len(value) > maximum or re.search(r"[\x00-\x1f\x7f]", value):
        raise ContextError("invalid_input", f"{name} must be a bounded string without control characters")
    return value

def within(path: Path, base: Path) -> bool:
    try:
        path.relative_to(base)
        return True
    except ValueError:
        return False

def git(root: Path, *args: str, optional: bool = False) -> str | None:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(GIT_OPTIONAL_LOCKS="0", GIT_TERMINAL_PROMPT="0", LC_ALL="C")
    try:
        r = subprocess.run(["git", "--no-optional-locks", "-c", "core.fsmonitor=false", "-C", str(root), *args], capture_output=True, env=env, timeout=5)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise ContextError("git_unavailable", "Git inspection failed or exceeded 5 seconds: " + type(e).__name__)
    if r.returncode and not optional:
        raise ContextError("git_failed", "Git inspection failed for " + args[0] + "; check the repository, ownership, and permissions")
    return r.stdout.decode("utf-8", "replace").rstrip("\n") if not r.returncode else None

def remote_info(raw: object) -> dict[str, Any]:
    """Never return embedded credentials or arbitrary credential-bearing URLs."""
    if not isinstance(raw, str) or not raw:
        return {"url": None, "github_repo": None}
    match = re.fullmatch(r"(?:git@github\.com:|ssh://git@github\.com/|https://(?:[^/@]+@)?github\.com/)([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?/?", raw)
    if match:
        name = match.group(1)
        return {"url": "https://github.com/" + name, "github_repo": name}
    try:
        host = urlsplit(raw).hostname
    except ValueError:
        host = None
    return {"url": "[non-GitHub remote redacted]", "hostname": host, "github_repo": None}

def repository_info(target: Path, override: str | None) -> tuple[Path, dict[str, Any]]:
    root_text = git(target, "rev-parse", "--show-toplevel")
    root = Path(root_text).resolve(strict=True)
    head = git(root, "rev-parse", "--verify", "HEAD", optional=True)
    branch = git(root, "symbolic-ref", "--quiet", "--short", "HEAD", optional=True)
    common = git(root, "rev-parse", "--git-common-dir")
    gitdir = git(root, "rev-parse", "--git-dir")
    upstream = git(root, "rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}", optional=True)
    raw_status = git(root, "status", "--porcelain=v1", "-z", "--untracked-files=normal") or ""
    records = raw_status.split("\0")
    changes, i = [], 0
    while i < len(records):
        row = records[i]
        i += 1
        if not row:
            continue
        change = {"status": row[:2], "path": row[3:]}
        if "R" in row[:2] or "C" in row[:2]:
            change["original_path"] = records[i] if i < len(records) else None
            i += 1
        changes.append(change)
    remotes = []
    names = (git(root, "remote") or "").splitlines()
    if len(names) > 8:
        raise ContextError("too_many_remotes", "More than 8 remotes; review this checkout explicitly")
    for name in names:
        fetch = git(root, "remote", "get-url", "--all", name, optional=True) or ""
        push = git(root, "remote", "get-url", "--push", "--all", name, optional=True) or ""
        remotes.append({"name": name, "fetch": [remote_info(x) for x in fetch.splitlines()], "push": [remote_info(x) for x in push.splitlines()]})
    identities = sorted({v["github_repo"] for r in remotes for v in r["fetch"] if v["github_repo"]})
    if override and override not in identities:
        raise ContextError("repository_mismatch", "github_repo must match an observed fetch remote")
    preferred = next((r for r in remotes if upstream and upstream.startswith(r["name"] + "/")), None)
    preferred = preferred or next((r for r in remotes if r["name"] == "origin"), None)
    chosen = override or next((v["github_repo"] for v in (preferred or {}).get("fetch", []) if v["github_repo"]), None)
    chosen = chosen or (identities[0] if len(identities) == 1 else None)
    unraid_push = any((v.get("github_repo") or "").lower().startswith("unraid/") for r in remotes for v in r["push"])
    info = {"root": str(root), "requested_path": str(target), "head": head, "branch": branch, "detached": head is not None and branch is None,
            "git_dir": str((root / gitdir).resolve()), "common_dir": str((root / common).resolve()), "upstream": upstream,
            "dirty": bool(changes), "changes_count": len(changes), "changes": changes[:30], "changes_truncated": len(changes) > 30,
            "status_sha256": digest(raw_status.encode()), "remotes": remotes, "github_repo": chosen,
            "github_url": "https://github.com/" + chosen if chosen else None, "remote_selection": "explicit" if override else "tracking remote, then origin, then unique remote",
            "multiple_github_repositories": len(identities) > 1, "unraid_push_target": unraid_push,
            "network_contacted": False, "remote_refs_freshness": "Local configuration only; no fetch, pull, or remote health check was performed"}
    return root, info

