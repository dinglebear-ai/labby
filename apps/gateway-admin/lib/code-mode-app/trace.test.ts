import assert from 'node:assert/strict'
import { test } from 'node:test'

import { describeResultShape, parseCodeModeTrace, stringifyRedactedParams } from './trace'

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
    input_tokens: 412,
    output_tokens: 96,
    logs_count: 2,
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  if (trace?.kind === 'code_mode_execute_trace') {
    assert.equal(trace.execution_id, 'exec-1')
    assert.equal(trace.input_tokens, 412)
    assert.equal(trace.output_tokens, 96)
    assert.equal(trace.logs_count, 2)
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

test('rejects unknown trace shapes', () => {
  // Includes the never-emitted `code_mode_search_trace`: nothing server-side
  // produces it, so it falls through to the malformed-payload path.
  assert.equal(parseCodeModeTrace({ kind: 'tool_explorer' }), null)
  assert.equal(
    parseCodeModeTrace({ kind: 'code_mode_search_trace', match_count: 0, matches: [] }),
    null,
  )
})
