// Synthetic presentation catalog. These names are deliberately not executable
// Depot operations; the fixture server rejects every attempted invocation.
export const administrationFixtureOperations = [
  ['catalog', 'Inspect artifact metadata', 'Browse a synthetic artifact descriptor.'],
  ['catalog', 'Inspect source inventory', 'Review synthetic source inventory details.'],
  ['access', 'Inspect access policy', 'Review a synthetic read-only access policy.'],
  ['access', 'Inspect token metadata', 'Display synthetic token metadata without credentials.'],
  ['operations', 'Inspect storage health', 'Review synthetic storage health observations.'],
  ['operations', 'Inspect job history', 'Browse synthetic completed-job metadata.'],
].map(([group, title, description], index) => ({
  name: `fixture.${group}.inspect_${index + 1}`,
  title,
  description: `${description} Fixture only; execution is disabled.`,
  group,
  inputSchema: { type: 'object', properties: {}, additionalProperties: false },
  annotations: { readOnlyHint: true, destructiveHint: false },
}))
