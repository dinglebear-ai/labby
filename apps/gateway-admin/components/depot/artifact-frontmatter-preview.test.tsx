import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { ArtifactFrontmatterPreview } from './artifact-frontmatter-preview'

const metadata = { name: 'repo-triage', description: '<script>literal</script>', tags: ['review', 'github'], license: 'MIT', compatibility: '', allowedTools: 'Read Search' }
test('frontmatter preview uses composed fields as literal text, not executable HTML', () => {
  const html = renderToStaticMarkup(<ArtifactFrontmatterPreview kind="Skill" metadata={metadata} />)
  assert.match(html, /name: &quot;repo-triage&quot;/)
  assert.match(html, /tags: \[review, github\]/)
  assert.match(html, /license: &quot;MIT&quot;/)
  assert.match(html, /allowed-tools: &quot;Read Search&quot;/)
  assert.match(html, /&lt;script&gt;literal&lt;\/script&gt;/)
  assert.doesNotMatch(html, /<script>/)
  assert.match(html, /aria-label="Copy frontmatter"/)
})
test('non-Markdown artifacts do not acquire invented YAML frontmatter', () => {
  assert.equal(renderToStaticMarkup(<ArtifactFrontmatterPreview kind="MCP" metadata={metadata} />), '')
})
