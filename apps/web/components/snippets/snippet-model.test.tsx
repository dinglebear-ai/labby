import test from 'node:test'
import assert from 'node:assert/strict'

import type { SnippetInfo } from '@/lib/types/snippets'
import {
  buildSnippetParams,
  collectSnippetTags,
  filterSnippets,
  inputPlaceholder,
  parseSnippetBody,
  sortSnippetsByName,
  tokenizeSnippetSource,
} from './snippet-model'

const BODY = [
  '---',
  'name: homelab-readonly-pulse',
  'description: Read-only homelab pulse',
  'tags: [homelab, readonly, ops]',
  '---',
  '',
  '# Homelab Pulse',
  '',
  'Prose before the executable block.',
  '',
  '```js',
  '// comment',
  'async (input) => {',
  '  const a = await callTool("dozzle::list_containers", { host: input.host });',
  "  const b = await callTool('cortex::search_memory', {});",
  '  return { a, b };',
  '}',
  '```',
  '',
  'Trailing prose.',
].join('\n')

test('parseSnippetBody splits frontmatter, tutorial, source and callTool ids', () => {
  const parsed = parseSnippetBody(BODY)

  assert.equal(parsed.frontmatter?.startsWith('---'), true)
  assert.match(parsed.frontmatter ?? '', /tags: \[homelab, readonly, ops\]/)
  assert.equal(parsed.tutorial, '# Homelab Pulse\n\nProse before the executable block.')
  assert.match(parsed.source, /^---/)
  assert.match(parsed.source, /async \(input\) => \{/)
  assert.doesNotMatch(parsed.source, /Trailing prose/)
  assert.deepEqual(parsed.tools, ['dozzle::list_containers', 'cortex::search_memory'])
  assert.deepEqual(parsed.servers, ['dozzle', 'cortex'])
})

test('parseSnippetBody falls back to the whole body when there is no code fence', () => {
  const parsed = parseSnippetBody('# Just prose\n\nNo code here.')
  assert.equal(parsed.frontmatter, null)
  assert.equal(parsed.source, '# Just prose\n\nNo code here.')
  assert.deepEqual(parsed.tools, [])
})

test('parseSnippetBody tolerates empty input', () => {
  const parsed = parseSnippetBody(null)
  assert.deepEqual(parsed, {
    frontmatter: null,
    tutorial: null,
    source: '',
    tools: [],
    servers: [],
  })
})

test('tokenizeSnippetSource colours frontmatter, comments, strings and keywords', () => {
  const tokens = tokenizeSnippetSource(parseSnippetBody(BODY).source)
  const kinds = new Map<string, string[]>()
  for (const token of tokens) {
    if (!kinds.has(token.kind)) kinds.set(token.kind, [])
    kinds.get(token.kind)?.push(token.text)
  }

  assert.ok(kinds.get('meta')?.includes('---'))
  assert.ok(kinds.get('key')?.includes('name'))
  assert.ok(kinds.get('comment')?.some((text) => text.includes('// comment')))
  assert.ok(kinds.get('string')?.includes('"dozzle::list_containers"'))
  assert.ok(kinds.get('keyword')?.includes('async'))
  assert.ok(kinds.get('keyword')?.includes('await'))
  assert.ok(kinds.get('keyword')?.includes('return'))
  // `const` is deliberately not highlighted — the mock leaves it plain.
  assert.equal(kinds.get('keyword')?.includes('const'), false)

  // Round-trips: highlighting must never drop or duplicate source text.
  assert.equal(tokens.map((token) => token.text).join(''), parseSnippetBody(BODY).source)
})

const SNIPPETS: SnippetInfo[] = [
  { name: 'beta', description: 'Beta sweep', tags: ['ops', 'network'], source: 'user', path: 'b', shadowed: false },
  { name: 'alpha', description: 'Alpha pulse', tags: ['ops'], source: 'builtin', path: 'a', shadowed: false },
  { name: 'gamma', description: null, tags: [], source: 'builtin', path: 'g', shadowed: true },
]

test('collectSnippetTags orders by usage then name', () => {
  assert.deepEqual(collectSnippetTags(SNIPPETS), ['ops', 'network'])
})

test('filterSnippets matches name, description and tags, and respects the tag pill', () => {
  assert.deepEqual(filterSnippets(SNIPPETS, 'sweep', null).map((s) => s.name), ['beta'])
  assert.deepEqual(filterSnippets(SNIPPETS, 'network', null).map((s) => s.name), ['beta'])
  assert.deepEqual(filterSnippets(SNIPPETS, '', 'ops').map((s) => s.name), ['beta', 'alpha'])
  assert.deepEqual(filterSnippets(SNIPPETS, 'alpha', 'network').map((s) => s.name), [])
})

test('sortSnippetsByName sorts both directions without mutating', () => {
  const input = [...SNIPPETS]
  assert.deepEqual(sortSnippetsByName(input, 'asc').map((s) => s.name), ['alpha', 'beta', 'gamma'])
  assert.deepEqual(sortSnippetsByName(input, 'desc').map((s) => s.name), ['gamma', 'beta', 'alpha'])
  assert.deepEqual(input.map((s) => s.name), ['beta', 'alpha', 'gamma'])
})

test('inputPlaceholder renders the declared default', () => {
  assert.equal(inputPlaceholder({ ty: 'string', default: 'node-a' }), 'node-a')
  assert.equal(inputPlaceholder({ ty: 'array', default: ['a', 'b'] }), '["a","b"]')
  assert.equal(inputPlaceholder({ ty: 'string' }), '')
})

test('buildSnippetParams coerces typed inputs and skips blanks', () => {
  const inputs = {
    host: { ty: 'string' as const },
    limit: { ty: 'integer' as const },
    ratio: { ty: 'number' as const },
    verbose: { ty: 'boolean' as const },
    hosts: { ty: 'array' as const },
    extra: { ty: 'object' as const },
  }

  const ok = buildSnippetParams(inputs, {
    host: 'node-a',
    limit: '5',
    ratio: '1.5',
    verbose: 'yes',
    hosts: '["a"]',
    extra: '{"k":1}',
  })
  assert.equal(ok.ok, true)
  assert.deepEqual(ok.ok && ok.params, {
    host: 'node-a',
    limit: 5,
    ratio: 1.5,
    verbose: true,
    hosts: ['a'],
    extra: { k: 1 },
  })

  assert.deepEqual(buildSnippetParams(inputs, { host: '   ' }), { ok: true, params: {} })
  assert.deepEqual(buildSnippetParams(undefined, undefined), { ok: true, params: {} })

  const badInt = buildSnippetParams(inputs, { limit: '1.5' })
  assert.equal(badInt.ok, false)
  assert.match(!badInt.ok ? badInt.error : '', /must be an integer/)

  const badJson = buildSnippetParams(inputs, { hosts: '[' })
  assert.equal(badJson.ok, false)
  assert.match(!badJson.ok ? badJson.error : '', /must be valid JSON/)

  const notArray = buildSnippetParams(inputs, { hosts: '{"a":1}' })
  assert.equal(notArray.ok, false)
  assert.match(!notArray.ok ? notArray.error : '', /must be a JSON array/)

  const badBool = buildSnippetParams(inputs, { verbose: 'maybe' })
  assert.equal(badBool.ok, false)
})
