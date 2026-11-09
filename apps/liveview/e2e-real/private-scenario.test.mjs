import {test} from 'node:test'
import assert from 'node:assert/strict'
import {mkdtempSync, readFileSync, writeFileSync, rmSync, statSync} from 'node:fs'
import {join} from 'node:path'
import {tmpdir} from 'node:os'

test('private scenario persists actual after-cleanup counts and fails measured postcondition budgets', async () => {
  const {runPrivateScenario} = await import('./private-scenario.mjs')
  const root = mkdtempSync(join(tmpdir(), 'labby-private-accounting-'))
  const consoleHttpReceipt = join(root, 'console-http-counts.json')
  const output = join(root, 'private-accounting.json')
  const counts = {version: 1, cap: 2048, total: 0, gatewayPost: 0, authPreflight: 0, otherServices: 0, other: 0}
  const page = {setDefaultTimeout() {}, setDefaultNavigationTimeout() {}, isClosed: () => false}
  const attachments = []
  try {
    for (const [gatewayPost, authPreflight, direct, expected] of [[5, 6, 7, undefined], [34, 0, 7, 'PRIVATE_SCENARIO_GATEWAY_ACTUAL_BUDGET_EXCEEDED'], [2, 127, 1, 'PRIVATE_SCENARIO_CONSOLE_HTTP_BUDGET_EXCEEDED']]) {
      writeFileSync(consoleHttpReceipt, JSON.stringify(counts), {mode: 0o600})
      const run = runPrivateScenario({page, consoleHttpReceipt,
        info: {outputPath: () => output, attach: async (_, value) => attachments.push(value)}, accounting: {gatewayDirect: direct},
        cleanup: async () => writeFileSync(consoleHttpReceipt, JSON.stringify({...counts, total: gatewayPost + authPreflight, gatewayPost, authPreflight}))}, async () => {})
      if (expected) await assert.rejects(run, error => error.message === expected)
      else await run
      const body = JSON.parse(readFileSync(output, 'utf8'))
      assert.equal(body.gatewayActualCombined, gatewayPost + direct)
      assert.equal(body.authPreflightActual, authPreflight)
      assert.equal(body.consoleHttpActual, gatewayPost + authPreflight)
      assert.equal(body.cleanup, 'complete')
      assert.equal(statSync(output).mode & 0o777, 0o600)
      assert.equal(attachments.at(-1).path, output)
    }
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})

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
