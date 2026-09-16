import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

const source = readFileSync(new URL('./console-global-tools.tsx', import.meta.url), 'utf8')

test('Phoenix composer accepts pasted files and keeps typing enabled while a turn runs', () => {
  assert.match(source, /onPaste=\{\(event\) => \{ if \(event\.clipboardData\.files\.length\)/)
  const textarea = source.match(/<Textarea aria-label="Message Phoenix"[\s\S]*?\/>/)?.[0] ?? ''
  assert.ok(textarea, 'Phoenix textarea must exist')
  assert.doesNotMatch(textarea, /disabled=\{sending/)
  assert.match(textarea, /Message Phoenix while it works/)
})

test('Phoenix send and steer permit attachment-only turns', () => {
  assert.match(source, /\(!text && outgoingAttachments\.length === 0\) \|\| sending/)
  assert.match(source, /\(!text && outgoingAttachments\.length === 0\) \|\| steering/)
  assert.match(source, /disabled=\{sending \|\| \(!input\.trim\(\) && attachments\.length === 0\)\}/)
  assert.match(source, /disabled=\{\(!input\.trim\(\) && attachments\.length === 0\) \|\| steering\}/)
})

test('Phoenix classifies bounded UTF-8 text and code files as text attachments', () => {
  assert.match(source, /textLike = \(file: File\)/)
  assert.match(source, /file\.size <= 512 \* 1024\) return 'text'/)
  assert.match(source, /type === 'text' \? 'data:text\/plain;base64,'/)
})
