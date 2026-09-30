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
    assert.match(view.container.textContent ?? '', /Invalid skill URI/)
    assert.match(view.container.textContent ?? '', /Serve the manifest from a canonical skill resource URI/)
  } finally {
    await view.unmount()
  }
})

test('rejected skills collapse repeated failures into one remediation group', async () => {
  installTestDom()
  const view = await renderClient(<RejectedSkillsList rejected={[
    { uri: 'skill://labby/one/SKILL.md', reason: 'invalid_frontmatter', detail: 'metadata must be strings' },
    { uri: 'skill://labby/two/SKILL.md', reason: 'invalid_frontmatter', detail: 'allowed-tools must be a string' },
  ]} />)
  try {
    assert.equal(view.container.querySelectorAll('details').length, 1)
    assert.match(view.container.querySelector('summary')?.textContent ?? '', /Invalid manifest frontmatter2/)
    assert.match(view.container.textContent ?? '', /pinned SEP-2640 Agent Skills contract/)
    assert.match(view.container.textContent ?? '', /allowed-tools is a space-separated string/)
  } finally {
    await view.unmount()
  }
})
