import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { SafeMarkdown } from './safe-markdown'

test('rewrites markdown links while preserving the shared URL safety gate', () => {
  const rewritten = renderToStaticMarkup(
    <SafeMarkdown
      text="[Next](./NEXT.md)"
      transformUrl={(url) => (url === './NEXT.md' ? '/docs?doc=NEXT.md' : url)}
    />,
  )

  assert.match(rewritten, /href="\/docs\?doc=NEXT\.md"/)

  const blocked = renderToStaticMarkup(
    <SafeMarkdown
      text="[Unsafe](./NEXT.md)"
      transformUrl={() => 'javascript:alert(1)'}
    />,
  )

  assert.doesNotMatch(blocked, /javascript:/i)
})

test('drops raw HTML, images, and unsafe URL schemes', () => {
  const rendered = renderToStaticMarkup(
    <SafeMarkdown
      text={[
        '<script>alert(1)</script>',
        '<img src="https://example.com/raw.png" onerror="alert(1)">',
        '![Markdown image](https://example.com/image.png)',
        '[Data](data:text/html,boom)',
        '[File](file:///tmp/secret)',
        '[Protocol relative](//evil.example/path)',
        '[HTTPS](https://example.com/docs)',
        '[Mail](mailto:docs@example.com)',
      ].join('\n\n')}
    />,
  )

  assert.doesNotMatch(rendered, /<script|<img|onerror="/i)
  assert.match(rendered, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/)
  assert.doesNotMatch(rendered, /data:text|file:\/\/|href="\/\/evil\.example/i)
  assert.match(rendered, /href="https:\/\/example\.com\/docs"/)
  assert.match(rendered, /href="mailto:docs@example.com"/)
})
