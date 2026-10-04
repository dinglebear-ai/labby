import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import McpServerSettingsPage from './page'

test('MCP settings send users to supported connection flows without legacy editors', () => {
  const markup = renderToStaticMarkup(<McpServerSettingsPage />)
  assert.deepEqual(Array.from(markup.matchAll(/href="([^"]+)"/g), match => match[1]), ['/depot', '/gateways'])
  assert.match(markup, /Add to Library saves an artifact without starting a server/)
  assert.match(markup, /A saved connection does not prove a tool works/)
})
