import test from 'node:test'
import assert from 'node:assert/strict'

import type { OrganizationBootstrapCreateResponse } from '../api/setup-client.ts'
import {
  normalizeOrganizationProfileOffer,
  normalizePersonalLabbyOrigin,
  organizationProfileHandoffHref,
  organizationProfileOfferFromHash,
  parseOrganizationProfileOfferJson,
} from './organization-profile-handoff.ts'

function offer(): OrganizationBootstrapCreateResponse {
  return {
    signer_fingerprint: 'ab'.repeat(32),
    profile: {
      schema_version: 'labby.organization-bootstrap/v1',
      organization_id: 'org-team',
      issued_at: 123,
      key_id: 'team-key',
      verifying_key: 'verify-key',
      integrations: [
        {
          name: 'team-depot',
          display_name: 'Team Depot',
          url: 'https://depot.example.com/mcp',
          proxy_resources: true,
          proxy_prompts: true,
          proxy_skills: true,
        },
      ],
      signature: 'signature',
    },
  }
}

test('personal Labby handoff accepts HTTPS and loopback HTTP origins only', () => {
  assert.equal(normalizePersonalLabbyOrigin('https://personal.example.com/'), 'https://personal.example.com')
  assert.equal(normalizePersonalLabbyOrigin('http://127.0.0.1:8765'), 'http://127.0.0.1:8765')
  assert.equal(normalizePersonalLabbyOrigin('http://localhost:8765'), 'http://localhost:8765')
  assert.equal(normalizePersonalLabbyOrigin('http://192.168.1.25:8765'), undefined)
  assert.equal(normalizePersonalLabbyOrigin('https://personal.example.com/path'), undefined)
  assert.equal(normalizePersonalLabbyOrigin('https://user:secret@personal.example.com'), undefined)
})

test('organization profile handoff stays entirely in the URL fragment', () => {
  const href = organizationProfileHandoffHref('https://personal.example.com', offer())
  const [requestUrl, fragment] = href.split('#', 2)
  assert.equal(requestUrl, 'https://personal.example.com/')
  assert.equal(requestUrl.includes('org-team'), false)
  assert.equal(requestUrl.includes('abababab'), false)
  assert.match(fragment, /^organization-bootstrap=/)
  assert.deepEqual(organizationProfileOfferFromHash('#' + fragment), offer())
})

test('organization profile handoff parser rejects query-shaped and malformed data', () => {
  const valid = offer()
  const encoded = new URLSearchParams({ 'organization-bootstrap': JSON.stringify(valid) }).toString()
  assert.equal(organizationProfileOfferFromHash('?' + encoded), undefined)
  assert.equal(parseOrganizationProfileOfferJson('{not-json'), undefined)
  assert.equal(normalizeOrganizationProfileOffer({ ...valid, signer_fingerprint: 'short' }), undefined)
  assert.equal(normalizeOrganizationProfileOffer({ signer_fingerprint: 'ab'.repeat(32), profile: null }), undefined)
})
