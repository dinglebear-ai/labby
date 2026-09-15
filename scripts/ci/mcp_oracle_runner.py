#!/usr/bin/env python3
"""Run exactly one fully qualified unittest oracle and fail closed."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

SELECTOR = re.compile(
    r"scripts\.ci\.test_[A-Za-z0-9_]+\."
    r"[A-Za-z_][A-Za-z0-9_]*\.test_[A-Za-z0-9_]+\Z"
)


def validate_selector(selector: str) -> None:
    if not SELECTOR.fullmatch(selector):
        raise ValueError(
            "oracle selector must be a full scripts.ci.test_module.TestCase.test_method name"
        )


def run_exact_test(selector: str) -> int:
    validate_selector(selector)
    suite = unittest.defaultTestLoader.loadTestsFromName(selector)
    if suite.countTestCases() != 1:
        print(
            f"MCP oracle runner: selector resolved to {suite.countTestCases()} tests, expected exactly 1",
            file=sys.stderr,
        )
        return 2
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    disallowed = (
        result.skipped
        or result.expectedFailures
        or result.unexpectedSuccesses
        or result.failures
        or result.errors
    )
    return 0 if result.testsRun == 1 and result.wasSuccessful() and not disallowed else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("selector")
    args = parser.parse_args()
    try:
        return run_exact_test(args.selector)
    except ValueError as error:
        print(f"MCP oracle runner: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
