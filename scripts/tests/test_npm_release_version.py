"""Verify a release bump updates companion identities without changing dependencies."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class NpmReleaseVersionTest(unittest.TestCase):
    def test_next_release_updates_all_package_and_lock_roots(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ('labby-mcp', 'labby-microsandbox', 'labby-tailcat-browser'):
                directory = root / 'packages' / name
                directory.mkdir(parents=True)
                (directory / 'package.json').write_text(json.dumps({
                    'name': name, 'version': '2.5.0', 'dependencies': {'fixture': '1.2.3'}}))
                if name != 'labby-mcp':
                    (directory / 'package-lock.json').write_text(json.dumps({
                        'version': '2.5.0', 'lockfileVersion': 3, 'packages': {
                            '': {'name': name, 'version': '2.5.0'},
                            'node_modules/fixture': {'version': '1.2.3', 'integrity': 'unchanged'}}}))
            subprocess.run(['python3', str(REPO / 'scripts/sync-npm-release-version.py'),
                            str(root), '2.6.0'], check=True, timeout=10)
            for name in ('labby-mcp', 'labby-microsandbox', 'labby-tailcat-browser'):
                directory = root / 'packages' / name
                manifest = json.loads((directory / 'package.json').read_text())
                self.assertEqual(manifest['version'], '2.6.0')
                self.assertEqual(manifest['dependencies'], {'fixture': '1.2.3'})
                if name != 'labby-mcp':
                    lock = json.loads((directory / 'package-lock.json').read_text())
                    self.assertEqual(lock['version'], '2.6.0')
                    self.assertEqual(lock['packages']['']['version'], '2.6.0')
                    self.assertEqual(lock['packages']['node_modules/fixture'],
                                     {'version': '1.2.3', 'integrity': 'unchanged'})
            # Repeating the same bump is byte-stable.
            before = {path: path.read_bytes() for path in root.rglob('*.json')}
            subprocess.run(['python3', str(REPO / 'scripts/sync-npm-release-version.py'),
                            str(root), '2.6.0'], check=True, timeout=10)
            self.assertTrue(all(path.read_bytes() == value for path, value in before.items()))


if __name__ == '__main__':
    unittest.main()
