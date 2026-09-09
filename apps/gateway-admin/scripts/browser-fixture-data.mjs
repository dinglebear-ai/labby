// Synthetic browser metadata only: no extension connection or document execution.
export function createBrowserFixture(now = Math.floor(Date.now() / 1000)) {
  const browsers = [
    { id: 'fixture-chrome', display_name: 'Fixture Chrome', extension_id: 'fixture-extension-chrome', paired_at: now - 86400, last_seen_at: now - 10, revoked_at: null, connected: true },
    { id: 'fixture-edge', display_name: 'Fixture Edge', extension_id: 'fixture-extension-edge', paired_at: now - 172800, last_seen_at: now - 3600, revoked_at: null, connected: false },
  ]
  const pairings = [{ id: 'fixture-pairing', display_name: 'Fixture workstation', extension_id: 'fixture-extension-pending', status: 'pending', expires_at: now + 300, browser_id: null }]
  const sessions = [{
    id: 'fixture-document-session', browser_id: 'fixture-chrome', tab_id: 7, document_id: 'fixture-document', origin: 'https://catalog.example.invalid', sanitized_path: '/library', page_title: 'Fixture artifact catalog',
    catalog_revision: 3, catalog_fingerprint: 'fixture-fingerprint', enabled: false, status: 'active', last_seen_at: now - 10,
    tools: ['search_artifacts', 'inspect_artifact'].map(name => ({ name, description: `Synthetic ${name.replaceAll('_', ' ')} metadata. Execution is disabled in this preview.`, input_schema: { type: 'object', properties: {} }, annotations: { readOnlyHint: true } })),
  }]
  return { browsers, pairings, sessions }
}
