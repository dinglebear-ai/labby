#!/usr/bin/env python3
"""Fail when the official OpenAI auth guide drifts from the reviewed snapshot."""

import argparse
import hashlib
import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "tools/verification/conformance/openai-auth-normative.json"


def check_source(source: bytes, matrix: dict) -> None:
    """Apply the same fail-closed checks to downloaded or captured source bytes."""
    actual = hashlib.sha256(source).hexdigest()
    if actual != matrix["source_sha256"]:
        raise SystemExit(
            f"OpenAI auth guide drifted: expected {matrix['source_sha256']}, got {actual}"
        )
    text = source.decode()
    for row in matrix["requirements"]:
        if row["source_excerpt"] not in text:
            raise SystemExit(f"OpenAI clause excerpt missing: {row['id']}")
    print(f"OpenAI auth denominator matches reviewed source: {len(matrix['requirements'])} clauses, sha256={actual}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", required=True)
    parser.add_argument("--source-file", type=Path, help="Verify an immutable captured guide offline; CI fetches the live guide by default")
    args = parser.parse_args()
    matrix = json.loads(MATRIX.read_text())
    if args.source_file is not None:
        source = args.source_file.read_bytes()
    else:
        with urllib.request.urlopen(matrix["source_url"], timeout=30) as response:
            source = response.read()
    check_source(source, matrix)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
