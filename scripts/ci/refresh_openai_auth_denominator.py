#!/usr/bin/env python3
"""Fail when the official OpenAI auth guide drifts from the reviewed snapshot."""

import argparse
import hashlib
import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "conformance/openai-auth-normative.json"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", required=True)
    args = parser.parse_args()
    del args
    matrix = json.loads(MATRIX.read_text())
    with urllib.request.urlopen(matrix["source_url"], timeout=30) as response:
        source = response.read()
    actual = hashlib.sha256(source).hexdigest()
    if actual != matrix["source_sha256"]:
        raise SystemExit(
            f"OpenAI auth guide drifted: expected {matrix['source_sha256']}, got {actual}"
        )
    text = source.decode()
    for row in matrix["requirements"]:
        if row["source_excerpt"] not in text:
            raise SystemExit(f"OpenAI clause excerpt missing: {row['id']}")
    print(f"OpenAI auth denominator current: {len(matrix['requirements'])} clauses, sha256={actual}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
