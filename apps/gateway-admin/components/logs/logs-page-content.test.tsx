import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { LogsPageContent, mergeKnownLogSources } from './logs-page-content.tsx'

const logEntry = (service: string) => ({
  timestamp: '2026-09-09T12:00:00Z',
  level: 'INFO',
  service,
  target: null,
  message: 'message',
  action: null,
  kind: null,
  file: 'labby.jsonl',
  fields: {},
})

test('source choices remain stable when a later filtered response omits a source', () => {
  const initial = mergeKnownLogSources([], [logEntry('gateway'), logEntry('upstream.pool')])
  const filtered = mergeKnownLogSources(initial, [logEntry('gateway')])

  assert.deepEqual(filtered, ['gateway', 'upstream.pool'])
})

test('log stream retains pause and filter controls with accessible literal row details', async () => {
  const window = installTestDom()
  const previousFetch = globalThis.fetch
  let copiedText = ''
  Object.defineProperty(window.navigator, 'clipboard', {
    configurable: true,
    value: { writeText: async (value: string) => { copiedText = value } },
  })
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'admin' }, expiresAt: 100, csrfToken: 'csrf', isAdmin: true })
  const requests: Array<{ params: Record<string, unknown> }> = []
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body))
    requests.push(request)
    return new Response(JSON.stringify({ kind: 'server_logs', entries: [{
      timestamp: '2026-09-09T12:00:00Z', level: 'ERROR', service: 'gateway', target: null,
      message: 'Request timed out', action: null, kind: 'timeout', file: 'labby.jsonl',
      fields: { message: '<script>not executable</script>', elapsed_ms: 250 },
    }], filters: { levels: request.params.levels ?? [] }, available_sources: ['gateway', 'upstream.pool'], available_sources_complete: true, matched: 1, scanned_lines: 3, malformed_lines: 0, scanned_bytes: 100, max_scan_bytes: 1024 * 1024, truncated: false }), { status: 200 })
  }
  const view = await renderClient(<LogsPageContent />)
  try {
    const sourcePicker = view.container.querySelector('[role="combobox"][aria-label="Log source"]')
    assert.ok(sourcePicker)
    assert.equal(sourcePicker.getAttribute('data-visible-label'), '1')
    assert.equal(view.container.querySelector('select[aria-label="Log source"]'), null)
    const stream = view.container.querySelector('[role="log"]'); assert.ok(stream)
    assert.equal(stream.getAttribute('aria-live'), 'polite')
    assert.ok(stream.querySelector('.sticky'))
    const row = stream.querySelector<HTMLElement>('[data-log-line="1"]'); assert.ok(row)
    assert.equal(row.getAttribute('aria-expanded'), 'false')
    await act(async () => { row.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.equal(row.getAttribute('aria-expanded'), 'true')
    assert.match(stream.querySelector('[data-log-detail="1"]')?.textContent ?? '', /<script>not executable<\/script>/)
    assert.equal(stream.querySelector('script'), null)
    const copy = row.querySelector<HTMLButtonElement>('button[aria-label="Copy log line"]'); assert.ok(copy)
    await act(async () => { copy.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)); await Promise.resolve() })
    assert.match(copiedText, /2026-09-09T12:00:00Z ERROR gateway Request timed out/)
    assert.equal(row.getAttribute('aria-expanded'), 'true')
    assert.equal(copy.getAttribute('aria-label'), 'Log line copied')
    const pause = view.container.querySelector('[title="Pause live tail"]'); assert.ok(pause)
    await act(async () => { pause.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.equal(stream.getAttribute('aria-live'), 'off')
    const errorFilter = [...view.container.querySelectorAll('button[aria-pressed]')].find(button => button.textContent?.includes('ERROR')); assert.ok(errorFilter)
    await act(async () => { errorFilter.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.deepEqual(requests.at(-1)?.params.levels, ['ERROR'])
    const warnFilter = [...view.container.querySelectorAll('button[aria-pressed]')].find(button => button.textContent?.includes('WARN')); assert.ok(warnFilter)
    await act(async () => { warnFilter.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.deepEqual(requests.at(-1)?.params.levels, ['ERROR', 'WARN'])
    assert.equal(requests.at(-1)?.params.stop_after_limit, false)
    assert.equal(stream.getAttribute('aria-live'), 'off')
    assert.ok(view.container.querySelector('[title="Follow live tail"]'))
    assert.match(view.container.textContent ?? '', /source list complete for this retained-file query within the 1 MiB scan budget/i)
  } finally {
    await view.unmount()
    globalThis.fetch = previousFetch
  }
})

test('embedded logs scope exact upstream fields inside the bounded query sample', async () => {
  installTestDom()
  const previousFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'admin' }, expiresAt: 100, csrfToken: 'csrf', isAdmin: true })
  let params: Record<string, unknown> = {}
  globalThis.fetch = async (_input, init) => {
    params = JSON.parse(String(init?.body)).params
    const base = { timestamp: '2026-09-09T12:00:00Z', level: 'INFO', service: 'upstream.pool', target: null, action: null, kind: null, file: 'labby.jsonl' }
    return new Response(JSON.stringify({ kind: 'server_logs', entries: [
      { ...base, message: 'Exact upstream row', fields: { upstream: 'git' } },
      { ...base, message: 'Substring upstream row', fields: { upstream: 'github' } },
      { ...base, message: 'Unrelated text mentions git', fields: {} },
    ], available_sources: ['upstream.pool'], available_sources_complete: true, matched: 3, scanned_lines: 3, malformed_lines: 0, scanned_bytes: 100, max_scan_bytes: 1000, truncated: false }), { status: 200 })
  }
  const view = await renderClient(<LogsPageContent embedded upstream="git" />)
  try {
    assert.equal(view.container.querySelector('h1'), null)
    assert.equal(view.container.querySelector('header'), null)
    assert.equal(params.query, 'git')
    assert.equal(params.upstream, undefined)
    assert.equal(params.limit, 250)
    assert.equal(params.stop_after_limit, true)
    assert.match(view.container.textContent ?? '', /Exact upstream row/)
    assert.doesNotMatch(view.container.textContent ?? '', /Substring upstream row|Unrelated text mentions git/)
    assert.match(view.container.textContent ?? '', /git · bounded sample/)
  } finally {
    await view.unmount()
    globalThis.fetch = previousFetch
  }
})
