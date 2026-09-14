"""Fault tests for the compliance reporter; a valid catalog is not a pass."""

from copy import deepcopy
from pathlib import Path
import tempfile
import signal
import subprocess
import unittest
from unittest.mock import patch

from scripts.ci.mcp_spec_compliance import (
    InvalidCatalog, apply_dispositions, coverage, dependency_fingerprint, digest, execute, invalidate_evidence, oracle_command, schema_requirements, source_check, validate_catalog, validate_oracles,
)


def fixture():
    path = "docs/specification/2026-07-28/basic/index.mdx"
    url = "https://github.com/modelcontextprotocol/modelcontextprotocol/blob/" + "a" * 40 + "/" + path
    sources = {"schema_version": 1, "protocol_version": "2026-07-28", "repository": "https://github.com/modelcontextprotocol/modelcontextprotocol", "revision": "a" * 40, "pages": [{"path": path, "url": url, "sha256": "b" * 64, "normative_count": 1}]}
    catalog = {"schema_version": 1, "protocol_version": "2026-07-28", "source_revision": "a" * 40, "requirements": [{"id": "MCP-001", "source_path": path, "source_url": url, "source_sha256": "c" * 64, "source_lines": [1, 2], "strength": "must", "requirement": "A request MUST contain an id.", "roles": ["client"], "transports": ["all"], "applicability": "applicable", "applicability_reason": "Labby sends MCP requests."}]}
    mapping = {"schema_version": 1, "protocol_version": "2026-07-28", "oracles": [{"id": "request-id", "requirement_ids": ["MCP-001"], "expected": "A missing id is rejected before dispatch.", "evidence_level": "product_wire", "kind": "cargo_nextest", "package": "labby", "target": "test:mcp_primitives_qualification", "test": "request_id", "timeout_seconds": 30}]}
    sources["pages"][0]["classification"] = "normative"
    mapping["oracles"][0]["scopes"] = [{"role": "client", "transport": "http"}]
    catalog["requirements"][0].update(role_inference="explicit_actor", capability="core", source_anchor="#messages", requirement_kind="direct", external_normative_references=[], existing_auth_ids=[])
    return sources, catalog, mapping


class ExecutionTests(unittest.TestCase):
    def test_sigterm_cleans_process_group_and_restores_handler(self):
        previous = signal.getsignal(signal.SIGTERM)
        with patch("scripts.ci.mcp_spec_compliance.subprocess.Popen") as spawn, patch("scripts.ci.mcp_spec_compliance.os.killpg") as kill:
            process = spawn.return_value
            process.pid = 12345

            def terminate_on_first_wait(*args, **kwargs):
                process.wait.side_effect = None
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)

            process.wait.side_effect = terminate_on_first_wait
            with self.assertRaises(SystemExit) as result:
                execute(["test-command"], 30)
            self.assertEqual(result.exception.code, 128 + signal.SIGTERM)
            self.assertEqual([call.args for call in kill.call_args_list], [(12345, signal.SIGTERM), (12345, signal.SIGKILL)])
        self.assertEqual(signal.getsignal(signal.SIGTERM), previous)

    def test_interrupt_cleans_entire_process_group_and_propagates(self):
        with patch("scripts.ci.mcp_spec_compliance.subprocess.Popen") as spawn, patch("scripts.ci.mcp_spec_compliance.os.killpg") as kill:
            process = spawn.return_value
            process.pid = 12345
            process.wait.side_effect = [KeyboardInterrupt(), 0, 0]
            with self.assertRaises(KeyboardInterrupt):
                execute(["test-command"], 30)
            self.assertEqual([call.args for call in kill.call_args_list], [(12345, signal.SIGTERM), (12345, signal.SIGKILL)])
            self.assertTrue(spawn.call_args.kwargs["start_new_session"])

    def test_timeout_cleans_descendants_after_parent_exits(self):
        with patch("scripts.ci.mcp_spec_compliance.subprocess.Popen") as spawn, patch("scripts.ci.mcp_spec_compliance.os.killpg") as kill:
            process = spawn.return_value
            process.pid = 12345
            process.wait.side_effect = [subprocess.TimeoutExpired("test-command", 30), 0, 0]
            self.assertEqual(execute(["test-command"], 30), (None, "timeout"))
            self.assertEqual([call.args for call in kill.call_args_list], [(12345, signal.SIGTERM), (12345, signal.SIGKILL)])


class CatalogTests(unittest.TestCase):
    def test_valid_intent_does_not_imply_compliance(self):
        sources, catalog, mapping = fixture()
        rows = validate_catalog(sources, catalog)
        oracles = validate_oracles(mapping, rows)
        report = coverage(rows, oracles, None, {})
        self.assertFalse(report["compliant"])
        self.assertEqual(report["counts"], {"not_run": 1})

    def test_missing_oracle_is_not_a_pass(self):
        sources, catalog, _ = fixture()
        self.assertEqual(coverage(validate_catalog(sources, catalog), {}, None, {})["counts"], {"missing_oracle": 1})

    def test_unreviewed_and_conditional_are_unresolved(self):
        for status in ("unreviewed", "conditional"):
            sources, catalog, _ = fixture()
            catalog["requirements"][0]["applicability"] = status
            report = coverage(validate_catalog(sources, catalog), {}, None, {})
            self.assertFalse(report["compliant"])
            self.assertEqual(report["counts"], {"applicability_unresolved": 1})

    def test_justified_nonapplicability_is_explicit(self):
        sources, catalog, _ = fixture()
        catalog["requirements"][0].update(applicability="not_applicable", applicability_reason="Requirement belongs to an unsupported optional host capability.")
        self.assertEqual(coverage(validate_catalog(sources, catalog), {}, None, {})["counts"], {"not_applicable": 1})

    def test_invalid_catalogs_fail_closed(self):
        mutations = [
            lambda s, c: s.update(revision="main"),
            lambda s, c: c.update(source_revision="d" * 40),
            lambda s, c: s["pages"][0].update(normative_count=0),
            lambda s, c: c["requirements"].append(deepcopy(c["requirements"][0])),
            lambda s, c: c["requirements"][0].update(source_lines=[0, 1]),
            lambda s, c: c["requirements"][0].update(source_path="../escape"),
            lambda s, c: c["requirements"][0].update(applicability_reason=""),
            lambda s, c: c["requirements"][0].update(status="pass"),
            lambda s, c: c["requirements"][0].update(roles=["invented"]),
            lambda s, c: c["requirements"][0].update(transports=["invented"]),
            lambda s, c: c["requirements"][0].update(role_inference="unspecified_actor"),
            lambda s, c: c["requirements"][0].update(requirement_kind="external_reference"),
            lambda s, c: s["pages"][0].update(classification="nonnormative"),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                sources, catalog, _ = fixture()
                mutate(sources, catalog)
                with self.assertRaises(InvalidCatalog):
                    validate_catalog(sources, catalog)

    def test_oracle_selects_exact_test_and_rejects_empty_selection(self):
        _, _, mapping = fixture()
        command = oracle_command(mapping["oracles"][0])
        self.assertIn("--no-tests=fail", command)
        self.assertEqual(command[-2:], ["-E", "test(=request_id)"])

    def test_oracle_does_not_accept_shell_or_filter_injection(self):
        for value in ("x); all()", "x; touch /tmp/owned", "$(whoami)", "", "test(*)"):
            _, _, mapping = fixture()
            mapping["oracles"][0]["test"] = value
            with self.assertRaises(InvalidCatalog):
                oracle_command(mapping["oracles"][0])

    def test_unknown_requirement_or_static_outcome_rejected(self):
        sources, catalog, mapping = fixture()
        rows = validate_catalog(sources, catalog)
        mapping["oracles"][0]["requirement_ids"] = ["unknown"]
        with self.assertRaises(InvalidCatalog):
            validate_oracles(mapping, rows)
        mapping["oracles"][0].update(requirement_ids=["MCP-001"], status="pass")
        with self.assertRaises(InvalidCatalog):
            validate_oracles(mapping, rows)


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        sources, catalog, mapping = fixture()
        self.rows = validate_catalog(sources, catalog)
        self.oracles = validate_oracles(mapping, self.rows)
        self.binding = {"catalog_sha256": digest([sources, catalog, mapping]), "worktree_sha256": "d" * 64}
        self.receipt = {"schema_version": 1, "binding": self.binding, "results": [{"oracle_id": "request-id", "command": oracle_command(mapping["oracles"][0]), "returncode": 0, "outcome": "passed"}]}

    def test_matching_executed_oracle_is_counted(self):
        self.assertTrue(coverage(self.rows, self.oracles, self.receipt, self.binding)["compliant"])

    def test_failed_or_timeout_is_not_passing(self):
        for outcome, code in (("failed", 1), ("timeout", None)):
            self.receipt["results"][0].update(outcome=outcome, returncode=code)
            self.assertEqual(coverage(self.rows, self.oracles, self.receipt, self.binding)["counts"], {"failed": 1})

    def test_missing_execution_is_not_passing(self):
        self.receipt["results"] = []
        self.assertFalse(coverage(self.rows, self.oracles, self.receipt, self.binding)["compliant"])

    def test_stale_source_or_catalog_rejected(self):
        for key in self.binding:
            changed = {**self.binding, key: "e" * 64}
            with self.assertRaises(InvalidCatalog):
                coverage(self.rows, self.oracles, self.receipt, changed)

    def test_forged_command_duplicate_or_inconsistent_result_rejected(self):
        for mutate in (
            lambda r: r["results"][0].update(command=["true"]),
            lambda r: r["results"].append(deepcopy(r["results"][0])),
            lambda r: r["results"][0].update(returncode=1),
            lambda r: r["results"][0].update(outcome="skipped"),
            lambda r: r["results"][0].update(outcome="timeout", returncode=1),
            lambda r: r["results"][0].update(outcome="failed", returncode=None),
        ):
            receipt = deepcopy(self.receipt)
            mutate(receipt)
            with self.assertRaises(InvalidCatalog):
                coverage(self.rows, self.oracles, receipt, self.binding)


class SourceCompletenessTests(unittest.TestCase):
    def test_dirty_source_and_matching_catalog_cannot_claim_pinned_revision(self):
        sources, catalog, _ = fixture()
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            path = checkout / sources["pages"][0]["path"]
            path.parent.mkdir(parents=True)
            path.write_bytes(b"modified normative source\n")
            import hashlib
            sources["pages"][0]["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            with patch("scripts.ci.mcp_spec_compliance.subprocess.check_output", side_effect=["a" * 40, b"immutable source\n"]):
                with self.assertRaisesRegex(InvalidCatalog, "pinned Git blob"):
                    source_check(sources, catalog, checkout)

    def test_lockstep_row_deletion_cannot_match_regenerated_denominator(self):
        sources, catalog, _ = fixture()
        tampered_sources = deepcopy(sources)
        tampered_catalog = deepcopy(catalog)
        tampered_sources["pages"][0]["normative_count"] = 0
        tampered_catalog["requirements"] = []
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            path = checkout / sources["pages"][0]["path"]
            path.parent.mkdir(parents=True)
            path.write_bytes(b"source\n")
            import hashlib
            tampered_sources["pages"][0]["sha256"] = hashlib.sha256(b"source\n").hexdigest()
            with patch("scripts.ci.mcp_spec_compliance.subprocess.check_output", side_effect=["a" * 40, b"source\n"]), patch("scripts.ci.mcp_spec_compliance.extract", return_value=(sources, catalog)):
                with self.assertRaisesRegex(InvalidCatalog, "differs from deterministic extraction"):
                    source_check(tampered_sources, tampered_catalog, checkout)

    def test_interpreted_text_cannot_change_without_source_regeneration(self):
        sources, catalog, _ = fixture()
        with tempfile.TemporaryDirectory() as directory:
            checkout = Path(directory)
            path = checkout / sources["pages"][0]["path"]
            path.parent.mkdir(parents=True)
            source = b"A request MUST contain an id.\n"
            path.write_bytes(source)
            import hashlib
            sources["pages"][0]["sha256"] = hashlib.sha256(source).hexdigest()
            catalog["requirements"][0].update(source_lines=[1, 1], source_sha256=hashlib.sha256(source).hexdigest())
            tampered = deepcopy(catalog)
            tampered["requirements"][0]["requirement"] = "Requests may omit all identifiers."
            with patch("scripts.ci.mcp_spec_compliance.subprocess.check_output", side_effect=["a" * 40, source]), patch("scripts.ci.mcp_spec_compliance.extract", return_value=(sources, catalog)):
                with self.assertRaisesRegex(InvalidCatalog, "requirement inventory differs"):
                    source_check(sources, tampered, checkout)


class MachineSchemaTests(unittest.TestCase):
    def setUp(self):
        from scripts.ci.extract_mcp_schema_requirements import canonical, extract_schema
        schema = {"$defs": {"Example": {"type": "string", "enum": ["café"]}}}
        self.document = extract_schema(schema, canonical(schema))

    def test_unicode_constraints_are_inventory_not_passes(self):
        rows = schema_requirements(self.document, None)
        self.assertTrue(rows)
        self.assertTrue(all(row["applicability"] == "unreviewed" for row in rows.values()))
        self.assertTrue(all(row["requirement_kind"] == "structural_schema" for row in rows.values()))
        self.assertTrue(all("status" not in row for row in rows.values()))

    def test_tampered_constraint_hash_pointer_id_and_revision_rejected(self):
        for mutate in (
            lambda d: d.update(source_revision="0" * 40),
            lambda d: d["constraints"][0].update(pointer="/wrong"),
            lambda d: d["constraints"][0].update(id="MCP-SCHEMA-CONSTRAINT-forged"),
            lambda d: d["constraints"][0].update(value_sha256="0" * 64),
            lambda d: d["constraints"][0].update(constraint_sha256="0" * 64),
            lambda d: d["constraints"][0].update(status="passed"),
        ):
            document = deepcopy(self.document)
            mutate(document)
            with self.assertRaises(InvalidCatalog):
                schema_requirements(document, None)

    def test_lockstep_constraint_deletion_rejected_by_regeneration(self):
        document = deepcopy(self.document)
        document["constraints"].pop()
        with patch("scripts.ci.extract_mcp_schema_requirements.extract", return_value=self.document):
            with self.assertRaisesRegex(InvalidCatalog, "deterministic extraction"):
                schema_requirements(document, Path("unused"))


class DispositionTests(unittest.TestCase):
    def setUp(self):
        sources, catalog, self.mapping = fixture()
        self.rows = validate_catalog(sources, catalog)
        self.document = {"schema_version": 1, "protocol_version": "2026-07-28", "dispositions": [{
            "requirement_id": "MCP-001", "requirement_sha256": digest(self.rows["MCP-001"]),
            "applicability": "applicable", "rationale": "Client sends requests over HTTP.",
            "review_reference": "test-reviewed", "required_evidence": [{"role": "client", "transport": "http", "level": "product_wire"}],
        }]}

    def test_disposition_does_not_mutate_generated_source(self):
        original = deepcopy(self.rows)
        applied = apply_dispositions(self.document, self.rows)
        self.assertEqual(self.rows, original)
        self.assertEqual(applied["MCP-001"]["required_evidence"], self.document["dispositions"][0]["required_evidence"])

    def test_stale_or_unjustified_decision_fails(self):
        for changes in ({"requirement_sha256": "a" * 64}, {"rationale": ""}, {"review_reference": ""}, {"required_evidence": []}):
            document = deepcopy(self.document)
            document["dispositions"][0].update(changes)
            with self.assertRaises(InvalidCatalog):
                apply_dispositions(document, self.rows)

    def test_wrong_level_role_or_transport_cannot_qualify(self):
        rows = apply_dispositions(self.document, self.rows)
        for change in ({"evidence_level": "sdk_unit"}, {"scopes": [{"role": "server", "transport": "http"}]}, {"scopes": [{"role": "client", "transport": "stdio"}]}):
            mapping = deepcopy(self.mapping)
            mapping["oracles"][0].update(change)
            report = coverage(rows, validate_oracles(mapping, rows), None, {})
            self.assertEqual(report["counts"], {"insufficient_evidence_scope": 1})

    def test_python_module_and_class_are_not_exact_oracles(self):
        for selector in ("scripts.ci.test_empty", "scripts.ci.test_empty.Cases"):
            with self.assertRaises(InvalidCatalog):
                oracle_command({"kind": "python_unittest", "test": selector})


class DependencyBindingTests(unittest.TestCase):
    def test_external_path_source_changes_invalidate_binding(self):
        import json
        import subprocess
        with tempfile.TemporaryDirectory() as directory:
            root, sdk = Path(directory) / "labby", Path(directory) / "sdk"
            root.mkdir()
            sdk.mkdir()
            subprocess.run(["git", "init", "--quiet", str(sdk)], check=True)
            source = sdk / "lib.rs"
            source.write_text("old source")
            metadata = {"packages": [{"id": "sdk", "source": None, "version": "3.3.0", "manifest_path": str(sdk / "Cargo.toml")}], "resolve": {}}
            original = subprocess.check_output
            def output(command, **kwargs):
                if command[0] == "cargo":
                    return json.dumps(metadata)
                if command[0] == "rustc":
                    return "fixed test toolchain"
                return original(command, **kwargs)
            with patch("scripts.ci.mcp_spec_compliance.subprocess.check_output", side_effect=output):
                before = dependency_fingerprint(root)
                source.write_text("changed source")
                self.assertNotEqual(before, dependency_fingerprint(root))


class EvidenceLifecycleTests(unittest.TestCase):
    def test_start_invalidates_only_declared_old_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            receipt, report, unrelated = root / "receipt.json", root / "report.json", root / "other.json"
            for path in (receipt, report, unrelated):
                path.write_text("old output")
            invalidate_evidence(receipt, report)
            self.assertFalse(receipt.exists())
            self.assertFalse(report.exists())
            self.assertEqual(unrelated.read_text(), "old output")

    def test_output_paths_must_be_distinct(self):
        with self.assertRaises(InvalidCatalog):
            invalidate_evidence(Path("same.json"), Path("same.json"))


if __name__ == "__main__":
    unittest.main()
