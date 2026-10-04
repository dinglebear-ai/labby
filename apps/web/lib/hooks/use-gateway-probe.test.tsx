import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../testing/dom-install.ts'
import type { Gateway, TestGatewayResult } from '../types/gateway.ts'
const window = installTestDom()

for (const fails of [false, true]) {
  test(`closing a probe panel cancels its pending replacement and suppresses late ${fails ? 'failure' : 'success'}`, async () => {
    const React = await import('react')
    const { act } = React
    const { renderClient } = await import('../testing/dom-test-utils.tsx')
    const { useGatewayProbe } = await import('./use-gateway-probe')
    const requests: Array<{ signal?: AbortSignal; resolve: (result: TestGatewayResult) => void; reject: (error: Error) => void }> = []
    const probe = (_id: string, signal?: AbortSignal) => new Promise<TestGatewayResult>((resolve, reject) => { requests.push({ signal, resolve, reject }) })
    const completions: string[] = []
    function Panel() {
      const { run, close, result, isTesting } = useGatewayProbe(probe)
      return React.createElement('div', null,
        React.createElement('button', { onClick: () => { void run({ id: 'alpha', name: 'alpha' } as Gateway).then(value => { if (value) completions.push(value.message) }).catch(error => completions.push(error.message)) } }, 'Test'),
        React.createElement('button', { onClick: close }, 'Close'),
        React.createElement('output', null, `${isTesting ? 'Testing' : 'Idle'}: ${result?.result.message ?? 'No result'}`))
    }
    const view = await renderClient(React.createElement(Panel))
    try {
      await act(async () => view.container.querySelectorAll('button')[0].click())
      await act(async () => requests[0].resolve({ success: true, message: 'First result' }))
      assert.match(view.container.textContent ?? '', /Idle: First result/)
      await act(async () => view.container.querySelectorAll('button')[0].click())
      assert.match(view.container.textContent ?? '', /Testing: First result/)
      await act(async () => view.container.querySelectorAll('button')[1].dispatchEvent(new window.MouseEvent('click', { bubbles: true }) as unknown as Event))
      assert.match(view.container.textContent ?? '', /Idle: No result/)
      assert.equal(requests[1].signal?.aborted, true)
      await act(async () => {
        if (fails) requests[1].reject(new Error('Obsolete failure'))
        else requests[1].resolve({ success: true, message: 'Obsolete success' })
      })
      assert.match(view.container.textContent ?? '', /Idle: No result/)
      assert.deepEqual(completions, ['First result'])
    } finally { await view.unmount() }
  })
}
