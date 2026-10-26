#!/usr/bin/env python3
"""Validate immutable release sources and reproduce development metadata trees."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
from release_channel import identity


def run(root, *args, timeout=60):
    return subprocess.check_output(args, cwd=root, text=True, stderr=subprocess.PIPE, timeout=timeout).strip()


def require_green_main(root, source, repository):
    if not re.fullmatch(r'[0-9a-f]{40}', source):
        raise ValueError('source must resolve to a full commit SHA')
    rows = []
    for page in range(1, 11):
        endpoint = (f'repos/{repository}/actions/workflows/ci.yml/runs?event=push'
                    f'&branch=main&head_sha={source}&per_page=100&page={page}')
        data = json.loads(run(root, 'gh', 'api', endpoint))
        rows.extend(r for r in data['workflow_runs'] if r['head_sha'] == source
                    and r['head_branch'] == 'main' and r['event'] == 'push')
        if len(data['workflow_runs']) < 100:
            break
    else:
        raise ValueError('source CI lookup exceeded pagination budget')
    if not rows:
        raise ValueError('source has no main push CI qualification')
    latest = max(rows, key=lambda row: (row['run_number'], row.get('run_attempt', 1)))
    if latest['status'] != 'completed' or latest['conclusion'] != 'success':
        raise ValueError('latest source main CI must complete successfully')
    return latest['id']


def stamp(root, version):
    helper = root / 'scripts/ci/sync-release-version.py'
    if not helper.is_file():
        raise ValueError('selected source predates development-channel support')
    run(root, sys.executable, str(helper), str(root), version, timeout=120)


def validate(root, candidate, tag, main_ref='origin/main', repository=None):
    release = identity(tag)
    if not tag.startswith('v'):
        raise ValueError('release tags must start with v')
    commit = run(root, 'git', 'rev-parse', candidate + '^{commit}')
    if release['channel'] == 'stable':
        run(root, 'git', 'merge-base', '--is-ancestor', commit, main_ref)
        return commit
    parents = run(root, 'git', 'rev-list', '--parents', '-n', '1', commit).split()
    if len(parents) != 2:
        raise ValueError('dev candidate must be a single version-only child of main')
    source = parents[1]
    run(root, 'git', 'merge-base', '--is-ancestor', source, main_ref)
    with tempfile.TemporaryDirectory(prefix='labby-dev-source-') as temp:
        checkout = Path(temp) / 'source'
        run(root, 'git', 'clone', '--quiet', '--shared', '--no-checkout', str(root), str(checkout))
        run(checkout, 'git', 'checkout', '--quiet', '--detach', source)
        stamp(checkout, release['version'])
        run(checkout, 'git', 'add', '--all')
        expected = run(checkout, 'git', 'write-tree')
    if expected != run(root, 'git', 'rev-parse', commit + '^{tree}'):
        raise ValueError('dev candidate differs from the exact version-only source transformation')
    if repository:
        require_green_main(root, source, repository)
    return source


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--candidate', default='HEAD')
    parser.add_argument('--main-ref', default='origin/main')
    parser.add_argument('--repository', default=os.environ.get('GITHUB_REPOSITORY'))
    args = parser.parse_args()
    print(validate(Path.cwd(), args.candidate, args.tag, args.main_ref, args.repository))
