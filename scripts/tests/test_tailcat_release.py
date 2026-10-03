"""Exercise release staging with deterministic external build/install fixtures."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


class TailcatReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.build = self.root / 'build'
        self.build.mkdir()
        names = ['tailcat-bridge', 'tailcat.wasm', 'tailcat.wasm.gz',
                 'wasm_exec.js', 'THIRD_PARTY_NOTICES.md']
        checksums = []
        for name in names:
            data = name.encode()
            (self.build / name).write_bytes(data)
            checksums.append(f'{hashlib.sha256(data).hexdigest()}  {name}')
        (self.build / 'SHA256SUMS').write_text('\n'.join(checksums) + '\n')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        npm = self.bin / 'npm'
        npm.write_text('''#!/bin/sh
set -eu
[ "$*" = "ci --omit=dev --ignore-scripts --no-audit --no-fund" ]
mkdir -p node_modules/fixture node_modules/.bin
printf 'locked dependency' > node_modules/fixture/index.js
ln -s ../fixture/index.js node_modules/.bin/fixture
if [ "${TEST_SYMLINK:-}" = yes ]; then ln -s /etc/passwd node_modules/fixture/unsafe; fi
''')
        npm.chmod(0o755)
        self.env = {**os.environ, 'PATH': str(self.bin) + os.pathsep + os.environ['PATH']}
        self.version = json.loads((REPO / 'packages/labby-microsandbox/package.json').read_text())['version']

    def stage(self):
        return subprocess.run(['bash', str(REPO / 'scripts/package-tailcat-release.sh'),
                               str(self.root / 'dist'), self.version,
                               'aarch64-apple-darwin', str(self.build)],
                              env=self.env, capture_output=True, text=True, timeout=20)

    def test_complete_manifest_covers_locked_runtime_and_all_files(self):
        result = self.stage()
        self.assertEqual(result.returncode, 0, result.stderr)
        bundle = self.root / 'dist/tailcat'
        manifest = json.loads((bundle / 'manifest.json').read_text())
        self.assertEqual((manifest['schemaVersion'], manifest['protocol'], manifest['version']),
                         (1, 1, self.version))
        entries = {entry['path']: entry['sha256'] for entry in manifest['components']}
        actual = {str(file.relative_to(bundle)) for file in bundle.rglob('*')
                  if file.is_file() and file.name != 'manifest.json'}
        self.assertEqual(set(entries), actual)
        self.assertIn('adapter/node_modules/fixture/index.js', entries)
        self.assertIn('adapter/cleanup.mjs', entries)
        self.assertFalse((bundle / 'adapter/node_modules/.bin').exists())
        for name, digest in entries.items():
            self.assertEqual(hashlib.sha256((bundle / name).read_bytes()).hexdigest(), digest)
        self.assertNotIn('derpMapURL', manifest)
        self.assertTrue((bundle / 'native/tailcat-bridge').stat().st_mode & 0o111)
        # A consumer can detect any changed packaged component using the manifest.
        changed = bundle / 'browser/hook.mjs'
        changed.write_text('tampered')
        self.assertNotEqual(hashlib.sha256(changed.read_bytes()).hexdigest(), entries['browser/hook.mjs'])

    def test_corrupt_bridge_is_rejected_without_partial_bundle(self):
        (self.build / 'tailcat.wasm').write_bytes(b'changed')
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('checksum mismatch', result.stderr)
        self.assertFalse((self.root / 'dist/tailcat').exists())
        self.assertEqual(list((self.root / 'dist').iterdir()), [])

    def test_dependency_symlink_is_rejected(self):
        self.env['TEST_SYMLINK'] = 'yes'
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unsupported companion entry', result.stderr)
        self.assertFalse((self.root / 'dist/tailcat').exists())

    def test_existing_bundle_is_preserved(self):
        self.assertEqual(self.stage().returncode, 0)
        before = (self.root / 'dist/tailcat/manifest.json').read_bytes()
        result = self.stage()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(before, (self.root / 'dist/tailcat/manifest.json').read_bytes())


if __name__ == '__main__':
    unittest.main()
