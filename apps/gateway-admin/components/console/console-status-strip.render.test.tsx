import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ConsoleStatusContent } from './console-status-strip'

test('status strip renders nothing while loading or when the viewer lacks admin scope', () => {
  assert.equal(renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'loading' }} />), '')
  assert.equal(renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'unauthorized' }} />), '')
})

test('status strip shows a visible unavailable marker that carries the reason', () => {
  const html = renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'unavailable', reason: 'HTTP 502' }} />)
  assert.match(html, /data-console-status-unavailable="1"/)
  assert.match(html, /title="Gateway status is unavailable: HTTP 502"/)
  assert.match(html, />status unavailable</)
})

test('status strip metrics reflect health and omit sessions when clients were not listed', () => {
  const healthy = renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'ready', snapshot: { connected: 3, total: 3, sessions: 2, tools: 41 } }} />)
  assert.match(healthy, />3\/3<\/span><span>up</)
  assert.match(healthy, /color:var\(--aurora-success\)[^>]*>3\/3</)
  assert.match(healthy, />2<\/span><span>sessions</)
  assert.match(healthy, />41<\/span><span>tools</)
  const degraded = renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'ready', snapshot: { connected: 1, total: 3, tools: 4 } }} />)
  assert.match(degraded, /color:var\(--aurora-warn\)[^>]*>1\/3</)
  assert.doesNotMatch(degraded, /sessions/)
  const clientsDown = renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'ready', snapshot: { connected: 3, total: 3, tools: 4, sessionsUnavailable: 'HTTP 500' } }} />)
  assert.match(clientsDown, /aria-label="Session count is unavailable: HTTP 500"/)
  assert.match(clientsDown, />—<\/span><span>sessions</)
})

test('status strip unavailable marker is announced with its reason', () => {
  const html = renderToStaticMarkup(<ConsoleStatusContent state={{ kind: 'unavailable', reason: 'HTTP 502' }} />)
  assert.match(html, /role="status"/)
  assert.match(html, /aria-label="Gateway status is unavailable: HTTP 502"/)
})
