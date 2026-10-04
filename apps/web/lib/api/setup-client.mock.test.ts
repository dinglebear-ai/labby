import test from 'node:test'
import assert from 'node:assert/strict'

process.env.NEXT_PUBLIC_MOCK_DATA = 'true'

async function loadSetupModules() {
  const { setupApi } = await import('./setup-client.ts')
  return { setupApi }
}

test('mock setup fixtures preserve secret and control metadata', async () => {
  const { setupApi } = await loadSetupModules()
  const schema = await setupApi.schemaGet()

  assert.deepEqual(Object.keys(schema.services).sort(), ['apprise', 'unifi'])
  assert.ok(Object.values(schema.services).every((service) => service.env.length > 0))
  assert.equal(schema.services.unifi.env.find(field => field.name === 'UNIFI_API_KEY')?.secret, true)
  assert.equal(schema.services.unifi.env.find(field => field.name === 'UNIFI_URL')?.ui?.kind, 'url')
})

test('mock setup state retains a genuine incomplete secret field', async () => {
  const { setupApi } = await loadSetupModules()
  const snapshot = await setupApi.state()

  assert.equal(snapshot.state.kind, 'partially_configured')
  assert.deepEqual(snapshot.state.missing, ['APPRISE_TOKEN'])
})
