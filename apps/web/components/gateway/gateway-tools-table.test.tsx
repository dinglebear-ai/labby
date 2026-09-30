import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { GatewayToolsTable } from './gateway-tools-table'

test('gateway tools table renders dense operational rows and exposure state', () => {
  const markup = renderToStaticMarkup(
    <GatewayToolsTable
      rows={[
        {
          gatewayId: 'gw_1',
          gatewayName: 'Lab Core',
          source: 'in_process',
          sourceFacet: 'lab',
          transport: 'in_process',
          toolName: 'unifi',
          description: 'UniFi Network Application local API',
          exposed: true,
        },
      ]}
    />,
  )

  assert.match(markup, /UniFi Network Application local API/)
  assert.match(markup, /Exposed/)
  assert.match(markup, /Lab Core/)
  assert.match(markup, /unifi/)
})
