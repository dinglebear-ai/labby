import test from 'node:test'
import assert from 'node:assert/strict'

import { mockGateways } from '../api/mock-data.ts'
import {
  buildGatewayActivityFeed,
  buildGatewayDocsSnapshot,
  buildGatewaySettingsSnapshot,
} from './admin-insights.ts'

test('buildGatewayActivityFeed sorts the newest gateway events first', () => {
  const events = buildGatewayActivityFeed(mockGateways)

  assert.ok(events.length > 0)
  assert.equal(events[0]?.gatewayName, 'database-server')
  assert.equal(events[0]?.kind, 'status')
  assert.match(events[0]?.detail ?? '', /Connection refused/i)
  assert.equal(events[1]?.kind, 'warning')
  assert.match(events[1]?.title ?? '', /CONNECTION_FAILED|DISCOVERY_FAILED/)
})

test('buildGatewayActivityFeed pushes invalid timestamps behind valid events deterministically', () => {
  const [firstGateway, secondGateway] = mockGateways
  assert.ok(firstGateway)
  assert.ok(secondGateway)

  const events = buildGatewayActivityFeed([
    {
      ...firstGateway,
      updated_at: 'not-a-timestamp',
      warnings: [],
    },
    {
      ...secondGateway,
      updated_at: '2026-04-15T16:42:00Z',
      warnings: [],
    },
  ])

  assert.equal(events[0]?.gatewayId, secondGateway.id)
  assert.equal(events.at(-1)?.gatewayId, firstGateway.id)
})

test('buildGatewaySettingsSnapshot summarizes gateway fleet posture and auth mode', () => {
  const snapshot = buildGatewaySettingsSnapshot(mockGateways, {
    hasStandaloneBearerAuth: true,
    hasMockData: true,
  })

  assert.equal(snapshot.authModeLabel, 'API token')
  assert.equal(snapshot.runtimeLabel, 'Mock preview')
  assert.equal(snapshot.totalGateways, 5)
  assert.equal(snapshot.connectedGateways, 4)
  assert.equal(snapshot.disconnectedGateways, 1)
  assert.equal(snapshot.warningCount, 3)
  assert.equal(snapshot.proxyResourceGateways, 4)
  assert.equal(snapshot.bearerTokenGateways, 2)
})

test('buildGatewaySettingsSnapshot reports browser session mode when a token exists but standalone bearer mode is off', () => {
  const snapshot = buildGatewaySettingsSnapshot(mockGateways, {
    hasStandaloneBearerAuth: false,
    hasMockData: false,
  })

  assert.equal(snapshot.authModeLabel, 'Browser session')
  assert.equal(snapshot.runtimeLabel, 'Live control plane')
})

test('buildGatewayDocsSnapshot derives operator-facing guidance from the current fleet', () => {
  const docs = buildGatewayDocsSnapshot(mockGateways, 4)

  assert.equal(docs.totalGateways, 5)
  assert.equal(docs.connectedGateways, 4)
  assert.equal(docs.warningCount, 3)
  assert.equal(docs.httpGateways, 2)
  assert.equal(docs.stdioGateways, 3)
  assert.equal(docs.supportedServices, 4)
  assert.equal(docs.exposedTools, 24)
})
