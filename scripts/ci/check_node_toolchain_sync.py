#!/usr/bin/env python3
"""Keep the Gateway Admin build on its declared Node major."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    package = json.loads((root / "apps/gateway-admin/package.json").read_text())
    declared = package.get("engines", {}).get("node")
    if declared != "22.x":
        print("apps/gateway-admin/package.json: engines.node must be exactly 22.x", file=sys.stderr)
        return 1

    action_path = root / ".github/actions/build-gateway-admin/action.yml"
    action = action_path.read_text()
    versions = re.findall(r"(?m)^\s*node-version:\s*[\"']?([^\s\"']+)", action)
    if versions != ["22"]:
        print(f"{action_path}: expected one node-version: 22, found {versions}", file=sys.stderr)
        return 1

    workflow = (root / ".github/workflows/ci.yml").read_text()
    if "uses: ./.github/actions/build-gateway-admin" not in workflow:
        print("ci.yml: Gateway Admin must build through build-gateway-admin", file=sys.stderr)
        return 1
    print("Gateway Admin Node contract is synchronized at Node 22.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
