// Local visual QA only. These bodies contain no tool calls and are never executed.
export const snippetFixtures = [
  ['inventory-summary', 'Summarize a synthetic inventory.', ['inventory', 'ops']],
  ['gateway-review', 'Review a synthetic gateway report.', ['gateway', 'ops']],
  ['catalog-audit', 'Inspect example catalog metadata.', ['catalog']],
  ['release-notes', 'Format an illustrative release summary.', ['release']],
  ['log-digest', 'Group sample log messages for review.', ['logs', 'ops']],
  ['source-check', 'Review synthetic source descriptors.', ['catalog']],
  ['health-report', 'Format example health observations.', ['ops']],
  ['workflow-template', 'A reusable example with typed inputs.', ['template']],
].map(([name, description, tags], index) => ({
  name: `fixture-${name}`,
  description: `${description} Fixture only.`,
  tags,
  inputs: { scope: { ty: 'string', required: false, default: 'example', description: 'Illustrative scope; execution is disabled.' } },
  source: index < 4 ? 'builtin' : 'user',
  path: `/fixture/snippets/${name}.md`,
  shadowed: false,
  body: `# ${name}\n\n${description}\n\nThis is synthetic visual QA data, not an installed workflow.\n\n\`\`\`js\nasync () => ({ fixture: true, scope: "example" })\n\`\`\`\n`,
}))
