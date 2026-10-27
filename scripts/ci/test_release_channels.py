#!/usr/bin/env python3
"""Deterministic channel and metadata/pointer isolation regression tests."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
try:
    from release_channel import identity, order
except ModuleNotFoundError:
    from scripts.ci.release_channel import identity, order

ROOT = Path(__file__).resolve().parents[2]

def module(name):
    spec = importlib.util.spec_from_file_location('helper', ROOT / 'scripts/ci' / name)
    item = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(item)
    return item

class Channels(unittest.TestCase):
    def test_identity_and_order(self):
        self.assertEqual('dev', identity('v2.7.0-dev.10')['npm_tag'])
        self.assertGreater(order('2.7.0-dev.10'), order('2.7.0-dev.9'))
        self.assertGreater(order('2.7.0'), order('2.7.0-dev.10'))
        for value in ('v01.2.3', '1.2.3-dev.0', '1.2.3-dev.01', '1.2.3-beta.1', '1.2.3+build'):
            with self.assertRaises(ValueError): identity(value)

    def test_metadata_stamping_is_idempotent_and_preserves_dependencies(self):
        helper = module('sync-release-version.py')
        paths = ['Cargo.toml', 'Cargo.lock', 'apps/tauri/package.json', 'apps/tauri/src-tauri/Cargo.toml', 'apps/tauri/src-tauri/Cargo.lock', 'apps/tauri/src-tauri/tauri.conf.json', 'tools/verification/hosts/labby/Cargo.toml', 'tools/verification/Cargo.lock', '.release-please-manifest.json', 'apm.yml', 'plugins/labby/apm.yml', 'server.json', 'scripts/sync-npm-release-version.py']
        for name in ('labby-mcp', 'labby-microsandbox', 'labby-tailcat-browser'):
            paths += [str(p.relative_to(ROOT)) for p in (ROOT / 'packages' / name).glob('package*.json')]
        paths += [str(path.relative_to(ROOT)) for path in (ROOT / 'crates').glob('*/Cargo.toml')]
        # Exercise the same stamped package consumed by release preflight, not
        # just stable checkout metadata: channel-sensitive package tests must
        # also pass after the real version stamper runs.
        paths += subprocess.check_output(['git', 'ls-files', 'packages/labby-mcp/'], cwd=ROOT, text=True).splitlines()
        paths += ['README.md'] + [path.name for path in ROOT.glob('LICENSE*') if path.is_file()]
        paths = sorted(set(path for path in paths if Path(path).name not in {'AGENTS.md', 'CLAUDE.md', 'GEMINI.md'}))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for path in paths:
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / path, root / path)
            helper.sync(root, '2.7.0-dev.10')
            first = {path: (root / path).read_bytes() for path in paths}
            helper.sync(root, '2.7.0-dev.10')
            self.assertEqual(first, {path: (root / path).read_bytes() for path in paths})
            import tomllib
            for path in ('Cargo.lock', 'apps/tauri/src-tauri/Cargo.lock', 'tools/verification/Cargo.lock'):
                before = tomllib.loads((ROOT / path).read_text())['package']
                after = tomllib.loads((root / path).read_text())['package']
                for old, new in zip(before, after):
                    if 'source' in old:
                        self.assertEqual(old, new)
                    elif old['name'] == 'labby-verify':
                        self.assertEqual('0.1.0', new['version'])
                    elif old['name'] == 'xtask':
                        self.assertEqual('2.7.0-dev.10', new['version'])
                if path == 'Cargo.lock':
                    self.assertEqual('2.7.0-dev.10', next(item['version'] for item in after if item['name'] == 'xtask'))
                if path == 'tools/verification/Cargo.lock':
                    self.assertEqual('0.1.0', next(item['version'] for item in after if item['name'] == 'labby-verify'))
            env = dict(os.environ, npm_config_cache=str(root / 'npm-cache'))
            for command in (['npm', 'test'], ['npm', 'run', 'check']):
                result = subprocess.run(command, cwd=root / 'packages/labby-mcp', env=env,
                                        capture_output=True, text=True, timeout=60)
                self.assertEqual(0, result.returncode, result.stdout + result.stderr)


    def test_dev_baseline_uses_only_older_published_stable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            releases = root / 'releases.json'
            releases.write_text(json.dumps([dict(tag_name=tag, draft=False, prerelease=dev, assets=[dict(name='archive', state='uploaded')]) for tag, dev in [('v2.6.2', False), ('v2.7.0', False), ('v2.7.0-dev.9', True)]]))
            merged = root / 'merged'; merged.write_text('v2.6.2\nv2.7.0\nv2.7.0-dev.9\n')
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/ci/resolve-n-minus-one-baseline.py'), '--candidate', 'v2.7.0-dev.10', '--releases', str(releases), '--merged-tags', str(merged), '--asset', 'archive'], capture_output=True, text=True, check=True)
            self.assertEqual('v2.6.2', result.stdout.strip())

    def test_dev_manifest_has_no_registry_distribution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / 'lab-test.tar.gz'; archive.write_bytes(b'archive')
            sbom = root / 'lab-test.spdx.json'; sbom.write_text('{}')
            output = root / 'manifest.json'
            subprocess.run([sys.executable, str(ROOT / 'scripts/ci/create-release-manifest.py'), '--tag', 'v2.7.0-dev.1', '--repository', 'example/labby', '--output', str(output), str(archive), str(sbom)], check=True)
            data = json.loads(output.read_text())
            self.assertNotIn('mcp', data['distributions'])
            self.assertEqual('dev', data['distributions']['npm']['tag'])
            self.assertTrue(data['distributions']['github']['prerelease'])

    def test_dev_github_promotion_forces_prerelease_and_never_latest(self):
        import os
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fake = root / 'gh'
            fake.write_text('#!/usr/bin/env python3\nimport json,os,sys,pathlib\np=pathlib.Path(os.environ["GH_STATE"])\ns=json.loads(p.read_text())\na=sys.argv[1:]\nif a[:2]==["release","edit"]:\n assert "--latest=false" in a and "--prerelease" in a\n s["draft"]=False; p.write_text(json.dumps(s))\nelif a[0]=="api": print("v2.6.2")\nelif "isPrerelease" in a: print("true")\nelse: print(str(s["draft"]).lower())\n')
            fake.chmod(0o755)
            state = root / 'state'; state.write_text('{"draft":true}')
            env = dict(os.environ, PATH=str(root)+os.pathsep+os.environ['PATH'], RELEASE_TAG='v2.7.0-dev.1', GITHUB_REPOSITORY='example/labby', GH_STATE=str(state))
            subprocess.run(['bash', str(ROOT / 'scripts/ci/promote-release.sh')], env=env, check=True)
            self.assertFalse(json.loads(state.read_text())['draft'])

    def test_dev_pointer_success_rollback_and_receipt_channel_guard(self):
        helper = module('promote-npm-pointer.py')
        tags = {'latest': '2.6.2', 'dev': '2.7.0-dev.9'}
        def run(*args):
            if args[0] == 'view': return json.dumps(tags)
            if args[:2] == ('dist-tag', 'add'):
                tags[args[3]] = args[2].rsplit('@', 1)[1]
                return ''
            raise AssertionError(args)
        with tempfile.TemporaryDirectory() as temporary, patch.object(helper, 'run', side_effect=run):
            receipt = Path(temporary) / 'receipt.json'
            def invoke(mode):
                with patch.object(sys, 'argv', ['pointer', mode, '--package', '@example/labby', '--version', '2.7.0-dev.10', '--receipt', str(receipt)]): helper.main()
            invoke('prepare'); invoke('promote')
            self.assertEqual({'latest': '2.6.2', 'dev': '2.7.0-dev.10'}, tags)
            invoke('rollback')
            self.assertEqual({'latest': '2.6.2', 'dev': '2.7.0-dev.9'}, tags)
            data = json.loads(receipt.read_text()); data['npm_tag'] = 'latest'; receipt.write_text(json.dumps(data))
            with self.assertRaises(ValueError): invoke('rollback')
            self.assertEqual('2.6.2', tags['latest'])

if __name__ == '__main__': unittest.main()
