#!/usr/bin/env python3
"""Fail-closed end-to-end canary for public OAuth callback relay cutovers."""

from __future__ import annotations

import argparse
import hashlib
import http.server
import ipaddress
import json
import os
import secrets
import socketserver
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


NO_REDIRECT_OPENER = urllib.request.build_opener(NoRedirect)


def validate_admin_base(raw: str) -> str:
    parsed = urllib.parse.urlsplit(raw)
    if (
        not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise RuntimeError("admin base must be an absolute origin without credentials, query, or fragment")
    is_loopback = parsed.hostname == "localhost"
    if not is_loopback:
        try:
            is_loopback = ipaddress.ip_address(parsed.hostname).is_loopback
        except ValueError:
            pass
    if parsed.scheme != "https" and not (parsed.scheme == "http" and is_loopback):
        raise RuntimeError("admin base must use HTTPS except for explicit loopback HTTP")
    return raw.rstrip("/")


class CanaryTarget(http.server.BaseHTTPRequestHandler):
    expected_machine = ""
    expected_code = ""
    expected_state = ""
    received = threading.Event()

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler contract
        parsed = urllib.parse.urlsplit(self.path)
        query = urllib.parse.parse_qs(parsed.query, strict_parsing=True)
        ok = (
            parsed.path == f"/callback/{self.expected_machine}"
            and query.get("code") == [self.expected_code]
            and query.get("state") == [self.expected_state]
        )
        body = json.dumps(
            {
                "canary": "oauth-relay",
                "ok": ok,
                "state_sha256": hashlib.sha256(self.expected_state.encode()).hexdigest(),
            },
            separators=(",", ":"),
        ).encode()
        self.send_response(200 if ok else 422)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        if ok:
            self.received.set()

    def log_message(self, _format: str, *_args: object) -> None:
        return


class ThreadingServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True


def request_json(
    url: str,
    *,
    method: str = "GET",
    token: str | None = None,
    payload: dict[str, object] | None = None,
    timeout: float = 10.0,
) -> tuple[int, object]:
    headers = {"Accept": "application/json"}
    data = None
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if payload is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(payload, separators=(",", ":")).encode()
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with NO_REDIRECT_OPENER.open(request, timeout=timeout) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            body: object = json.loads(raw) if raw else None
        except json.JSONDecodeError:
            body = None
        return error.code, body


def run(args: argparse.Namespace) -> dict[str, object]:
    admin_base = validate_admin_base(args.admin_base)
    token = os.environ.get(args.admin_token_env)
    if not token:
        raise RuntimeError(f"required environment variable {args.admin_token_env} is unset")

    machine = f"canary-{secrets.token_hex(8)}"
    code = secrets.token_urlsafe(24)
    state = secrets.token_urlsafe(32)
    CanaryTarget.expected_machine = machine
    CanaryTarget.expected_code = code
    CanaryTarget.expected_state = state
    CanaryTarget.received = threading.Event()
    server = ThreadingServer((args.bind_host, args.target_port), CanaryTarget)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    admin_machine = f"{admin_base}/v1/oauth/relay/machines/{machine}"
    public_callback = f"{args.public_base.rstrip('/')}/callback/{machine}"
    target_url = f"http://{args.target_host}:{args.target_port}/callback/{machine}"
    primary_error: BaseException | None = None
    cleanup_error: BaseException | None = None
    try:
        status, _ = request_json(
            admin_machine,
            method="PUT",
            token=token,
            payload={"target_url": target_url, "description": f"run-owned {args.phase} canary"},
            timeout=args.timeout,
        )
        if status != 200:
            raise RuntimeError(f"canary registration failed with HTTP {status}")
        query = urllib.parse.urlencode({"code": code, "state": state})
        status, body = request_json(f"{public_callback}?{query}", timeout=args.timeout)
        expected_hash = hashlib.sha256(state.encode()).hexdigest()
        if status != 200 or body != {
            "canary": "oauth-relay",
            "ok": True,
            "state_sha256": expected_hash,
        }:
            raise RuntimeError(f"public callback canary failed with HTTP {status}")
        if not CanaryTarget.received.wait(args.timeout):
            raise RuntimeError("target did not record exact callback delivery")
    except BaseException as error:
        primary_error = error
    finally:
        # Always remove the unique identity, even when registration timed out
        # after committing but before its response arrived. DELETE is allowed
        # to report an already-absent identity; the following GET is the
        # authoritative residual audit.
        try:
            status, _ = request_json(
                admin_machine, method="DELETE", token=token, timeout=args.timeout
            )
            if status not in (200, 404):
                raise RuntimeError(f"canary cleanup failed with HTTP {status}")
            status, _ = request_json(admin_machine, token=token, timeout=args.timeout)
            if status != 404:
                raise RuntimeError(f"canary residual audit returned HTTP {status}, expected 404")
        except BaseException as error:
            cleanup_error = error
        server.shutdown()
        server.server_close()
        thread.join(timeout=args.timeout)

    if cleanup_error:
        raise RuntimeError(f"cleanup/residual audit failed: {cleanup_error}") from primary_error
    if primary_error:
        raise primary_error
    return {
        "status": "passed",
        "phase": args.phase,
        "machine_removed": True,
        "exact_delivery": True,
        "exact_response": True,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True, choices=("pre-cutover", "post-cutover", "post-rollback"))
    parser.add_argument("--public-base", required=True)
    parser.add_argument("--admin-base", required=True)
    parser.add_argument("--target-host", required=True)
    parser.add_argument("--bind-host", default="0.0.0.0")
    parser.add_argument("--target-port", type=int, default=38935)
    parser.add_argument("--admin-token-env", default="LABBY_ADMIN_BEARER_TOKEN")
    parser.add_argument("--timeout", type=float, default=10.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        result = run(args)
    except BaseException as error:
        print(
            json.dumps(
                {
                    "status": "failed",
                    "phase": args.phase,
                    "transition_allowed": False,
                    "rollback_required": args.phase == "post-cutover",
                    "error": str(error),
                },
                separators=(",", ":"),
            )
        )
        return 1
    print(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
