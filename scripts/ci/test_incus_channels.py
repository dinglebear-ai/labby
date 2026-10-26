#!/usr/bin/env python3
"""Local bare-remote Incus channel isolation and recovery fixtures."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[2]

class IncusChannels(unittest.TestCase):
    def transaction(self, *, newer=False, absent=False):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary); source = work/'source'; source.mkdir()
            def git(*args, cwd=source):
                return subprocess.check_output(['git', *args], cwd=cwd, text=True, stderr=subprocess.DEVNULL).strip()
            git('init', '-b', 'main'); git('config', 'user.name', 'Fixture'); git('config', 'user.email', 'fixture@example.invalid')
            (source/'source').write_text('old'); git('add', '.'); git('commit', '-m', 'old')
            previous = git('rev-parse', 'HEAD'); git('tag', 'labby-incus-latest')
            if not absent: git('tag', 'labby-incus-dev')
            (source/'source').write_text('candidate'); git('commit', '-am', 'candidate'); candidate = git('rev-parse', 'HEAD')
            git('branch', 'candidate')
            if newer:
                (source/'source').write_text('newer'); git('commit', '-am', 'newer'); git('tag', '-f', 'labby-incus-dev'); previous = git('rev-parse', 'HEAD')
            remote=work/'remote.git'; checkout=work/'checkout'
            git('clone', '--bare', str(source), str(remote))
            git('clone', '--depth=1', '--no-tags', '--branch', 'candidate', remote.as_uri(), str(checkout))
            git('config', f'url.{remote.as_uri()}.insteadOf', 'https://x-access-token:fixture@github.com/example/labby.git', cwd=checkout)
            for relative in ('scripts/ci/verify-incus-image-manifest.py', 'scripts/with_timeout.sh'):
                target=checkout/relative; target.parent.mkdir(parents=True,exist_ok=True); shutil.copyfile(ROOT/relative,target)
            assets=work/'assets'; assets.mkdir()
            image=assets/'labby-incus-x86_64-unknown-linux-gnu.tar.xz'; image.write_bytes(b'image')
            digest=hashlib.sha256(b'image').hexdigest()
            (assets/(image.name+'.sha256')).write_text(f'{digest}  {image.name}\n')
            (assets/'SHA256SUMS').write_text(f'{digest}  {image.name}\n')
            tag=f'incus-dev-{candidate}'
            subprocess.run([sys.executable,str(ROOT/'scripts/ci/create-incus-image-manifest.py'),'--channel','dev','--tag',tag,'--commit',candidate,'--repository','example/labby','--directory',str(assets)],check=True)
            gh=work/'gh'; gh.write_text('#!/usr/bin/env python3\nimport os,sys,shutil,pathlib\na=sys.argv[1:]\nif "--pattern" in a: sys.exit(1)\nif a[:2]==["release","download"]:\n dest=pathlib.Path(a[a.index("--dir")+1]); dest.mkdir(exist_ok=True)\n for p in pathlib.Path(os.environ["ASSETS"]).iterdir(): shutil.copyfile(p,dest/p.name)\n'); gh.chmod(0o755)
            receipt=work/'receipt'
            env=dict(os.environ,GH_TOKEN='fixture',GITHUB_REPOSITORY='example/labby',GITHUB_SHA=candidate,RELEASE_TAG=tag,RELEASE_CHANNEL='dev',INCUS_POINTER_RECEIPT=str(receipt),GH_BIN=str(gh),ASSETS=str(assets))
            command=['bash',str(ROOT/'scripts/ci/promote-incus-pointer.sh')]
            stable=git('rev-parse','refs/tags/labby-incus-latest',cwd=remote)
            result=subprocess.run(command+['promote'],cwd=checkout,env=env,capture_output=True,text=True,timeout=30)
            if newer:
                self.assertNotEqual(0,result.returncode)
                self.assertEqual(previous,git('rev-parse','refs/tags/labby-incus-dev',cwd=remote))
                self.assertEqual(stable,git('rev-parse','refs/tags/labby-incus-latest',cwd=remote)); return
            self.assertEqual(0,result.returncode,result.stderr)
            self.assertEqual(candidate,git('rev-parse','refs/tags/labby-incus-dev',cwd=remote))
            bad=dict(env,RELEASE_CHANNEL='stable',RELEASE_TAG=f'incus-{candidate}')
            denied=subprocess.run(command+['rollback'],cwd=checkout,env=bad,capture_output=True,text=True)
            self.assertNotEqual(0,denied.returncode)
            subprocess.run(command+['rollback'],cwd=checkout,env=env,check=True,capture_output=True,timeout=30)
            self.assertEqual(stable,git('rev-parse','refs/tags/labby-incus-latest',cwd=remote))
            if absent:
                self.assertEqual('',git('ls-remote','origin','refs/tags/labby-incus-dev',cwd=checkout))
            else:
                self.assertEqual(previous,git('rev-parse','refs/tags/labby-incus-dev',cwd=remote))

    def test_dev_success_and_rollback_preserve_stable(self): self.transaction()
    def test_first_dev_rollback_removes_only_dev_pointer(self): self.transaction(absent=True)
    def test_newer_dev_pointer_blocks_stale_candidate(self): self.transaction(newer=True)

    def test_manifest_tag_and_channel_mismatch_is_rejected(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/ci/create-incus-image-manifest.py'),'--channel','dev','--tag','incus-'+'a'*40,'--commit','a'*40,'--repository','example/labby','--directory','/nonexistent'],capture_output=True,text=True)
        self.assertNotEqual(0,result.returncode)
        self.assertIn('tag',result.stderr)

if __name__=='__main__': unittest.main()
