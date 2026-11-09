import {test} from 'node:test'
import assert from 'node:assert/strict'
import {mkdtempSync, writeFileSync, rmSync} from 'node:fs'
import {join} from 'node:path'
import {tmpdir} from 'node:os'

test('private accounting combines measured UI Gateway deltas with exact direct calls', async () => {
  const {consoleHttpDelta} = await import('./console-http-counts.mjs')
  const before = {version: 1, cap: 2048, total: 10, gatewayPost: 3, authPreflight: 4, otherServices: 2, other: 1}
  const after = {version: 1, cap: 2048, total: 22, gatewayPost: 8, authPreflight: 10, otherServices: 3, other: 1}
  assert.deepEqual(consoleHttpDelta(before, after, 7), {
    gatewayUiActual: 5, gatewayActualCombined: 12, consoleHttpActual: 12,
    authPreflightActual: 6, otherServicesActual: 1, otherHttpActual: 0,
    consoleHttpBaseline: 10, consoleHttpFinal: 22, consoleHttpHardCap: 2048,
  })
  assert.throws(() => consoleHttpDelta(before, {...after, gatewayPost: 2}, 7), /PRIVATE_HTTP_ACCOUNTING_INVALID/)
})

test('counter reader bounds and validates local receipts without exposing malformed contents', async () => {
  const {readConsoleHttpCounts} = await import('./console-http-counts.mjs')
  const root = mkdtempSync(join(tmpdir(), 'labby-counter-reader-'))
  const path = join(root, 'console-http-counts.json')
  try {
    const counts = {version: 1, cap: 2048, total: 1, gatewayPost: 1, authPreflight: 0, otherServices: 0, other: 0}
    writeFileSync(path, JSON.stringify(counts), {mode: 0o600})
    assert.deepEqual(readConsoleHttpCounts(path), counts)
    for (const contents of ['private-malformed-value', JSON.stringify({...counts, total: 3}), 'x'.repeat(4097), JSON.stringify({...counts, secret: 'private'})]) {
      writeFileSync(path, contents)
      assert.throws(() => readConsoleHttpCounts(path), error => error.message === 'PRIVATE_HTTP_ACCOUNTING_INVALID')
    }
  } finally {
    rmSync(root, {recursive: true, force: true})
  }
})
