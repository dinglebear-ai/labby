import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ContainerVersionPins } from './container-version-pins'

test('version panel exposes named draft inputs and remove callbacks', async () => {
  installTestDom()
  let edited: string[] = []
  let removed = ''
  const view = await renderClient(<ContainerVersionPins names={['Python', 'Docker']} versions={{ Python: '3.12.7' }} onVersion={(...args) => { edited = args }} onRemove={name => { removed = name }}/>)
  try {
    const input = view.container.querySelector<HTMLInputElement>('[aria-label="Pin version for Python"]')!
    assert.equal(input.value, '3.12.7')
    assert.equal(view.container.querySelector<HTMLInputElement>('[aria-label="Pin version for Docker"]')!.value, 'latest')
    const key = Object.keys(input).find(key => key.startsWith('__reactProps$'))!
    const props = (input as unknown as Record<string, { onChange: (event: { target: { value: string } }) => void }>)[key]
    await act(async () => props.onChange({ target: { value: '3.13.0' } }))
    assert.deepEqual(edited, ['Python', '3.13.0'])
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Remove Python"]')!.click())
    assert.equal(removed, 'Python')
  } finally { await view.unmount() }
})
