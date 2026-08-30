import json
import unittest
import subprocess
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MATRIX = ROOT / "conformance/auth-requirements.json"
NORMATIVE = ROOT / "conformance/mcp-auth-normative.json"
COVERAGE = ROOT / "conformance/mcp-auth-coverage-manifest.json"
OPENAI_NORMATIVE = ROOT / "conformance/openai-auth-normative.json"


class AuthSpecificationMatrixTests(unittest.TestCase):
    def test_authoritative_denominator_is_complete_and_evidenced(self) -> None:
        data = json.loads(MATRIX.read_text())
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertEqual(data["mcp_protocol_version"], "2026-07-28")
        requirements = data["requirements"]
        self.assertGreaterEqual(len(requirements), 20)
        ids = [item["id"] for item in requirements]
        self.assertEqual(len(ids), len(set(ids)))
        for item in requirements:
            with self.subTest(item=item["id"]):
                self.assertRegex(item["source_url"], r"^https://(modelcontextprotocol\.io|developers\.openai\.com)/")
                self.assertTrue(item["requirement"])
                self.assertIn(item["strength"], {"must", "should", "openai_required", "openai_recommended"})
                self.assertIn(item["applicability"], {"applicable", "not_applicable"})
                self.assertIn(item["status"], {"pass", "partial", "gap", "not_applicable"})
                self.assertTrue(item["implementation"])
                self.assertTrue(item["test_id"] or item.get("subordinate_row_ids"))
                if item["status"] == "pass":
                    self.assertTrue(item.get("verification_commands"), item["id"])
                    for command in item["verification_commands"]:
                        if command.startswith("scripts/ci/openai-auth-conformance.sh "):
                            self.assertIn("scripts/ci/openai-auth-conformance.sh", workflow)
                        elif command.startswith("python3 scripts/ci/mcp_auth_normative_conformance.py "):
                            self.assertIn("python3 scripts/ci/mcp_auth_normative_conformance.py", workflow)
                        else:
                            self.assertIn(command, workflow, f"{item['id']} command is not run by CI")
                for path in item["evidence_paths"]:
                    self.assertTrue((ROOT / path.split(":", 1)[0]).exists(), path)

    def test_openai_tool_auth_denominator_is_explicit(self) -> None:
        data = json.loads(MATRIX.read_text())
        by_id = {item["id"]: item for item in data["requirements"]}
        for requirement_id in ["OAI-AUTH-001", "OAI-AUTH-002", "OAI-AUTH-003", "OAI-AUTH-004"]:
            self.assertIn(requirement_id, by_id)

    def test_openai_pass_rows_use_requirement_specific_ci_harness(self) -> None:
        data = json.loads(MATRIX.read_text())
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        harness = ROOT / "scripts/ci/openai-auth-conformance.sh"
        self.assertTrue(harness.exists())
        self.assertIn("scripts/ci/openai-auth-conformance.sh", workflow)
        listed = subprocess.run(
            [str(harness), "--list"], cwd=ROOT, capture_output=True, text=True, check=False,
        )
        self.assertEqual(listed.returncode, 0, listed.stderr)
        supported = set(listed.stdout.splitlines())
        for item in data["requirements"]:
            if not item["id"].startswith("OAI-AUTH-") or item["status"] != "pass":
                continue
            with self.subTest(item=item["id"]):
                expected = f"scripts/ci/openai-auth-conformance.sh {item['id']}"
                self.assertEqual(item.get("verification_commands"), [expected])
                self.assertIn(item["id"], supported)

    def test_current_openai_denominator_is_clause_bound_and_dispositioned(self) -> None:
        data = json.loads(OPENAI_NORMATIVE.read_text())
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertEqual(
            data["source_url"], "https://developers.openai.com/plugins/build/auth.md"
        )
        self.assertRegex(data["source_sha256"], r"^[0-9a-f]{64}$")
        self.assertIn("refresh_openai_auth_denominator.py --check", workflow)
        rows = data["requirements"]
        self.assertGreaterEqual(len(rows), 20)
        self.assertEqual(len({row["id"] for row in rows}), len(rows))
        supported = set(
            subprocess.run(
                [str(ROOT / "scripts/ci/openai-auth-conformance.sh"), "--list"],
                cwd=ROOT, capture_output=True, text=True, check=True,
            ).stdout.splitlines()
        )
        for row in rows:
            with self.subTest(row=row["id"]):
                self.assertTrue(row["source_excerpt"])
                self.assertTrue(row["disposition"])
                self.assertIn(row["status"], {"pass", "not_applicable"})
                if row["status"] == "pass":
                    self.assertEqual(row["applicability"], "applicable")
                    self.assertIn(row["verification_id"], supported)
                else:
                    self.assertEqual(row["applicability"], "not_applicable")
                    self.assertIsNone(row["verification_id"])

    def test_openai_recommended_operations_are_content_evidenced(self) -> None:
        oauth = (ROOT / "docs/runtime/OAUTH.md").read_text()
        operations = (ROOT / "docs/OPERATIONS.md").read_text()
        self.assertIn("https://chatgpt.com/connector_platform_oauth_redirect", oauth)
        self.assertIn("https://chatgpt.com/oauth/client.json", oauth)
        self.assertIn("MCP Inspector", operations)
        self.assertIn("trusted testers", operations)
        for topic in ["revocation", "refresh", "scope"]:
            self.assertIn(topic, operations.lower())

    def test_backup_restore_drill_is_executable(self) -> None:
        completed = subprocess.run(
            ["python3", str(ROOT / "scripts/ci/auth_backup_restore_drill.py")],
            cwd=ROOT, capture_output=True, text=True, check=False,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("auth backup restore drill passed", completed.stdout)

    def test_full_mcp_normative_denominator_is_preserved(self) -> None:
        data = json.loads(NORMATIVE.read_text())
        self.assertEqual(data["protocol_version"], "2026-07-28")
        self.assertEqual(len(data["sources"]), 4)
        # Two keyword-parser artifacts ("SHOULD to" and "MUST.") were not
        # actor obligations and are deliberately excluded from the denominator.
        self.assertEqual(len(data["requirements"]), 132)
        strengths = {}
        for row in data["requirements"]:
            strengths[row["strength"]] = strengths.get(row["strength"], 0) + 1
        self.assertEqual(strengths, {"must": 82, "must_not": 11, "should": 37, "should_not": 2})
        self.assertEqual(set(data["source_sha256"]), set(data["sources"]))
        self.assertEqual(len({row["id"] for row in data["requirements"]}), len(data["requirements"]))
        for row in data["requirements"]:
            self.assertIn(row["strength"], {"must", "must_not", "should", "should_not"})
            self.assertTrue(row["requirement"])
            self.assertIn(row["applicability"], {"applicable", "not_applicable"})
            self.assertIn(row["status"], {"pass", "partial", "gap", "not_applicable"})
            self.assertTrue(row["implementation"])
            self.assertTrue(row["actor"])
            self.assertTrue(row["evidence_paths"])
            expected_status = "not_applicable" if row["id"] in {
                "MCP-2026-AUTH-CLIENT-REGISTRATION-004",
                "MCP-2026-AUTH-CLIENT-REGISTRATION-005",
                "MCP-2026-AUTH-CLIENT-REGISTRATION-006",
                "MCP-2026-AUTH-CLIENT-REGISTRATION-007",
            } else "pass"
            self.assertEqual(row["status"], expected_status)
            if expected_status == "pass":
                self.assertTrue(row["assertion_test_ids"] or row.get("subordinate_row_ids"))
                self.assertFalse(row["assertion_test_ids"] and row.get("subordinate_row_ids"))
            else:
                self.assertIsNone(row["test_id"])
                self.assertEqual(row["assertion_test_ids"], [])
                self.assertEqual(row.get("subordinate_row_ids", []), [])
            self.assertEqual(
                row["applicability"],
                "not_applicable" if expected_status == "not_applicable" else "applicable",
            )
            self.assertEqual(
                row.get("verification_commands"),
                [f"python3 scripts/ci/mcp_auth_normative_conformance.py {row['id']}"],
            )
            for path in row["evidence_paths"]:
                self.assertTrue((ROOT / path).exists(), path)

    def test_every_mcp_normative_row_resolves_to_an_invoked_test(self) -> None:
        data = json.loads(NORMATIVE.read_text())
        harness = ROOT / "scripts/ci/mcp_auth_normative_conformance.py"
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("python3 scripts/ci/mcp_auth_normative_conformance.py", workflow)
        listed = subprocess.run(
            ["python3", str(harness), "--list"], cwd=ROOT,
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(listed.returncode, 0, listed.stderr)
        self.assertEqual(set(listed.stdout.splitlines()), {row["id"] for row in data["requirements"]})
        for row in data["requirements"]:
            resolved = subprocess.run(
                ["python3", str(harness), "--resolve", row["id"]], cwd=ROOT,
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(resolved.returncode, 0, resolved.stderr)
            resolved_ids = json.loads(resolved.stdout)
            if row.get("subordinate_row_ids"):
                self.assertTrue(resolved_ids)
            else:
                self.assertEqual(resolved_ids, row["assertion_test_ids"])

    def test_mcp_assertion_manifest_is_explicit_and_source_bound(self) -> None:
        import hashlib
        normative = json.loads(NORMATIVE.read_text())
        coverage = json.loads(COVERAGE.read_text())
        by_id = {entry["row_id"]: entry for entry in coverage["coverage"]}
        self.assertEqual(len(by_id), 132)
        self.assertEqual(set(by_id), {row["id"] for row in normative["requirements"]})
        publisher = (ROOT / "scripts/ci/publish_mcp_auth_disposition.py").read_text()
        self.assertNotIn("if number <=", publisher)
        for row in normative["requirements"]:
            entry = by_id[row["id"]]
            self.assertEqual(
                entry["source_requirement_sha256"],
                hashlib.sha256(row["requirement"].encode()).hexdigest(),
            )
            self.assertEqual(entry["assertion_test_ids"], row["assertion_test_ids"])
            self.assertEqual(entry["assertion_evidence"], row["assertion_evidence"])
            self.assertEqual(
                entry.get("subordinate_row_ids", []),
                row.get("subordinate_row_ids", []),
            )
            self.assertEqual(
                [item["test_id"] for item in row["assertion_evidence"]],
                row["assertion_test_ids"],
            )
            self.assertTrue(all(item["behavior"].strip() for item in row["assertion_evidence"]))
            self.assertTrue(entry["asserted_obligation"])
            self.assertTrue(entry["implementation"])

        graph = {
            row["id"]: row.get("subordinate_row_ids", [])
            for row in normative["requirements"]
        }
        def visit(row_id: str, path: tuple[str, ...] = ()) -> None:
            self.assertNotIn(row_id, path, f"aggregate cycle: {path + (row_id,)}")
            for subordinate_id in graph[row_id]:
                self.assertIn(subordinate_id, graph)
                visit(subordinate_id, path + (row_id,))
        for row_id in graph:
            visit(row_id)

        reuse = Counter(
            test_id
            for row in normative["requirements"]
            for test_id in row["assertion_test_ids"]
        )
        self.assertFalse(
            {test_id: count for test_id, count in reuse.items() if count > 10},
            "one behavioral test cannot credibly prove an unbounded set of heterogeneous clauses",
        )

    def test_curated_mcp_summary_cannot_contradict_normative_disposition(self) -> None:
        summary = json.loads(MATRIX.read_text())
        mcp = [row for row in summary["requirements"] if row["id"].startswith("MCP-AUTH-")]
        self.assertFalse([row for row in mcp if row["status"] in {"partial", "gap"}])
        for row in mcp:
            if row["status"] == "pass":
                self.assertTrue(row.get("verification_commands"))
                self.assertFalse(any("cargo test -p" in command for command in row["verification_commands"]))

    def test_source_refresh_and_vendor_provenance_are_required_ci_gates(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("python3 scripts/ci/refresh_mcp_auth_denominator.py --check", workflow)
        self.assertIn("python3 scripts/ci/check_vendor_rmcp_provenance.py", workflow)
        provenance = json.loads((ROOT / "conformance/vendor-rmcp-provenance.json").read_text())
        self.assertRegex(provenance["upstream_commit"], r"^[0-9a-f]{40}$")
        self.assertRegex(provenance["upstream_archive_sha256"], r"^[0-9a-f]{64}$")
        self.assertRegex(provenance["unified_diff_sha256"], r"^[0-9a-f]{64}$")
        self.assertGreaterEqual(len(provenance["patches"]), 5)

    def test_pr_coverage_gate_has_meaningful_auth_floor(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        coverage = workflow.split("  rust-coverage:", 1)[1].split("\n  ci-gate:", 1)[0]
        self.assertNotIn("github.event_name != 'pull_request'", coverage)
        self.assertIn("--critical-minimum 30", coverage)
        self.assertIn("--critical crates/labby-auth/src/", coverage)


if __name__ == "__main__":
    unittest.main()
