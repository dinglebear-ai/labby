import test from 'node:test'
import assert from 'node:assert/strict'

process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

const [{ setupApi }, { isKnownService }] = await Promise.all([
  import('./setup-client.ts'),
  import('../setup/buildServiceSlugs.ts'),
])

test('mock setup services all resolve to pre-rendered detail routes', async () => {
  const schema = await setupApi.schemaGet()

  assert.deepEqual(Object.keys(schema.services).sort(), ['apprise', 'unifi'])
  assert.ok(Object.keys(schema.services).every(isKnownService))
  assert.ok(Object.values(schema.services).every((service) => service.env.length > 0))
})

test('mock setup state retains a genuine incomplete secret field', async () => {
  const snapshot = await setupApi.state()

  assert.equal(snapshot.state.kind, 'partially_configured')
  assert.deepEqual(snapshot.state.missing, ['APPRISE_TOKEN'])
})
