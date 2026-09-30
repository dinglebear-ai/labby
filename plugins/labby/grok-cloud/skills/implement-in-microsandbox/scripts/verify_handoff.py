#!/usr/bin/env python3
"""Verify a narrow, retained, loopback-only staging handoff.

This checks receipt consistency and bytes, not the truth of human-entered test
claims. Preserve raw command receipts and independently verify the published
Git ref. It is not a generic deployment tool or a security boundary.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, build_opener

SHA = re.compile(r"[0-9a-f]{40}")
DIGEST = re.compile(r"[0-9a-f]{64}")
NAME = re.compile(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}")

def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def argv(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(isinstance(x,str) and x and "\x00" not in x for x in value)

def validate(manifest: object, artifact: Path) -> list[str]:
    errors: list[str] = []
    def require(condition: bool, message: str) -> None:
        if not condition: errors.append(message)
    if not isinstance(manifest, dict): return ["manifest must be an object"]
    require(type(manifest.get("schema_version")) is int and manifest.get("schema_version") == 1, "schema_version must be 1")
    require(manifest.get("environment") == "staging", "only staging is supported")
    require("snapshot" not in manifest, "snapshots are checkpoints, not release inputs")
    source = manifest.get("source")
    if not isinstance(source,dict): source={}; errors.append("source must be an object")
    require(isinstance(source.get("repository"),str) and bool(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+",source.get("repository",""))), "repository must be owner/name")
    require(isinstance(source.get("branch"),str) and bool(source.get("branch")), "branch is required")
    commit=source.get("commit")
    require(isinstance(commit,str) and bool(SHA.fullmatch(commit)), "commit must be a full lowercase Git SHA")
    require(source.get("published_commit") == commit and bool(commit), "published ref must match the source commit")
    require(source.get("dirty") is False, "source tree must be explicitly clean")
    release=manifest.get("artifact")
    if not isinstance(release,dict): release={}; errors.append("artifact must be an object")
    require(release.get("kind") in ("workflow-fixture","application"), "artifact kind must distinguish fixture from application")
    expected=release.get("sha256")
    require(isinstance(expected,str) and bool(DIGEST.fullmatch(expected)), "artifact SHA-256 is required")
    try:
        if artifact.is_symlink() or not artifact.is_file(): errors.append("artifact must be a regular, non-symlink file")
        elif digest(artifact) != expected: errors.append("artifact SHA-256 mismatch")
    except OSError: errors.append("artifact could not be read")
    sb=manifest.get("sandbox")
    if not isinstance(sb,dict): sb={}; errors.append("sandbox must be an object")
    for key in ("name","development_name"):
        value=sb.get(key)
        require(isinstance(value,str) and bool(NAME.fullmatch(value)), key + " must be an explicit sandbox name")
    require(sb.get("name") != sb.get("development_name"), "development and staging must differ")
    image=sb.get("image")
    require(isinstance(image,str) and bool(re.fullmatch(r"[a-zA-Z0-9./:_-]+@sha256:[0-9a-f]{64}",image)), "image must be digest-pinned")
    for key in ("cpus","memory_mib"):
        require(type(sb.get(key)) is int and sb[key]>0, key + " must be a positive integer")
    workdir=sb.get("workdir")
    require(isinstance(workdir,str) and workdir.startswith("/") and workdir != "/" and ".." not in PurePosixPath(workdir).parts, "workdir must be explicit and absolute without traversal")
    require(argv(sb.get("command")), "command must be a nonempty argument list")
    require(sb.get("persistent") is True, "staging must be persistent")
    for key in ("max_duration_secs","idle_timeout_secs"):
        require(key in sb and sb[key] is None, key + " must explicitly disable expiry")
    require(sb.get("mounts") == [], "this reference contract does not accept host mounts")
    require(sb.get("env") == {}, "this reference contract does not accept environment values")
    secret_names=sb.get("secret_names")
    require(isinstance(secret_names,list) and all(isinstance(x,str) and bool(re.fullmatch(r"[A-Z_][A-Z0-9_]*",x)) for x in secret_names), "secret_names must contain names only")
    ports=sb.get("ports")
    if not isinstance(ports,list) or not ports: ports=[]; errors.append("explicit port mapping is required")
    host_ports=set()
    for port in ports:
        if not isinstance(port,dict): errors.append("port must be an object"); continue
        require(port.get("host_bind") == "127.0.0.1", "host listener must use 127.0.0.1")
        for key in ("host_port","guest_port"):
            require(type(port.get(key)) is int and 1 <= port[key] <= 65535, key + " is invalid")
        hp=port.get("host_port")
        if type(hp) is int:
            require(hp not in host_ports,"duplicate host port"); host_ports.add(hp)
    checks=manifest.get("checks")
    if not isinstance(checks,list): checks=[]; errors.append("checks must be a list")
    seen=set()
    for check in checks:
        if not isinstance(check,dict): errors.append("check must be an object"); continue
        name=check.get("name")
        if isinstance(name,str):
            require(name not in seen,"duplicate check name"); seen.add(name)
        require(check.get("passed") is True,"every declared check must have passed")
        require(argv(check.get("command")),"check command must be an argument list")
        if name=="tests": require(type(check.get("count")) is int and check["count"]>0,"at least one test must execute")
    require({"tests","docs"} <= seen,"tests and docs checks are required")
    try:
        u=urlsplit(manifest.get("health_url",""))
        require(u.scheme=="http" and u.hostname=="127.0.0.1" and u.port in host_ports and u.path=="/healthz" and not u.username and not u.password and not u.query and not u.fragment,"health URL must match a mapped loopback listener")
    except (ValueError,TypeError,AttributeError): errors.append("health URL is invalid")
    return errors

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl): return None

def probe(manifest: dict) -> list[str]:
    """Call only after validate(); never follow redirects or ambient proxies."""
    try:
        opener=build_opener(ProxyHandler({}),NoRedirect())
        with opener.open(manifest["health_url"],timeout=5) as response:
            body=response.read(65537)
            if response.status != 200 or len(body)>65536: return ["external health response failed its bounds"]
            observed=json.loads(body)
        if not isinstance(observed,dict): return ["external health must return an object"]
        expected={"environment":"staging", "sandbox":manifest["sandbox"]["name"], "commit":manifest["source"]["commit"], "artifact_sha256":manifest["artifact"]["sha256"]}
        return ["external health identity mismatch: " + key for key,value in expected.items() if observed.get(key)!=value]
    except Exception as error:
        if isinstance(error,HTTPError): error.close()
        # Do not echo a remote response, URL credentials, or arbitrary exception text.
        return ["external health probe failed: " + type(error).__name__]

def main() -> int:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest",type=Path)
    parser.add_argument("--artifact",required=True,type=Path)
    parser.add_argument("--probe",action="store_true",help="Run from the host, outside the staging VM")
    args=parser.parse_args()
    try: manifest=json.loads(args.manifest.read_text())
    except (OSError,ValueError):
        print(json.dumps({"ok":False,"errors":["manifest cannot be read as JSON"]})); return 2
    errors=validate(manifest,args.artifact)
    if not errors and args.probe: errors.extend(probe(manifest))
    print(json.dumps({"ok":not errors,"external_probe_executed":args.probe and not bool(validate(manifest,args.artifact)),"errors":errors},sort_keys=True))
    return int(bool(errors))

if __name__ == "__main__": raise SystemExit(main())
