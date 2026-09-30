import test from 'node:test'
import assert from 'node:assert/strict'

import type { Gateway } from '../types/gateway.ts'
import { buildGatewayClientConfig } from './gateway-client-config.ts'

test('buildGatewayClientConfig returns minimal http client JSON without bearer env fields', () => {
  const gateway: Gateway = {
    id: 'gw-http',
    name: 'github-server',
    transport: 'http',
    config: {
      url: 'http://localhost:3001/mcp',
      bearer_token_env: 'GITHUB_TOKEN',
      proxy_resources: true,
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 12,
      exposed_tool_count: 12,
      discovered_resource_count: 3,
      exposed_resource_count: 3,
      discovered_prompt_count: 2,
      exposed_prompt_count: 2,
    },
    discovery: {
      tools: [],
      resources: [],
      prompts: [],
    },
    warnings: [],
    created_at: '2024-01-10T08:00:00Z',
    updated_at: '2024-01-22T09:15:00Z',
  }

  assert.deepEqual(buildGatewayClientConfig(gateway), {
    name: 'github-server',
    type: 'http',
    url: 'http://localhost:3001/mcp',
  })
})

test('buildGatewayClientConfig includes stdio command args and env map', () => {
  const gateway: Gateway = {
    id: 'gw-stdio',
    name: 'postgres-server',
    transport: 'stdio',
    config: {
      command: '/usr/local/bin/mcp-postgres',
      args: ['--connection-string', 'postgresql://localhost/mydb'],
      bearer_token_env: 'DATABASE_TOKEN',
      proxy_resources: true,
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 4,
      exposed_tool_count: 4,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [],
      resources: [],
      prompts: [],
    },
    warnings: [],
    created_at: '2024-01-10T08:00:00Z',
    updated_at: '2024-01-22T09:15:00Z',
  }

  assert.deepEqual(buildGatewayClientConfig(gateway), {
    name: 'postgres-server',
    type: 'stdio',
    command: '/usr/local/bin/mcp-postgres',
    args: ['--connection-string', 'postgresql://localhost/mydb'],
    env: {
      DATABASE_TOKEN: '$DATABASE_TOKEN',
    },
  })
})
