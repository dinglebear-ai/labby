#!/usr/bin/env python3
"""Refresh the normative MCP 2026-07-28 authorization denominator."""

import json
import hashlib
import re
import urllib.request
import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASE = "https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/"
PAGES = ["index.md", "authorization-server-discovery.md", "client-registration.md", "security-considerations.md"]


def fetch_denominator() -> dict:
    rows = []
    source_digests = {}
    for page in PAGES:
        url = BASE + page
        text = urllib.request.urlopen(url, timeout=20).read().decode()
        source_digests[url] = hashlib.sha256(text.encode()).hexdigest()
        paragraphs = re.split(r"\n\s*\n", text)
        ordinal = 0
        for paragraph in paragraphs:
            normalized = " ".join(line.strip() for line in paragraph.splitlines())
            if not re.search(r"\b(?:MUST|SHOULD)(?: NOT)?\b", normalized):
                continue
            for clause in re.split(r"(?<=[.!?])\s+|\s+(?=\*\*)", normalized):
                strengths = re.findall(r"\b(MUST NOT|SHOULD NOT|MUST|SHOULD)\b", clause)
                for strength in strengths:
                    ordinal += 1
                    rows.append({
                        "id": f"MCP-2026-AUTH-{page.removesuffix('.md').upper()}-{ordinal:03d}",
                        "source_url": url,
                        "strength": strength.lower().replace(" ", "_"),
                        "requirement": clause[:2000],
                        "applicability": "applicable",
                        "implementation": "Independent normative clause; dedicated behavior mapping remains partial until an executable assertion is recorded.",
                        "evidence_paths": (["crates/labby-auth/src/cimd.rs", "crates/labby-auth/src/authorize.rs"] if page == "client-registration.md" else ["crates/labby-auth/src/metadata.rs", "crates/labby-auth/src/upstream/runtime.rs"] if page == "authorization-server-discovery.md" else ["docs/runtime/OAUTH.md"] if page == "security-considerations.md" else ["crates/labby-auth/src"]),
                        "test_id": f"normative-{page.removesuffix('.md')}-{ordinal:03d}",
                        "status": "partial",
                    })
    return {"protocol_version": "2026-07-28", "sources": [BASE + page for page in PAGES], "source_sha256": source_digests, "requirements": rows}


def denominator_projection(data: dict) -> dict:
    fields = ("id", "source_url", "strength", "requirement")
    return {
        "protocol_version": data["protocol_version"],
        "sources": data["sources"],
        "source_sha256": data["source_sha256"],
        "requirements": [{field: row[field] for field in fields} for row in data["requirements"]],
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    output = fetch_denominator()
    path = ROOT / "conformance/mcp-auth-normative.json"
    if args.check:
        current = json.loads(path.read_text())
        if denominator_projection(current) != denominator_projection(output):
            raise SystemExit("MCP authorization source digest or extracted denominator drifted")
        print(f"verified {len(output['requirements'])} reproducible normative MCP authorization requirements")
        return
    path.write_text(json.dumps(output, indent=2) + "\n")
    print(f"refreshed {len(output['requirements'])} normative MCP authorization requirements; run publish_mcp_auth_disposition.py to apply assertion coverage")


if __name__ == "__main__":
    main()
