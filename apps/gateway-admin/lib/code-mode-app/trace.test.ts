import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  describeMarkdown,
  describeResultShape,
  parseCodeModeTrace,
  parseDiscoveryResult,
  stringifyRedactedParams,
} from './trace'

test('parses execute traces with redacted params', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'github::search_issues',
        namespace: 'github',
        tool: 'search_issues',
        ok: true,
        elapsed_ms: 12,
        params: { query: 'bug', token: '[redacted]' },
      },
    ],
    result_shape: { type: 'object', key_count: 2 },
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  const calls = trace?.kind === 'code_mode_execute_trace' ? trace.calls : []
  assert.equal(calls.length, 1)
  assert.equal(calls[0].upstream, 'github')
  assert.equal(stringifyRedactedParams(calls[0].params).includes('[redacted]'), true)
})

test('parses per-call upstream MCP UI metadata', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'quick-shell::run_command',
        namespace: 'quick-shell',
        tool: 'run_command',
        ok: true,
        elapsed_ms: 18,
        ui: {
          resourceUri: 'ui://quick-shell/app.html',
          preferredSize: { height: 420 },
        },
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  const call = trace?.kind === 'code_mode_execute_trace' ? trace.calls[0] : undefined
  assert.equal(call?.ui?.resourceUri, 'ui://quick-shell/app.html')
  assert.deepEqual(call?.ui?.preferredSize, { height: 420 })
})

test('derives upstream and tool from the call id when fields are absent', () => {
  // History entries (CodeModeExecutedCall) carry only `id`, never
  // namespace/tool — the parser must split `upstream::tool` itself.
  const trace = parseCodeModeTrace({
    kind: 'code_mode_history',
    entries: [
      {
        seq: 3,
        kind: 'execute',
        ok: true,
        elapsed_ms: 24,
        calls: [{ id: 'github::search_issues', ok: true, elapsed_ms: 12 }],
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_history')
  const call = trace?.kind === 'code_mode_history' ? trace.entries[0].calls?.[0] : undefined
  assert.equal(call?.upstream, 'github')
  assert.equal(call?.tool, 'search_issues')
})

test('parses execute trace token and log metadata', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 0,
    calls: [],
    execution_id: 'exec-1',
    elapsed_ms: 348,
    input_tokens: 412,
    output_tokens: 96,
    logs_count: 2,
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  if (trace?.kind === 'code_mode_execute_trace') {
    assert.equal(trace.execution_id, 'exec-1')
    assert.equal(trace.elapsed_ms, 348)
    assert.equal(trace.input_tokens, 412)
    assert.equal(trace.output_tokens, 96)
    assert.equal(trace.logs_count, 2)
  }
})

test('parses failed-run traces with error kind and call start offsets', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    error_kind: 'timeout',
    elapsed_ms: 30012,
    calls: [
      {
        id: 'rustarr::qbittorrent.transfer_info',
        namespace: 'rustarr',
        tool: 'qbittorrent.transfer_info',
        ok: false,
        elapsed_ms: 1010,
        start_ms: 226,
        error_kind: 'upstream_timeout',
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  if (trace?.kind === 'code_mode_execute_trace') {
    assert.equal(trace.error_kind, 'timeout')
    assert.equal(trace.calls[0].start_ms, 226)
  }
})

test('parses artifact receipts', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 0,
    calls: [],
    artifacts: [
      { path: 'report.md', content_type: 'text/markdown', bytes: 2048, sha256: 'abc' },
      { not_a_receipt: true },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  if (trace?.kind === 'code_mode_execute_trace') {
    assert.equal(trace.artifacts?.length, 1)
    assert.equal(trace.artifacts?.[0].path, 'report.md')
    assert.equal(trace.artifacts?.[0].bytes, 2048)
  }
})

test('describes result shapes', () => {
  assert.equal(
    describeResultShape({ type: 'object', key_count: 2, keys: ['total', 'upstreams'] }),
    'object · 2 keys — keys: total, upstreams',
  )
  assert.equal(
    describeResultShape({ type: 'array', length: 3, item_types: ['string'] }),
    'array · 3 items — items: string',
  )
  assert.equal(describeResultShape(undefined), '')
})

test('parses history traces with nested execute calls', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_history',
    entries: [
      {
        seq: 7,
        kind: 'execute',
        ok: true,
        elapsed_ms: 24,
        input_tokens: 388,
        output_tokens: 44,
        calls: [
          {
            id: 'github::search_issues',
            ok: true,
            elapsed_ms: 12,
          },
        ],
      },
      {
        seq: 8,
        kind: 'search',
        ok: false,
        elapsed_ms: 4,
        error_kind: 'invalid_query',
        match_count: 0,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_history')
  if (trace?.kind === 'code_mode_history') {
    assert.equal(trace.entries.length, 2)
    assert.equal(trace.entries[0].calls?.[0].tool, 'search_issues')
    assert.equal(trace.entries[0].input_tokens, 388)
    assert.equal(trace.entries[1].error_kind, 'invalid_query')
  }
})

test('reports dropped malformed history rows', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_history',
    entries: [
      { seq: 1, kind: 'search', ok: true, elapsed_ms: 3 },
      { seq: 2, kind: 'unknown', ok: true, elapsed_ms: 3 },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_history')
  assert.equal(trace?.entries.length, 1)
  assert.deepEqual(trace?.warnings, [
    { kind: 'dropped_rows', message: 'Dropped 1 malformed history entry.' },
  ])
})

test('accepts only literal booleans for status fields', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'github::search_issues',
        namespace: 'github',
        tool: 'search_issues',
        ok: 'false',
        elapsed_ms: 12,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  const calls = trace?.kind === 'code_mode_execute_trace' ? trace.calls : []
  assert.equal(calls[0].ok, false)
})

test('stringifies unsupported params without throwing', () => {
  const cyclic: { child?: unknown } = {}
  cyclic.child = cyclic

  const params = stringifyRedactedParams(cyclic)

  assert.match(params, /^\[unsupported params:/)
  assert.ok(params.length < 160)
})

test('detects in-sandbox codemode.search results', () => {
  const discovery = parseDiscoveryResult({
    results: [
      {
        id: 'unifi::device.list',
        path: 'codemode.unifi.device_list',
        kind: 'tool',
        namespace: 'unifi',
        name: 'device_list',
        description: 'List UniFi devices.',
        score: 0.9,
      },
    ],
    total: 42,
    truncated: true,
  })

  assert.ok(discovery)
  assert.equal(discovery.hits.length, 1)
  assert.equal(discovery.hits[0].namespace, 'unifi')
  assert.equal(discovery.total, 42)
  assert.equal(discovery.truncated, true)

  const empty = parseDiscoveryResult({
    results: [],
    total: 0,
    truncated: false,
    hint: 'No matches. Broaden or try synonyms.',
  })
  assert.ok(empty)
  assert.equal(empty.hits.length, 0)
  assert.equal(empty.hint, 'No matches. Broaden or try synonyms.')

  // Non-search shapes stay null so ordinary results render as plain values.
  assert.equal(parseDiscoveryResult({ containers: 24 }), null)
  assert.equal(parseDiscoveryResult({ results: [{ score: 1 }], total: 1 }), null)
  assert.equal(parseDiscoveryResult([1, 2, 3]), null)
})

test('detects codemode.describe markdown docs', () => {
  assert.equal(
    describeMarkdown({ id: 'unifi::device.list', kind: 'tool', path: 'x', markdown: '# device_list' }),
    '# device_list',
  )
  assert.equal(describeMarkdown({ markdown: '# no id' }), null)
  assert.equal(describeMarkdown({ containers: 24 }), null)
})

test('rejects unknown trace shapes', () => {
  // Includes the never-emitted `code_mode_search_trace`: nothing server-side
  // produces it, so it falls through to the malformed-payload path.
  assert.equal(parseCodeModeTrace({ kind: 'tool_explorer' }), null)
  assert.equal(
    parseCodeModeTrace({ kind: 'code_mode_search_trace', match_count: 0, matches: [] }),
    null,
  )
})
