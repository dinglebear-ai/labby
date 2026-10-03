import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2500
  let last: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      last = error
    }
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20))
    })
  }
  throw last
}
test('replay previews current safe defaults, requires drift acknowledgment, and sends guard only on Run now', async () => {
  const window = installTestDom()
  for (const name of [
    'NodeFilter',
    'HTMLInputElement',
    'HTMLTextAreaElement',
  ] as const)
    Object.defineProperty(globalThis, name, {
      value: window[name],
      configurable: true,
    })
  const { ExecutionPreview } = await import('./execution-preview')
  const requests: Array<{ action: string; params: Record<string, unknown> }> =
    []
  globalThis.fetch = (async (_url, init) => {
    const body = JSON.parse(String(init?.body))
    requests.push(body)
    return new Response(
      JSON.stringify(
        body.action === 'snippets.get'
          ? {
              name: 'workflow',
              body: 'async()=>({ok:true})',
              inputs: {
                host: { ty: 'string', default: 'node' },
                token: { ty: 'string', default: 'secret' },
              },
            }
          : body.action === 'snippets.preview'
            ? {
                name: 'workflow',
                mode: 'metadata',
                dynamic_unknown: true,
                coverage: 'declared_tools_only',
                can_execute: true,
                input_summary: {
                  keys: ['host'],
                  provided_keys: ['host'],
                  defaulted_keys: [],
                },
                declared_tools: [],
                fingerprints: {},
                preview_fingerprint: 'guard',
                drift: [{ field: 'input', status: 'changed' }],
                warnings: [],
              }
            : { result: { ok: true } },
      ),
      { headers: { 'content-type': 'application/json' } },
    )
  }) as typeof fetch
  const view = await renderClient(
    <ExecutionPreview
      name="workflow"
      executionId="old-run"
      initialParams={{}}
      onClose={() => {}}
      onRun={(run) => {
        void run()
      }}
    />,
  )
  try {
    await waitFor(() =>
      assert.ok(
        requests.some((request) => request.action === 'snippets.preview'),
      ),
    )
    const field =
      document.body.querySelector<HTMLTextAreaElement>('#preview-inputs')!
    assert.deepEqual(JSON.parse(field.value), { host: 'node' })
    assert.doesNotMatch(document.body.textContent ?? '', /secret/)
    const run = Array.from(document.body.querySelectorAll('button')).find(
      (button) => button.textContent === 'Run now',
    )!
    assert.equal(run.disabled, true)
    assert.equal(
      requests.some((request) => request.action === 'snippets.replay'),
      false,
    )
    await act(async () =>
      document.body
        .querySelector<HTMLInputElement>('input[type="checkbox"]')!
        .click(),
    )
    assert.equal(run.disabled, false)
    await act(async () => run.click())
    await waitFor(() =>
      assert.ok(
        requests.some((request) => request.action === 'snippets.replay'),
      ),
    )
    assert.deepEqual(
      requests.find((request) => request.action === 'snippets.replay')!.params,
      {
        execution_id: 'old-run',
        params: { host: 'node' },
        expected_preview_fingerprint: 'guard',
        acknowledged_drift: ['input'],
      },
    )
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype, 'value',
      )!.set!.call(field, 'null')
      // React retains the first test window's event constructors.
      const propsKey = Object.keys(field).find((key) => key.startsWith('__reactProps$'))!
      const props = (field as unknown as Record<string, {
        onChange: (event: { target: { value: string } }) => void
      }>)[propsKey]
      props.onChange({ target: { value: 'null' } })
    })
    await waitFor(() =>
      assert.match(document.body.textContent ?? '', /Inputs must be a JSON object/),
    )
    assert.ok(document.body.querySelector('#preview-inputs'))
  } finally {
    await view.unmount()
  }
})
