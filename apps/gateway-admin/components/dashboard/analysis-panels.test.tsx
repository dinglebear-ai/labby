import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { CallOutcomesPanel } from './analysis-panels'

test('compact outcomes keep exact drill targets and use total calls as the bar denominator', async () => {
  const dom = installTestDom()
  const selected: string[] = []
  const view = await renderClient(<CallOutcomesPanel window="24h" toolCalls={{ total: 100, succeeded: 90, failed: 10 }} errors={{ total: 10, by_kind: [{ kind: 'timeout', count: 4 }, { kind: 'custom_failure', count: 3 }] }} onSelectOutcome={value => selected.push(value)} onSelectError={value => selected.push(value)}/>)
  try {
    assert.match(document.body.textContent ?? '', /Succeeded90/)
    assert.match(document.body.textContent ?? '', /Timed out4/)
    assert.match(document.body.textContent ?? '', /Other failures3/)
    assert.ok(document.querySelector('[style="width: 90%;"]'))
    assert.ok(document.querySelector('[style="width: 4%;"]'))
    const buttons = [...document.querySelectorAll<HTMLButtonElement>('button')]
    for (const label of ['Succeeded90', 'Timed out4', 'custom_failure3', 'Other failures3', '10 failed']) {
      await act(async () => buttons.find(button => button.textContent === label)!.click())
    }
    assert.deepEqual(selected, ['ok', 'timeout', 'custom_failure', 'failed', 'failed'])
  } finally { await view.unmount(); await dom.happyDOM.close() }
})

test('zero traffic produces no false failure percentage and full classifications need no remainder', () => {
  const empty = renderToStaticMarkup(<CallOutcomesPanel window="1h" toolCalls={{ total: 0, succeeded: 0, failed: 0 }} errors={{ total: 0, by_kind: [] }}/>)
  assert.doesNotMatch(empty, /100%|NaN|Infinity/)
  assert.match(empty, /width:0%/)
  const complete = renderToStaticMarkup(<CallOutcomesPanel window="1h" toolCalls={{ total: 10, succeeded: 8, failed: 2 }} errors={{ total: 2, by_kind: [{ kind: 'timeout', count: 2 }] }}/>)
  assert.doesNotMatch(complete, /Other failures/)
})
