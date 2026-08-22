import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { RejectedSkillsList } from './skills-page-content.tsx'

test('rejected skill details render literally and omit absent detail rows', async () => {
  installTestDom()
  const hostile = '<script>globalThis.pwned = true</script>'
  const view = await renderClient(<RejectedSkillsList rejected={[
    { uri: `skill://labby/${hostile}/SKILL.md`, reason: 'invalid_skill_uri', detail: hostile },
    { uri: 'skill://labby/no-detail/SKILL.md', reason: 'missing_manifest' },
  ]} />)
  try {
    assert.match(view.container.textContent ?? '', /globalThis\.pwned = true/)
    assert.equal(view.container.querySelectorAll('script').length, 0)
    assert.equal(view.container.querySelectorAll('li > span').length, 1)
  } finally {
    await view.unmount()
  }
})
