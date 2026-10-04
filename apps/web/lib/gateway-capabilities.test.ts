import test from 'node:test'
import assert from 'node:assert/strict'
import { capabilityDescription, capabilityLabel, summarizeCapabilities } from './gateway-capabilities.ts'
import { normalizeServerView } from './server/gateway-adapter.ts'
const unknown = { state: 'unknown' as const, discovered: null, exposed: null }
const known = { state: 'known' as const, discovered: 91, exposed: 91 }
const observation = { scope: 'credential' as const, tools: known, resources: unknown, prompts: unknown, skills: unknown }
test('normalization retains credential observation and authoritative counts', () => {
 const gateway = normalizeServerView({ id: 'linear', name: 'linear', source: 'custom_gateway', discovered_tool_count: 0, capability_observation: observation })
 assert.deepEqual(gateway.status.capability_observation, observation)
 assert.equal(capabilityLabel(gateway.status, 'tools'), '91/91')
 assert.equal(capabilityLabel(gateway.status, 'resources'), 'Not discovered')
})
test('known empty, unknown, stale and failed catalogs remain distinct', () => {
 const status = normalizeServerView({ id:'x',name:'x',source:'custom_gateway', capability_observation: observation }).status
 for (const [state, expected] of [['known','0/0'], ['unknown','Not discovered'], ['failed','Discovery failed'], ['stale','0/0 · stale']] as const) {
  assert.equal(capabilityLabel({ ...status, capability_observation: { ...observation, tools: {state, discovered:0, exposed:0} } }, 'tools'), expected)
 }
 assert.deepEqual(summarizeCapabilities([status], 'resources'), {discovered:0,exposed:0,incomplete:1})
 assert.deepEqual(summarizeCapabilities([status], 'tools'), {discovered:91,exposed:91,incomplete:0})
})

test('failure diagnostics preserve backend classification in accessible descriptions', () => {
 const status = normalizeServerView({id:'linear',name:'linear',source:'custom_gateway',capability_observation:{...observation,tools:{...unknown,state:'failed',error:'Discovery timed out. Refresh discovery.'}}}).status
 assert.equal(capabilityDescription(status,'tools'),'Discovery failed — Discovery timed out. Refresh discovery.')
})
