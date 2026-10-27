import test from 'node:test'
import assert from 'node:assert/strict'
import { listNotifications } from './notifications-client'

const notification = {
  id: 'fixture', createdAtUnixMs: 1, level: 'info', title: 'Ready',
  body: 'Fixture ready', source: 'fixture', dedupeKey: 'fixture-ready',
}

test('notification fetch preserves validated fields and rejects invalid wire data', async () => {
  const original = globalThis.fetch
  let body: unknown = { notifications: [notification] }
  globalThis.fetch = async () => Response.json(body)
  try {
    assert.deepEqual(await listNotifications(), [notification])
    for (const invalid of [
      { notifications: [{ ...notification, createdAtUnixMs: -1 }] },
      { notifications: [{ ...notification, createdAtUnixMs: 1.5 }] },
      { notifications: [{ ...notification, id: '' }] },
      { notifications: [{ ...notification, title: 'x'.repeat(257) }] },
      { notifications: [{ ...notification, unknown: true }] },
      { notifications: Array(2001).fill(notification) },
      { notifications: [], unknown: true },
    ]) {
      body = invalid
      await assert.rejects(listNotifications())
    }
  } finally { globalThis.fetch = original }
})
