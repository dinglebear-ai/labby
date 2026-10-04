import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests, loadBrowserSession } from '@/lib/auth/session-store'
import type { SnippetExecutionReceipt } from '@/lib/types/snippets'

const receipt: SnippetExecutionReceipt = {
  execution_id: 'run-1', snippet_name: 'pulse', snippet_digest: 'source-digest', input_digest: 'input-digest', effective_scope_fingerprint: 'scope', runtime_version: '3.0.0', surface: 'api', created_at_ms: 100, elapsed_ms: 12, status: 'failed', error_kind: 'tool_error', result_digest: null, result_bytes: null, calls: [{ tool: 'host::status', params_digest: null, ok: false, elapsed_ms: 8, error_kind: 'timeout' }], tool_calls: 1, omitted_calls: 0, artifacts: [{ path: 'runs/run-1/output.json', sha256: 'hash', bytes: 2, content_type: 'application/json' }],
}
async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2000
  let last: unknown
  while (Date.now() < deadline) { try { assertion(); return } catch (error) { last = error }; await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) }) }
  throw last
}
function session(subject: string) { __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: subject }, expiresAt: 9999, csrfToken: 'csrf' }) }

test('history pages persisted receipts and displays call failures plus authorized artifact errors', async () => {
  const window = installTestDom(); session('history-owner')
  const { SnippetHistory } = await import('./snippet-history')
  let artifactAvailable = false
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = (async (_url, init) => {
    const payload = JSON.parse(String(init?.body)); requests.push(payload)
    const body = payload.action === 'snippets.history' ? { receipts: [receipt], next_cursor: payload.params.cursor ? null : 'next', receipt_status: 'persisted' } : payload.action === 'snippets.receipt' ? receipt : artifactAvailable ? { path: receipt.artifacts[0].path, sha256: 'hash', bytes: 2, content_type: 'application/json', content_base64: 'e30=' } : { message: 'Artifact is no longer retained', code: 'artifact_unavailable' }
    return new Response(JSON.stringify(body), { status: payload.action === 'snippets.artifact' && !artifactAvailable ? 404 : 200, headers: { 'content-type': 'application/json' } })
  }) as typeof fetch
  const view = await renderClient(<SnippetHistory name="pulse" revision={0} />)
  const click = async (text: string) => { const button = Array.from(view.container.querySelectorAll('button')).find((button) => button.textContent?.includes(text)); assert.ok(button, text); await act(async () => button.click()) }
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /run-1/))
    assert.deepEqual(requests[0].params, { name: 'pulse', limit: 20 })
    await click('run-1')
    await waitFor(() => assert.match(view.container.textContent ?? '', /host::status · failed \(timeout\) · 8 ms/))
    assert.equal(view.container.querySelector('a'), null, 'receipt paths must never become filesystem links')
    await click('Download artifact')
    await waitFor(() => assert.match(view.container.textContent ?? '', /Artifact is no longer retained/))
    assert.deepEqual(requests.find((request) => request.action === 'snippets.artifact')?.params, { execution_id: 'run-1', path: 'runs/run-1/output.json' })
    artifactAvailable = true
    const originalCreate = URL.createObjectURL, originalRevoke = URL.revokeObjectURL
    const originalClick = window.HTMLAnchorElement.prototype.click
    let downloaded = '', revoked = ''
    URL.createObjectURL = () => 'blob:authorized-artifact'
    URL.revokeObjectURL = (url) => { revoked = url }
    window.HTMLAnchorElement.prototype.click = function () { downloaded = this.href }
    try {
      await click('Download artifact')
      await waitFor(() => assert.equal(downloaded, 'blob:authorized-artifact'))
      assert.equal(revoked, 'blob:authorized-artifact')
      assert.equal(view.container.querySelector('iframe'), null)
    } finally { URL.createObjectURL = originalCreate; URL.revokeObjectURL = originalRevoke; window.HTMLAnchorElement.prototype.click = originalClick }
    await click('Next history page')
    await waitFor(() => assert.ok(requests.some((request) => request.params.cursor === 'next')))
  } finally { await view.unmount() }
})

test('history clears retained receipts on authority changes and cannot republish delayed prior-owner data', async () => {
  installTestDom(); session('old-owner')
  const { SnippetHistory } = await import('./snippet-history')
  let release: ((response: Response) => void) | undefined
  let calls = 0
  globalThis.fetch = ((input) => {
    if (String(input) === '/auth/session') return Promise.resolve(new Response(JSON.stringify({ authenticated: false }), { headers: { 'content-type': 'application/json' } }))
    calls++
    if (calls === 1) return new Promise<Response>((resolve) => { release = resolve })
    return Promise.resolve(new Response(JSON.stringify({ receipts: [], next_cursor: null, receipt_status: 'disabled' }), { headers: { 'content-type': 'application/json' } }))
  }) as typeof fetch
  const view = await renderClient(<SnippetHistory name="pulse" revision={0} />)
  try {
    await act(async () => { await loadBrowserSession() })
    await waitFor(() => assert.match(view.container.textContent ?? '', /Persistent receipts are disabled/))
    await act(async () => release?.(new Response(JSON.stringify({ receipts: [receipt], next_cursor: null, receipt_status: 'persisted' }), { headers: { 'content-type': 'application/json' } })))
    assert.doesNotMatch(view.container.textContent ?? '', /run-1|source-digest/)
  } finally { await view.unmount() }
})
