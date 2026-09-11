import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { __setBrowserSessionStateForTests } from '../../lib/auth/session-store.ts'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { LogsPageContent } from './logs-page-content.tsx'

test('log stream retains pause and filter controls with accessible literal row details', async () => {
  const window = installTestDom()
  const previousFetch = globalThis.fetch
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'admin' }, expiresAt: 100, csrfToken: 'csrf', isAdmin: true })
  const requests: Array<{ params: Record<string, unknown> }> = []
  globalThis.fetch = async (_input, init) => {
    requests.push(JSON.parse(String(init?.body)))
    return new Response(JSON.stringify({ kind: 'server_logs', entries: [{
      timestamp: '2026-09-09T12:00:00Z', level: 'ERROR', service: 'gateway', target: null,
      message: 'Request timed out', action: null, kind: 'timeout', file: 'labby.jsonl',
      fields: { message: '<script>not executable</script>', elapsed_ms: 250 },
    }], matched: 1, scanned_lines: 3, malformed_lines: 0, scanned_bytes: 100, max_scan_bytes: 1000, truncated: false }), { status: 200 })
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
    const row = stream.querySelector('button'); assert.ok(row)
    assert.equal(row.getAttribute('aria-expanded'), 'false')
    await act(async () => { row.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.equal(row.getAttribute('aria-expanded'), 'true')
    assert.match(stream.querySelector('pre')?.textContent ?? '', /<script>not executable<\/script>/)
    assert.equal(stream.querySelector('script'), null)
    const pause = view.container.querySelector('[title="Pause live tail"]'); assert.ok(pause)
    await act(async () => { pause.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.equal(stream.getAttribute('aria-live'), 'off')
    const errorFilter = [...view.container.querySelectorAll('button[aria-pressed]')].find(button => button.textContent?.includes('ERROR')); assert.ok(errorFilter)
    await act(async () => { errorFilter.dispatchEvent((new window.MouseEvent('click', { bubbles: true }) as unknown as Event)) })
    assert.equal(requests.at(-1)?.params.level, 'ERROR')
    assert.equal(requests.at(-1)?.params.stop_after_limit, true)
    assert.equal(stream.getAttribute('aria-live'), 'off')
    assert.ok(view.container.querySelector('[title="Follow live tail"]'))
  } finally {
    await view.unmount()
    globalThis.fetch = previousFetch
  }
})
