import test from 'node:test'
import assert from 'node:assert/strict'

import { mockGateways } from '../api/mock-data.ts'
import {
  ACTIVITY_SUBSYSTEMS,
  buildActivityItemsFromLogs,
  buildGatewayActivityFeed,
  buildGatewayDocsSnapshot,
  buildGatewaySettingsSnapshot,
} from './admin-insights.ts'
import type { LogEvent } from '../types/logs.ts'

function logEvent(overrides: Partial<LogEvent> = {}): LogEvent {
  return {
    event_id: overrides.event_id ?? 'evt-activity',
    ts: overrides.ts ?? Date.now(),
    level: overrides.level ?? 'info',
    subsystem: overrides.subsystem ?? 'gateway',
    surface: overrides.surface ?? 'api',
    action: overrides.action ?? 'gateway.add',
    message: overrides.message ?? 'gateway added',
    request_id: overrides.request_id ?? 'req-activity',
    session_id: overrides.session_id ?? null,
    correlation_id: overrides.correlation_id ?? null,
    trace_id: overrides.trace_id ?? null,
    span_id: overrides.span_id ?? null,
    instance: overrides.instance ?? null,
    auth_flow: overrides.auth_flow ?? null,
    outcome_kind: overrides.outcome_kind ?? 'ok',
    fields_json: overrides.fields_json ?? {},
    source_kind: overrides.source_kind ?? 'local',
    source_node_id: overrides.source_node_id ?? null,
    source_device_id: overrides.source_device_id ?? null,
    ingest_path: overrides.ingest_path ?? 'tracing',
    upstream_event_id: overrides.upstream_event_id ?? null,
  }
}

test('activity subsystem query includes app, gateway, node, MCP, and auth sources', () => {
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('gateway'))
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('api'))
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('web'))
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('syslog'))
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('mcp_server'))
  assert.ok(ACTIVITY_SUBSYSTEMS.includes('oauth_relay'))
})

test('buildActivityItemsFromLogs describes gateway and device activity as first-class activity', () => {
  const items = buildActivityItemsFromLogs([
    logEvent({ action: 'gateway.add', message: 'Added controller gateway' }),
    logEvent({
      event_id: 'evt-node',
      subsystem: 'syslog',
      surface: 'core_runtime',
      action: 'node.status',
      message: 'controller is online',
      source_node_id: 'controller',
    }),
  ])

  assert.equal(items[0]?.kind, 'gateway')
  assert.equal(items[0]?.title, 'Added gateway')
  assert.match(items[0]?.detail ?? '', /Added controller gateway/)
  assert.equal(items[1]?.kind, 'device')
  assert.equal(items[1]?.title, 'Device status changed')
  assert.match(items[1]?.detail ?? '', /node=controller/)
})

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
