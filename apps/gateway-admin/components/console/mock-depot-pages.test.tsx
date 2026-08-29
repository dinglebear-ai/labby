import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockCreatePage } from './mock-create-page'
import { MockLibraryPage } from './mock-library-page'

for (const [name, Page, evidence] of [
  ['Create', MockCreatePage, /Artifact body/],
  ['Library', MockLibraryPage, /Behind Upstream/],
] as const) test(`${name} uses its page-specific mock structure`, () => {
  const markup = renderToStaticMarkup(<ConsoleShellProvider><Page /></ConsoleShellProvider>)
  assert.match(markup, evidence)
  assert.match(markup, /data-mock-region=/)
  assert.match(markup, /data-mock-surface="true"/)
  assert.match(markup, /no controls call a Labby service/i)
  assert.match(markup, /disabled=""/)
})
