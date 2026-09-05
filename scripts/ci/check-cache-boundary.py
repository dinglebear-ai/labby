#!/usr/bin/env python3
"""Reject shared-cache credentials outside the protected main capability."""
from __future__ import annotations
import argparse, os

parser = argparse.ArgumentParser()
parser.add_argument("--event", required=True)
parser.add_argument("--ref", required=True)
parser.add_argument("--run-id", required=True)
args = parser.parse_args()
access = os.environ.get("KACHE_S3_ACCESS_KEY", "")
secret = os.environ.get("KACHE_S3_SECRET_KEY", "")
prefix = os.environ.get("KACHE_S3_PREFIX", "")
prefix_enforced = os.environ.get("KACHE_S3_PREFIX_ENFORCED", "") == "true"
if bool(access) != bool(secret):
    raise SystemExit("partial shared-cache credentials are forbidden")
if access and (args.event != "push" or args.ref != "refs/heads/main"):
    raise SystemExit("shared-cache credentials are forbidden outside protected main pushes")
if access and not args.run_id.isdigit():
    raise SystemExit("shared-cache writer requires numeric run-scoped namespace")
if access and not prefix.endswith("/main"):
    raise SystemExit("shared-cache writer requires the protected /main prefix")
if access and not prefix_enforced:
    raise SystemExit("shared-cache writer requires server-enforced prefix capability")
print("shared-cache capability boundary passed")
