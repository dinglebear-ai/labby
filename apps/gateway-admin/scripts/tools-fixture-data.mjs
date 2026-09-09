// Read-only presentation fixtures. These declarations cannot be executed.
export const fixtureTools = [
  ['search', 'Search the synthetic document catalog.', true, false],
  ['inspect', 'Inspect metadata for a synthetic artifact.', true, false],
  ['publish', 'Example write-capable tool; execution is disabled in this preview.', false, false],
  ['remove', 'Example destructive tool; execution is disabled in this preview.', false, true],
].map(([name, description, read_only, destructive], index) => ({
  id: `fixture-catalog::${name}`, path: `fixture.catalog.${name}`, kind: 'tool', namespace: 'fixture.catalog', name,
  description, signature: `${name}(input: { query: string }): Promise<unknown>`, tags: ['fixture', 'catalog'], score: 1 - index / 10,
  safety: { read_only, destructive },
}))

export function describeFixtureTool(target) {
  const hit = fixtureTools.find(tool => tool.id === target)
  if (!hit) return null
  const { kind, score, ...description } = hit
  return { ...description, helper: hit.path, typescript: `/** ${hit.description} */\ndeclare function ${hit.name}(input: {\n  query: string;\n}): Promise<unknown>;` }
}
