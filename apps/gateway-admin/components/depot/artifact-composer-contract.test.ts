import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

const source = readFileSync(new URL('./artifact-composer.tsx', import.meta.url), 'utf8')

test('Creator starts with writing tips collapsed', () => {
  assert.match(source, /const \[tipsOpen, setTipsOpen\] = React\.useState\(false\)/)
})

test('Creator exposes a dedicated icon-only new-artifact action', () => {
  assert.match(source, /aria-label=\"Create new artifact\"/)
  assert.match(source, /onClick=\{newArtifact\}/)
  assert.match(source, /<CirclePlus[^>]*\/>/)
})

test('Creator renders the official per-kind frontmatter field registry', () => {
  assert.match(source, /artifactFrontmatterFields\(kind\)/)
  assert.match(source, /Official frontmatter fields/)
  assert.match(source, /field\.ecosystem === 'Claude'/)
  assert.match(source, /field\.ecosystem === 'Codex'/)
})
