import test from 'node:test'
import assert from 'node:assert/strict'
import { initialProviderAuthMode, providerRequiresFreshProof, providerInputError } from './depot-provider-dialog.tsx'
import type { DepotProvider } from '@/lib/api/depot-client'

const provider = (authMode: 'anonymous'|'bearer', credentialConfigured: boolean): DepotProvider => ({
  id: 'team', name: 'Team', endpoint: 'https://depot.example', enabled: false,
  authMode, builtin: false, configVersion: 'v1', credentialConfigured,
  health: { state: 'unknown', observedAt: null, provenance: null, retryNotBefore: null },
})

test('anonymous provider creation does not require fresh authentication', () => {
  assert.equal(providerRequiresFreshProof(undefined, 'https://depot.example', 'anonymous', 'retain'), false)
})

test('configured bearer mode is preserved independently of credential availability', () => {
  const configured = provider('bearer', false)
  assert.equal(initialProviderAuthMode(configured), 'bearer')
  assert.equal(providerRequiresFreshProof(configured, configured.endpoint, configured.authMode, 'retain'), false)
})


test('provider form validates IDs and HTTPS endpoints before testing or saving', () => {
  assert.equal(providerInputError('my-catalog', 'My catalog', 'https://catalog.example.com/depot'), undefined)
  for (const id of ['all', 'legacy', 'public', '-catalog', 'catalog-', 'CATALOG', 'a'.repeat(65)]) {
    assert.match(providerInputError(id, 'Catalog', 'https://catalog.example.com') ?? '', /ID/)
  }
  for (const endpoint of ['http://catalog.example.com', 'https://user:token@catalog.example.com', 'https://catalog.example.com?token=x', 'https://catalog.example.com#fragment', ' https://catalog.example.com']) {
    assert.match(providerInputError('catalog', 'Catalog', endpoint) ?? '', /HTTPS/)
  }
  assert.match(providerInputError('catalog', '', 'https://catalog.example.com') ?? '', /display name/)
})
