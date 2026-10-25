import test from 'node:test'
import assert from 'node:assert/strict'

import type { SnippetInfo } from '@/lib/types/snippets'
import {
  buildSnippetParams,
  collectSnippetTags,
  filterSnippets,
  inputPlaceholder,
  parseSnippetBody,
  withSnippetDescription,
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

test('nullable inputs accept explicit JSON null across declared types', () => {
  assert.deepEqual(buildSnippetParams({ host: { ty: 'string', nullable: true }, count: { ty: 'integer', nullable: true } }, { host: 'null', count: 'null' }), { ok: true, params: { host: null, count: null } })
  assert.deepEqual(buildSnippetParams({ host: { ty: 'string' } }, { host: 'null' }), { ok: true, params: { host: 'null' } })
})

test('typed inputs omit inherited prototype values while preserving explicitly supplied prototype names', () => {
  const inputs = { toString: { ty: 'string' as const }, constructor: { ty: 'string' as const }, host: { ty: 'string' as const } }
  assert.deepEqual(buildSnippetParams(inputs, { host: 'node' }), { ok: true, params: { host: 'node' } })
  assert.deepEqual(buildSnippetParams(inputs, { toString: 'explicit', constructor: 'own' }), { ok: true, params: { toString: 'explicit', constructor: 'own' } })
})


test('description updates preserve literal replacement characters and CRLF metadata bytes', () => {
  const body = '---\r\nname: example\r\ndescription: Original\r\ntools: []\r\n---\r\ncode'
  assert.equal(withSnippetDescription(body, 'Literal $& value'), body.replace('Original', () => 'Literal $& value'))
  const missing = '---\nname: example\ntools: []\n---\ncode'
  assert.equal(withSnippetDescription(missing, 'Literal $& value'), '---\ndescription: Literal $& value\nname: example\ntools: []\n---\ncode')
})

test('description edits update every parser-accepted top-level field without touching nested descriptions', () => {
  const declarations = [
    ['description : Original', 'description : Updated'],
    [' description: Original', ' description: Updated'],
    ['\tdescription:\tOriginal', '\tdescription: Updated'],
    ['\u0085description\u0085: Original', '\u0085description\u0085: Updated'],
    ['description: Earlier\ndescription: Original', 'description: Updated\ndescription: Updated'],
  ]
  for (const [original, updated] of declarations) {
    for (const newline of ['\n', '\r\n']) {
      const body = ['---', 'name: example', original, 'tags: [kept]', 'inputs:',
        '  count:', '    type: integer', '    description: Keep input description',
        '---', '', '```js', 'async () => ({ok:true})', '```'].join('\n').replaceAll('\n', newline)
      const expected = body.replace(original.replaceAll('\n', newline), updated.replaceAll('\n', newline))
      assert.equal(withSnippetDescription(body, 'Updated'), expected)
    }
  }
})

test('description edits stay responsive with long unrelated metadata keys', async () => {
  const { spawnSync } = await import('node:child_process')
  const { resolve } = await import('node:path')
  const { pathToFileURL } = await import('node:url')
  const moduleUrl = pathToFileURL(resolve('components/snippets/snippet-model.ts')).href
  // Run in a separate process so a synchronous regression cannot block the
  // test runner's own timeout. This header fits the native snippet size limit.
  const script = `
    const { withSnippetDescription } = await import(${JSON.stringify(moduleUrl)});
    const key = 'x' + ' '.repeat(500_000) + 'y';
    const body = ['---', 'name: example', 'description: Original', key + ': ignored',
      '---', '', '\x60\x60\x60js', 'async () => ({ok:true})', '\x60\x60\x60'].join('\\n');
    const updated = withSnippetDescription(body, 'Updated');
    if (!updated.includes('description: Updated') || !updated.includes(key + ': ignored'))
      throw new Error('Description edit changed unrelated metadata');
    console.log('complete');
  `
  const child = spawnSync(process.execPath,
    ['--experimental-strip-types', '--input-type=module', '--eval', script],
    { encoding: 'utf8', timeout: 10_000 })
  assert.ifError(child.error)
  assert.equal(child.status, 0, child.stderr)
  assert.equal(child.stdout.trim(), 'complete')
})

test('explicit __proto__ inputs remain own JSON keys without changing the returned plain object prototype', () => {
  for (const [ty, raw, expected] of [
    ['string', 'explicit value', 'explicit value'],
    ['object', '{"marker":"own object value"}', { marker: 'own object value' }],
    ['json', 'null', null],
  ] as const) {
    const inputs = { ['__proto__']: { ty, nullable: true } }
    const values = JSON.parse(JSON.stringify({ ['__proto__']: raw }))
    const result = buildSnippetParams(inputs, values)
    assert.equal(result.ok, true)
    if (!result.ok) throw new Error('Expected valid explicit input')
    assert.equal(Object.getPrototypeOf(result.params), Object.prototype)
    assert.equal(Object.prototype.hasOwnProperty.call(result.params, '__proto__'), true)
    assert.deepEqual(result.params['__proto__'], expected)
    assert.equal(JSON.stringify(result.params), JSON.stringify({ ['__proto__']: expected }))
  }
})
