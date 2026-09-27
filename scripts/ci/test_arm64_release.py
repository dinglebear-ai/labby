"""Exercise agreement between native release targets and installer selection."""
import json
from pathlib import Path
import re
import subprocess
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[2]

class Arm64ReleaseTests(unittest.TestCase):
    def test_native_release_and_installers_select_the_same_arm64_archive(self):
        release = yaml.load((ROOT / '.github/workflows/release.yml').read_text(), Loader=yaml.BaseLoader)
        rows = release['jobs']['build']['strategy']['matrix']['include']
        row = next(r for r in rows if r['target'] == 'aarch64-unknown-linux-gnu')
        self.assertEqual('ubuntu-24.04-arm', json.loads(row['runner']))
        source = (ROOT / 'scripts/install.sh').read_text()
        function = re.search(r'target_triple\(\) \{.*?\n\}', source, re.S).group()
        for arch in ('aarch64', 'arm64'):
            shell = 'uname() { if [ "$1" = -s ]; then echo Linux; else echo '+arch+'; fi; }; fail() { exit 1; };\n'+function+'\ntarget_triple'
            result = subprocess.run(['sh', '-c', shell], capture_output=True, text=True, check=True)
            self.assertEqual(row['archive'], 'lab-'+result.stdout.strip()+'.tar.gz')
        result = subprocess.run(['node', '-e', 'console.log(require("./packages/labby-mcp/lib/platform").targetFor("linux", "arm64").asset)'], cwd=ROOT, capture_output=True, text=True, check=True)
        self.assertEqual(row['archive'], result.stdout.strip())

if __name__ == '__main__':
    unittest.main()
