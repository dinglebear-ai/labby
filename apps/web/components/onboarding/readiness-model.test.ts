import assert from 'node:assert/strict'
import test from 'node:test'
import type { AgentRunResult, AgentView } from '@/lib/agent-tasks/client'
import type { DiscoveryPage } from '@/lib/api/depot-client'
import type { Gateway } from '@/lib/types/gateway'
import { agentRunVerified, discoveryVerified, firstUseSummary, mcpConnectionVerified } from './readiness-model'

const agent: AgentView = { agent_id: 'starter-test', owner_kind: 'personal', owner_id: 'principal-test', version: 1, state: 'active', catalog_generation: '1' }
const receipt: AgentRunResult = { agent_id: agent.agent_id, agent_version: 1, session_id: 'session-test', status: 'completed', output_digest: 'sha256:fixture', output: 'Hello', authority_expires_at: 9999999999 }
const gateway: Gateway = {
  id: 'first-server', name: 'first-server', transport: 'http', enabled: true, config: { url: 'https://server.example/mcp' },
  status: { healthy: true, connected: true, discovered_tool_count: 1, exposed_tool_count: 1, discovered_resource_count: 0, exposed_resource_count: 0, discovered_prompt_count: 0, exposed_prompt_count: 0 },
  discovery: { tools: [], resources: [], prompts: [] }, warnings: [],
}
const page: DiscoveryPage = {
  schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'fixture',
  items: [{ providerId: 'public', artifactId: 'fixture/server', kind: 'mcp-server' }],
  providerOutcomes: [{ providerId: 'public', state: 'exhausted' }], failures: [],
  coverageComplete: true, totalIsExact: true, state: 'complete',
}

test('an Agent definition is not a completed run', () => {
  assert.equal(agentRunVerified(agent, null), false)
  assert.equal(agentRunVerified(null, receipt), false)
  assert.equal(agentRunVerified(agent, receipt), true)
})

test('Agent evidence is bound to the exact definition revision and a durable receipt', () => {
  for (const patch of [{ status: 'failed' }, { status: 'running' }, { agent_id: 'other-agent' }, { agent_version: 2 }, { session_id: '' }, { output_digest: '' }]) {
    assert.equal(agentRunVerified(agent, { ...receipt, ...patch }), false, JSON.stringify(patch))
  }
})

test('MCP configuration must be healthy, live, and exposing tools', () => {
  assert.equal(mcpConnectionVerified(gateway), true)
  assert.equal(mcpConnectionVerified(null), false)
  assert.equal(mcpConnectionVerified({ ...gateway, transport: 'in_process' }), false)
  assert.equal(mcpConnectionVerified({ ...gateway, enabled: false }), false)
  for (const patch of [{ healthy: false }, { connected: false }, { catalog_warming: true }, { exposed_tool_count: 0 }]) {
    assert.equal(mcpConnectionVerified({ ...gateway, status: { ...gateway.status, ...patch } }), false)
  }
})

test('Discover requires real results and a responding provider, not a successful empty envelope', () => {
  assert.equal(discoveryVerified(page), true)
  assert.equal(discoveryVerified(page, true), false, 'mock results never qualify readiness')
  assert.equal(discoveryVerified(null), false)
  assert.equal(discoveryVerified({ ...page, items: [] }), false)
  assert.equal(discoveryVerified({ ...page, providerOutcomes: [{ providerId: 'public', state: 'failed' }] }), false)
  for (const state of ['all_failed', 'all_disabled'] as const) assert.equal(discoveryVerified({ ...page, state }), false)
})

test('partial catalog availability remains usable without suppressing its failures', () => {
  assert.equal(discoveryVerified({ ...page, state: 'partial', coverageComplete: false, failures: [{ providerId: 'private', kind: 'unauthorized' }] }), true)
})

test('connection checks never claim downstream tool execution or five-minute completion', () => {
  assert.deepEqual(firstUseSummary(true, true, true, true), { verified: 4, total: 4, coreReady: true, toolCallVerified: false })
  assert.equal(firstUseSummary(true, false, true, true).coreReady, false)
  assert.equal(firstUseSummary(false, false, false, false).verified, 0)
})
