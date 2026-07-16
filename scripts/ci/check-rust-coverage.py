#!/usr/bin/env python3
"""Enforce aggregate and security-critical Rust line coverage from LCOV."""

from __future__ import annotations

import argparse
from pathlib import Path


def load_lcov(path: Path) -> dict[str, tuple[int, int]]:
    files: dict[str, tuple[int, int]] = {}
    source: str | None = None
    found: set[int] = set()
    hit: set[int] = set()
    for raw in path.read_text().splitlines():
        if raw.startswith("SF:"):
            source = raw[3:].replace("\\", "/")
            found, hit = set(), set()
        elif source and raw.startswith("DA:"):
            line, count, *_ = raw[3:].split(",")
            found.add(int(line))
            if int(count) > 0:
                hit.add(int(line))
        elif source and raw == "end_of_record":
            files[source] = (len(hit), len(found))
            source = None
    return files


def aggregate(files: dict[str, tuple[int, int]], selector: str) -> tuple[int, int]:
    selector = selector.replace("\\", "/")
    selected = [counts for path, counts in files.items() if selector in path]
    return (sum(v[0] for v in selected), sum(v[1] for v in selected))


def percent(counts: tuple[int, int]) -> float:
    hit, found = counts
    return 100.0 * hit / found if found else 0.0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("lcov", type=Path)
    parser.add_argument("--overall-minimum", type=float, required=True)
    parser.add_argument("--critical-minimum", type=float, required=True)
    parser.add_argument("--critical", action="append", default=[])
    parser.add_argument("--summary", type=Path)
    args = parser.parse_args()

    files = load_lcov(args.lcov)
    if not files:
        raise SystemExit("coverage report contains no source records")
    rows = [("all Rust", (sum(v[0] for v in files.values()), sum(v[1] for v in files.values())), args.overall_minimum)]
    rows.extend((selector, aggregate(files, selector), args.critical_minimum) for selector in args.critical)

    lines = ["| Scope | Lines | Coverage | Required |", "|---|---:|---:|---:|"]
    failed = False
    for scope, counts, minimum in rows:
        coverage = percent(counts)
        lines.append(f"| `{scope}` | {counts[0]}/{counts[1]} | {coverage:.2f}% | {minimum:.2f}% |")
        if counts[1] == 0 or coverage < minimum:
            failed = True
    report = "\n".join(lines) + "\n"
    print(report, end="")
    if args.summary:
        args.summary.write_text("## Rust coverage\n\n" + report)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
