import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils'
import { TopToolsChart } from './top-tools-chart'

installTestDom()
test('rows preserve supplied counts and exact tool selection identity', async () => {
  const selected: string[] = []
  const view = await renderClient(<TopToolsChart tools={[{ name: 'corpus::search', label: 'Search OAuth', calls: 80, failed: 2 }, { name: 'labby::list', calls: 20, failed: 0 }]} onSelect={name => selected.push(name)}/>)
  try {
    const buttons = [...document.querySelectorAll<HTMLButtonElement>('button')]
    assert.equal(buttons.length, 2)
    assert.match(buttons[0].getAttribute('aria-label')!, /80 calls, 2 failed/)
    await act(async () => buttons[0].click())
    assert.deepEqual(selected, ['corpus::search'])
    assert.equal(buttons[1].querySelector<HTMLElement>('[style]')!.style.width, '25%')
  } finally { await view.unmount() }
})
test('zero and empty data do not invent actions or invalid bar widths', () => {
  const html = renderToStaticMarkup(<TopToolsChart tools={[{ name: 'zero', calls: 0, failed: 0 }]}/>)
  assert.doesNotMatch(html, /<button|NaN|Infinity|failed/)
  assert.match(html, /width:0%/)
  assert.match(renderToStaticMarkup(<TopToolsChart tools={[]}/>), /<ol/)
})
