import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
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


// Bundle the installed ESM consumer rather than Mermaid's prebundled distribution,
// so this exercises the KaTeX selected by the package override.
test('resolved Mermaid renders math and rejects active diagram content', { timeout: 60_000 }, async () => {
  const tsxRequire = createRequire(import.meta.resolve('tsx'))
  const { build } = tsxRequire('esbuild')
  const katexEntry = mermaidRequire.resolve('katex')
  const katexPackage = JSON.parse(readFileSync(join(dirname(katexEntry), '..', 'package.json'), 'utf8'))
  assert.equal(katexPackage.version, '0.18.10')
  const bundle = await build({
    entryPoints: [streamdownRequire.resolve('mermaid')],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'MermaidDependency',
    platform: 'browser',
    metafile: true,
    logLevel: 'silent',
  })
  assert.ok(Object.keys(bundle.metafile.inputs).some((path: string) => path.includes('katex') && path.endsWith('katex.mjs')))
  const browser = await chromium.launch({ headless: true })
  try {
    const page = await browser.newPage()
    await page.route('**/*', (route) => route.abort())
    await page.addScriptTag({ content: bundle.outputFiles[0].text })
    const result = await page.evaluate(async () => {
      const mermaid = (globalThis as typeof globalThis & {
        MermaidDependency: { default: {
          initialize(config: object): void
          render(id: string, diagram: string): Promise<{ svg: string }>
        } }
      }).MermaidDependency.default
      mermaid.initialize({ startOnLoad: false, securityLevel: 'strict', forceLegacyMathML: true })
      const safe = await mermaid.render('safe-math', String.raw`flowchart LR
        A["$$x^2 + y^2 = z^2$$"] --> B["Result"]`)
      document.body.innerHTML = safe.svg
      const mathRendered = document.querySelector('math') !== null
      const safeLabel = document.body.textContent?.includes('Result') ?? false
      const unsafe = await mermaid.render('unsafe-math', String.raw`flowchart LR
        A["<img src=x onerror=alert(1)>"] --> B["$$\href{javascript:alert(1)}{unsafe}$$"]`)
      document.body.innerHTML = unsafe.svg
      const activeAttributes = [...document.querySelectorAll('*')].flatMap((element) =>
        [...element.attributes].filter((attribute) =>
          /^on/i.test(attribute.name) ||
          (/^(?:href|xlink:href|src)$/i.test(attribute.name) && /^\s*(?:javascript|data):/i.test(attribute.value))
        ).map((attribute) => attribute.name)
      )
      return { mathRendered, safeLabel, activeAttributes, scripts: document.body.querySelectorAll('script').length }
    })
    assert.equal(result.mathRendered, true)
    assert.equal(result.safeLabel, true)
    assert.deepEqual(result.activeAttributes, [])
    assert.equal(result.scripts, 0)
  } finally {
    await browser.close()
  }
})
