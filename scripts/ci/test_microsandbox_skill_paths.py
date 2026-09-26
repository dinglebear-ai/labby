"""Executable skill files must trigger the job that runs their tests."""
import unittest
from changed_paths import classify

class MicrosandboxSkillPaths(unittest.TestCase):
    def test_every_skill_input_routes_to_docs_check(self):
        for suffix in ("SKILL.md", "scripts/verify_handoff.py", "scripts/fixture_server.py",
                       "scripts/bootstrap_ubuntu.sh", "tests/test_verify_handoff.py",
                       "references/ubuntu-arm64.packages.lock", "agents/openai.yaml"):
            with self.subTest(suffix=suffix):
                result=classify("pull_request", ["plugins/labby/skills/implement-in-microsandbox/"+suffix])
                self.assertIs(result["docs_check"], True)

if __name__ == "__main__": unittest.main()
