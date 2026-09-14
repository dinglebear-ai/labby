"""Focused tests for the pinned MCP normative-requirement extractor."""

from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

if __package__:
    from .extract_mcp_spec_requirements import MODAL, SOURCE_REVISION, STRENGTH, attach_colon_lists, infer_roles, inherited_list_clauses, normative_reference_classification, paragraph_units, reference_definitions, sentence_units, split_modal_clauses, verify_pinned_pages
else:
    from extract_mcp_spec_requirements import MODAL, SOURCE_REVISION, STRENGTH, attach_colon_lists, infer_roles, inherited_list_clauses, normative_reference_classification, paragraph_units, reference_definitions, sentence_units, split_modal_clauses, verify_pinned_pages


class ExtractMcpSpecRequirementsTests(unittest.TestCase):
    def test_modified_page_is_rejected_even_when_head_matches_pin(self):
        committed = b"Clients MUST retain this exact text.\n"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            page = root / "docs/specification/2026-07-28/example.mdx"
            page.parent.mkdir(parents=True)
            page.write_bytes(b"Clients MUST accept locally modified text.\n")
            with patch(
                f"{verify_pinned_pages.__module__}.subprocess.run",
                side_effect=[
                    SimpleNamespace(stdout=f"{SOURCE_REVISION}\n"),
                    SimpleNamespace(stdout=committed),
                ],
            ), self.assertRaisesRegex(ValueError, "differs from pinned Git blob"):
                verify_pinned_pages(root, [page])

    def test_multiline_mdx_preserves_lines_and_excludes_fenced_code(self):
        lines = [
            "## Rules\n", "Clients **MUST** send the first\n", "value.\n", "\n",
            "```json\n", '{"note":"Servers MUST ignore this example"}\n', "```\n",
            "Servers **SHOULD** reply.\n",
        ]
        units = paragraph_units(lines)
        self.assertEqual([(start, end) for start, end, _, _ in units], [(2, 3), (8, 8)])
        self.assertNotIn("ignore this example", "".join(text for _, _, text, _ in units))

    def test_bcp_definition_is_detectable_without_becoming_obligations(self):
        paragraph = 'The key words "MUST", "SHOULD", and "MAY" are to be interpreted as described in BCP 14.'
        self.assertIn('The key words "MUST"', paragraph)
        self.assertIn("interpreted as described in", paragraph)

    def test_two_must_clauses_are_distinct(self):
        sentence = "Clients **MUST** retain credentials, and servers **MUST** reject stale credentials."
        clauses = split_modal_clauses(sentence)
        self.assertEqual(len(clauses), 2)
        self.assertEqual(clauses[0][0], "Clients **MUST** retain credentials")
        self.assertEqual(clauses[1][0], "servers **MUST** reject stale credentials.")
        self.assertNotEqual(clauses[0][0], clauses[1][0])

    def test_sentence_split_preserves_multiline_modal_clause(self):
        parts = sentence_units("Clients **MUST** send\nthe value. Servers **MAY** retry.")
        self.assertEqual(parts, ["Clients **MUST** send\nthe value.", "Servers **MAY** retry."])

    def test_role_inference_uses_actor_before_modal_only(self):
        roles, confidence = infer_roles(
            "MCP servers **MUST** publish metadata describing authorization servers."
        )
        self.assertEqual(roles, ["server"])
        self.assertEqual(confidence, "explicit_actor")

    def test_pronoun_actor_stays_explicitly_unspecified(self):
        roles, confidence = infer_roles("otherwise, they **MUST** retry.")
        self.assertEqual(roles, ["unspecified"])
        self.assertEqual(confidence, "unspecified_actor")

    def test_external_normative_pointer_is_classified(self):
        kind, references = normative_reference_classification(
            "Clients **MUST** follow [RFC 9728](https://datatracker.ietf.org/doc/html/rfc9728)."
        )
        self.assertEqual(kind, "external_reference")
        self.assertEqual(references, ["https://datatracker.ietf.org/doc/html/rfc9728"])

    def test_colon_led_list_inherits_modal_for_each_item(self):
        clauses = inherited_list_clauses(
            "Cancellation notifications **MUST** only reference requests that:\n"
            "  - Were previously issued by the client\n"
            "  - Are believed to still be in-progress\n"
        )
        self.assertEqual(len(clauses), 2)
        self.assertIn("MUST", clauses[0][0])
        self.assertIn("previously issued", clauses[0][0])
        self.assertIn("still be in-progress", clauses[1][0])

    def test_blank_line_between_colon_leader_and_list_is_attached(self):
        units = attach_colon_lists(paragraph_units([
            "Clients MUST:\n", "\n", "- Validate input\n", "- Reject invalid input\n", "\n",
            "Normal prose.\n",
        ]))
        self.assertEqual(len(units), 2)
        self.assertEqual(units[0][0:2], (1, 4))
        self.assertIn("Reject invalid input", units[0][2])

    def test_new_modal_numbered_item_ends_preceding_inherited_list(self):
        units = attach_colon_lists(paragraph_units([
            "1. Clients MUST:\n", "   - Validate input\n",
            "1. Servers SHOULD:\n", "   - Reject invalid input\n",
        ]))
        self.assertEqual(len(units), 2)
        self.assertNotIn("Servers SHOULD", units[0][2])

    def test_fenced_content_prevents_distant_list_attachment(self):
        lines = ["Clients MUST handle races:\n", "```text\n", "race\n", "```\n", "- Servers SHOULD log\n"]
        units = attach_colon_lists(paragraph_units(lines), lines)
        self.assertEqual(len(units), 2)

    def test_all_bcp14_keywords_use_word_boundaries(self):
        expected = {
            "REQUIRED": "must", "SHALL": "must", "SHALL NOT": "must_not",
            "OPTIONAL": "may", "MUST": "must", "MUST NOT": "must_not",
            "SHOULD": "should", "SHOULD NOT": "should_not", "MAY": "may",
            "RECOMMENDED": "should", "NOT RECOMMENDED": "should_not",
        }
        for keyword, strength in expected.items():
            match = MODAL.search(f"Clients **{keyword}** comply.")
            self.assertIsNotNone(match, keyword)
            self.assertEqual(STRENGTH[match.group(1)], strength)
        self.assertIsNone(MODAL.search("MATCHES and OPTIONALITY are identifiers"))

    def test_reference_style_mdx_and_bare_rfc_references_are_resolved(self):
        definitions = reference_definitions(["[oauth]: https://www.rfc-editor.org/rfc/rfc6749.html\n"])
        kind, references = normative_reference_classification(
            "Clients MUST follow [OAuth][oauth], <Link href=\"https://openid.net/specs/x\">OIDC</Link>, and RFC 8707.",
            definitions,
        )
        self.assertEqual(kind, "external_reference")
        self.assertEqual(references, [
            "https://openid.net/specs/x",
            "https://www.rfc-editor.org/rfc/rfc6749.html",
            "https://www.rfc-editor.org/rfc/rfc8707.html",
        ])

    def test_unresolved_reference_style_pointer_is_explicit(self):
        kind, references = normative_reference_classification("Clients MUST follow [OAuth][oauth-missing].")
        self.assertEqual(kind, "external_reference")
        self.assertEqual(references, ["unresolved-reference:oauth-missing"])

    def test_internal_links_are_not_external_normative_references(self):
        definitions = reference_definitions([
            "[discover]: /specification/2026-07-28/schema#discoverrequest\n",
        ])
        for clause in (
            "Clients MUST follow [discovery](/specification/2026-07-28/schema#discoverrequest).",
            "Clients MUST follow [discovery][discover].",
            'Clients MUST follow <Link href="/specification/2026-07-28/schema#discoverrequest">discovery</Link>.',
        ):
            with self.subTest(clause=clause):
                kind, references = normative_reference_classification(clause, definitions)
                self.assertEqual(kind, "direct")
                self.assertEqual(references, [])


if __name__ == "__main__":
    unittest.main()
