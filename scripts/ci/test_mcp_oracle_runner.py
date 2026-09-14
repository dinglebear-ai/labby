"""Tests for fail-closed, exact-one MCP Python oracle execution."""

from __future__ import annotations

import unittest
from unittest.mock import patch

from scripts.ci.mcp_oracle_runner import run_exact_test, validate_selector


class PassingCase(unittest.TestCase):
    def test_pass(self):
        self.assertTrue(True)


class SkippedCase(unittest.TestCase):
    @unittest.skip("intentional")
    def test_skip(self):
        pass


class ExpectedFailureCase(unittest.TestCase):
    @unittest.expectedFailure
    def test_expected_failure(self):
        self.fail("expected")


class UnexpectedSuccessCase(unittest.TestCase):
    @unittest.expectedFailure
    def test_unexpected_success(self):
        pass


class McpOracleRunnerTests(unittest.TestCase):
    PREFIX = "scripts.ci.test_mcp_oracle_runner"

    def test_accepts_one_fully_qualified_passing_test(self):
        self.assertEqual(run_exact_test(f"{self.PREFIX}.PassingCase.test_pass"), 0)

    def test_rejects_module_class_and_invalid_selectors(self):
        for selector in (
            self.PREFIX,
            f"{self.PREFIX}.PassingCase",
            f"{self.PREFIX}.PassingCase.not_a_test",
            "other.test_module.Case.test_method",
        ):
            with self.subTest(selector=selector), self.assertRaises(ValueError):
                validate_selector(selector)

    def test_rejects_zero_or_multiple_loaded_tests(self):
        for count in (0, 2):
            suite = unittest.TestSuite(
                PassingCase("test_pass") for _ in range(count)
            )
            with self.subTest(count=count), patch(
                "scripts.ci.mcp_oracle_runner.unittest.defaultTestLoader.loadTestsFromName",
                return_value=suite,
            ):
                self.assertEqual(
                    run_exact_test(f"{self.PREFIX}.PassingCase.test_pass"), 2
                )

    def test_rejects_skip_expected_failure_and_unexpected_success(self):
        selectors = (
            "SkippedCase.test_skip",
            "ExpectedFailureCase.test_expected_failure",
            "UnexpectedSuccessCase.test_unexpected_success",
        )
        for suffix in selectors:
            with self.subTest(selector=suffix):
                self.assertEqual(run_exact_test(f"{self.PREFIX}.{suffix}"), 1)


def load_tests(loader, _tests, _pattern):
    """Keep outcome fixtures out of normal module-wide test execution."""
    return loader.loadTestsFromTestCase(McpOracleRunnerTests)


if __name__ == "__main__":
    unittest.main()
