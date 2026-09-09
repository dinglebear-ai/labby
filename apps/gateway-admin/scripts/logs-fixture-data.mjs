// Deterministic synthetic logs for visual QA; never reads host log files.
export function createLogsFixture(params = {}) {
  const rows = [
    ['INFO', 'fixture.gateway', 'gateway.list', 'finish', 'Synthetic server inventory loaded.', null],
    ['DEBUG', 'fixture.catalog', 'catalog.search', 'start', 'Synthetic catalog search started.', null],
    ['INFO', 'fixture.catalog', 'catalog.search', 'finish', 'Synthetic catalog search returned four tools.', null],
    ['WARN', 'fixture.browser', 'browser.list', 'finish', 'Synthetic browser is offline; metadata retained.', 'offline'],
    ['ERROR', 'fixture.upstream', 'tool.call', 'error', 'Synthetic upstream request timed out.', 'timeout'],
    ['INFO', 'fixture.gateway', 'gateway.health', 'finish', 'Synthetic gateway is ready for read-only inspection.', null],
  ].map(([level, service, action, event, message, kind], index) => ({
    timestamp: new Date(Date.UTC(2026, 8, 9, 12, 0, index * 8)).toISOString(), level, service, action, kind,
    target: 'fixture::observability', message, file: 'fixture.jsonl',
    fields: { request_id: index === 1 || index === 2 ? 'fixture-request-search' : `fixture-request-${index}`, event, surface: 'api', service, action, ...(index === 1 || index === 2 || index === 4 ? { upstream: 'Fixture Corpus', operation: 'tool.call' } : {}), ...(event === 'start' ? {} : { elapsed_ms: index === 4 ? 5000 : 24 }), ...(kind ? { kind } : {}) },
  }))
  const matched = rows.filter(row => (!params.level || row.level === params.level) && (!params.service || row.service === params.service) && (!params.action || row.action === params.action) && (!params.kind || row.kind === params.kind) && (!params.query || JSON.stringify(row).toLowerCase().includes(params.query.toLowerCase())))
  const entries = [...matched].sort((left, right) => right.timestamp.localeCompare(left.timestamp)).slice(0, params.limit ?? 250)
  return { kind: 'server_logs', entries, matched: matched.length, scanned_lines: rows.length, malformed_lines: 0, scanned_bytes: 4096, max_scan_bytes: params.max_scan_bytes ?? 8388608, truncated: entries.length < matched.length }
}
