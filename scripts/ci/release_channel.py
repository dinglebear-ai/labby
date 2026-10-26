#!/usr/bin/env python3
"""Strict release identity shared by qualification and publication."""
import json
import re
import sys

PATTERN = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-dev\.([1-9][0-9]*))?')


def identity(value):
    version = value.removeprefix('v')
    match = PATTERN.fullmatch(version)
    if not match:
        raise ValueError('release must be canonical X.Y.Z or X.Y.Z-dev.N (N positive)')
    dev = match[4] is not None
    return dict(channel='dev' if dev else 'stable', version=version,
                npm_tag='dev' if dev else 'latest', prerelease=dev)


def order(value):
    item = identity(value)
    match = PATTERN.fullmatch(item['version'])
    return (*map(int, match.group(1, 2, 3)), 0 if match[4] else 1, int(match[4] or 0))


if __name__ == '__main__':
    print(json.dumps(identity(sys.argv[1]), sort_keys=True))
