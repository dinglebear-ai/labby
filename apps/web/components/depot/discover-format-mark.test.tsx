import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { DiscoverFormatMark } from './discover-format-mark'

test('Claude marks identify only explicit Claude formats', () => {
  for (const format of ['claude-skill', 'claude-agent', 'claude-command', 'claude-plugin']) {
    const markup = renderToStaticMarkup(<DiscoverFormatMark format={format} />)
    assert.match(markup, /fill-current text-axon-orange/)
    assert.match(markup, /aria-hidden="true"/)
  }
  for (const format of ['agent-skill', 'unknown', 'not-claude-skill', 'github']) {
    assert.doesNotMatch(renderToStaticMarkup(<DiscoverFormatMark format={format} />), /text-axon-orange/)
  }
})

test('format marks are self-contained with no remote image dependency', () => {
  for (const format of ['claude-skill', 'mcp', 'unknown']) {
    const markup = renderToStaticMarkup(<DiscoverFormatMark format={format} />)
    assert.match(markup, /<svg/)
    assert.doesNotMatch(markup, /<img|href=|src=/)
  }
})
