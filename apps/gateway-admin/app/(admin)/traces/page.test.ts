import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('./page.tsx', import.meta.url), 'utf8')

test('traces reference hero describes its actual bounded collection contract', () => {
  assert.match(source, /description="Correlated request flows reconstructed from the retained server log window\. Requires the lab:admin scope\."/)
  assert.match(source, /label: `correlated_only · \$\{TRACE_QUERY_LIMIT\} line window`/)
  assert.match(source, /limit: TRACE_QUERY_LIMIT/)
  assert.match(source, /stop_after_limit: true,\s+correlated_only: true/)
  assert.match(source, /aria-label="Search request traces"/)
})

test('traces preserves keyed URL handoff and local filtering without changing query authority', () => {
  assert.match(source, /<Suspense fallback=\{null\}><TracesRoute \/><\/Suspense>/)
  assert.match(source, /const initialSearch = params\.get\('search'\) \?\? ''/)
  assert.match(source, /<TracesExplorer key=\{initialSearch\} initialSearch=\{initialSearch\} \/>/)
  assert.match(source, /useState\(initialSearch\)/)
  assert.match(source, /trace\.upstreams\.join\(' '\)/)
  assert.match(source, /refreshInterval: 30_000, revalidateOnFocus: false/)
})

test('trace rows retain native expansion, keyboard focus, and compact responsive structure', () => {
  assert.match(source, /<details\s+key=\{trace\.id\}/)
  assert.match(source, /<summary className="grid min-h-\[52px\]/)
  assert.match(source, /focus-visible:ring-inset/)
  assert.match(source, /sm:grid-cols-\[88px_minmax\(0,1fr\)_70px_64px_20px\]/)
  assert.match(source, /trace\.events\.map\(\(event, index\)/)
  assert.match(source, /event\.fields\[key\] === undefined \? null/)
})
