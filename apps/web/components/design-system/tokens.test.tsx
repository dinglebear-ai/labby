import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

import { auroraColorTokens } from './demo-data'

const here = dirname(fileURLToPath(import.meta.url))
const globalsCssPath = resolve(here, '..', '..', 'app', 'globals.css')
const css = readFileSync(globalsCssPath, 'utf8')

test('auroraColorTokens hex samples appear in app/globals.css (drift guard)', () => {
  for (const token of auroraColorTokens) {
    assert.ok(
      css.includes(token.value),
      `token "${token.label}" value ${token.value} missing from app/globals.css — update demo-data.ts or globals.css`,
    )
  }
})

test('auroraColorTokens covers the post-eixf.3 aurora status palette', () => {
  const labels: string[] = auroraColorTokens.map((t) => t.label)
  for (const expected of ['Aurora warn', 'Aurora error', 'Aurora success']) {
    assert.ok(
      labels.includes(expected),
      `expected demo-data.ts to include "${expected}" sample (eixf.3 added --aurora-success)`,
    )
  }
})
