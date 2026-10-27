#!/usr/bin/env python3
"""Maintain one Release Please-style dev request PR; consume only qualified merges."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
from release_source import require_green_main, validate, stamp

BRANCH = 'release-please--branches--main--components--labby-dev'
STABLE = 'release-please--branches--main--components--labby'
REQUEST = '.release-please-dev.json'
SHA = re.compile(r'[0-9a-f]{40}')
DEV = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-dev\.([1-9][0-9]*)')


def command(root, *args, data=None, env=None):
    result = subprocess.run(args, cwd=root, input=data, env=env, text=True,
                            capture_output=True, timeout=180)
    if result.returncode:
        raise ValueError(f'{args[0]} operation failed: {result.stderr[-2000:]}')
    if len(result.stdout) > 8 * 1024 * 1024:
        raise ValueError('operation output exceeds 8 MiB budget')
    return result.stdout.strip()


class Coordinator:
    def __init__(self, root, repository):
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repository):
            raise ValueError('invalid repository')
        self.root, self.repository = root, repository
        self.retired_head = None

    def git(self, *args, **kwargs):
        return command(self.root, 'git', *args, **kwargs)

    def api(self, endpoint):
        return json.loads(command(self.root, 'gh', 'api', f'repos/{self.repository}/{endpoint}'))

    def fetch(self, ref):
        self.git('fetch', '--no-tags', 'origin', ref)
        return self.git('rev-parse', 'FETCH_HEAD^{commit}')

    def request(self, commit):
        value = json.loads(self.git('show', f'{commit}:{REQUEST}'))
        if (not isinstance(value, dict) or set(value) != {'version', 'source_sha'}
                or not isinstance(value['version'], str) or not DEV.fullmatch(value['version'])
                or not isinstance(value['source_sha'], str) or not SHA.fullmatch(value['source_sha'])):
            raise ValueError('invalid dev release request schema')
        return value

    def canonical(self, pr, branch, merged=False):
        if (pr['head']['ref'] != branch or pr['base']['ref'] != 'main'
                or pr['head']['repo']['full_name'] != self.repository
                or pr['base']['repo']['full_name'] != self.repository
                or bool(pr.get('merged_at')) != merged):
            raise ValueError('release request PR must use canonical same-repository branches')

    def open_pr(self, branch):
        owner = self.repository.split('/')[0]
        prs = self.api(f'pulls?state=open&base=main&head={owner}:{branch}&per_page=100')
        if len(prs) > 1:
            raise ValueError('multiple canonical release PRs')
        if prs:
            self.canonical(prs[0], branch)
        return prs[0] if prs else None

    def merged_request(self, source):
        prs = self.api(f'commits/{source}/pulls?per_page=100')
        if len(prs) >= 100:
            raise ValueError('commit PR discovery incomplete')
        matches = [p for p in prs if p.get('merged_at') and p.get('merge_commit_sha') == source
                   and p['head']['ref'] == BRANCH]
        if len(matches) > 1:
            raise ValueError('ambiguous merged dev requests')
        if not matches:
            return None
        pr = self.api(f'pulls/{matches[0]["number"]}')
        self.canonical(pr, BRANCH, merged=True)
        if pr['merge_commit_sha'] != source:
            raise ValueError('merged PR identity changed')
        files = self.api(f'pulls/{pr["number"]}/files?per_page=100')
        if len(files) != 1 or files[0]['filename'] != REQUEST or files[0]['status'] not in ('added', 'modified'):
            raise ValueError('dev request PR must change only the request file')
        head = self.fetch(pr['head']['sha'])
        if head != pr['head']['sha']:
            raise ValueError('PR head identity changed')
        if self.git('rev-parse', f'{head}:{REQUEST}') != self.git('rev-parse', f'{source}:{REQUEST}'):
            raise ValueError('merged request blob differs from reviewed PR head')
        return self.request(source)

    def pending_merge(self, source):
        rows = []
        for page in range(1, 11):
            batch = self.api(f'pulls?state=closed&base=main&head={self.repository.split("/")[0]}:{BRANCH}&sort=updated&direction=desc&per_page=100&page={page}')
            rows.extend(p for p in batch if p.get('merged_at') and p['head']['ref'] == BRANCH)
            if len(batch) < 100:
                break
        else:
            raise ValueError('merged request lookup exceeded pagination budget')
        if not rows:
            return None
        pr = max(rows, key=lambda p: (p['merged_at'], p['number']))
        self.canonical(pr, BRANCH, merged=True)
        merged = pr['merge_commit_sha']
        self.fetch(merged)
        self.git('merge-base', '--is-ancestor', merged, source)
        require_green_main(self.root, merged, self.repository)
        request = self.merged_request(merged)
        if request is None:
            raise ValueError('merged dev request association unavailable')
        self.retired_head = pr['head']['sha']
        return self.consume(merged, request)

    def consume(self, source, request):
        recorded = request['source_sha']
        self.git('merge-base', '--is-ancestor', recorded, 'origin/main')
        require_green_main(self.root, recorded, self.repository)
        tag = 'v' + request['version']
        remote = self.git('ls-remote', 'origin', f'refs/tags/{tag}')
        if remote:
            candidate = self.fetch(f'refs/tags/{tag}')
            if validate(self.root, candidate, tag, repository=self.repository) != recorded:
                raise ValueError('existing dev tag has another source parent')
            return dict(action='already-tagged', tag=tag, source_sha=recorded)
        command(self.root, sys.executable, str(self.root/'scripts/ci/prepare-dev-release.py'),
                '--tag', tag, '--source', recorded, '--repository', self.repository)
        return dict(action='tagged', tag=tag, source_sha=recorded)

    def version_core(self, source):
        stable = self.open_pr(STABLE)
        if stable:
            head = self.fetch(stable['head']['sha'])
            if head != stable['head']['sha']:
                raise ValueError('stable PR head identity changed')
            core = json.loads(self.git('show', f'{head}:.release-please-manifest.json'))['.']
        else:
            value = tomllib.loads(self.git('show', source + ':Cargo.toml'))['workspace']['package']['version']
            major, minor, patch = map(int, value.split('.'))
            core = f'{major}.{minor}.{patch + 1}'
        if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', core):
            raise ValueError('stable release core must be canonical')
        return core

    def normalized_tree(self, source):
        with tempfile.TemporaryDirectory(prefix='labby-dev-product-') as scratch:
            checkout = Path(scratch)/'source'
            command(self.root, 'git', 'clone', '--quiet', '--shared', '--no-checkout', str(self.root), str(checkout))
            command(checkout, 'git', 'checkout', '--quiet', '--detach', source)
            stamp(checkout, '2.0.0')
            for path in (REQUEST, 'CHANGELOG.md'):
                (checkout/path).unlink(missing_ok=True)
            command(checkout, 'git', 'add', '--all')
            return command(checkout, 'git', 'write-tree')

    def product_changed(self, source):
        releases = []
        for page in range(1, 11):
            batch = self.api(f'releases?per_page=100&page={page}')
            for release in batch:
                tag = release['tag_name']
                match = DEV.fullmatch(tag[1:]) if tag.startswith('v') else None
                if match and not release['draft'] and release['prerelease']:
                    releases.append((tuple(map(int, match.groups())), tag))
            if len(batch) < 100:
                break
        else:
            raise ValueError('published dev release discovery incomplete')
        if not releases:
            return True
        tag = max(releases)[1]
        candidate = self.fetch('refs/tags/' + tag)
        baseline = validate(self.root, candidate, tag, repository=self.repository)
        return self.normalized_tree(source) != self.normalized_tree(baseline)

    def pr_details(self, request):
        return (f'chore(release): request v{request["version"]}',
                f'Development release request for green main source {request["source_sha"]}. Human merge approves this recorded source; stable version and changelog are unchanged.')

    def create_pr(self, request):
        title, body = self.pr_details(request)
        return command(self.root, 'gh', 'pr', 'create', '--repo', self.repository,
                       '--head', BRANCH, '--base', 'main', '--title', title, '--body', body)

    def repair_pr(self, pr, request):
        title, body = self.pr_details(request)
        command(self.root, 'gh', 'pr', 'edit', str(pr['number']), '--repo', self.repository,
                '--title', title, '--body', body)

    def recover_generated(self, head, source):
        request = self.request(head)
        recorded = request['source_sha']
        parents = self.git('rev-list', '--parents', '-n', '1', head).split()[1:]
        canonical = json.dumps(request, indent=2, sort_keys=True)
        expected = [recorded] + ([self.retired_head] if self.retired_head else [])
        if (parents != expected
                or self.git('diff', '--name-only', recorded, head) != REQUEST
                or self.git('show', f'{head}:{REQUEST}') != canonical
                or self.git('show', '-s', '--format=%an <%ae>%n%cn <%ce>', head) != 'Labby release bot <release@users.noreply.github.com>\nLabby release bot <release@users.noreply.github.com>'
                or self.git('show', '-s', '--format=%B', head) != f'chore(release): request v{request["version"]}'):
            raise ValueError('orphan dev branch is not an exact generated request')
        self.git('merge-base', '--is-ancestor', recorded, source)
        require_green_main(self.root, recorded, self.repository)
        url = self.create_pr(request)
        match = re.fullmatch(r'https://github.com/' + re.escape(self.repository) + r'/pull/([1-9][0-9]*)', url)
        if not match:
            raise ValueError('created request PR identity unavailable; do not retry blindly')
        return dict(number=int(match[1]), html_url=url, head=dict(sha=head))

    def refresh(self, source):
        # A late successful event must never replace a newer request.
        if source != self.git('rev-parse', 'origin/main'):
            return dict(action='stale', source_sha=source)
        parents = self.git('rev-list', '--parents', '-n', '1', source).split()[1:]
        if parents and self.git('diff', '--name-only', parents[0], source) == REQUEST:
            return dict(action='metadata-only', source_sha=source)
        if not self.product_changed(source):
            return dict(action='metadata-only', source_sha=source)
        pr = self.open_pr(BRANCH)
        remote = self.git('ls-remote', '--heads', 'origin', 'refs/heads/' + BRANCH)
        previous = self.fetch('refs/heads/' + BRANCH) if remote else None
        pending = None
        if previous:
            if not pr and previous != self.retired_head:
                pr = self.recover_generated(previous, source)
            if pr and pr['head']['sha'] != previous:
                raise ValueError('canonical branch differs from open PR head')
            base = self.git('merge-base', source, previous)
            if previous != self.retired_head and self.git('diff', '--name-only', base, previous) != REQUEST:
                raise ValueError('canonical dev branch contains unknown edits')
            pending = self.request(previous)
        core = self.version_core(source)
        revisions = []
        for line in self.git('ls-remote', '--tags', '--refs', 'origin').splitlines():
            tag = line.split()[1].removeprefix('refs/tags/v')
            match = DEV.fullmatch(tag)
            if match and tag.startswith(core + '-dev.'):
                revisions.append(int(match[4]))
        revision = max(revisions, default=0) + 1
        if pr and pending and pending['version'].startswith(core + '-dev.'):
            revision = max(revision, int(pending['version'].rsplit('.', 1)[1]))
        request = dict(version=f'{core}-dev.{revision}', source_sha=source)
        if pr and pending == request:
            self.repair_pr(pr, request)
            return dict(action='unchanged', **pending)
        content = json.dumps(request, indent=2, sort_keys=True) + '\n'
        with tempfile.TemporaryDirectory(prefix='labby-dev-pr-') as scratch:
            env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch)/'index'))
            self.git('read-tree', source, env=env)
            blob = self.git('hash-object', '-w', '--stdin', data=content)
            self.git('update-index', '--add', '--cacheinfo', f'100644,{blob},{REQUEST}', env=env)
            tree = self.git('write-tree', env=env)
            parents = ['-p', source] + (['-p', previous] if previous else [])
            identity = dict(os.environ, GIT_AUTHOR_NAME='Labby release bot', GIT_AUTHOR_EMAIL='release@users.noreply.github.com',
                            GIT_COMMITTER_NAME='Labby release bot', GIT_COMMITTER_EMAIL='release@users.noreply.github.com')
            commit = self.git('commit-tree', tree, *parents, env=identity,
                              data=f'chore(release): request v{request["version"]}\n')
        # Recheck immediately before the only branch publication. No force push.
        if self.git('ls-remote', 'origin', 'refs/heads/main').split()[0] != source:
            return dict(action='stale', source_sha=source)
        self.git('push', 'origin', f'{commit}:refs/heads/{BRANCH}')
        if self.git('ls-remote', 'origin', 'refs/heads/' + BRANCH).split()[0] != commit:
            raise ValueError('published request head mismatch; do not retry blindly')
        if pr:
            self.repair_pr(pr, request)
            url = pr['html_url']
        else:
            url = self.create_pr(request)
        return dict(action='requested', pr=url, **request)

    def execute(self, source):
        if not SHA.fullmatch(source):
            raise ValueError('source must be a full green main commit SHA')
        self.git('fetch', '--no-tags', 'origin', 'refs/heads/main:refs/remotes/origin/main')
        if self.git('rev-parse', source + '^{commit}') != source:
            raise ValueError('source did not resolve exactly')
        self.git('merge-base', '--is-ancestor', source, 'origin/main')
        require_green_main(self.root, source, self.repository)
        merged = self.merged_request(source)
        consumed = self.consume(source, merged) if merged else None
        latest = self.git('rev-parse', 'origin/main')
        if source != latest:
            try:
                require_green_main(self.root, latest, self.repository)
            except ValueError as error:
                if str(error) not in ('source has no main push CI qualification', 'latest source main CI must complete successfully'):
                    raise
                return consumed or dict(action='stale', source_sha=source)
            source = latest
            merged = self.merged_request(source)
            if merged:
                return self.consume(source, merged)
        elif consumed:
            return consumed
        pending = self.pending_merge(source)
        outcome = self.refresh(source)
        if pending or consumed:
            outcome['consumed'] = pending or consumed
        return outcome


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--source', required=True)
    args = parser.parse_args()
    print(json.dumps(Coordinator(Path.cwd(), args.repository).execute(args.source)))


if __name__ == '__main__':
    main()
