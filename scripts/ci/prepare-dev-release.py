#!/usr/bin/env python3
"""Create an immutable version-only dev tag; release.yml owns qualification/publication."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
from release_channel import identity
from release_source import run, require_green_main, stamp, validate

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--tag', required=True)
parser.add_argument('--source', default='main')
parser.add_argument('--repository', required=True)
args = parser.parse_args()
root = Path.cwd()
release = identity(args.tag)
if release['channel'] != 'dev' or not args.tag.startswith('v'):
    raise SystemExit('manual dev release requires vMAJOR.MINOR.PATCH-dev.N')
if args.source != 'main' and not re.fullmatch(r'[0-9a-f]{40}', args.source):
    raise SystemExit('source must be main or a full commit SHA')
if run(root, 'git', 'status', '--porcelain'):
    raise SystemExit('dev release preparation requires a clean checkout')
run(root, 'git', 'fetch', '--no-tags', 'origin', 'refs/heads/main:refs/remotes/origin/main')
source = run(root, 'git', 'rev-parse', ('origin/main' if args.source == 'main' else args.source) + '^{commit}')
run(root, 'git', 'merge-base', '--is-ancestor', source, 'origin/main')
ci_run = require_green_main(root, source, args.repository)
if run(root, 'git', 'ls-remote', 'origin', 'refs/tags/' + args.tag):
    raise SystemExit('release tag already exists; immutable releases cannot be replaced')
run(root, 'git', 'checkout', '--quiet', '--detach', source)
old = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
if tuple(map(int, release['version'].split('-')[0].split('.'))) < tuple(map(int, old.split('-')[0].split('.'))):
    raise SystemExit('dev version core cannot precede the source workspace version')
stamp(root, release['version'])
run(root, 'git', 'add', '--all')
run(root, 'git', '-c', 'commit.gpgSign=false', 'commit', '-m', f'chore(release): prepare {args.tag}')
source_checked = validate(root, 'HEAD', args.tag)
if source_checked != source:
    raise SystemExit('dev source identity changed')
run(root, 'git', 'tag', args.tag)
# The workflow supplies GitHub CLI's supported environment credential helper.
# Do not force or replace tags, and never update a branch/default pointer.
run(root, 'git', 'push', 'origin', 'refs/tags/' + args.tag)
candidate = run(root, 'git', 'rev-parse', 'HEAD')
remote = run(root, 'git', 'ls-remote', 'origin', 'refs/tags/' + args.tag).split()
if not remote or remote[0] != candidate:
    raise SystemExit('published dev tag did not match the prepared candidate; do not retry blindly')
print(json.dumps({'tag': args.tag, 'source_sha': source, 'candidate_sha': candidate, 'source_ci_run': ci_run}))
