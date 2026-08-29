import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ConsoleShellProvider } from './console-shell-context'
import { MockTeamPage, type MockTeamPageKind } from './mock-team-page'

const evidence: Record<MockTeamPageKind, string[]> = {
  overview: ['team-overview-launchers', '40 shared artifacts', 'Needs You', 'edge-minimal'],
  library: ['team-library-submissions', 'rust-reviewer', 'Review Diff', 'Shared Artifacts'],
  projects: ['team-project-selector', 'tootie-tv/labby', 'project-a-loadout', 'Project Updates'],
  activity: ['team-activity-feed', 'Suggested by Axon', 'changelog-writer', 'pre-commit-guard'],
  stash: ['team-stash-files', 'stash://team/AGENTS.md', '194 MB', 'Agent Reads'],
}

for (const kind of Object.keys(evidence) as MockTeamPageKind[]) {
  test(`team ${kind} marks identity and fixture content as mock`, () => {
    const markup = renderToStaticMarkup(<ConsoleShellProvider><MockTeamPage kind={kind} /></ConsoleShellProvider>)
    assert.match(markup, /data-mock-surface="true"/)
    assert.match(markup, /visual mock/i)
    assert.match(markup, /disabled=""/)
    assert.match(markup, /no controls call a Labby service/i)
    for (const expected of evidence[kind]) assert.match(markup, new RegExp(expected.replaceAll('/', '\\/')))
  })
}
