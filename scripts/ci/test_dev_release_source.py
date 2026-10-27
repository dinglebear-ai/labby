"""Real Git fixtures for immutable development source selection and tagging."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_source import validate, require_green_main

ROOT = Path(__file__).resolve().parents[2]

class DevSource(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.root = self.home / 'source'; self.root.mkdir()
        self.git('init', '-b', 'main', '--quiet')
        self.git('config', 'user.name', 'Release fixture')
        self.git('config', 'user.email', 'release@example.invalid')
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "2.6.2"\n')
        (self.root / 'product.rs').write_text('const PRODUCT: bool = true;\n')
        helper = self.root / 'scripts/ci/sync-release-version.py'; helper.parent.mkdir(parents=True)
        helper.write_text('import pathlib,re,sys\np=pathlib.Path(sys.argv[1])/"Cargo.toml"\np.write_text(re.sub(r\'version = "[^"]+"\', f\'version = "{sys.argv[2]}"\', p.read_text()))\n')
        self.git('add', '--all'); self.git('commit', '--quiet', '-m', 'fixture')
        self.source = self.git('rev-parse', 'HEAD')
        self.git('update-ref', 'refs/remotes/origin/main', self.source)

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, text=True, stderr=subprocess.PIPE, timeout=15).strip()

    def child(self):
        subprocess.run([sys.executable, str(self.root/'scripts/ci/sync-release-version.py'), str(self.root), '2.7.0-dev.1'], check=True, timeout=15)
        self.git('add', '--all'); self.git('commit', '--quiet', '-m', 'dev metadata')
        return self.git('rev-parse', 'HEAD')

    def test_accepts_exact_metadata_child_but_rejects_product_changes(self):
        candidate = self.child()
        self.assertEqual(self.source, validate(self.root, candidate, 'v2.7.0-dev.1'))
        (self.root/'product.rs').write_text('const PRODUCT: bool = false;\n')
        self.git('add', 'product.rs'); self.git('commit', '--quiet', '--amend', '--no-edit')
        with self.assertRaisesRegex(ValueError, 'exact version-only'):
            validate(self.root, 'HEAD', 'v2.7.0-dev.1')

    def fake_gh(self, rows):
        tools = self.home/'tools'; tools.mkdir(exist_ok=True)
        gh = tools/'gh'
        gh.write_text('#!/usr/bin/env python3\nimport os\nprint(os.environ["FIXTURE_CI_ROWS"])\n'); gh.chmod(0o755)
        return dict(os.environ, PATH=str(tools)+os.pathsep+os.environ['PATH'], FIXTURE_CI_ROWS=json.dumps({'workflow_runs':rows}))

    def row(self, **updates):
        row=dict(id=1, head_sha=self.source, head_branch='main', event='push', status='completed', conclusion='success', run_number=1, run_attempt=1)
        row.update(updates); return row

    def test_source_requires_latest_successful_main_ci_attempt(self):
        from unittest.mock import patch
        with patch.dict(os.environ, self.fake_gh([self.row(), self.row(run_attempt=2, conclusion='failure')]), clear=True):
            with self.assertRaisesRegex(ValueError, 'latest source main CI'):
                require_green_main(self.root, self.source, 'example/labby')
        with patch.dict(os.environ, self.fake_gh([self.row()]), clear=True):
            self.assertEqual(1, require_green_main(self.root, self.source, 'example/labby'))
        with patch.dict(os.environ, self.fake_gh([self.row(head_branch='other')]), clear=True):
            with self.assertRaisesRegex(ValueError, 'no main push CI'):
                require_green_main(self.root, self.source, 'example/labby')

    def test_preparation_pushes_only_immutable_dev_tag(self):
        remote=self.home/'remote.git'
        subprocess.run(['git','init','--bare','--quiet',str(remote)],check=True,timeout=15)
        self.git('remote','add','origin',str(remote)); self.git('push','--quiet','origin','main')
        env=self.fake_gh([self.row()])
        command=[sys.executable,str(ROOT/'scripts/ci/prepare-dev-release.py'),'--tag','v2.7.0-dev.1','--source','main','--repository','example/labby']
        result=subprocess.run(command,cwd=self.root,env=env,capture_output=True,text=True,timeout=60)
        self.assertEqual(0,result.returncode,result.stderr)
        receipt=json.loads(result.stdout); self.assertEqual(self.source,receipt['source_sha'])
        self.assertEqual(self.source,self.git('--git-dir',str(remote),'rev-parse','refs/heads/main'))
        self.assertEqual(receipt['candidate_sha'],self.git('--git-dir',str(remote),'rev-parse','refs/tags/v2.7.0-dev.1'))
        repeated=subprocess.run(command,cwd=self.root,env=env,capture_output=True,text=True,timeout=60)
        self.assertNotEqual(0,repeated.returncode)
        self.assertIn('already exists',repeated.stderr)
