#!/usr/bin/env python3
"""One reviewed pin inventory generates reusable and standalone trust code."""
import json
from pathlib import Path
root = Path(__file__).resolve().parents[2]
pins = json.loads((root / 'scripts/ci/github-verifier-bootstrap-pins.json').read_text())
assert pins['version'] == '2.102.0' and pins['release_immutable'] is True
platforms = {'linux_amd64.tar.gz': 'Linux/x86_64', 'linux_arm64.tar.gz': 'Linux/aarch64|Linux/arm64', 'macOS_arm64.zip': 'Darwin/arm64', 'windows_amd64.zip': 'MINGW*/x86_64|MSYS*/x86_64|CYGWIN*/x86_64', 'windows_arm64.zip': 'MINGW*/aarch64|MINGW*/arm64|MSYS*/aarch64|MSYS*/arm64'}
rows = []
for asset in pins['assets']:
    suffix = asset['platform_archive']
    if suffix not in platforms: continue
    digest = asset['sha256']
    assert len(digest) == 64 and all(c in '0123456789abcdef' for c in digest)
    folder = 'gh_2.102.0_' + suffix.removesuffix('.tar.gz').removesuffix('.zip')
    member = 'bin/gh.exe' if suffix.startswith('windows_') else folder + '/bin/gh'
    rows.append(f'        {platforms[suffix]}) verifier_asset="gh_2.102.0_{suffix}"; verifier_digest="{digest}"; verifier_member="{member}" ;;')
code = (root / 'scripts/ci/github-verifier-bootstrap.sh.in').read_text().replace('@PLATFORM_PINS@', '\n'.join(rows))
(root / 'scripts/ci/github-verifier-bootstrap.sh').write_text(code)
installer = root / 'scripts/install.sh'
source = installer.read_text()
begin, end = '# BEGIN GENERATED GITHUB VERIFIER BOOTSTRAP', '# END GENERATED GITHUB VERIFIER BOOTSTRAP'
block = begin + '\n' + code + end
if begin in source:
    first, rest = source.split(begin, 1)
    _, last = rest.split(end, 1)
    source = first + block + last
else:
    source = source.replace('# Require the verifier before downloading release bytes.', block + '\n\n# Require the verifier before downloading release bytes.', 1)
installer.write_text(source)

windows_rows = []
for asset in pins['assets']:
    platform = asset['platform_archive']
    if platform not in ('windows_amd64.zip', 'windows_arm64.zip'): continue
    architecture = 'X64' if platform == 'windows_amd64.zip' else 'Arm64'
    windows_rows.append(f"        '{architecture}' {{ return @{{ Url = 'https://github.com/cli/cli/releases/download/v2.102.0/gh_2.102.0_{platform}'; Sha256 = '{asset['sha256']}' }} }}")
ps_code = (root / 'scripts/ci/github-verifier-bootstrap.ps1.in').read_text().replace('@WINDOWS_PINS@', '\n'.join(windows_rows))
ps_installer = root / 'scripts/install.ps1'
ps_source = ps_installer.read_text()
ps_begin, ps_end = '# BEGIN GENERATED GITHUB VERIFIER BOOTSTRAP', '# END GENERATED GITHUB VERIFIER BOOTSTRAP'
ps_block = ps_begin + '\n' + ps_code + ps_end
if ps_begin in ps_source:
    first, rest = ps_source.split(ps_begin, 1)
    _, last = rest.split(ps_end, 1)
    ps_source = first + ps_block + last
else:
    ps_source = ps_source.replace('function Test-LabbyGitHubCliCommand {', ps_block + '\n\nfunction Test-LabbyGitHubCliCommand {', 1)
ps_installer.write_text(ps_source)
