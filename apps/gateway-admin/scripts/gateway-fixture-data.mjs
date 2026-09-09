// Synthetic upstreams for the loopback visual preview only. Never probe these targets.
export function createGatewayFixtureRows() {
  return [
    ['fixture-corpus', 'Fixture Corpus', 42, true],
    ['fixture-labby', 'Fixture Labby', 18, true],
    ['fixture-cortex', 'Fixture Cortex', 24, false],
  ].map(([id, name, tools, connected]) => ({
    id, name, source: 'custom', configured: true, enabled: true, connected,
    discovered_tool_count: tools, exposed_tool_count: tools,
    discovered_resource_count: 0, exposed_resource_count: 0,
    discovered_prompt_count: 0, exposed_prompt_count: 0,
    discovered_skill_count: 0, exposed_skill_count: 0, supports_skills: false,
    surfaces: { mcp: { enabled: true, connected } },
    warnings: connected ? [] : [{ code: 'connection_failed', message: 'Synthetic preview: upstream unavailable' }],
    config_summary: { transport: 'http', target: `https://${id}.example.invalid/mcp` },
  }))
}

export function createGatewayFixtureRuntime() {
  return createGatewayFixtureRows().map(({ id, source, configured, surfaces, warnings, config_summary, ...runtime }) => ({
    ...runtime, transport: config_summary.transport, target: config_summary.target,
  }))
}
