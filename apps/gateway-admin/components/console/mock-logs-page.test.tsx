import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ConsoleShellProvider } from '@/components/console/console-shell-context'
import { MockLogsPage } from './mock-logs-page'

test('Logs uses the mock stream structure and labels all fixture lines', () => {
  const markup = renderToStaticMarkup(<ConsoleShellProvider><MockLogsPage /></ConsoleShellProvider>)
  assert.match(markup, /Streaming · all sources/)
  assert.match(markup, /upstream_connect_error/)
  assert.match(markup, /data-mock-region="log-line"/)
  assert.match(markup, /data-mock-surface="true"/)
  assert.match(markup, /no controls call a Labby service/i)
  assert.match(markup, /disabled=""/)
})
