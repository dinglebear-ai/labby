import test from 'node:test'
import assert from 'node:assert/strict'
import { testResultFromProbe } from './gateway-test-result.ts'

test('connection probes preserve per-capability observation independently of transport verdict', () => {
 const unknown = { state: 'unknown' as const, discovered: null, exposed: null }
 const observation = { scope: 'credential' as const, tools: { state: 'known' as const, discovered: 91, exposed: 91 }, resources: unknown, prompts: unknown, skills: unknown }
 for (const connected of [true, false]) {
  const result = testResultFromProbe({ name:'linear', connected, tool_count:0, resource_count:0, prompt_count:0, capability_observation:observation }, {connected,healthy:connected})
  assert.deepEqual(result.capability_observation,observation)
  assert.equal(result.success,connected)
 }
})
