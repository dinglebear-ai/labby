import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { ConsoleThemeControl } from './ConsolePreferences'

test('console theme segments expose the selected preference and dispatch all three choices', async () => {
  installTestDom()
  const choices: string[] = []
  const view = await renderClient(<ConsoleThemeControl theme="light" onTheme={theme => choices.push(theme)}/>)
  try {
    const buttons = Array.from(view.container.querySelectorAll('button'))
    assert.deepEqual(buttons.map(button => button.textContent), ['Dark', 'Light', 'System'])
    assert.deepEqual(buttons.map(button => button.getAttribute('aria-pressed')), ['false', 'true', 'false'])
    await act(async () => { for (const button of buttons) button.click() })
    assert.deepEqual(choices, ['dark', 'light', 'system'])
  } finally { await view.unmount() }
})
