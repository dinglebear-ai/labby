import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { Window } from 'happy-dom'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from './console-shell-context'
import { MockAgentsPage, MockTasksPage } from './mock-agents-tasks-page'
import { MockCreatePage } from './mock-create-page'
import { MockDiscoveryPage } from './mock-discovery-page'
import { MockContainersPage, MockInstancePage, MockStashPage } from './mock-infrastructure-pages'
import { MockLibraryPage } from './mock-library-page'
import { MockLogsPage } from './mock-logs-page'
import { MockConsolePage } from './mock-console-page'
import { MockTeamPage, type MockTeamPageKind } from './mock-team-page'

const teamKinds: MockTeamPageKind[] = ['overview', 'library', 'projects', 'activity', 'stash']
const pages: ReadonlyArray<readonly [string, React.ReactNode]> = [
  ['discovery', <MockDiscoveryPage />],
  ['create', <MockCreatePage />],
  ['library', <MockLibraryPage />],
  ['agents', <MockAgentsPage />],
  ['tasks', <MockTasksPage />],
  ['stash', <MockStashPage />],
  ['containers', <MockContainersPage />],
  ['instance', <MockInstancePage />],
  ['logs', <MockLogsPage />],
  ['sessions', <MockConsolePage kind="sessions" />],
  ...teamKinds.map((kind) => [`team-${kind}`, <MockTeamPage kind={kind} />] as const),
]

for (const [name, page] of pages) {
  test(`${name} has no enabled controls inside mock regions`, () => {
    const markup = renderToStaticMarkup(
      <ConsoleShellProvider>{page}</ConsoleShellProvider>,
    )
    const window = new Window()
    window.document.write(markup)
    const regions = [...window.document.querySelectorAll('[data-mock-region]')]
    assert.ok(regions.length > 0, `${name} must declare at least one mock region`)

    for (const region of regions) {
      for (const control of region.querySelectorAll('button, input, select, textarea')) {
        assert.ok(control.hasAttribute('disabled'), `${name} contains an enabled ${control.tagName}`)
      }
      for (const link of region.querySelectorAll('a')) {
        assert.ok(!link.hasAttribute('href'), `${name} contains an actionable mock link`)
      }
      for (const control of region.querySelectorAll('[role="button"], [role="switch"]')) {
        assert.ok(
          control.hasAttribute('disabled') || control.getAttribute('aria-disabled') === 'true',
          `${name} contains an enabled ${control.getAttribute('role')}`,
        )
      }
    }

    window.close()
  })
}
