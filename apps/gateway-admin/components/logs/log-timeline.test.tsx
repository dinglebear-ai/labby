import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { LogEventInspector } from './log-event-inspector'
import { LogTimeline } from './log-timeline'
import type { LogEvent } from '@/lib/types/logs'

const event: LogEvent = {
  event_id: 'evt_1',
  ts: 1713381600000,
  level: 'info',
  subsystem: 'gateway',
  surface: 'api',
  action: 'gateway.health',
  message: 'health ok',
  request_id: 'req_1',
  session_id: null,
  correlation_id: null,
  trace_id: null,
  span_id: null,
  instance: null,
  auth_flow: null,
  outcome_kind: null,
  fields_json: { host: 'media.example.com' },
  source_kind: null,
  source_node_id: null,
  source_device_id: null,
  ingest_path: null,
  upstream_event_id: null,
}

test('log timeline uses display headers and strong lifted panel styling', () => {
  const markup = renderToStaticMarkup(
    React.createElement(LogTimeline, {
      events: [event],
      isLoading: false,
      selectedEventId: event.event_id,
      expandedEventId: null,
      showLiveEdgeBadge: true,
      viewportRef: { current: null },
      onViewportScroll: () => {},
      onSelectEvent: () => {},
      onToggleExpanded: () => {},
    }),
  )

  assert.match(markup, /font-display/)
  assert.match(markup, /bg-aurora-panel-strong/)
  assert.match(markup, /Live edge/)
  assert.match(markup, /aria-expanded="false"/)
  assert.match(markup, /aria-controls="log-event-details-evt_1"/)
  assert.match(markup, /aria-label="Expand log details"/)
})

test('log inspector uses display headers and stronger elevated metadata panels', () => {
  const markup = renderToStaticMarkup(
    React.createElement(LogEventInspector, {
      event,
    }),
  )

  assert.match(markup, /font-display/)
  assert.match(markup, /Primary message/)
  assert.match(markup, /Metadata JSON/)
  assert.match(markup, /bg-aurora-panel-strong/)
})
