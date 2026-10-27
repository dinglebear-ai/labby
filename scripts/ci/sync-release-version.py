#!/usr/bin/env python3
"""Stamp only owned product version metadata, without dependency resolution."""
import json
from pathlib import Path
import re
import runpy
import sys
import tomllib
try:
    from release_channel import identity
except ModuleNotFoundError:
    from scripts.ci.release_channel import identity


def sync(root, value):
    version = identity(value)['version']
    def patch(path, pattern, replacement, count=1):
        target = root / path
        text, found = re.subn(pattern, replacement, target.read_text(), count=count, flags=re.M)
        if found != count:
            raise ValueError(f'{path}: expected {count} version substitutions, got {found}')
        target.write_text(text)
    def document(path, mutate):
        target = root / path
        item = json.loads(target.read_text())
        mutate(item)
        target.write_text(json.dumps(item, indent=2) + '\n')
    # Restrict each substitution to its owning TOML section.
    def toml_version(path, section):
        target = root / path
        text = target.read_text()
        pattern = r'(' + re.escape('[' + section + ']') + r'\n[^\[]*?\bversion = ")[^"]+(")'
        changed, found = re.subn(pattern, lambda m: m[1] + version + m[2], text, count=1)
        if found != 1:
            raise ValueError(f'{path}: missing {section} version')
        target.write_text(changed)
    toml_version('Cargo.toml', 'workspace.package')
    toml_version('apps/tauri/src-tauri/Cargo.toml', 'package')
    patch('tools/verification/hosts/labby/Cargo.toml', r'(\blabby-model = \{ version = "=)[^"]+(")', lambda m: m[1]+version+m[2])
    for path in ('apps/tauri/package.json', 'apps/tauri/src-tauri/tauri.conf.json'):
        document(path, lambda item: item.update(version=version))
    document('.release-please-manifest.json', lambda item: item.update({'.': version}))
    for path in ('apm.yml', 'plugins/labby/apm.yml'):
        patch(path, r'^(version: )[^\s]+', lambda m: m[1]+version)
    runpy.run_path(str(root / 'scripts/sync-npm-release-version.py'))['sync'](root, version)
    npm = json.loads((root / 'packages/labby-mcp/package.json').read_text())['name']
    def registry(item):
        item['version'] = version
        for package in item.get('packages', []):
            package['version'] = version
        publisher = item.get('_meta', {}).get('io.modelcontextprotocol.registry/publisher-provided', {})
        if isinstance(publisher.get('distribution'), dict):
            publisher['distribution']['npm'] = f'{npm}@{version}'
        if isinstance(publisher.get('buildInfo'), dict):
            publisher['buildInfo']['version'] = version
    document('server.json', registry)
    # Resolve release ownership from the root workspace inheritance policy.
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    owned = set()
    for member in workspace['members']:
        for directory in root.glob(member):
            package = tomllib.loads((directory / 'Cargo.toml').read_text())['package']
            if package.get('version') == {'workspace': True}:
                owned.add(package['name'])
    desktop = tomllib.loads((root / 'apps/tauri/src-tauri/Cargo.toml').read_text())['package']['name']
    for path in ('Cargo.lock', 'apps/tauri/src-tauri/Cargo.lock', 'tools/verification/Cargo.lock'):
        target = root / path
        blocks = target.read_text().split('[[package]]')
        for index in range(1, len(blocks)):
            data = tomllib.loads('[[package]]' + blocks[index])['package'][0]
            if 'source' not in data and data['name'] in owned | {desktop}:
                blocks[index], found = re.subn(r'(^version = ")[^"]+("$)', lambda m: m[1]+version+m[2], blocks[index], count=1, flags=re.M)
                if found != 1:
                    raise ValueError(f'{path}: missing package version')
        target.write_text('[[package]]'.join(blocks))


if __name__ == '__main__':
    sync(Path(sys.argv[1]), sys.argv[2])
