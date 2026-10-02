"""Behavioral regressions for historical release observation."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from scripts.ci.mcp_registry_canonical import matches_legacy_manifest
import hashlib

ROOT = Path(__file__).resolve().parents[2]

class HistoricalObserverTests(unittest.TestCase):
    def observe(self, historical=False, incus=False, bad_checksum=False):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            assets = work / 'assets'
            assets.mkdir()
            dist = {'github': {'repository': 'a/b', 'tag': 'v1.0.0'},
                    'npm': {'package': '@a/b', 'tag': 'latest', 'version': '1.0.0'},
                    'mcp': {'name': 'a/b', 'version': '1.0.0', 'manifest_sha256': 'x'}}
            if incus:
                import hashlib
                image = assets / 'image.tar.xz'
                image.write_bytes(b'image')
                digest = hashlib.sha256(image.read_bytes()).hexdigest()
                dist['incus'] = {'asset': image.name, 'sha256': digest}
                (assets / 'image.tar.xz.sha256').write_text(('0'*64 if bad_checksum else digest)+'  image.tar.xz\n')
            (assets / 'image.spdx.json').write_text(json.dumps({'spdxVersion':'SPDX-2.3','SPDXID':'SPDXRef-DOCUMENT','packages':[]}))
            (assets / 'surprise.txt').write_text('unexpected')
            manifest = work / 'manifest.json'
            manifest.write_text(json.dumps({'tag':'v1.0.0','repository':'a/b','subjects':[], 'distributions':dist}))
            mock = work / 'mock'
            mock.write_text('#!/usr/bin/env python3\nimport sys,json\na=sys.argv[1:]\nif a[0]=="release": print("false")\nelif a[0]=="view": print(json.dumps("1.0.0" if a[1].endswith("@1.0.0") else "2.0.0"))\nelse: print(json.dumps({"server":{"name":"a/b","version":"1.0.0"}}))\n')
            mock.chmod(0o755)
            out = work / 'observed.json'
            env = dict(os.environ, GH_BIN=str(mock), NPM_BIN=str(mock), CURL_BIN=str(mock))
            subprocess.run(['python3', str(ROOT/'scripts/ci/observe-release.py'), '--manifest',str(manifest),'--assets',str(assets),'--output',str(out)]+(['--historical'] if historical else []), env=env, check=True)
            return json.loads(out.read_text()),dist

    def test_history_checks_exact_npm_version(self):
        result, dist = self.observe(historical=True)
        self.assertEqual(result['distributions']['npm'],dist['npm'])

    def test_promotion_still_requires_current_dist_tag(self):
        result, dist = self.observe()
        self.assertNotEqual(result['distributions']['npm'],dist['npm'])

    def test_legacy_sidecars_only_recognized_for_incus(self):
        result,_ = self.observe(incus=True)
        self.assertEqual(result['unexpected_assets'], ['surprise.txt'])
        result,_ = self.observe()
        self.assertIn('image.spdx.json',result['unexpected_assets'])

    def test_wrong_checksum_remains_failure(self):
        result,_ = self.observe(incus=True,bad_checksum=True)
        self.assertIn('image.tar.xz.sha256',result['unexpected_assets'])

class LegacyManifestTests(unittest.TestCase):
    def test_default_omission_matches_bound_legacy_source(self):
        source = {'name':'a/b','packages':[{'isRequired':False}]}
        served = {'name':'a/b','packages':[{}]}
        digest = hashlib.sha256(json.dumps(source,sort_keys=True,separators=(',',':')).encode()).hexdigest()
        self.assertTrue(matches_legacy_manifest(source,served,digest))
        self.assertFalse(matches_legacy_manifest(source,served,'0'*64))
        self.assertFalse(matches_legacy_manifest(source,dict(served,name='evil'),digest))

if __name__ == '__main__': unittest.main()
