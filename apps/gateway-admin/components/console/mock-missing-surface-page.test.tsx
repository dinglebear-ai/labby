import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockMissingSurfacePage, type MissingMockSurfaceKind } from './mock-missing-surface-page'

const kinds: MissingMockSurfaceKind[] = ['discovery', 'create', 'library', 'agents', 'stash', 'containers', 'instance']

for (const kind of kinds) {
  test(`${kind} surface labels all illustrative content`, () => {
    const markup = renderToStaticMarkup(<ConsoleShellProvider><MockMissingSurfacePage kind={kind} /></ConsoleShellProvider>)
    assert.match(markup, /data-mock-surface="true"/)
    assert.match(markup, new RegExp(`data-mock-region="${kind}"`))
    assert.match(markup, /no controls call a Labby service/i)
    assert.match(markup, /disabled=""/)
  })
}
