#!/usr/bin/env python3
"""Reject broken local links and known stale OpenWiki contracts."""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WIKI = ROOT / "openwiki"
STALE = {
    "<service>_<action>": "MCP uses one action-dispatched tool per service",
    "LAB_LOG": "the logging prefix is LABBY_LOG",
    "LAB_SERVER_": "HTTP bind variables use LABBY_MCP_HTTP_*",
    "Gateway polls store": "gateway mutation/reload is explicit",
    "cargo run --all-features --": "workspace commands must select package labby",
}


def main() -> int:
    errors: list[str] = []
    for document in sorted(WIKI.glob("*.md")):
        text = document.read_text()
        for stale, explanation in STALE.items():
            if stale in text:
                errors.append(f"{document.relative_to(ROOT)}: stale `{stale}` ({explanation})")
        for target in re.findall(r"\[[^]]*\]\(([^)]+)\)", text):
            target = target.split("#", 1)[0]
            if not target or "://" in target or target.startswith("mailto:"):
                continue
            resolved = (document.parent / target).resolve()
            if not resolved.exists():
                errors.append(f"{document.relative_to(ROOT)}: broken local link `{target}`")
    if errors:
        print("\n".join(errors))
        return 1
    print(f"validated {len(list(WIKI.glob('*.md')))} OpenWiki documents")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
