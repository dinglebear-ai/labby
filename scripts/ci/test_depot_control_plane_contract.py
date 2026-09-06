#!/usr/bin/env python3
"""Regression tests for the checked Labby/Depot compatibility denominator."""

import copy
import importlib.util
import json
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
CHECKER = ROOT / "scripts/check-depot-control-plane-contract.py"
SPEC = importlib.util.spec_from_file_location("depot_contract_checker", CHECKER)
assert SPEC and SPEC.loader
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)


class DepotControlPlaneContractTest(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = json.loads(checker.MANIFEST.read_text())

    def rejects(self, mutate) -> None:
        candidate = copy.deepcopy(self.contract)
        mutate(candidate)
        with self.assertRaises((KeyError, TypeError, ValueError)):
            checker.validate(candidate)

    def test_checked_contract_is_valid(self) -> None:
        checker.validate(self.contract)

    def test_exact_import_cannot_be_deferred(self) -> None:
        self.rejects(lambda data: data["flows"]["sendToLabby"].update(status="deferred"))

    def test_service_credential_boundary_cannot_be_weakened(self) -> None:
        self.rejects(lambda data: data["actorPolicy"].update(serviceCredential="read-only-unless-explicitly-approved"))

    def test_operation_fingerprint_is_required(self) -> None:
        self.rejects(lambda data: data["administrationContract"]["operationFingerprint"].update(required=False))

    def test_schema_bounds_are_required(self) -> None:
        self.rejects(lambda data: data["administrationContract"]["inputSchema"].update(maxProperties=0))


if __name__ == "__main__":
    unittest.main()
