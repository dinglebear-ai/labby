import test from 'node:test'
import assert from 'node:assert/strict'
import { externalClientCommand } from './external-clients'

test('client commands use supported selected values and no credentials', () => {
  assert.equal(externalClientCommand([], 'saved-cli'), undefined)
  assert.equal(externalClientCommand(['claude-code', 'codex', 'codex'], 'saved-cli'), 'labby setup clients connect --clients codex,claude-code')
  assert.equal(externalClientCommand(['codex'], 'oauth', 'https://lab.example/mcp'), "labby setup clients connect --clients codex --connection oauth --gateway-url 'https://lab.example/mcp'")
  for (const endpoint of ['http://localhost/mcp', 'http://127.0.0.1/mcp', 'http://[::1]/mcp', 'https://lab.example/base/mcp', "https://example.com/a'b", ' https://lab.example/mcp', 'https://lab.example/\\mcp', 'http://remote.example/mcp', 'https://user:secret@example.com/mcp', 'https://example.com/mcp?token=secret', 'https://example.com/mcp#fragment', 'bad', undefined]) {
    assert.equal(externalClientCommand(['codex'], 'oauth', endpoint), undefined)
  }
})
