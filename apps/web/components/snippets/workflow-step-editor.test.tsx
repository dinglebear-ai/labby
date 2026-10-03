import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act, useState } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { WorkflowPlan } from './workflow-model'
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
      await new Promise((resolve) => setTimeout(resolve, 10))
    })
  }
  throw last
}
test('duplicate steps coalesce automatic schemas and keep separate edited mappings and references', async () => {
  installTestDom()
  const { WorkflowStepEditor } = await import('./workflow-step-editor')
  let requests = 0
  let current: WorkflowPlan = {
    version: 1,
    steps: [
      {
        id: 'first',
        tool: 'host::status',
        mapping: { host: 'edited' },
        dependsOn: [],
      },
      {
        id: 'second',
        tool: 'host::status',
        mapping: { host: '$steps.first.host' },
        dependsOn: [],
      },
    ],
  }
  globalThis.fetch = (async () => {
    requests++
    return new Response(
      JSON.stringify({
        path: 'host.status',
        id: 'host::status',
        namespace: 'host',
        name: 'status',
        description: 'Status',
        helper: 'host.status',
        signature: '()',
        tags: [],
        input_schema: {
          type: 'object',
          properties: { host: { type: 'string', default: 'suggested' } },
        },
        output_schema: {
          type: 'object',
          properties: { host: { type: 'string' } },
        },
      }),
      { headers: { 'content-type': 'application/json' } },
    )
  }) as typeof fetch
  function Harness() {
    const [plan, setPlan] = useState(current)
    const [schemas, setSchemas] = useState({})
    return (
      <WorkflowStepEditor
        plan={plan}
        onChange={(value) => {
          current = value
          setPlan(value)
        }}
        inputs={{}}
        onInputsChange={() => {}}
        schemas={schemas}
        onSchema={(id, schema) =>
          setSchemas((value) => ({ ...value, [id]: schema }))
        }
        onValidationError={() => {}}
        onInputSchema={() => {}}
      />
    )
  }
  const view = await renderClient(<Harness />)
  try {
    await waitFor(() => assert.ok(view.container.querySelector('#tool-0-host')))
    assert.equal(requests, 1)
    assert.equal(
      (view.container.querySelector('#tool-0-host') as HTMLInputElement).value,
      'edited',
    )
    assert.match(view.container.textContent ?? '', /Wave 2: second/)
    const field =
      view.container.querySelector<HTMLInputElement>('#step-name-0')!
    const key = Object.keys(field).find((key) =>
      key.startsWith('__reactProps$'),
    )!
    type Props = {
      onChange: (event: { target: { value: string } }) => void
      onBlur: () => void
    }
    await act(async () => {
      ;(field as unknown as Record<string, Props>)[key].onChange({
        target: { value: 'renamed' },
      })
    })
    assert.equal(
      view.container.querySelector('#step-name-0'),
      field,
      'typing must not remount focused input',
    )
    assert.equal(current.steps[0].id, 'first', 'rename commits on blur')
    await act(async () => {
      ;(field as unknown as Record<string, Props>)[key].onBlur()
    })
    assert.equal(current.steps[0].id, 'renamed')
    assert.equal(current.steps[1].mapping.host, '$steps.renamed.host')
    const remove = Array.from(view.container.querySelectorAll('button')).find(
      (button) => button.textContent === 'Delete step',
    )!
    await act(async () => remove.click())
    assert.match(
      view.container.textContent ?? '',
      /unknown dependency "renamed"/,
    )
    assert.equal(current.steps[0].mapping.host, '$steps.renamed.host')
  } finally {
    await view.unmount()
  }
})
