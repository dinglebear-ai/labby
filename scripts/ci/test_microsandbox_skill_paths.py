"""Executable skill files must trigger the job that runs their tests."""
import unittest
from pathlib import Path
from changed_paths import classify

class MicrosandboxSkillPaths(unittest.TestCase):
    def test_every_skill_input_routes_to_docs_check(self):
        for suffix in ("SKILL.md", "scripts/verify_handoff.py", "scripts/fixture_server.py",
                       "scripts/bootstrap_ubuntu.sh", "tests/test_verify_handoff.py",
                       "references/ubuntu-arm64.packages.lock", "agents/openai.yaml"):
            with self.subTest(suffix=suffix):
                result=classify("pull_request", ["plugins/labby/skills/implement-in-microsandbox/"+suffix])
                self.assertIs(result["docs_check"], True)

    def test_task_docs_have_required_frontmatter(self):
        root=Path(__file__).resolve().parents[2]
        for path in (root / "docs/tasks/microsandbox-implementation-process").glob("*.md"):
            with self.subTest(path=path.name):
                text=path.read_text()
                self.assertTrue(text.startswith("---"))
                metadata=text.split("---",2)[1]
                for field in ("title:","created:","updated:"):
                    self.assertIn(field,metadata)

if __name__ == "__main__": unittest.main()
