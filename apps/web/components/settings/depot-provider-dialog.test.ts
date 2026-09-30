import test from 'node:test'
import assert from 'node:assert/strict'
import { initialProviderAuthMode, providerRequiresFreshProof } from './depot-provider-dialog.tsx'
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
