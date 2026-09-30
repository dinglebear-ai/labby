import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverReadme } from './discover-readme'

test('README preview renders headings but never raw HTML or external images', () => {
  const html = renderToStaticMarkup(<DiscoverReadme path="README.md" content={'# Actual source\n\n<script>alert(1)</script>\n\n![tracking](https://example.com/pixel.png)\n\n[unsafe](javascript:alert(1))'} />)
  assert.match(html, /aria-label="README"/)
  assert.match(html, /Actual source/)
  assert.doesNotMatch(html, /<script|<img|href="javascript:/)
})

test('source fallback and empty files are labeled without manufacturing README text', () => {
  const html = renderToStaticMarkup(<DiscoverReadme path="skills/example/SKILL.md" content="" />)
  assert.match(html, /aria-label="Source document"/)
  assert.match(html, /skills\/example\/SKILL.md/)
  assert.match(html, /This file is empty/)
  assert.doesNotMatch(html, /Install|depot add/)
})

test('compact README presentation preserves supplied links and code without adding commands', () => {
  const html = renderToStaticMarkup(<DiscoverReadme path="docs/README.md" content={'## Usage\n\n[Docs](https://example.com/docs)\n\n```text\nactual supplied command\n```'} />)
  assert.match(html, /py-\[9px\]/)
  assert.match(html, /text-\[12\.5px\]/)
  assert.match(html, /href="https:\/\/example.com\/docs"/)
  assert.match(html, /actual supplied command/)
  assert.match(html, /data-streamdown="code-block-header"/)
  assert.match(html, /data-streamdown="code-block-body"/)
  assert.ok(html.includes('[&amp;&gt;div&gt;*]:!my-0'))
  assert.ok(html.includes('[&amp;&gt;div]:gap-[11px]'))
  assert.ok(html.includes('[&amp;_[data-streamdown=code-block-body]]:!border-0'))
  assert.ok(html.includes('[&amp;_pre]:!p-0'))
  assert.doesNotMatch(html, /Copy install command|depot add/)
})
