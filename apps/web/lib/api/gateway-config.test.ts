import test from 'node:test'
import assert from 'node:assert/strict'

import {
  extractActionUrl,
  gatewayActionUrl,
  gatewayDetailHref,
  normalizeGatewayApiBase,
} from './gateway-config.ts'

test('normalizeGatewayApiBase trims a trailing slash and defaults to /v1', () => {
  assert.equal(normalizeGatewayApiBase('http://127.0.0.1:8765/v1/'), 'http://127.0.0.1:8765/v1')
  assert.equal(normalizeGatewayApiBase(undefined), '/v1')
})

test('gatewayActionUrl appends /gateway to the normalized base', () => {
  assert.equal(gatewayActionUrl('http://127.0.0.1:8765/v1/'), 'http://127.0.0.1:8765/v1/gateway')
})

test('extractActionUrl appends /extract to the normalized base', () => {
  assert.equal(extractActionUrl('http://127.0.0.1:8765/v1/'), 'http://127.0.0.1:8765/v1/extract')
})

test('gatewayDetailHref uses the export-safe query-string route', () => {
  assert.equal(gatewayDetailHref('synapse-stdio'), '/gateway/?id=synapse-stdio')
  assert.equal(gatewayDetailHref('lab core'), '/gateway/?id=lab%20core')
})
