import type { ArtifactSourceOrigin, DiscoveryPage, DepotProviderOption, FederatedArtifact } from './depot-client.ts'

const MOCK_NOW = Date.parse('2026-09-15T11:30:00Z')
export const mockDepotNow = MOCK_NOW

type Seed = {
  name: string
  kind: string
  publisher: string
  source: string
  description: string
  tags: string[]
  stars: string
  installs: string
  forks: number
  updated: string
  verified?: boolean
  mine?: boolean
  fork?: boolean
  behind?: number
  visibility?: 'Public' | 'Team' | 'Private'
}

const PROVIDERS = [
  ['mcp-registry', 'MCP Registry', 'mcp-registry'],
  ['acp-registry', 'ACP Registry', 'acp-registry'],
  ['skills-sh', 'skills.sh', 'skills-sh'],
  ['ard', 'ARD', 'ard'],
  ['github', 'GitHub', 'github'],
  ['claude', 'Claude', 'claude'],
  ['gemini', 'Gemini', 'gemini'],
  ['agent-plugins', 'Agent Plugins', 'agent-plugins'],
  ['web-crawl', 'Web Crawl', 'web-crawl'],
] as const

const SEEDS: Seed[] = [
  { name:'repo-triage', kind:'skill', publisher:'jmagar', source:'skills.sh', description:'Walks open PRs and issues, clusters them by subsystem, and drafts a triage note per cluster.', tags:['review','github'], stars:'2.4k', installs:'18k', forks:312, updated:'2d', verified:true, mine:true, visibility:'Public' },
  { name:'rust-reviewer', kind:'agent', publisher:'tootie.tv', source:'ARD', description:'Reviews Rust diffs against the workspace lint profile and flags unsafe blocks with rationale.', tags:['rust','review'], stars:'1.8k', installs:'9.2k', forks:204, updated:'5h', verified:true, mine:true, fork:true, behind:3, visibility:'Team' },
  { name:'corpus', kind:'mcp', publisher:'jmagar/corpus', source:'MCP Registry', description:'Self-hosted RAG control plane — crawl, scrape, ingest, embed, search and ask over any corpus.', tags:['rag','crawl'], stars:'5.1k', installs:'42k', forks:890, updated:'1d', verified:true, mine:true, visibility:'Public' },
  { name:'labby', kind:'mcp', publisher:'jmagar/labby', source:'MCP Registry', description:'Gateway control plane exposing every downstream MCP server behind one scoped endpoint.', tags:['gateway','mcp'], stars:'3.3k', installs:'27k', forks:511, updated:'6h', verified:true, mine:true, visibility:'Public' },
  { name:'unraid-ops', kind:'mcp', publisher:'community', source:'MCP Registry', description:'Array status, share management, docker lifecycle and notification hooks for unRAID hosts.', tags:['homelab','ops'], stars:'940', installs:'6.8k', forks:88, updated:'3d' },
  { name:'/ship', kind:'command', publisher:'jmagar', source:'Claude', description:'Runs the release checklist: changelog, version bump, tag, and a GitHub release draft.', tags:['release'], stars:'760', installs:'5.4k', forks:61, updated:'1w', mine:true, visibility:'Team' },
  { name:'/scope-audit', kind:'command', publisher:'tootie.tv', source:'Claude', description:'Audits a loadout for write-capable tools and prints the least-privilege delta.', tags:['security'], stars:'412', installs:'2.9k', forks:34, updated:'4d', mine:true, fork:true, behind:1, visibility:'Team' },
  { name:'pre-commit-guard', kind:'hook', publisher:'community', source:'Agent Plugins', description:'Blocks a commit when the agent touched files outside the declared scope.', tags:['safety','git'], stars:'1.1k', installs:'8.1k', forks:147, updated:'2d', verified:true },
  { name:'cost-ceiling', kind:'hook', publisher:'depot', source:'Agent Plugins', description:'Halts a session when projected token spend crosses the per-run budget.', tags:['safety','cost'], stars:'680', installs:'4.2k', forks:52, updated:'9h', mine:true, visibility:'Private' },
  { name:'incident-postmortem', kind:'prompt', publisher:'tootie.tv', source:'ARD', description:'Structures raw incident logs into a blameless postmortem with a timeline and action items.', tags:['ops','writing'], stars:'1.3k', installs:'7.7k', forks:96, updated:'1d', verified:true, mine:true, visibility:'Public' },
  { name:'schema-from-sample', kind:'prompt', publisher:'community', source:'skills.sh', description:'Infers a strict JSON schema from a handful of sample payloads, including nullability.', tags:['data'], stars:'520', installs:'3.1k', forks:28, updated:'5d' },
  { name:'zed-acp-bridge', kind:'acp', publisher:'zed-industries', source:'ACP Registry', description:'Agent Client Protocol bridge exposing editor context, diagnostics and edits to any agent.', tags:['editor','acp'], stars:'2.9k', installs:'15k', forks:233, updated:'12h', verified:true },
  { name:'acp-terminal', kind:'acp', publisher:'community', source:'ACP Registry', description:'Terminal session surface over ACP with scrollback capture and safe-command policy.', tags:['terminal','acp'], stars:'640', installs:'3.9k', forks:41, updated:'6d' },
  { name:'homelab-pack', kind:'plugin', publisher:'jmagar', source:'Claude', description:'Marketplace bundle: 6 skills, 3 commands and the unraid-ops server, wired for a single host.', tags:['homelab','bundle'], stars:'1.6k', installs:'11k', forks:178, updated:'3d', verified:true, mine:true, visibility:'Public' },
  { name:'review-suite', kind:'plugin', publisher:'depot', source:'Agent Plugins', description:'plugin.json bundling rust-reviewer, repo-triage and the pre-commit guard hook.', tags:['review','bundle'], stars:'890', installs:'5.9k', forks:74, updated:'1d', mine:true, visibility:'Team' },
  { name:'gemini-docs-ext', kind:'extension', publisher:'google', source:'Gemini', description:'Gemini extension exposing internal docs search with citation-grade retrieval.', tags:['docs','search'], stars:'2.1k', installs:'19k', forks:260, updated:'2d', verified:true },
  { name:'gemini-sql-ext', kind:'extension', publisher:'community', source:'Gemini', description:'Read-only warehouse access with row-level policy enforcement per requesting user.', tags:['data','sql'], stars:'730', installs:'4.6k', forks:55, updated:'1w' },
  { name:'project-a-loadout', kind:'loadout', publisher:'tootie.tv', source:'ARD', description:'Everything a project-A dev needs: 4 skills, 2 agents, 3 MCP servers and the release command.', tags:['project-a'], stars:'310', installs:'1.9k', forks:22, updated:'4h', mine:true, visibility:'Team' },
  { name:'oncall-loadout', kind:'loadout', publisher:'tootie.tv', source:'ARD', description:'Incident-response bundle — log search, postmortem prompt, paging hooks and the gateway server.', tags:['oncall'], stars:'284', installs:'1.6k', forks:19, updated:'2d', mine:true, fork:true, behind:2, visibility:'Team' },
  { name:'reconcile-loop', kind:'snippet', publisher:'jmagar', source:'GitHub', description:'Labby snippet that re-probes every disconnected server and reports the delta as a table.', tags:['gateway'], stars:'198', installs:'1.2k', forks:14, updated:'8h', mine:true, visibility:'Private' },
  { name:'fleet-digest', kind:'snippet', publisher:'jmagar', source:'GitHub', description:"Rolls 24h usage into a per-server digest and posts it to the announcements feed.", tags:['reporting'], stars:'164', installs:'980', forks:11, updated:'3d', mine:true, visibility:'Team' },
  { name:'playwright', kind:'mcp', publisher:'microsoft', source:'MCP Registry', description:'Browser automation surface — navigate, snapshot, click and extract with accessibility trees.', tags:['browser','testing'], stars:'6.2k', installs:'58k', forks:1240, updated:'4h', verified:true, mine:true, visibility:'Public' },
  { name:'doc-crawler', kind:'skill', publisher:'corpus', source:'Web Crawl', description:'Crawls a docs domain, normalizes headings and emits a citation-ready corpus for embedding.', tags:['crawl','docs'], stars:'1.4k', installs:'8.8k', forks:121, updated:'1d', verified:true },
  { name:'changelog-writer', kind:'skill', publisher:'community', source:'skills.sh', description:'Turns a commit range into a human changelog grouped by feature, fix and chore.', tags:['release','writing'], stars:'980', installs:'6.1k', forks:79, updated:'6d', mine:true, fork:true, behind:4, visibility:'Public' },
  { name:'secrets-sweeper', kind:'skill', publisher:'depot', source:'ARD', description:'Scans a working tree for committed credentials and proposes the .env migration.', tags:['security'], stars:'1.7k', installs:'12k', forks:190, updated:'11h', verified:true },
  { name:'deploy-warden', kind:'agent', publisher:'community', source:'GitHub', description:'Watches a rollout, compares replica health against the baseline and rolls back on drift.', tags:['deploy','ops'], stars:'1.2k', installs:'7.3k', forks:104, updated:'2d' },
]

const ORIGIN = new Map<string, { id: string; origin: ArtifactSourceOrigin }>(PROVIDERS.map(([id,name,origin]) => [name, { id, origin }]))
const metric = (value: string) => Math.round(Number.parseFloat(value) * (value.toLowerCase().includes('k') ? 1000 : 1))
const ageMs = (value: string) => { const match=/^(\d+)([hdw])$/.exec(value); if(!match)return 0; return Number(match[1])*({h:3600000,d:86400000,w:604800000} as const)[match[2] as 'h'|'d'|'w'] }
const visibility = (value?: Seed['visibility']) => (value ?? 'Private').toLowerCase()

function artifact(seed: Seed, index: number): FederatedArtifact {
  const source = ORIGIN.get(seed.source)!
  const id = seed.name
  const revision = 'rev-' + String(index + 1)
  const isSkill = seed.kind === 'skill'
  return {
    providerId: source.id,
    artifactId: id,
    id,
    kind: seed.kind,
    sourceOrigin: source.origin,
    namespace: seed.publisher,
    name: seed.name,
    title: seed.name,
    publisherVerified: seed.verified ?? false,
    metrics: { stars: metric(seed.stars), installs: metric(seed.installs), forks: seed.forks },
    upstreamBehind: seed.behind,
    updatedLabel: seed.updated,
    description: seed.description,
    currentRevisionId: revision,
    createdAt: new Date(MOCK_NOW - ageMs(seed.updated) - 7 * 86400000).toISOString(),
    updatedAt: new Date(MOCK_NOW - ageMs(seed.updated)).toISOString(),
    license: { declared: 'MIT', redistribution: 'allowed', reviewState: 'reviewed', takedownState: 'clear' },
    publication: { state: 'published', visibility: visibility(seed.visibility), distribution: 'allowed' },
    revisionCount: seed.fork ? 3 : 1,
    readme: { state: 'available', kind: isSkill ? 'skill' : 'readme', path: isSkill ? 'SKILL.md' : 'README.md', revisionId: revision, content: '# ' + seed.name + '\n\n' + seed.description + '\n\nTags: ' + seed.tags.join(', ') },
    provenance: { originalFormat: seed.source, originalVersion: null },
    lineage: { upstreamArtifactId: seed.fork ? seed.name.replace(/[^a-z0-9]+/g,'-') + '-upstream' : null, upstreamRevisionId: null, forkedFromArtifactId: seed.fork ? seed.name.replace(/[^a-z0-9]+/g,'-') + '-upstream' : null, forkedFromRevisionId: null, following: Boolean(seed.fork), lastObservedUpstreamRevisionId: null },
    descriptor: { id, kind: seed.kind, namespace: seed.publisher, name: seed.name, title: seed.name, description: seed.description, tags: seed.tags },
    currentRevision: { id: revision, authoredAt: new Date(MOCK_NOW - ageMs(seed.updated)).toISOString(), fileCount: isSkill ? 3 : seed.kind === 'mcp' ? 8 : 2 },
  }
}

export const mockDepotArtifacts: FederatedArtifact[] = SEEDS.map(artifact)
export const mockDepotLibraryArtifactIds = new Set(SEEDS.filter(seed => seed.mine).map(seed => seed.name))

const MOCK_SEED_BY_ID = new Map(SEEDS.map(seed => [seed.name, seed]))
export function mockDepotMetricLabels(artifact: FederatedArtifact) {
  const seed = MOCK_SEED_BY_ID.get(artifact.artifactId)
  return seed ? { stars: seed.stars, installs: seed.installs, forks: String(seed.forks) } : undefined
}

/** Reference-only type teaching chips. Never use these as production artifact evidence. */
export function mockDepotSpecLabels(artifact: FederatedArtifact): string[] {
  const name = artifact.name ?? artifact.title ?? artifact.artifactId
  const n = name.length
  const seed = (x: number) => 2 + ((n * 7 + x) % 7)
  switch ((artifact.kind ?? artifact.descriptor?.kind ?? 'skill').toLowerCase()) {
    case 'skill': return [seed(1) + ' files', 'SKILL.md', 'progressive']
    case 'agent': return [seed(3) + ' tools', 'sonnet-4.5', 'loop']
    case 'command': return [name, seed(0) % 3 ? '$ARGUMENTS' : 'no args', 'slash']
    case 'hook': return [seed(2) % 2 ? 'PreToolUse' : 'SessionEnd', 'blocking', 'exit 2']
    case 'prompt': return [seed(4) + ' variables', '~' + (seed(5) * 180) + ' tokens']
    case 'mcp': return [seed(6) + ' tools', seed(1) % 2 ? 'stdio' : 'http', 'oauth']
    case 'acp': return [seed(2) + ' surfaces', 'editor bridge']
    case 'plugin': return [seed(1) + ' skills', seed(3) + ' commands', 'plugin.json']
    case 'extension': return [seed(2) + ' tools', 'gemini-extension.json']
    case 'loadout': return [seed(4) + ' artifacts', 'pinned', 'apm@0.9']
    case 'snippet': return [seed(0) + ' lines', 'labby-native']
    default: return []
  }
}

export const mockDepotProviderOptions: DepotProviderOption[] = PROVIDERS.map(([id,name]) => ({
  id, name, enabled: true,
  health: { state: 'healthy', observedAt: MOCK_NOW, provenance: 'mock', retryNotBefore: null },
}))

export function mockListArtifacts(input: { provider?: string; query?: string; kind?: string } = {}): DiscoveryPage {
  const provider = input.provider ?? 'all'
  const query = (input.query ?? '').trim().toLowerCase()
  let items = mockDepotArtifacts.filter(item => provider === 'all' || item.providerId === provider)
  if (input.kind && input.kind !== 'all') items = items.filter(item => item.kind === input.kind)
  if (query) items = items.filter(item => [item.name, item.title, item.namespace, item.description, ...(item.descriptor?.tags ?? [])].filter(Boolean).join(' ').toLowerCase().includes(query))
  return {
    schemaVersion: 'labby.depot-compatibility/v2', scope: provider, scopeEpoch: 'mock-bazaar-v1', items,
    providerOutcomes: (provider === 'all' ? mockDepotProviderOptions : mockDepotProviderOptions.filter(item => item.id === provider)).map(item => ({ providerId: item.id, state: 'exhausted' as const })),
    failures: [], coverageComplete: true, knownTotal: items.length, totalIsExact: true,
    state: items.length ? 'complete' : 'empty', nextCursor: null,
  }
}

export function mockGetArtifact(providerId: string, artifactId: string): { schemaVersion: 'labby.depot-compatibility/v2'; providerId: string; artifactId: string; artifact: FederatedArtifact } {
  const found = mockDepotArtifacts.find(item => item.providerId === providerId && item.artifactId === artifactId)
  if (!found) throw new Error('Artifact not found')
  return { schemaVersion: 'labby.depot-compatibility/v2', providerId, artifactId, artifact: structuredClone(found) }
}
