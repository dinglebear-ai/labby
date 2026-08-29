import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockAgentsPage, MockTasksPage } from './mock-agents-tasks-page'

for (const [name, Page, region, evidence] of [
  ['Agents', MockAgentsPage, 'agents-session', /Refactor gateway reconcile loop/],
  ['Tasks', MockTasksPage, 'tasks-row', /Loadout Scope Audit/],
] as const) test(`${name} uses its page-specific marked mock`, () => {
  const markup = renderToStaticMarkup(<ConsoleShellProvider><Page /></ConsoleShellProvider>)
  assert.match(markup, evidence)
  assert.match(markup, new RegExp(`data-mock-region="${region}"`))
  assert.match(markup, /data-mock-surface="true"/)
  assert.match(markup, /no controls call a Labby service/i)
  assert.match(markup, /disabled=""/)
})
