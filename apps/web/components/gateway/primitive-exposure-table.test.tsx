import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

for (const primitive of ['resources', 'prompts']) test(`${primitive}: typing into controlled search preserves an exposure draft`, async () => {
  const window = installTestDom()
  const React = await import('react')
  const { act } = React
  const { renderClient } = await import('../../lib/testing/dom-test-utils.tsx')
  const { File } = await import('lucide-react')
  const { PrimitiveExposureTable } = await import('./primitive-exposure-table')
  const saves: Array<string[] | null> = []
  function Harness({ exposed = true }: { exposed?: boolean }) {
    const [searchValue, onSearchValueChange] = React.useState('')
    return React.createElement(PrimitiveExposureTable, {
      title: primitive, description: 'Primitive exposure', icon: File,
      searchPlaceholder: `Search ${primitive}`, manageLabel: `Manage ${primitive}`, emptyLabel: 'No items',
      exposureEnabled: true, searchValue, onSearchValueChange,
      items: ['alpha', 'beta'].map((name) => ({ name, exposed })),
      onSaveSelection: async (names) => { saves.push(names) },
    })
  }
  const view = await renderClient(React.createElement(Harness))
  const search = async (value: string) => {
    const input = view.container.querySelector('input[placeholder]')
    assert.ok(input)
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set?.call(input, value)
      input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
    })
  }
  const click = async (text: string) => {
    const button = [...view.container.querySelectorAll('button')].find((item) => item.textContent?.trim() === text)
    assert.ok(button, `missing ${text}`)
    await act(async () => button.click())
  }
  try {
    await click(`Manage ${primitive}`)
    await click('Hide')
    await search('beta')
    await search('')
    await click('Save changes')
    assert.deepEqual(saves, [['beta']])
    // A real server policy revision replaces the draft baseline.
    await view.rerender(React.createElement(Harness, { exposed: false }))
    await click(`Manage ${primitive}`)
    await click('Expose')
    await click('Save changes')
    assert.deepEqual(saves, [['beta'], ['alpha']])
  } finally {
    await view.unmount()
  }
})
