import test from 'node:test'
import assert from 'node:assert/strict'

import type { Gateway } from '../types/gateway.ts'
import { EXPOSE_NONE_PATTERN } from './tool-exposure-draft.ts'
import {
  applyMockGatewayOverride,
  type MockGatewayOverride,
} from './mock-gateway-overrides.ts'

const gateway: Gateway = {
  id: 'gw-2',
  name: 'github-server',
  transport: 'http',
  config: {
    url: 'http://localhost:3001/mcp',
    proxy_resources: true,
  },
  status: {
    healthy: true,
    connected: true,
    discovered_tool_count: 3,
    exposed_tool_count: 3,
    discovered_resource_count: 2,
    exposed_resource_count: 2,
    discovered_prompt_count: 1,
    exposed_prompt_count: 1,
  },
  discovery: {
    tools: [
      { name: 'alpha', description: 'Alpha', exposed: true, matched_by: '*' },
      { name: 'beta', description: 'Beta', exposed: true, matched_by: '*' },
      { name: 'gamma', description: 'Gamma', exposed: true, matched_by: '*' },
    ],
    resources: [
      { name: 'repo', uri: 'github://repo' },
      { name: 'issue', uri: 'github://issue' },
    ],
    prompts: [
      { name: 'summarize', description: 'Summarize a repo' },
    ],
  },
  warnings: [],
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
}

test('applyMockGatewayOverride applies allowlist exposure and proxy resource overrides', () => {
  const override: MockGatewayOverride = {
    exposurePolicy: {
      mode: 'allowlist',
      patterns: ['beta'],
    },
    proxyResources: false,
  }

  const overridden = applyMockGatewayOverride(gateway, override)

  assert.deepEqual(
    overridden.discovery.tools.map((tool) => [tool.name, tool.exposed, tool.matched_by]),
    [
      ['alpha', false, null],
      ['beta', true, 'beta'],
      ['gamma', false, null],
    ],
  )
  assert.equal(overridden.status.exposed_tool_count, 1)
  assert.equal(overridden.config.proxy_resources, false)
  assert.equal(overridden.status.exposed_resource_count, 0)
})

test('applyMockGatewayOverride treats the expose none sentinel as hiding every tool', () => {
  const overridden = applyMockGatewayOverride(gateway, {
    exposurePolicy: {
      mode: 'allowlist',
      patterns: [EXPOSE_NONE_PATTERN],
    },
  })

  assert.equal(overridden.status.exposed_tool_count, 0)
  assert.deepEqual(
    overridden.discovery.tools.map((tool) => tool.exposed),
    [false, false, false],
  )
})

test('applyMockGatewayOverride persists generic gateway config patches', () => {
  const overridden = applyMockGatewayOverride(gateway, {
    config: {
      proxy_skills: true,
      expose_skills: ['review-*'],
      proxy_prompts: false,
    },
  })

  assert.equal(overridden.config.proxy_skills, true)
  assert.deepEqual(overridden.config.expose_skills, ['review-*'])
  assert.equal(overridden.config.proxy_prompts, false)
  assert.equal(overridden.config.proxy_resources, true)
})

test('applyMockGatewayOverride honors wildcard allowlist patterns', () => {
  const overridden = applyMockGatewayOverride(gateway, {
    exposurePolicy: {
      mode: 'allowlist',
      patterns: ['a*', 'gamma'],
    },
  })

  assert.deepEqual(
    overridden.discovery.tools.map((tool) => [tool.name, tool.exposed, tool.matched_by]),
    [
      ['alpha', true, 'a*'],
      ['beta', false, null],
      ['gamma', true, 'gamma'],
    ],
  )
})
