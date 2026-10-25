import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'
import { DEFAULT_UI } from '@/lib/setup/schemaBuilder'

test('ServiceForm blocks invalid saves and keeps a stored secret on valid save', async () => {
  const window = installTestDom()
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { ServiceForm } = await import('./ServiceForm')
  const saves: Record<string, string>[] = []
  const fields = [
    { name: 'URL', description: '', example: '', required: true, secret: false, ui: DEFAULT_UI },
    { name: 'TOKEN', description: '', example: '', required: true, secret: true, hasStoredSecret: true, ui: DEFAULT_UI },
  ]
  const view = await renderClient(<ServiceForm fields={fields} defaultValues={{ URL: '', TOKEN: '' }} onSave={async values => { saves.push(values) }} />)
  try {
    const form = view.container.querySelector('form')!
    await act(async () => { form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })) })
    assert.deepEqual(saves, [])
    assert.equal(view.container.querySelector('#field-URL')?.getAttribute('aria-invalid'), 'true')
    assert.match(view.container.textContent ?? '', /Required/)
    const input = view.container.querySelector<HTMLInputElement>('#field-URL')!
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'configured')
      input.dispatchEvent(new Event('input', { bubbles: true }))
    })
    await act(async () => { form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })) })
    assert.deepEqual(saves, [{ URL: 'configured' }])
  } finally {
    await view.unmount()
    await window.happyDOM.close()
  }
})
