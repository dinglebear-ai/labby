import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockConsolePage } from '@/components/console/mock-console-page'

for (const kind of ['sessions', 'tasks', 'logs'] as const) {
  test(`${kind} mock page marks the page and fixture region as mock data`, () => {
    const markup = renderToStaticMarkup(
      <ConsoleShellProvider>
        <MockConsolePage kind={kind} />
      </ConsoleShellProvider>,
    )

    assert.match(markup, /data-mock-surface="true"/)
    assert.match(markup, new RegExp(`data-mock-region="${kind}"`))
    assert.match(markup, /illustrative/i)
    assert.match(markup, /disabled=""/)
  })
}
