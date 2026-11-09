import {test} from 'node:test'
import assert from 'node:assert/strict'

test('private scenario preserves its sanitized primary stage when cleanup also fails', async () => {
  const {runPrivateScenario} = await import('./private-scenario.mjs')
  const attachments = []
  const timeouts = []
  const page = {setDefaultTimeout: value => timeouts.push(value), setDefaultNavigationTimeout: value => timeouts.push(value), isClosed: () => false}
  await assert.rejects(runPrivateScenario({page, info: {attach: async (_, value) => attachments.push(value)},
    accounting: {gatewayDirect: 1}, cleanup: async () => {throw Error('cleanup-private-value')}}, async step => {
    await step('literal-map-replacement', async () => {throw Error('input-private-value')})
  }), error => {
    assert.equal(error.message, 'PRIVATE_SCENARIO_FAILED stage=literal-map-replacement kind=action')
    assert.equal(error.cause, undefined)
    return true
  })
  assert.deepEqual(timeouts, [10000, 15000])
  const body = attachments[0].body
  assert.equal(JSON.parse(body).gatewayDirect, 1)
  assert.match(body, /unconfirmed/)
  assert.equal(body.includes('private-value'), false)
})

test('private scenario reports cleanup failure after a successful body', async () => {
  const {runPrivateScenario} = await import('./private-scenario.mjs')
  const page = {setDefaultTimeout() {}, setDefaultNavigationTimeout() {}, isClosed: () => false}
  await assert.rejects(runPrivateScenario({page, info: {attach: async () => {}}, accounting: {},
    cleanup: async () => {throw Error('sensitive backend response')}}, async () => {}), /PRIVATE_SCENARIO_CLEANUP_FAILED/)
})

test('private scenario skips browser cleanup after closure and keeps the first failure if attachment fails', async () => {
  const {runPrivateScenario} = await import('./private-scenario.mjs')
  let cleanupCalls = 0
  const page = {setDefaultTimeout() {}, setDefaultNavigationTimeout() {}, isClosed: () => true}
  await assert.rejects(runPrivateScenario({page, info: {attach: async () => {throw Error('reporter closed')}}, accounting: {},
    cleanup: async () => {cleanupCalls++}}, async step => {
    await step('initial-private-input', async () => {throw Object.assign(Error('sensitive timeout'), {name: 'TimeoutError'})})
  }), /PRIVATE_SCENARIO_FAILED stage=initial-private-input kind=timeout/)
  assert.equal(cleanupCalls, 0)
})
