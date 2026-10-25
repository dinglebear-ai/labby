import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import test from 'node:test'
import { chromium } from 'playwright'

const streamdownRequire = createRequire(import.meta.resolve('streamdown'))
const mermaidRequire = createRequire(streamdownRequire.resolve('mermaid'))
const sanitizerPath = mermaidRequire.resolve('dompurify').replace('purify.cjs.js', 'purify.js')

test('Mermaid sanitizer preserves safe markup and removes active content', { timeout: 30_000 }, async () => {
  const browser = await chromium.launch({ headless: true })
  try {
    const page = await browser.newPage()
    await page.addScriptTag({ path: sanitizerPath })
    const safe = await page.evaluate(`DOMPurify.sanitize('<strong>Diagram</strong><a href="https://example.org/">Source</a>')`)
    assert.match(String(safe), /<strong>Diagram<\/strong>/)
    assert.match(String(safe), /href="https:\/\/example.org\/"/)
    const unsafe = await page.evaluate(`DOMPurify.sanitize('<script>alert(1)</script><img src="x" onerror="alert(1)"><a href="javascript:alert(1)">link</a>')`)
    assert.doesNotMatch(String(unsafe), /<script|onerror|javascript:/i)
    assert.match(String(unsafe), /link/)
  } finally {
    await browser.close()
  }
})
