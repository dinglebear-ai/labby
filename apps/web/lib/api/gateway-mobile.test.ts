import test from 'node:test'
import assert from 'node:assert/strict'

import { mockGateways } from './mock-data.ts'
import {
  buildGatewayEndpointPreview,
  filterGatewayTools,
  summarizeGatewayTools,
} from './gateway-mobile.ts'

test('buildGatewayEndpointPreview returns http url for http gateways', () => {
  const gateway = mockGateways.find((item) => item.transport === 'http')

  assert.ok(gateway)
  assert.equal(buildGatewayEndpointPreview(gateway), gateway.config.url)
})

test('buildGatewayEndpointPreview joins stdio command and args for stdio gateways', () => {
  const gateway = mockGateways.find((item) => item.transport === 'stdio')

  assert.ok(gateway)
  assert.equal(
    buildGatewayEndpointPreview(gateway),
    [gateway.config.command, ...(gateway.config.args ?? [])].join(' '),
  )
})

test('buildGatewayEndpointPreview returns labby mcp command for in-process gateways', () => {
  const gateway = {
    ...mockGateways[0],
    name: 'gateway_alpha',
    transport: 'in_process' as const,
    config: {
      proxy_resources: true,
    },
  }

  assert.equal(buildGatewayEndpointPreview(gateway), 'labby mcp --services gateway_alpha')
})

test('filterGatewayTools matches name and description case-insensitively', () => {
  const tools = mockGateways[1]?.discovery.tools ?? []

  assert.deepEqual(
    filterGatewayTools(tools, 'pull'),
    tools.filter((tool) => tool.name.includes('pull')),
  )
  assert.deepEqual(
    filterGatewayTools(tools, 'Natural Language'),
    tools.filter((tool) => tool.description?.includes('natural language')),
  )
})

test('summarizeGatewayTools reports exposed and filtered totals', () => {
  const tools = mockGateways[2]?.discovery.tools ?? []

  assert.deepEqual(summarizeGatewayTools(tools), {
    exposedCount: 5,
    filteredCount: 10,
  })
})
