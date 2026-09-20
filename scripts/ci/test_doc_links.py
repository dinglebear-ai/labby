#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-doc-links.py"
SPEC = importlib.util.spec_from_file_location("check_doc_links", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class DocLinkAnchorTests(unittest.TestCase):
    def test_github_slug_keeps_underscores_and_repeated_whitespace_hyphens(self) -> None:
        self.assertEqual(
            MODULE.github_heading_slug("Upstream OAuth (authorization_code + PKCE)"),
            "upstream-oauth-authorization_code--pkce",
        )

    def test_unicode_symbols_and_punctuation_match_github_slugger(self) -> None:
        self.assertEqual(MODULE.github_heading_slug("Widget → host callbacks"), "widget--host-callbacks")
        self.assertEqual(
            MODULE.github_heading_slug("Pre-flight — `labby doctor auth`"),
            "pre-flight--labby-doctor-auth",
        )
        self.assertEqual(MODULE.github_heading_slug("😄 emoji"), "-emoji")
        self.assertEqual(
            MODULE.github_heading_slug("Привет non-latin 你好"),
            "привет-non-latin-你好",
        )

    def test_duplicate_headings_receive_github_style_suffixes(self) -> None:
        anchors = MODULE.heading_anchors(
            "# Environment Variables\n## Environment Variables\n### Environment Variables\n"
        )
        self.assertEqual(
            anchors,
            {"environment-variables", "environment-variables-1", "environment-variables-2"},
        )

    def test_fenced_headings_are_not_link_targets(self) -> None:
        anchors = MODULE.heading_anchors(
            "# Real\n\n```markdown\n# Not real\n```\n\n## Also real\n"
        )
        self.assertEqual(anchors, {"real", "also-real"})

    def test_link_markup_contributes_visible_heading_text(self) -> None:
        self.assertEqual(
            MODULE.github_heading_slug("[HTTP API](./HTTP_API.md) Surface"),
            "http-api-surface",
        )

    def test_setext_headings_are_link_targets_but_frontmatter_is_not(self) -> None:
        anchors = MODULE.heading_anchors(
            "---\ntitle: Not a heading\nstatus: current\n---\n\nSetext One\n==========\nSetext Two\n----------\n"
        )
        self.assertEqual(anchors, {"setext-one", "setext-two"})

    def test_longer_fence_is_not_closed_by_shorter_marker(self) -> None:
        text = (
            "# Real\n\n````markdown\n# Hidden\n[Hidden](./MISSING.md)\n```\n## Still hidden\n````\n\n## Also real\n[Real](./README.md)\n"
        )
        self.assertEqual(MODULE.heading_anchors(text), {"real", "also-real"})
        self.assertEqual(list(MODULE.iter_targets(text)), [(11, "./README.md")])


if __name__ == "__main__":
    unittest.main()
