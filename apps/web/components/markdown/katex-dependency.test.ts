import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
import { Window } from 'happy-dom'

// Resolve through the same Streamdown -> Mermaid edge as the product, so an
// unrelated direct dependency cannot make these security checks pass.
const streamdownRequire = createRequire(import.meta.resolve('streamdown'))
const mermaidRequire = createRequire(streamdownRequire.resolve('mermaid'))
const katex = mermaidRequire('katex') as {
  version: string
  renderToString: (expression: string, options?: object) => string
}

test('Mermaid resolves fixed KaTeX and ordinary mathematical labels remain valid', async () => {
  assert.equal(katex.version, '0.18.10')
  const html = katex.renderToString(String.raw`x^2 + \frac{1}{2}`)
  assert.match(html, /class="katex"/)
  assert.doesNotMatch(html, /katex-error/)
  const window = new Window()
  const previous = new Map(['window', 'document'].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]))
  try {
    Object.defineProperty(globalThis, 'window', { value: window, configurable: true })
    Object.defineProperty(globalThis, 'document', { value: window.document, configurable: true })
    const { default: mermaid } = await import(pathToFileURL(streamdownRequire.resolve('mermaid')).href)
    const parsed = await mermaid.parse('flowchart LR\n A["$$x^2 + 1$$"] --> B["Result"]')
    assert.equal(parsed.diagramType, 'flowchart-v2')
  } finally {
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
      else Reflect.deleteProperty(globalThis, key)
    }
    await window.happyDOM.close()
  }
})

test('an inherited trust option cannot enable unsafe KaTeX links', () => {
  const expression = String.raw`\href{javascript:alert(1)}{click}`
  const inherited = Object.create({ trust: true }) as object
  const untrusted = katex.renderToString(expression, inherited)
  assert.doesNotMatch(untrusted, /href="javascript:/i)
  // Positive control proves this expression reaches the trust-sensitive path.
  const explicitlyTrusted = katex.renderToString(expression, { trust: true })
  assert.match(explicitlyTrusted, /href="javascript:/i)
})
