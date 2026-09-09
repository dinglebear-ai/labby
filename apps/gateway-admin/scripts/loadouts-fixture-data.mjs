// Synthetic configuration for local visual QA. No endpoint is contacted.
export const loadoutFixtures = ['research', 'operations', 'portable'].map((name, index) => ({
  name: `fixture-${name}`,
  description: `Synthetic ${name} profile for visual QA; not a deployed Loadout.`,
  upstreams: index === 0 ? ['fixture-docs', 'fixture-search'] : ['fixture-observer'],
  services: index === 1 ? ['fixture-logs'] : [],
  expose_code_mode: true,
  expose_tools: true,
  expose_resources: false,
  expose_prompts: false,
  expose_skills: index === 0,
}))

export const loadoutRouteFixtures = [{
  name: 'fixture-research-route',
  enabled: true,
  public_host: 'fixture.example.invalid',
  public_path: '/mcp/research',
  scopes: ['fixture:read'],
  target: { kind: 'gateway_subset', loadout: 'fixture-research' },
}]
