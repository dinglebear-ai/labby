"""Real Git and fake GitHub API fixtures for the dev request coordinator."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import shutil
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT/'scripts/ci'))

class DevReleasePR(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name); self.repo = self.home/'repo'; self.repo.mkdir()
        self.git('init', '-q', '-b', 'main'); self.git('config','user.name','Fixture'); self.git('config','user.email','fixture@example.invalid')
        (self.repo/'Cargo.toml').write_text('[workspace.package]\nversion="2.6.2"\n')
        helpers=self.repo/'scripts/ci';helpers.mkdir(parents=True)
        for helper in ['prepare-dev-release.py','release_source.py','release_channel.py']:
            shutil.copyfile(ROOT/'scripts/ci'/helper,helpers/helper)
        (helpers/'sync-release-version.py').write_text('import pathlib,re,sys\np=pathlib.Path(sys.argv[1])/"Cargo.toml"\np.write_text(re.sub(r\'version="[^"]+"\', f\'version="{sys.argv[2]}"\', p.read_text()))\n')
        self.git('add','.'); self.git('commit','-qm','product'); self.source=self.git('rev-parse','HEAD')
        self.origin=self.home/'origin.git'; subprocess.run(['git','init','--bare','-q',str(self.origin)],check=True)
        self.git('remote','add','origin',str(self.origin)); self.git('push','-q','origin','main')
        self.tools=self.home/'tools'; self.tools.mkdir(); self.state=self.home/'state.json'
        self.state.write_text(json.dumps({'source':self.source,'prs':[],'merged':[],'calls':[]}))
        gh=self.tools/'gh'; gh.write_text('''#!/usr/bin/env python3
import json,os,sys,urllib.parse
p=os.environ['FIXTURE_STATE'];d=json.load(open(p));a=sys.argv[1:];d['calls'].append(a)
endpoint=a[1] if a[0]=='api' else ''
if '/actions/workflows/' in endpoint and urllib.parse.parse_qs(endpoint.split('?',1)[1])['head_sha'][0] == d.get('malformed_ci_sha'): print('not JSON');raise SystemExit(0)
elif '/actions/workflows/' in endpoint: out={'workflow_runs':[dict(id=1,head_sha=urllib.parse.parse_qs(endpoint.split('?',1)[1])['head_sha'][0],head_branch='main',event='push',status='completed',conclusion=d.get('ci_by_sha',{}).get(urllib.parse.parse_qs(endpoint.split('?',1)[1])['head_sha'][0],d.get('ci_conclusion','success')),run_number=1)]}
elif '/commits/' in endpoint and endpoint.endswith('/pulls?per_page=100'): out=d.get('merged_by_sha',{}).get(endpoint.split('/commits/')[1].split('/')[0],d['merged'])
elif endpoint.endswith('/files?per_page=100'): out=d.get('files',[])
elif '/pulls/' in endpoint: out=d.get('detail',{})
elif '/releases?' in endpoint: out=d.get('releases',[])
elif '/pulls?state=closed' in endpoint: out=d.get('closed',[])
elif '/pulls?' in endpoint: out=[p for p in d['prs'] if (':'+p['head']['ref']+'&') in endpoint]
elif a[0:2]==['pr','edit']: out=''
elif a[0:2]==['pr','create']:
 if d.get('create_failure'): json.dump(d,open(p,'w'));raise SystemExit('fixture PR create failure')
 out='https://github.com/example/labby/pull/3'
else: raise SystemExit('unexpected fake gh call: '+repr(a))
json.dump(d,open(p,'w'));print(json.dumps(out) if not isinstance(out,str) else out)
''');gh.chmod(0o755)
        self.env=dict(os.environ,PATH=str(self.tools)+os.pathsep+os.environ['PATH'],FIXTURE_STATE=str(self.state))

    def git(self,*args):
        return subprocess.check_output(['git',*args],cwd=self.repo,text=True,stderr=subprocess.PIPE,timeout=15).strip()

    def execute(self):
        return subprocess.run([sys.executable,str(ROOT/'scripts/ci/dev_release_pr.py'),'--repository','example/labby','--source',self.source],cwd=self.repo,env=self.env,text=True,capture_output=True,timeout=30)

    def test_first_request_uses_next_patch_and_only_metadata(self):
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        head=self.git('rev-parse','refs/remotes/origin/'+ 'release-please--branches--main--components--labby-dev')
        self.assertEqual(self.git('diff','--name-only',self.source,head),'.release-please-dev.json')
        request=json.loads(self.git('show',head+':.release-please-dev.json'))
        self.assertEqual(request,{'version':'2.6.3-dev.1','source_sha':self.source})

    def test_remote_tags_use_numeric_max_and_stale_source_no_write(self):
        for tag in ['v2.6.3-dev.9','v2.6.3-dev.10']:
            self.git('checkout','-q','--detach',self.source)
            subprocess.run([sys.executable,str(self.repo/'scripts/ci/sync-release-version.py'),str(self.repo),tag[1:]],check=True)
            self.git('add','.');self.git('commit','-qm','version');self.git('tag',tag);self.git('push','-q','origin','refs/tags/'+tag)
        self.git('checkout','-q','main')
        (self.repo/'new-feature').write_text('new');self.git('add','.');self.git('commit','-qm','feature')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.11')

    def test_stale_green_event_does_not_open_request(self):
        (self.repo/'product').write_text('new');self.git('add','.');self.git('commit','-qm','new main');self.git('push','-q','origin','main')
        latest=self.git('rev-parse','HEAD');self.state_update(ci_by_sha={latest:'failure'})
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'stale')
        self.assertFalse(any(c[:2]==['pr','create'] for c in json.loads(self.state.read_text())['calls']))

    def state_update(self, **updates):
        state=json.loads(self.state.read_text());state.update(updates);self.state.write_text(json.dumps(state))

    def pr(self, head, branch='release-please--branches--main--components--labby-dev', **extra):
        value=dict(number=3,head=dict(ref=branch,sha=head,repo=dict(full_name='example/labby')),base=dict(ref='main',repo=dict(full_name='example/labby')),merged_at=None,html_url='https://github.com/example/labby/pull/3')
        value.update(extra);return value

    def initial_request(self):
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        head=self.git('rev-parse','refs/remotes/origin/release-please--branches--main--components--labby-dev')
        self.state_update(prs=[self.pr(head)]);return head

    def test_rerun_preserves_pending_version_and_fast_forward_refresh(self):
        previous=self.initial_request()
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'unchanged')
        (self.repo/'product').write_text('new source');self.git('add','.');self.git('commit','-qm','feature')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.1')
        head=self.git('rev-parse','refs/remotes/origin/release-please--branches--main--components--labby-dev')
        self.git('merge-base','--is-ancestor',previous,head)
        self.assertEqual(self.git('diff','--name-only',self.source,head),'.release-please-dev.json')

    def test_unknown_branch_edits_are_preserved_and_rejected(self):
        previous=self.initial_request();self.git('checkout','-q','--detach',previous)
        (self.repo/'foreign').write_text('must preserve');self.git('add','.');self.git('commit','-qm','foreign edit')
        head=self.git('rev-parse','HEAD');self.git('push','-q','origin','HEAD:refs/heads/release-please--branches--main--components--labby-dev')
        self.state_update(prs=[self.pr(head)])
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('unknown edits',result.stderr)
        self.assertEqual(self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0],head)

    def test_stable_pr_manifest_head_controls_core(self):
        (self.repo/'.release-please-manifest.json').write_text('{".":"2.7.0"}')
        self.git('add','.');self.git('commit','-qm','stable version')
        head=self.git('rev-parse','HEAD');self.git('push','-q','origin','HEAD:refs/heads/release-please--branches--main--components--labby')
        self.state_update(prs=[self.pr(head,'release-please--branches--main--components--labby')])
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['version'],'2.7.0-dev.1')

    def test_metadata_only_main_event_does_not_create_release_loop(self):
        (self.repo/'.release-please-dev.json').write_text(json.dumps(dict(version='2.6.3-dev.1',source_sha=self.source)))
        self.git('add','.');self.git('commit','-qm','merged metadata')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'metadata-only')

    def test_merged_request_requires_only_reviewed_file_and_same_repo(self):
        previous=self.initial_request()
        self.git('checkout','-q','main');self.git('merge','--no-ff','-m','merge request',previous)
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main')
        merged=self.pr(previous,merged_at='2026-10-27T00:00:00Z',merge_commit_sha=self.source)
        self.state_update(source=self.source,merged=[merged],detail=merged,files=[dict(filename='foreign',status='added')])
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('only the request file',result.stderr)
        merged['head']['repo']['full_name']='foreign/labby'
        self.state_update(detail=merged,files=[dict(filename='.release-please-dev.json',status='added')])
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('same-repository',result.stderr)

    def merge_request(self):
        head=self.initial_request();recorded=self.source
        self.git('checkout','-q','main');self.git('merge','--no-ff','-m','merge request',head)
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main')
        merged=self.pr(head,merged_at='2026-10-27T00:00:00Z',merge_commit_sha=self.source)
        self.state_update(source=self.source,merged=[merged],detail=merged,closed=[merged],prs=[],files=[dict(filename='.release-please-dev.json',status='added')])
        return recorded

    def test_human_merge_tags_recorded_source_and_existing_tag_rerun_is_validated(self):
        recorded=self.merge_request()
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'tagged')
        candidate=self.git('rev-parse','v2.6.3-dev.1')
        self.assertEqual(self.git('rev-parse',candidate+'^'),recorded)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'already-tagged')

    def test_existing_tag_with_wrong_source_is_never_overwritten(self):
        self.merge_request();self.git('tag','v2.6.3-dev.1',self.source);self.git('push','-q','origin','refs/tags/v2.6.3-dev.1')
        result=self.execute();self.assertNotEqual(result.returncode,0)
        self.assertEqual(self.git('ls-remote','origin','refs/tags/v2.6.3-dev.1').split()[0],self.source)

    def test_newer_green_event_consumes_prior_merged_request_first(self):
        recorded=self.merge_request()
        (self.repo/'feature').write_text('later');self.git('add','.');self.git('commit','-qm','later product')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source,merged=[])
        # Association lookup must retain the older merged request.
        state=json.loads(self.state.read_text());state['merged_by_sha']={state['detail']['merge_commit_sha']:[state['detail']]};self.state.write_text(json.dumps(state))
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'requested')
        self.assertEqual(json.loads(result.stdout)['consumed']['source_sha'],recorded)
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.2')

    def test_failed_qualification_never_opens_pr(self):
        self.state_update(ci_conclusion='failure')
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('CI must complete successfully',result.stderr)
        self.assertFalse(any(c[:2]==['pr','create'] for c in json.loads(self.state.read_text())['calls']))

    def test_refresh_updates_existing_pr_title_and_body(self):
        self.initial_request()
        (self.repo/'product').write_text('new');self.git('add','.');self.git('commit','-qm','feature')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(any(c[:2]==['pr','edit'] for c in json.loads(self.state.read_text())['calls']))

    def test_version_only_followup_is_deduplicated_but_dependency_change_is_product(self):
        self.merge_request();result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.state_update(releases=[dict(tag_name='v2.6.3-dev.1',draft=False,prerelease=True)])
        self.git('checkout','-q','main')
        (self.repo/'Cargo.toml').write_text('[workspace.package]\nversion="2.6.3"\n')
        (self.repo/'CHANGELOG.md').write_text('stable release notes')
        self.git('add','.');self.git('commit','-qm','stable metadata')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source,merged=[])
        state=json.loads(self.state.read_text());state['merged_by_sha']={state['detail']['merge_commit_sha']:[state['detail']]};self.state.write_text(json.dumps(state))
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'metadata-only')
        with (self.repo/'Cargo.toml').open('a') as f:f.write('[workspace.dependencies]\nnew_dependency="1"\n')
        self.git('add','.');self.git('commit','-qm','dependency change')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'requested')

    def test_request_schema_rejects_extra_fields_and_noncanonical_version(self):
        from dev_release_pr import Coordinator
        for request in [dict(version='2.6.3-dev.01',source_sha=self.source),dict(version='2.6.3-dev.1',source_sha=self.source,extra=True)]:
            (self.repo/'.release-please-dev.json').write_text(json.dumps(request))
            self.git('add','.');self.git('commit','-qm','bad request')
            with self.assertRaisesRegex(ValueError,'schema'):
                Coordinator(self.repo,'example/labby').request(self.git('rev-parse','HEAD'))

    def test_merge_commit_cannot_substitute_reviewed_request_blob(self):
        self.merge_request()
        request=json.loads((self.repo/'.release-please-dev.json').read_text());request['version']='2.6.3-dev.9'
        (self.repo/'.release-please-dev.json').write_text(json.dumps(request))
        self.git('add','.');self.git('commit','--amend','--no-edit','-q')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main','--force')
        state=json.loads(self.state.read_text());state['detail']['merge_commit_sha']=self.source
        self.state_update(source=self.source,merged=[state['detail']],detail=state['detail'])
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('blob differs',result.stderr)

    def test_failed_pr_creation_recovers_exact_generated_branch_without_new_commit(self):
        self.state_update(create_failure=True)
        result=self.execute();self.assertNotEqual(result.returncode,0)
        head=self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0]
        self.state_update(create_failure=False)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0],head)
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.1')

    def test_unchanged_request_repairs_pr_title_body(self):
        self.initial_request();result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertTrue(any(c[:2]==['pr','edit'] for c in json.loads(self.state.read_text())['calls']))

    def test_unpublished_or_draft_tag_reserves_number_without_suppressing_request(self):
        self.git('checkout','-q','--detach',self.source)
        subprocess.run([sys.executable,str(self.repo/'scripts/ci/sync-release-version.py'),str(self.repo),'2.6.3-dev.10'],check=True)
        self.git('add','.');self.git('commit','-qm','version');self.git('tag','v2.6.3-dev.10')
        self.git('push','-q','origin','refs/tags/v2.6.3-dev.10');self.git('checkout','-q','main')
        self.state_update(releases=[dict(tag_name='v2.6.3-dev.10',draft=True,prerelease=True)])
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'requested')
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.11')

    def test_orphan_branch_product_edits_cannot_be_adopted(self):
        self.state_update(create_failure=True);self.execute()
        head=self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0]
        self.git('checkout','-q','--detach',head);(self.repo/'foreign').write_text('preserve')
        self.git('add','.');self.git('commit','-qm','foreign edit');foreign=self.git('rev-parse','HEAD')
        self.git('push','-q','origin','HEAD:refs/heads/release-please--branches--main--components--labby-dev')
        self.state_update(create_failure=False)
        result=self.execute();self.assertNotEqual(result.returncode,0)
        self.assertIn('not an exact generated',result.stderr)
        self.assertEqual(self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0],foreign)

    def test_workflow_author_environment_does_not_break_orphan_recovery(self):
        self.env.update(GIT_AUTHOR_NAME='github-actions[bot]',GIT_AUTHOR_EMAIL='41898282+github-actions[bot]@users.noreply.github.com',GIT_COMMITTER_NAME='github-actions[bot]',GIT_COMMITTER_EMAIL='41898282+github-actions[bot]@users.noreply.github.com')
        self.state_update(create_failure=True);self.assertNotEqual(self.execute().returncode,0)
        self.state_update(create_failure=False)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)

    def test_second_generation_orphan_reuses_commit_after_creation_failure(self):
        self.merge_request();self.assertEqual(self.execute().returncode,0)
        self.git('checkout','-q','main');(self.repo/'next-feature').write_text('new')
        self.git('add','.');self.git('commit','-qm','feature');self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main')
        state=json.loads(self.state.read_text());self.state_update(source=self.source,merged=[],merged_by_sha={state['detail']['merge_commit_sha']:[state['detail']]},create_failure=True)
        self.assertNotEqual(self.execute().returncode,0)
        head=self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0]
        self.assertEqual(len(self.git('rev-list','--parents','-n','1',head).split()),3)
        self.state_update(create_failure=False)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(self.git('ls-remote','origin','refs/heads/release-please--branches--main--components--labby-dev').split()[0],head)

    def test_orphan_recovery_reconciles_new_main_and_reserved_version(self):
        self.state_update(create_failure=True);self.assertNotEqual(self.execute().returncode,0)
        self.git('tag','v2.6.3-dev.1');self.git('push','-q','origin','refs/tags/v2.6.3-dev.1')
        (self.repo/'new-product').write_text('new');self.git('add','.');self.git('commit','-qm','new main')
        self.source=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(source=self.source,create_failure=False)
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['source_sha'],self.source)
        self.assertEqual(json.loads(result.stdout)['version'],'2.6.3-dev.2')

    def test_old_green_event_catches_up_only_to_independently_green_main(self):
        event=self.source;(self.repo/'latest-feature').write_text('new');self.git('add','.');self.git('commit','-qm','latest')
        latest=self.git('rev-parse','HEAD');self.git('push','-q','origin','main')
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['source_sha'],latest)
        calls=json.loads(self.state.read_text())['calls']
        self.assertTrue(any('head_sha='+latest in ' '.join(c) for c in calls))
        self.assertNotEqual(event,latest)

    def test_latest_ci_malformed_response_is_not_silently_stale(self):
        (self.repo/'latest').write_text('new');self.git('add','.');self.git('commit','-qm','latest')
        latest=self.git('rev-parse','HEAD');self.git('push','-q','origin','main');self.state_update(malformed_ci_sha=latest)
        result=self.execute();self.assertNotEqual(result.returncode,0);self.assertIn('JSONDecodeError',result.stderr)

    def test_old_green_event_after_request_merge_consumes_recorded_source(self):
        recorded=self.merge_request();merged=self.source;self.source=recorded
        result=self.execute();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['action'],'tagged')
        self.assertEqual(json.loads(result.stdout)['source_sha'],recorded)
        self.assertNotEqual(recorded,merged)

if __name__=='__main__': unittest.main()
