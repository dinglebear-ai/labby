#!/usr/bin/env python3
"""Qualify an attested release on an explicitly disposable, fresh native machine."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import signal
import stat
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

CHECKS = {"gateway_authenticated", "agent_provider", "agent_run", "selected_clients", "catalog_search", "mcp_tool_call"}
STAGES = ("bootstrap", "agent", "clients", "discover", "mcp")


class QualificationError(Exception):
    pass


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise QualificationError("authenticated readiness redirects are forbidden")


def read_private_token(path: Path) -> str:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 16_384:
            raise QualificationError("readiness token must be a bounded regular file")
        if info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise QualificationError("readiness token must be owned by the invoking user and private")
        token = os.read(descriptor, 16_385).decode().strip()
        if not token or any(character.isspace() for character in token):
            raise QualificationError("readiness token is invalid")
        return token
    finally:
        os.close(descriptor)


def validate_readiness(state: object, started_at: int, clients: list[str], now: int) -> dict:
    if not isinstance(state, dict) or state.get("ready") is not True:
        raise QualificationError("required first-use checks are not ready")
    checks = state.get("checks")
    if not isinstance(checks, list) or len(checks) != len(CHECKS):
        raise QualificationError("first-use evidence is incomplete")
    found = set()
    result = {}
    for item in checks:
        if not isinstance(item, dict) or not isinstance(item.get("check"), str) or item["check"] not in CHECKS or item["check"] in found:
            raise QualificationError("first-use evidence contains invalid or repeated checks")
        name = item["check"]
        found.add(name)
        if name == "selected_clients" and not clients and item.get("status") == "deferred":
            result[name] = {"status": "deferred", "reason": "no external clients selected"}
            continue
        timestamp = item.get("verified_at")
        if item.get("status") != "verified" or not isinstance(timestamp, int) or isinstance(timestamp, bool):
            raise QualificationError(f"{name} has no verified result")
        if not started_at <= timestamp <= now + 5:
            raise QualificationError(f"{name} proof is stale or has an invalid timestamp")
        if not isinstance(item.get("resource_id"), str) or not item["resource_id"].strip():
            raise QualificationError(f"{name} has no concrete result")
        if name == "catalog_search" and item["resource_id"] != "public":
            raise QualificationError("the default public catalog did not earn verified search evidence")
        # Resource IDs can identify private servers, users, or sessions. Reports
        # retain status and timestamp, never credentials or private identifiers.
        result[name] = {"status": "verified", "verified_at": timestamp}
    return result


def command(argv: list[str], environment: dict[str, str], deadline: float) -> None:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise QualificationError("qualification deadline exceeded")
    # Do not persist driver/provider output or credential-bearing arguments.
    process = subprocess.Popen(argv, env=environment, stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, start_new_session=True)
    try:
        code = process.wait(timeout=remaining)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise QualificationError("qualification stage timed out") from None
    if code:
        raise QualificationError(f"qualification stage exited with status {code}")


def download(url: str, path: Path, limit: int, deadline: float) -> None:
    timeout = min(30, deadline - time.monotonic())
    if timeout <= 0:
        raise QualificationError("qualification deadline exceeded")
    with urllib.request.urlopen(url, timeout=timeout) as response:
        endpoint = urllib.parse.urlsplit(response.geturl())
        if endpoint.scheme != "https" or endpoint.hostname not in {
            "github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"
        }:
            raise QualificationError("release download left the trusted HTTPS delivery hosts")
        with path.open("xb") as output:
            written = 0
            while chunk := response.read(65536):
                written += len(chunk)
                if written > limit or time.monotonic() > deadline:
                    raise QualificationError("release download exceeded size or time limit")
                output.write(chunk)


def validate_plan(plan: object) -> dict:
    if not isinstance(plan, dict):
        raise QualificationError("qualification plan must be an object")
    conditions = plan.get("conditions")
    required = {"fresh_machine", "cold_caches", "provider_access_ready", "native_service", "public_catalog", "provider_cost_accepted"}
    if not isinstance(conditions, dict) or not required <= set(conditions) or set(conditions) - required - {"external_client_auth_ready", "external_client_model_ready"} or any(conditions.get(key) is not True for key in required) or any(not isinstance(value, bool) for value in conditions.values()):
        raise QualificationError("plan must explicitly declare supported fresh-machine starting conditions")
    clients = plan.get("selected_clients")
    if not isinstance(clients, list) or any(not isinstance(client, str) or client not in {"codex", "claude-code"} for client in clients) or len(set(clients)) != len(clients):
        raise QualificationError("selected_clients must list supported clients, or be empty")
    if clients and any(conditions.get(key) is not True for key in ("external_client_auth_ready", "external_client_model_ready")):
        raise QualificationError("selected clients require explicit authenticated-client and chosen-model starting conditions")
    stages = plan.get("stages")
    if stages is None:
        driver = str(Path(__file__).with_name("first-use-native-driver.py").resolve())
        stages = {stage: [sys.executable, driver, stage] for stage in STAGES}
        plan["stages"] = stages
    if not isinstance(stages, dict) or set(stages) != set(STAGES):
        raise QualificationError("plan must provide exactly bootstrap, agent, clients, discover, and mcp drivers")
    for argv in stages.values():
        if not isinstance(argv, list) or not argv or any(not isinstance(arg, str) or not arg or "\0" in arg for arg in argv):
            raise QualificationError("each driver must be a nonempty argv array; shell strings are unsupported")
    for key in ("state_root", "invoking_state_root", "install_dir", "token_file"):
        value = plan.get(key)
        if not isinstance(value, str) or not Path(value).is_absolute():
            raise QualificationError(f"{key} must be an absolute path")
    endpoint = urllib.parse.urlsplit(plan.get("gateway_url", ""))
    if endpoint.scheme not in {"https", "http"} or not endpoint.hostname or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
        raise QualificationError("gateway_url must be a clean HTTP(S) origin")
    if endpoint.scheme == "http" and endpoint.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise QualificationError("bearer readiness verification requires HTTPS or loopback")
    if endpoint.path not in {"", "/"}:
        raise QualificationError("gateway_url must be an origin without a path")
    return plan


def fresh_targets(plan: dict) -> None:
    invoking = Path.home() / ".labby"
    expected_server = Path("/home/labby/.labby") if sys.platform.startswith("linux") else invoking
    if Path(plan["invoking_state_root"]) != invoking or Path(plan["state_root"]) != expected_server:
        raise QualificationError("native qualification must name the actual native service and invoking-user state roots")
    if urllib.parse.urlsplit(plan["gateway_url"]).hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise QualificationError("native clean-machine qualification requires the local gateway origin")
    for key in ("state_root", "invoking_state_root", "install_dir"):
        path = Path(plan[key])
        if path.is_symlink() or path.exists():
            raise QualificationError(f"{key} already exists; use a fresh disposable machine")
    if Path(plan["token_file"]).exists() or Path(plan["token_file"]).is_symlink():
        raise QualificationError("token_file must be produced by this qualification run")


def installed_release(binary: Path, tag: str) -> str:
    receipt = binary.parent / ".labby-install" / "receipt"
    descriptor = os.open(receipt, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        info = os.fstat(descriptor)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 16_384 or info.st_mode & 0o077:
            raise QualificationError("activated release receipt is invalid")
        entries = {}
        for line in os.read(descriptor, 16_385).decode().splitlines():
            key, separator, value = line.partition("=")
            if not separator or key in entries:
                raise QualificationError("activated release receipt is malformed")
            entries[key] = value
    finally:
        os.close(descriptor)
    if entries.get("format") != "1" or entries.get("source") != "release" or entries.get("resolved_version") != tag:
        raise QualificationError("activated binary is not the requested attested release")
    digest = hashlib.sha256()
    with binary.open("rb") as candidate:
        while chunk := candidate.read(1024 * 1024):
            digest.update(chunk)
    actual = digest.hexdigest()
    if entries.get("sha256") != actual:
        raise QualificationError("activated release binary does not match its receipt")
    return actual


def run(args: argparse.Namespace) -> tuple[dict, int]:
    plan = validate_plan(json.loads(args.plan.read_text()))
    if not args.disposable_machine:
        raise QualificationError("--disposable-machine is required: installer and drivers change native service/client state")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repo) or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", args.tag):
        raise QualificationError("repo or immutable release tag is invalid")
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        raise QualificationError("native qualification supports macOS and Linux only")
    fresh_targets(plan)
    started_at = int(time.time())
    started = time.monotonic()  # Before the first download, including verifier work.
    deadline = started + args.timeout_seconds
    report = {"schema_version": 1, "repo": args.repo, "tag": args.tag,
              "platform": {"system": platform.system(), "machine": platform.machine()},
              "journey": "native_api_cli", "browser_ui_qualified": False,
              "started_at": started_at, "conditions": plan["conditions"],
              "selected_clients": plan["selected_clients"], "provider_protocol": os.environ.get("LABBY_QUALIFICATION_PROVIDER_PROTOCOL", "openai") if os.environ.get("LABBY_QUALIFICATION_PROVIDER_PROTOCOL", "openai") in {"openai", "phoenix"} else "invalid", "completion": False,
              "within_budget": False, "budget_seconds": args.budget_seconds,
              "prerequisites": ["Python 3.9 or newer", "native installer prerequisites"],
              "account_free": True, "standalone_bootstrap": True, "stages": []}
    environment = os.environ.copy()
    environment.update({"LABBY_INSTALL_DIR": plan["install_dir"], "LABBY_INSTALL_REPO": args.repo,
                        "LABBY_INSTALL_VERSION": args.tag, "LABBY_INSTALL_NO_SETUP": "1",
                        "LABBY_ALLOW_SOURCE_FALLBACK": "0"})
    # Reject lifecycle/local-candidate escapes inherited from the invoking shell.
    for key in ("GH_TOKEN", "GITHUB_TOKEN", "GH_HOST", "LABBY_INSTALL_LOCAL_BINARY", "LABBY_INSTALL_LOCAL_SHA256",
                "LABBY_INSTALL_RECOVER_ONLY", "LABBY_INSTALL_ROLLBACK", "LABBY_SKIP_SETUP", "LABBY_HOME"):
        environment.pop(key, None)
    stage = "download_verify_install"
    stage_started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="labby-release-qualification-") as directory:
            work = Path(directory)
            environment["GH_CONFIG_DIR"] = str(work / "gh-config")
            environment["XDG_CACHE_HOME"] = str(work / "cold-cache")
            Path(environment["GH_CONFIG_DIR"]).mkdir(mode=0o700)
            verifier_result = work / "verifier-path"
            helper = Path(__file__).with_name("github-verifier-bootstrap.sh").resolve()
            command(["sh", "-c", 'set -eu; . "$1"; ensure_github_verifier "$2"; printf "%s\\n" "$GH_VERIFIER" > "$3"', "verifier-bootstrap", str(helper), str(work), str(verifier_result)], environment, deadline)
            verifier = verifier_result.read_text().strip()
            if not Path(verifier).is_absolute() or not Path(verifier).is_file():
                raise QualificationError("verified bootstrap did not produce a verifier")
            report["verifier_minimum_version"] = "2.102.0"
            report["verifier_method"] = "embedded_digest_bootstrap" if Path(verifier).is_relative_to(work) else "trusted_existing_verifier"
            base = f"https://github.com/{args.repo}/releases/download/{args.tag}"
            installer = work / "labby-install.sh"
            checksum = work / "labby-install.sh.sha256"
            bundle = work / "labby-install.sh.sigstore.jsonl"
            for path, limit in ((installer, 4 * 1024 * 1024), (checksum, 4096), (bundle, 16 * 1024 * 1024)):
                download(f"{base}/{path.name}", path, limit, deadline)
            match = re.fullmatch(r"([0-9a-fA-F]{64})\s+\*?labby-install\.sh\s*", checksum.read_text())
            if not match or hashlib.sha256(installer.read_bytes()).hexdigest() != match[1].lower():
                raise QualificationError("released installer checksum verification failed")
            command([verifier, "attestation", "verify", str(installer), "--bundle", str(bundle),
                     "--hostname", "github.com", "--repo", args.repo,
                     "--signer-workflow", f"{args.repo}/.github/workflows/release.yml",
                     "--source-ref", f"refs/tags/{args.tag}", "--deny-self-hosted-runners"], environment, deadline)
            report["installer_sha256"] = match[1].lower()
            command(["sh", str(installer)], environment, deadline)
            binary = Path(plan["install_dir"]) / "labby"
            if not binary.is_file() or binary.is_symlink():
                raise QualificationError("release installer did not activate a regular Labby binary")
            report["binary_sha256"] = installed_release(binary, args.tag)
            report["stages"].append({"stage": stage, "status": "passed", "seconds": round(time.monotonic() - stage_started, 3)})
            environment["LABBY_QUALIFICATION_BINARY"] = str(binary)
            environment["LABBY_QUALIFICATION_GATEWAY_URL"] = plan["gateway_url"]
            environment["LABBY_QUALIFICATION_TOKEN_FILE"] = plan["token_file"]
            environment["LABBY_QUALIFICATION_SELECTED_CLIENTS"] = json.dumps(plan["selected_clients"])
            for stage in STAGES:
                stage_started = time.monotonic()
                command(plan["stages"][stage], environment, deadline)
                report["stages"].append({"stage": stage, "status": "passed", "seconds": round(time.monotonic() - stage_started, 3)})
            stage = "authenticated_readiness"
            stage_started = time.monotonic()
            if time.monotonic() >= deadline:
                raise QualificationError("qualification deadline exceeded")
            token = read_private_token(Path(plan["token_file"]))
            request = urllib.request.Request(plan["gateway_url"].rstrip("/") + "/v1/setup",
                data=json.dumps({"action": "readiness.state", "params": {}}).encode(),
                headers={"Content-Type": "application/json", "Authorization": "Bearer " + token})
            with urllib.request.build_opener(NoRedirect()).open(request, timeout=max(0.1, min(15, deadline - time.monotonic()))) as response:
                raw = response.read(128 * 1024 + 1)
                if len(raw) > 128 * 1024:
                    raise QualificationError("readiness response exceeded its size limit")
                state = json.loads(raw)
            if time.monotonic() >= deadline:
                raise QualificationError("qualification deadline exceeded")
            report["checks"] = validate_readiness(state, started_at, plan["selected_clients"], int(time.time()))
            report["stages"].append({"stage": stage, "status": "passed", "seconds": round(time.monotonic() - stage_started, 3)})
            report["completion"] = True
    except Exception as error:
        # Exceptions from network/JSON/provider code may include private values.
        # Only bounded, product-owned error messages are written to the report.
        message = str(error) if isinstance(error, QualificationError) else type(error).__name__
        report["stages"].append({"stage": stage, "status": "failed", "seconds": round(time.monotonic() - stage_started, 3), "reason": message})
    report["elapsed_seconds"] = round(time.monotonic() - started, 3)
    report["within_budget"] = report["completion"] and report["elapsed_seconds"] <= args.budget_seconds
    return report, 0 if report["within_budget"] else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", default="dinglebear-ai/labby")
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--disposable-machine", action="store_true")
    parser.add_argument("--budget-seconds", type=int, default=300)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    if not 1 <= args.budget_seconds <= args.timeout_seconds <= 3600:
        parser.error("require 1 <= budget-seconds <= timeout-seconds <= 3600")
    try:
        report, code = run(args)
    except Exception as error:
        message = str(error) if isinstance(error, QualificationError) else type(error).__name__
        report, code = {"schema_version": 1, "completion": False, "within_budget": False, "preflight_failure": message}, 1
    args.report.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(args.report, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(report, output, indent=2)
        output.write("\n")
    print("First-use qualification passed" if not code else "First-use qualification failed; see the private report")
    return code


if __name__ == "__main__":
    sys.exit(main())
