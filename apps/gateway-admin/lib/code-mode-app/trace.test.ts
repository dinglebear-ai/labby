import assert from 'node:assert/strict'
import { test } from 'node:test'

import { flattenTraceRows, parseCodeModeTrace, stringifyRedactedParams } from './trace'

test('parses execute traces with redacted params', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_execute_trace',
    call_count: 1,
    calls: [
      {
        id: 'github::search_issues',
        upstream: 'github',
        tool: 'search_issues',
        ok: true,
        elapsed_ms: 12,
        params: { query: 'bug', token: '[redacted]' },
      },
    ],
    result_shape: { type: 'object', key_count: 2 },
  })

  assert.equal(trace?.kind, 'code_mode_execute_trace')
  const rows = flattenTraceRows(trace)
  assert.equal(rows.calls.length, 1)
  assert.equal(stringifyRedactedParams(rows.calls[0].params).includes('[redacted]'), true)
})

test('parses search traces with matched tools', () => {
  const trace = parseCodeModeTrace({
    kind: 'code_mode_search_trace',
    query_kind: 'catalog_filter',
    match_count: 1,
    matches: [
      {
        id: 'axon::ask',
        upstream: 'axon',
        tool: 'ask',
        description: 'Ask indexed docs',
        has_schema: true,
        has_output_schema: false,
      },
    ],
  })

  assert.equal(trace?.kind, 'code_mode_search_trace')
  assert.equal(flattenTraceRows(trace).matches[0].tool, 'ask')
})

test('rejects unknown trace shapes', () => {
  assert.equal(parseCodeModeTrace({ kind: 'tool_explorer' }), null)
})
