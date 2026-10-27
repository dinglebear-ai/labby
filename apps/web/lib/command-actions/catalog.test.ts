import test from 'node:test'
import assert from 'node:assert/strict'
import { fetchCatalog } from './catalog'

test('catalog fetch retains action normalization and optional service defaults', async () => {
  const original = globalThis.fetch
  globalThis.fetch = async () => Response.json({ services: [{
    name: 'fixture', description: 'Fixture', actions: [{
      name: 'fixture.read', description: 'Read', destructive: false,
      params: [{ name: 'id', ty: 'string', required: true, description: 'Identifier' }], returns: 'object',
    }],
  }] })
  try {
    const catalog = await fetchCatalog()
    assert.equal(catalog.services[0].category, '')
    assert.equal(catalog.services[0].status, 'available')
    assert.equal(catalog.services[0].actions[0].action, 'fixture.read')
    assert.equal('name' in catalog.services[0].actions[0], false)
    assert.equal(catalog.services[0].actions[0].params[0].required, true)
  } finally { globalThis.fetch = original }
})

test('catalog fetch rejects incompatible wire shapes with decode_error', async () => {
  const original = globalThis.fetch
  globalThis.fetch = async () => Response.json({ services: [{ name: 7 }] })
  try { await assert.rejects(fetchCatalog(), { kind: 'decode_error' }) }
  finally { globalThis.fetch = original }
})
