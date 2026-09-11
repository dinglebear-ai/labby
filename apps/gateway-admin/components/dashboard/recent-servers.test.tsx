import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { RecentServers, type RecentServer } from './recent-servers'
import { gatewayDetailHref } from '@/lib/api/gateway-config'

const gateway: RecentServer = { id: 'actual gateway', name: 'Actual server', transport: 'stdio', status: { connected: true, healthy: true, exposed_tool_count: 42 } }
test('compact rows preserve actual gateway navigation status transport and exposed count', () => {
  const html = renderToStaticMarkup(<RecentServers gateways={[gateway]}/>)
  assert.ok(html.includes(`href="${gatewayDetailHref(gateway.id).replace('/?', '?')}"`))
  assert.match(html, /Actual server/)
  assert.match(html, /aria-label="Healthy"/)
  assert.match(html, />stdio</)
  assert.match(html, /42 exposed downstream tools/)
  assert.match(html, /py-2/)
})
test('same panel labels loading empty error and disconnected states without fabricated counts', () => {
  assert.match(renderToStaticMarkup(<RecentServers gateways={[]} loading/>), /Loading recent servers/)
  assert.match(renderToStaticMarkup(<RecentServers gateways={[]}/>), /No servers configured/)
  const error = renderToStaticMarkup(<RecentServers gateways={[gateway]} error/>)
  assert.match(error, /Recent servers are unavailable/)
  assert.doesNotMatch(error, /Actual server/)
  const disconnected = renderToStaticMarkup(<RecentServers gateways={[{ ...gateway, status: { ...gateway.status, connected: false } }]}/>)
  assert.match(disconnected, /aria-label="Disconnected"/)
})
test('at most five supplied rows are shown without sorting them', () => {
  const html = renderToStaticMarkup(<RecentServers gateways={Array.from({ length: 7 }, (_, index) => ({ ...gateway, id: String(index), name: `Server ${index}` }))}/>)
  assert.match(html, /Server 0/)
  assert.match(html, /Server 4/)
  assert.doesNotMatch(html, /Server 5|Server 6/)
})
