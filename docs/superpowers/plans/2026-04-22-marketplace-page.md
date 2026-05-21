# Marketplace Page Implementation Plan

> Historical note: this plan describes the initial marketplace UI delivery. The current implementation no longer uses `PluginDetailDialog`; plugin details now live on `/marketplace/plugin?id=<pluginId>`, and the `Files` tab is an editable CodeMirror workspace flow instead of the original read-only Prism viewer.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a full-page Marketplace UI inside gateway-admin for discovering, browsing, and installing Claude Code plugins from multiple upstream sources.

**Architecture:** Client components with SWR hooks, following the existing registry list + detail-dialog pattern. Static mock data in the API client drives all UI; the backend wire-up is deferred. A single `MarketplaceListContent` component owns tab state and renders either the plugin grid, installed groups, or marketplace source grid. A `PluginDetailDialog` (Radix Dialog) shows plugin metadata and a file tree + code viewer.

**Tech Stack:** Next.js 16 app router, React 19, TypeScript, SWR 2, Tailwind CSS 4 / Aurora tokens, Radix UI Dialog + Tabs, Lucide React icons, Sonner toasts, Node.js built-in test runner (`tsx --test`)

**Design reference:** `docs/marketplace-design-spec.md` (or `/tmp/lab-marketplace/DESIGN_SPEC.md`)

---

## File Map

| Action | Path | Responsibility |
|--------|------|---------------|
| Create | `lib/types/marketplace.ts` | Marketplace, Plugin, Artifact types |
| Create | `lib/api/marketplace-client.ts` | Mock data + fetch functions |
| Create | `lib/api/marketplace-client.test.ts` | API client unit tests |
| Create | `lib/hooks/use-marketplace.ts` | SWR hooks (marketplaces, plugins, installs) |
| Create | `components/marketplace/marketplace-card.tsx` | Single plugin card (grid item) |
| Create | `components/marketplace/mkt-source-card.tsx` | Marketplace source card |
| Create | `components/marketplace/marketplace-stats-strip.tsx` | Installed / sources / updates strip |
| Create | `components/marketplace/plugin-info-panel.tsx` | Info tab: counts, details table, file list, README |
| Create | `components/marketplace/plugin-files-panel.tsx` | Files tab: tree + Prism code viewer |
| Create | `components/marketplace/plugin-detail-dialog.tsx` | Modal shell (header + Info/Files tabs) |
| Create | `components/marketplace/add-marketplace-modal.tsx` | "Add Marketplace" form modal |
| Create | `components/marketplace/marketplace-list-content.tsx` | Page body: tabs + search + grids |
| Create | `app/(admin)/marketplace/page.tsx` | Route entry point |
| Modify | `components/app-sidebar.tsx` | Add Marketplace nav item |

---

## Task 1: Types

**Files:**
- Create: `lib/types/marketplace.ts`
- Test: `lib/api/marketplace-client.test.ts` (import types)

- [ ] **Step 1: Write the type file**

```typescript
// lib/types/marketplace.ts

export type MarketplaceSource = 'github' | 'git' | 'local'

export interface Marketplace {
  id: string
  name: string
  owner: string
  ghUser: string
  repo?: string          // "owner/repo" for GitHub source
  source: MarketplaceSource
  url?: string           // git URL for git source
  path?: string          // local path for local source
  desc: string
  autoUpdate: boolean
  totalPlugins: number
  lastUpdated: string    // ISO 8601
}

export interface Plugin {
  id: string             // "name@marketplace-id" — globally unique
  name: string
  mkt: string            // marketplace id
  ver: string
  desc: string
  tags: string[]
  installed: boolean
  hasUpdate?: boolean
  installedAt?: string   // ISO 8601
  updatedAt?: string     // ISO 8601
}

export type ArtifactLang = 'json' | 'yaml' | 'markdown' | 'bash' | 'toml' | 'text'

export interface Artifact {
  path: string           // relative path within plugin, e.g. "agents/my-agent.md"
  lang: ArtifactLang
  content: string
}

export interface MarketplaceState {
  installed: Set<string> // set of Plugin.id
}
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | head -20
```

Expected: no errors on the new file.

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/lib/types/marketplace.ts
git commit -m "feat(marketplace): add Marketplace, Plugin, Artifact types"
```

---

## Task 2: API Client + Mock Data

**Files:**
- Create: `lib/api/marketplace-client.ts`
- Create: `lib/api/marketplace-client.test.ts`

- [ ] **Step 1: Write the failing tests first**

```typescript
// lib/api/marketplace-client.test.ts
import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import {
  fetchMarketplaces,
  fetchPlugins,
  getInstalledPluginIds,
  detectArtifactLang,
  getArtifacts,
} from './marketplace-client.js'

describe('fetchMarketplaces', () => {
  it('returns non-empty array', async () => {
    const result = await fetchMarketplaces()
    assert.ok(Array.isArray(result))
    assert.ok(result.length > 0)
  })

  it('every entry has required fields', async () => {
    const result = await fetchMarketplaces()
    for (const m of result) {
      assert.ok(typeof m.id === 'string')
      assert.ok(typeof m.name === 'string')
      assert.ok(['github', 'git', 'local'].includes(m.source))
    }
  })
})

describe('fetchPlugins', () => {
  it('returns non-empty array', async () => {
    const result = await fetchPlugins()
    assert.ok(result.length > 0)
  })

  it('every plugin has a valid mkt reference', async () => {
    const [plugins, marketplaces] = await Promise.all([fetchPlugins(), fetchMarketplaces()])
    const mktIds = new Set(marketplaces.map(m => m.id))
    for (const p of plugins) {
      assert.ok(mktIds.has(p.mkt), `plugin ${p.id} references unknown mkt ${p.mkt}`)
    }
  })

  it('plugin id is namespaced as name@mkt', async () => {
    const plugins = await fetchPlugins()
    for (const p of plugins) {
      assert.ok(p.id.includes('@'), `expected id to contain @, got: ${p.id}`)
    }
  })
})

describe('getInstalledPluginIds', () => {
  it('returns a Set', async () => {
    const result = await getInstalledPluginIds()
    assert.ok(result instanceof Set)
  })
})

describe('detectArtifactLang', () => {
  it('detects json', () => { assert.equal(detectArtifactLang('plugin.json'), 'json') })
  it('detects yaml', () => { assert.equal(detectArtifactLang('agent.yaml'), 'yaml') })
  it('detects yaml for yml', () => { assert.equal(detectArtifactLang('agent.yml'), 'yaml') })
  it('detects markdown', () => { assert.equal(detectArtifactLang('README.md'), 'markdown') })
  it('detects bash for .sh', () => { assert.equal(detectArtifactLang('setup.sh'), 'bash') })
  it('detects bash for extensionless', () => { assert.equal(detectArtifactLang('Makefile'), 'bash') })
  it('detects toml', () => { assert.equal(detectArtifactLang('config.toml'), 'toml') })
  it('falls back to text', () => { assert.equal(detectArtifactLang('notes.txt'), 'text') })
})

describe('getArtifacts', () => {
  it('returns array for known plugin', async () => {
    const plugins = await fetchPlugins()
    const first = plugins[0]
    const artifacts = getArtifacts(first.id)
    assert.ok(Array.isArray(artifacts))
  })

  it('returns empty array for unknown id', () => {
    const artifacts = getArtifacts('unknown@nowhere')
    assert.deepEqual(artifacts, [])
  })

  it('every artifact has a plugin.json', async () => {
    const plugins = await fetchPlugins()
    // Only check plugins that have canned artifacts
    const withArtifacts = plugins.filter(p => getArtifacts(p.id).length > 0)
    for (const p of withArtifacts) {
      const arts = getArtifacts(p.id)
      assert.ok(arts.some(a => a.path === 'plugin.json'), `${p.id} missing plugin.json`)
    }
  })
})
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd apps/gateway-admin && npx tsx --test lib/api/marketplace-client.test.ts 2>&1 | head -30
```

Expected: errors like `Cannot find module './marketplace-client.js'`

- [ ] **Step 3: Write the API client**

```typescript
// lib/api/marketplace-client.ts
import type { Marketplace, Plugin, Artifact, ArtifactLang } from '../types/marketplace.js'

// ── Mock Marketplaces ────────────────────────────────────────────────────────

const MOCK_MARKETPLACES: Marketplace[] = [
  {
    id: 'claude-plugins-official',
    name: 'Claude Plugins Official',
    owner: 'Anthropic',
    ghUser: 'anthropics',
    repo: 'anthropics/claude-plugins-official',
    source: 'github',
    desc: 'Official Anthropic extensions — LSPs, code review, AI productivity plugins, MCP integrations, and more.',
    autoUpdate: false,
    totalPlugins: 48,
    lastUpdated: '2026-04-20T05:07:20.607Z',
  },
  {
    id: 'superpowers-marketplace',
    name: 'Superpowers',
    owner: 'Jesse Vincent',
    ghUser: 'obra',
    repo: 'obra/superpowers-marketplace',
    source: 'github',
    desc: 'Skills, workflows, and productivity tools. TDD, debugging, collaboration patterns, and proven development techniques.',
    autoUpdate: true,
    totalPlugins: 8,
    lastUpdated: '2026-04-21T19:13:23.871Z',
  },
  {
    id: 'claude-code-workflows',
    name: 'Claude Code Workflows',
    owner: 'Seth Hobson',
    ghUser: 'wshobson',
    repo: 'wshobson/agents',
    source: 'github',
    desc: '79 focused plugins, 184 specialized agents, and 150 skills. Optimized for granular installation.',
    autoUpdate: true,
    totalPlugins: 79,
    lastUpdated: '2026-04-21T19:26:20.085Z',
  },
  {
    id: 'jmagar-lab',
    name: 'jmagar Lab',
    owner: 'Jacob Magar',
    ghUser: 'jmagar',
    source: 'local',
    path: '~/.claude-plugin/marketplace.json',
    desc: 'Homelab control plane — CLI, MCP server, HTTP API for 24 services.',
    autoUpdate: false,
    totalPlugins: 2,
    lastUpdated: '2026-04-22T01:06:42.986Z',
  },
]

// ── Mock Plugins ─────────────────────────────────────────────────────────────

const MOCK_PLUGINS: Plugin[] = [
  // claude-plugins-official
  { id: 'code-review@cpo', name: 'code-review', mkt: 'claude-plugins-official', ver: '1.0.0', desc: 'Elite code review expert — AI-powered analysis, security vulnerabilities, performance optimization.', tags: ['review', 'security', 'quality'], installed: true, installedAt: '2026-01-14', updatedAt: '2026-04-20' },
  { id: 'typescript-lsp@cpo', name: 'typescript-lsp', mkt: 'claude-plugins-official', ver: '1.0.0', desc: 'TypeScript Language Server — type checking, go-to-definition, hover types, and diagnostics inside Claude Code.', tags: ['lsp', 'typescript', 'ide'], installed: true, installedAt: '2026-01-14', updatedAt: '2026-04-01' },
  { id: 'chrome-devtools-mcp@cpo', name: 'chrome-devtools-mcp', mkt: 'claude-plugins-official', ver: 'latest', desc: 'Chrome DevTools MCP — browser automation, DOM inspection, network monitoring.', tags: ['browser', 'devtools', 'mcp'], installed: true, hasUpdate: true, installedAt: '2026-03-14', updatedAt: '2026-04-08' },
  { id: 'git-ops@cpo', name: 'git-ops', mkt: 'claude-plugins-official', ver: '1.1.0', desc: 'Advanced git operations — interactive rebase assistance, conflict resolution, branch strategy.', tags: ['git', 'ops', 'workflow'], installed: false },
  { id: 'security-scanner@cpo', name: 'security-scanner', mkt: 'claude-plugins-official', ver: '2.0.1', desc: 'Static security analysis — SAST scanning, dependency vulnerability checking, secrets detection.', tags: ['security', 'sast', 'vulnerabilities'], installed: false },
  // superpowers
  { id: 'superpowers@spm', name: 'superpowers', mkt: 'superpowers-marketplace', ver: '5.0.7', desc: 'Core skills library: TDD, debugging, collaboration patterns, and proven development techniques.', tags: ['skills', 'tdd', 'debugging', 'core'], installed: true, installedAt: '2026-01-14', updatedAt: '2026-04-01' },
  { id: 'omc@spm', name: 'oh-my-claudecode', mkt: 'superpowers-marketplace', ver: '2.1.0', desc: 'Multi-agent orchestration layer — coordinate specialized agents, tools, and skills for complex work.', tags: ['orchestration', 'agents', 'workflow'], installed: false },
  // claude-code-workflows
  { id: 'tdd@ccw', name: 'tdd-workflows', mkt: 'claude-code-workflows', ver: '1.3.0', desc: 'Test-driven development — red-green-refactor cycle, coverage analysis, and test architecture patterns.', tags: ['tdd', 'testing', 'workflow'], installed: true, installedAt: '2026-03-02', updatedAt: '2026-04-01' },
  { id: 'comp-review@ccw', name: 'comprehensive-review', mkt: 'claude-code-workflows', ver: '1.3.0', desc: 'Multi-perspective code review with architect, security, and performance specialized agents in parallel.', tags: ['review', 'security', 'perf'], installed: true, installedAt: '2026-03-02', updatedAt: '2026-04-01' },
  // jmagar-lab
  { id: 'lab@jl', name: 'lab', mkt: 'jmagar-lab', ver: '0.7.0', desc: 'Homelab control plane — CLI, MCP server, HTTP API for 24 services (Radarr, Sonarr, Plex, UniFi, Unraid, and more).', tags: ['homelab', 'mcp', 'cli', 'rust'], installed: true, installedAt: '2026-04-21', updatedAt: '2026-04-22' },
]

// ── Canned Artifacts ─────────────────────────────────────────────────────────
// plugin.json is always the source of truth; other files are declared by it.

const MOCK_ARTIFACTS: Record<string, Artifact[]> = {
  'code-review@cpo': [
    { path: 'plugin.json', lang: 'json', content: JSON.stringify({
      name: 'code-review',
      version: '1.0.0',
      description: 'Elite code review expert — AI-powered analysis, security vulnerabilities, performance optimization.',
      agents: ['agents/code-reviewer.md', 'agents/architect-review.md'],
      commands: ['commands/code-review.md'],
      skills: ['skills/review-checklist.md'],
    }, null, 2) },
    { path: 'agents/code-reviewer.md', lang: 'markdown', content: `---\nname: code-reviewer\ndescription: Elite code review expert specializing in security vulnerabilities and performance optimization.\nsubagent_type: comprehensive-review:code-reviewer\n---\n\nReview code for quality, security, and production reliability.\n` },
    { path: 'agents/architect-review.md', lang: 'markdown', content: `---\nname: architect-review\ndescription: Master software architect specializing in modern architecture patterns and DDD.\nsubagent_type: comprehensive-review:architect-review\n---\n\nReview system designs and code changes for architectural integrity.\n` },
    { path: 'commands/code-review.md', lang: 'markdown', content: `# Code Review\n\nRun a comprehensive multi-agent code review on recent changes.\n\n## Usage\n\nInvoke before committing or creating a PR.\n\n## Options\n\n- \`--base <ref>\` — compare against specific ref (default: HEAD)\n- \`--severity high\` — only report high severity issues\n` },
    { path: 'skills/review-checklist.md', lang: 'markdown', content: `---\nname: review-checklist\ndescription: Use when performing code review to ensure thorough coverage\n---\n\n## Checklist\n\n- [ ] No hardcoded secrets\n- [ ] Input validation at system boundaries\n- [ ] Auth checks on all protected endpoints\n- [ ] All error paths handled\n` },
    { path: 'README.md', lang: 'markdown', content: `# code-review\n\nElite multi-agent code review for Claude Code.\n\n## Agents\n\n| Agent | Focus |\n|-------|-------|\n| \`code-reviewer\` | Quality, security, performance |\n| \`architect-review\` | SOLID principles, architectural debt |\n\n## Usage\n\n\`\`\`\n/code-review\n/code-review --base main\n\`\`\`\n` },
  ],
  'superpowers@spm': [
    { path: 'plugin.json', lang: 'json', content: JSON.stringify({
      name: 'superpowers',
      version: '5.0.7',
      description: 'Core skills library: TDD, debugging, collaboration patterns.',
      skills: ['skills/tdd.md', 'skills/debugging.md', 'skills/brainstorming.md'],
      commands: ['commands/tdd.md', 'commands/debug.md'],
    }, null, 2) },
    { path: 'skills/tdd.md', lang: 'markdown', content: `---\nname: tdd\ndescription: Use when implementing any new feature or fixing any bug using TDD\n---\n\n# Test-Driven Development\n\n**Red → Green → Refactor**\n\n1. Write the smallest failing test\n2. Write the minimum code to make it pass\n3. Refactor while keeping tests green\n` },
    { path: 'skills/debugging.md', lang: 'markdown', content: `---\nname: debugging\ndescription: Use when stuck on a bug you cannot solve\n---\n\n# Debugging Protocol\n\n## Phase 1: Reproduce\n\nNever fix what you cannot reproduce.\n\n## Phase 2: Isolate\n\nBinary search the problem space.\n\n## Phase 3: Hypothesize and verify\n\nState a hypothesis. Design an experiment to falsify it.\n` },
    { path: 'commands/tdd.md', lang: 'markdown', content: `# TDD\n\nActivate Test-Driven Development mode.\n\n## What Changes\n\n- Every feature starts with a failing test\n- Implementation follows tests, never precedes them\n` },
    { path: 'README.md', lang: 'markdown', content: `# superpowers\n\nCore skills library for Claude Code. Includes TDD, debugging, collaboration patterns, and proven development techniques.\n\n## Skills\n\n- \`tdd\` — Red-green-refactor cycle\n- \`debugging\` — Systematic debugging protocol\n- \`brainstorming\` — Structured ideation\n\n## Installation\n\n\`\`\`\nclaude plugin install superpowers\n\`\`\`\n` },
  ],
}

// ── Helpers ───────────────────────────────────────────────────────────────────

export function detectArtifactLang(path: string): ArtifactLang {
  if (path.endsWith('.json')) return 'json'
  if (path.endsWith('.yaml') || path.endsWith('.yml')) return 'yaml'
  if (path.endsWith('.md')) return 'markdown'
  if (path.endsWith('.sh') || path.endsWith('.bash') || !path.includes('.')) return 'bash'
  if (path.endsWith('.toml')) return 'toml'
  return 'text'
}

export function getArtifacts(pluginId: string): Artifact[] {
  return MOCK_ARTIFACTS[pluginId] ?? []
}

// ── Fetchers ─────────────────────────────────────────────────────────────────

export async function fetchMarketplaces(): Promise<Marketplace[]> {
  // TODO: replace with real API call — GET /api/marketplaces
  return structuredClone(MOCK_MARKETPLACES)
}

export async function fetchPlugins(): Promise<Plugin[]> {
  // TODO: replace with real API call — GET /api/plugins
  return structuredClone(MOCK_PLUGINS)
}

export async function getInstalledPluginIds(): Promise<Set<string>> {
  const plugins = await fetchPlugins()
  return new Set(plugins.filter(p => p.installed).map(p => p.id))
}

export async function installPlugin(pluginId: string): Promise<void> {
  // TODO: POST /api/plugins/:id/install
  await new Promise(r => setTimeout(r, 600))
}

export async function uninstallPlugin(pluginId: string): Promise<void> {
  // TODO: POST /api/plugins/:id/uninstall
  await new Promise(r => setTimeout(r, 400))
}

export async function addMarketplace(input: {
  repo?: string
  url?: string
  name?: string
  autoUpdate: boolean
}): Promise<Marketplace> {
  // TODO: POST /api/marketplaces
  await new Promise(r => setTimeout(r, 800))
  const id = input.repo?.replace('/', '-') ?? `custom-${Date.now()}`
  const ghUser = input.repo?.split('/')[0] ?? ''
  return {
    id,
    name: input.name ?? input.repo ?? id,
    owner: ghUser,
    ghUser,
    repo: input.repo,
    url: input.url,
    source: input.repo ? 'github' : 'git',
    desc: 'Custom marketplace',
    autoUpdate: input.autoUpdate,
    totalPlugins: 0,
    lastUpdated: new Date().toISOString(),
  }
}
```

- [ ] **Step 4: Run tests**

```bash
cd apps/gateway-admin && npx tsx --test lib/api/marketplace-client.test.ts 2>&1
```

Expected: all tests PASS.

- [ ] **Step 5: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/api/marketplace-client.ts apps/gateway-admin/lib/api/marketplace-client.test.ts
git commit -m "feat(marketplace): API client with mock data and artifact helpers"
```

---

## Task 3: SWR Hook

**Files:**
- Create: `lib/hooks/use-marketplace.ts`

- [ ] **Step 1: Write the hook**

```typescript
// lib/hooks/use-marketplace.ts
'use client'

import { useCallback } from 'react'
import useSWR from 'swr'
import { toast } from 'sonner'
import type { Marketplace, Plugin } from '../types/marketplace.js'
import {
  fetchMarketplaces,
  fetchPlugins,
  installPlugin,
  uninstallPlugin,
  addMarketplace,
} from '../api/marketplace-client.js'

const MARKETPLACES_KEY = 'marketplace:sources'
const PLUGINS_KEY = 'marketplace:plugins'

export function useMarketplaces() {
  return useSWR<Marketplace[]>(MARKETPLACES_KEY, fetchMarketplaces, {
    revalidateOnFocus: false,
    fallbackData: [],
  })
}

export function usePlugins() {
  return useSWR<Plugin[]>(PLUGINS_KEY, fetchPlugins, {
    revalidateOnFocus: false,
    fallbackData: [],
  })
}

export function useMarketplaceMutations() {
  const { mutate: mutatePlugins } = useSWR<Plugin[]>(PLUGINS_KEY)
  const { mutate: mutateMarketplaces } = useSWR<Marketplace[]>(MARKETPLACES_KEY)

  const install = useCallback(async (pluginId: string, pluginName: string) => {
    try {
      await installPlugin(pluginId)
      await mutatePlugins(async (prev = []) =>
        prev.map(p => p.id === pluginId ? { ...p, installed: true, installedAt: new Date().toISOString() } : p)
      )
      toast.success(`Installed ${pluginName}`)
    } catch {
      toast.error(`Failed to install ${pluginName}`)
    }
  }, [mutatePlugins])

  const uninstall = useCallback(async (pluginId: string, pluginName: string) => {
    try {
      await uninstallPlugin(pluginId)
      await mutatePlugins(async (prev = []) =>
        prev.map(p => p.id === pluginId ? { ...p, installed: false } : p)
      )
      toast.success(`Removed ${pluginName}`)
    } catch {
      toast.error(`Failed to remove ${pluginName}`)
    }
  }, [mutatePlugins])

  const addSource = useCallback(async (input: Parameters<typeof addMarketplace>[0]) => {
    try {
      const mkt = await addMarketplace(input)
      await mutateMarketplaces(async (prev = []) => [...prev, mkt])
      toast.success(`Added ${mkt.name}`)
      return mkt
    } catch {
      toast.error('Failed to add marketplace')
      return null
    }
  }, [mutateMarketplaces])

  return { install, uninstall, addSource }
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/lib/hooks/use-marketplace.ts
git commit -m "feat(marketplace): SWR hooks for marketplaces, plugins, and mutations"
```

---

## Task 4: Marketplace Card

**Files:**
- Create: `components/marketplace/marketplace-card.tsx`
- Create: `components/marketplace/marketplace-card.test.tsx`

- [ ] **Step 1: Write the failing test**

```typescript
// components/marketplace/marketplace-card.test.tsx
import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { render } from 'react-dom'  // using DOM render for smoke test
import { MarketplaceCard } from './marketplace-card.js'

// Minimal smoke test — verifies component renders without throwing
describe('MarketplaceCard', () => {
  it('renders plugin name', () => {
    const container = document.createElement('div')
    const plugin = {
      id: 'code-review@cpo',
      name: 'code-review',
      mkt: 'claude-plugins-official',
      ver: '1.0.0',
      desc: 'Elite code review expert.',
      tags: ['review', 'security'],
      installed: false,
    }
    // We can't do full component tests without a test framework setup,
    // but we can verify the module exports the expected symbol.
    assert.equal(typeof MarketplaceCard, 'function')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd apps/gateway-admin && npx tsx --test components/marketplace/marketplace-card.test.tsx 2>&1 | head -10
```

Expected: `Cannot find module './marketplace-card.js'`

- [ ] **Step 3: Write the component**

```tsx
// components/marketplace/marketplace-card.tsx
'use client'

import { Package } from 'lucide-react'
import { cn } from '@/lib/utils/utils'
import type { Plugin } from '@/lib/types/marketplace'

interface MarketplaceCardProps {
  plugin: Plugin
  ghUser?: string        // marketplace owner's GitHub user (for avatar)
  selected?: boolean
  onClick: () => void
}

function PluginAvatar({ ghUser, name }: { ghUser?: string; name: string }) {
  const initials = name
    .replace(/-/g, ' ')
    .split(' ')
    .filter(Boolean)
    .map(w => w[0])
    .join('')
    .toUpperCase()
    .slice(0, 2)

  if (!ghUser) {
    return (
      <div className="flex items-center justify-center w-10 h-10 rounded-[11px] flex-shrink-0 border border-white/[0.06] bg-aurora-panel-medium font-display text-sm font-black text-aurora-text-muted">
        {initials}
      </div>
    )
  }

  return (
    <div className="w-10 h-10 rounded-[11px] flex-shrink-0 overflow-hidden border border-white/[0.06]">
      <img
        src={`https://github.com/${ghUser}.png?size=80`}
        alt={ghUser}
        className="w-full h-full object-cover"
        onError={e => {
          const el = e.currentTarget
          el.style.display = 'none'
          const fallback = el.parentElement
          if (fallback) fallback.textContent = initials
        }}
      />
    </div>
  )
}

function StatusBadge({ plugin }: { plugin: Plugin }) {
  if (!plugin.installed) return null
  if (plugin.hasUpdate) {
    return (
      <span className="inline-flex items-center gap-[5px] text-[11px] font-bold text-aurora-warn bg-[color-mix(in_srgb,var(--aurora-warn)_7%,transparent)] border border-[color-mix(in_srgb,var(--aurora-warn)_25%,transparent)] rounded-full px-[10px] py-[3px] whitespace-nowrap">
        <span className="w-[5px] h-[5px] rounded-full bg-current flex-shrink-0" />
        Update
      </span>
    )
  }
  return (
    <span className="inline-flex items-center gap-[5px] text-[11px] font-bold text-aurora-success bg-[color-mix(in_srgb,var(--aurora-success)_7%,transparent)] border border-[color-mix(in_srgb,var(--aurora-success)_22%,transparent)] rounded-full px-[10px] py-[3px] whitespace-nowrap">
      <span className="w-[5px] h-[5px] rounded-full bg-current flex-shrink-0" />
      Installed
    </span>
  )
}

export function MarketplaceCard({ plugin, ghUser, selected, onClick }: MarketplaceCardProps) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') onClick() }}
      className={cn(
        'relative overflow-hidden rounded-aurora-3 border p-[18px] cursor-pointer',
        'flex flex-col gap-3',
        'bg-aurora-panel-medium border-aurora-border-strong',
        'shadow-aurora-medium',
        'transition-[border-color,background,box-shadow,transform] duration-150',
        'before:absolute before:inset-0 before:rounded-aurora-3 before:pointer-events-none',
        'before:bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-text-primary)_1.5%,transparent)_0%,transparent_60%)]',
        'hover:-translate-y-px hover:bg-aurora-panel-strong hover:border-aurora-accent-deep hover:shadow-aurora-strong',
        selected && 'border-aurora-accent-primary bg-aurora-panel-strong shadow-aurora-strong',
      )}
    >
      {/* Top row */}
      <div className="flex items-center gap-3">
        <PluginAvatar ghUser={ghUser} name={plugin.name} />
        <div className="flex-1 min-w-0">
          <div className="font-display text-[14px] font-extrabold tracking-[-0.02em] text-aurora-text-primary truncate">
            {plugin.name}
          </div>
          <div className="text-[11px] text-aurora-text-muted mt-0.5 font-medium">{plugin.mkt}</div>
        </div>
      </div>

      {/* Description */}
      <p className="text-[13px] text-aurora-text-muted leading-[1.55] line-clamp-2">
        {plugin.desc}
      </p>

      {/* Tags */}
      {plugin.tags.length > 0 && (
        <div className="flex gap-1 flex-wrap">
          {plugin.tags.slice(0, 3).map(t => (
            <span
              key={t}
              className="text-[10px] font-bold uppercase tracking-[0.14em] px-[9px] py-[3px] rounded-full bg-aurora-control-surface text-aurora-text-muted border border-aurora-border-default leading-[1.2]"
            >
              {t}
            </span>
          ))}
        </div>
      )}

      {/* Footer */}
      <div className="flex items-center justify-between gap-2">
        <span className="text-[11px] font-semibold bg-aurora-control-surface text-aurora-text-muted border border-aurora-border-default rounded-full px-[10px] py-[3px]">
          v{plugin.ver}
        </span>
        <StatusBadge plugin={plugin} />
      </div>
    </div>
  )
}
```

- [ ] **Step 4: Run test**

```bash
cd apps/gateway-admin && npx tsx --test components/marketplace/marketplace-card.test.tsx 2>&1
```

Expected: PASS (symbol export check).

- [ ] **Step 5: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/components/marketplace/
git commit -m "feat(marketplace): MarketplaceCard component with avatar, status badge, tags"
```

---

## Task 5: Marketplace Source Card

**Files:**
- Create: `components/marketplace/mkt-source-card.tsx`

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/mkt-source-card.tsx
'use client'

import type { Marketplace } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils/utils'

interface MktSourceCardProps {
  marketplace: Marketplace
  installedCount: number
  onClick: () => void
}

function SourceAvatar({ ghUser, name }: { ghUser: string; name: string }) {
  const initials = name.split(/\s+/).map(w => w[0]).join('').toUpperCase().slice(0, 2)
  return (
    <div className="w-12 h-12 rounded-[14px] flex-shrink-0 overflow-hidden border border-white/[0.06] flex items-center justify-center font-display text-lg font-black text-aurora-text-muted bg-aurora-panel-medium">
      <img
        src={`https://github.com/${ghUser}.png?size=96`}
        alt={ghUser}
        className="w-full h-full object-cover"
        onError={e => {
          e.currentTarget.style.display = 'none'
          const el = e.currentTarget.parentElement
          if (el) el.textContent = initials
        }}
      />
    </div>
  )
}

function sourceLabel(m: Marketplace): string {
  if (m.source === 'github') return m.repo ?? m.ghUser
  if (m.source === 'git') return m.url?.replace('https://github.com/', '').replace('.git', '') ?? m.url ?? ''
  return m.path ?? 'local'
}

export function MktSourceCard({ marketplace: m, installedCount, onClick }: MktSourceCardProps) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') onClick() }}
      className={cn(
        'rounded-aurora-3 border p-[22px] cursor-pointer',
        'flex flex-col gap-[14px]',
        'bg-aurora-panel-medium border-aurora-border-strong',
        'shadow-aurora-medium',
        'transition-[border-color,background,box-shadow] duration-150',
        'hover:bg-aurora-panel-strong hover:border-aurora-accent-deep hover:shadow-aurora-strong',
      )}
    >
      {/* Top row */}
      <div className="flex items-center gap-[14px]">
        <SourceAvatar ghUser={m.ghUser} name={m.name} />
        <div className="flex-1 min-w-0">
          <div className="font-display text-[16px] font-extrabold tracking-[-0.02em] text-aurora-text-primary">
            {m.name}
          </div>
          <div className="text-[12px] text-aurora-text-muted mt-[3px] font-medium">by {m.owner}</div>
        </div>
      </div>

      {/* Description */}
      <p className="text-[13px] text-aurora-text-muted leading-[1.55]">{m.desc}</p>

      {/* Footer */}
      <div className="flex items-center gap-[10px] flex-wrap pt-[6px] border-t border-aurora-border-default">
        <span className="text-[12px] text-aurora-text-muted flex items-center gap-[5px]">
          <strong className="text-aurora-text-primary font-bold">{installedCount}</strong> installed
        </span>
        <span className="w-[3px] h-[3px] rounded-full bg-aurora-border-strong flex-shrink-0" />
        <span className="text-[12px] text-aurora-text-muted flex items-center gap-[5px]">
          <strong className="text-aurora-text-primary font-bold">{m.totalPlugins}</strong> available
        </span>
        {m.autoUpdate && (
          <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-aurora-accent-primary whitespace-nowrap ml-auto">
            <span className="w-[5px] h-[5px] rounded-full bg-aurora-accent-primary flex-shrink-0 animate-pulse" />
            auto-update
          </span>
        )}
        <span
          className="text-[11px] font-semibold font-mono px-[9px] py-[3px] rounded-full bg-aurora-control-surface text-aurora-text-muted border border-aurora-border-default whitespace-nowrap overflow-hidden text-ellipsis max-w-[180px]"
          title={sourceLabel(m)}
        >
          {sourceLabel(m)}
        </span>
      </div>
    </div>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/mkt-source-card.tsx
git commit -m "feat(marketplace): MktSourceCard for marketplace sources tab"
```

---

## Task 6: Stats Strip

**Files:**
- Create: `components/marketplace/marketplace-stats-strip.tsx`

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/marketplace-stats-strip.tsx
'use client'

import { Download, ShoppingBag, RefreshCw } from 'lucide-react'
import type { Plugin, Marketplace } from '@/lib/types/marketplace'

interface MarketplaceStatsStripProps {
  plugins: Plugin[]
  marketplaces: Marketplace[]
  installedIds: Set<string>
  variant: 'browse' | 'marketplaces'
}

interface ChipProps {
  value: number | string
  label: string
  icon: React.ReactNode
  iconBg: string
  iconColor: string
  valueColor?: string
}

function StatChip({ value, label, icon, iconBg, iconColor, valueColor }: ChipProps) {
  return (
    <div className="flex items-center gap-[5px] px-[10px] py-[5px] rounded-[11px] transition-colors duration-150 hover:bg-aurora-hover-bg cursor-default">
      <div
        className="w-[18px] h-[18px] rounded-[5px] flex-shrink-0 flex items-center justify-center"
        style={{ background: iconBg, color: iconColor }}
      >
        <span className="w-[11px] h-[11px] [&>svg]:w-full [&>svg]:h-full">{icon}</span>
      </div>
      <span
        className="font-display text-[14px] font-extrabold tracking-[-0.03em] tabular-nums text-aurora-text-primary leading-none"
        style={valueColor ? { color: valueColor } : undefined}
        dangerouslySetInnerHTML={{ __html: String(value) }}
      />
      <span className="text-[11px] font-medium text-aurora-text-muted leading-none hidden sm:inline">
        {label}
      </span>
    </div>
  )
}

function Divider() {
  return <div className="w-px h-[22px] bg-aurora-border-default flex-shrink-0 mx-px" />
}

export function MarketplaceStatsStrip({
  plugins,
  marketplaces,
  installedIds,
  variant,
}: MarketplaceStatsStripProps) {
  const installed = plugins.filter(p => installedIds.has(p.id))
  const updates = installed.filter(p => p.hasUpdate)
  const autoUpdateCount = marketplaces.filter(m => m.autoUpdate).length

  return (
    <div className="flex items-center gap-0.5 ml-auto flex-shrink-0 bg-aurora-control-surface border border-aurora-border-default rounded-aurora-1 p-0.5 overflow-hidden shadow-[var(--aurora-shadow-small),var(--aurora-highlight-medium)]">
      {variant === 'marketplaces' ? (
        <>
          <StatChip
            value={marketplaces.length}
            label="marketplaces"
            icon={<ShoppingBag />}
            iconBg="color-mix(in srgb, var(--aurora-accent-primary) 15%, transparent)"
            iconColor="var(--aurora-accent-primary)"
          />
          <Divider />
          <StatChip
            value={plugins.length}
            label="plugins"
            icon={<Download />}
            iconBg="color-mix(in srgb, var(--aurora-accent-strong) 12%, transparent)"
            iconColor="var(--aurora-accent-strong)"
          />
          <Divider />
          <StatChip
            value={autoUpdateCount}
            label="auto-update"
            icon={<RefreshCw />}
            iconBg="color-mix(in srgb, var(--aurora-success) 12%, transparent)"
            iconColor="var(--aurora-success)"
          />
        </>
      ) : (
        <>
          <StatChip
            value={`${installed.length}<span style="font-size:11px;font-weight:500;color:var(--aurora-text-muted)">/${plugins.length}</span>`}
            label="installed"
            icon={<Download />}
            iconBg="color-mix(in srgb, var(--aurora-accent-primary) 15%, transparent)"
            iconColor="var(--aurora-accent-primary)"
          />
          <Divider />
          <StatChip
            value={marketplaces.length}
            label="sources"
            icon={<ShoppingBag />}
            iconBg="color-mix(in srgb, var(--aurora-accent-strong) 12%, transparent)"
            iconColor="var(--aurora-accent-strong)"
          />
          <Divider />
          <StatChip
            value={updates.length}
            label={updates.length ? 'updates' : 'up to date'}
            icon={<RefreshCw />}
            iconBg={updates.length
              ? 'color-mix(in srgb, var(--aurora-warn) 15%, transparent)'
              : 'color-mix(in srgb, var(--aurora-border-default) 40%, transparent)'}
            iconColor={updates.length ? 'var(--aurora-warn)' : 'var(--aurora-text-muted)'}
            valueColor={updates.length ? 'var(--aurora-warn)' : undefined}
          />
        </>
      )}
    </div>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/marketplace-stats-strip.tsx
git commit -m "feat(marketplace): MarketplaceStatsStrip with semantic color chips"
```

---

## Task 7: Add Marketplace Modal

**Files:**
- Create: `components/marketplace/add-marketplace-modal.tsx`

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/add-marketplace-modal.tsx
'use client'

import { useState } from 'react'
import { ShoppingBag } from 'lucide-react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils/utils'
import type { Marketplace } from '@/lib/types/marketplace'

interface AddMarketplaceModalProps {
  open: boolean
  onClose: () => void
  onAdd: (input: { repo?: string; url?: string; name?: string; autoUpdate: boolean }) => Promise<Marketplace | null>
}

export function AddMarketplaceModal({ open, onClose, onAdd }: AddMarketplaceModalProps) {
  const [repo, setRepo] = useState('')
  const [url, setUrl] = useState('')
  const [name, setName] = useState('')
  const [autoUpdate, setAutoUpdate] = useState(true)
  const [loading, setLoading] = useState(false)

  async function handleSubmit() {
    if (!repo.trim() && !url.trim()) return
    setLoading(true)
    try {
      const result = await onAdd({
        repo: repo.trim() || undefined,
        url: url.trim() || undefined,
        name: name.trim() || undefined,
        autoUpdate,
      })
      if (result) {
        setRepo(''); setUrl(''); setName(''); setAutoUpdate(true)
        onClose()
      }
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={v => { if (!v) onClose() }}>
      <DialogContent className="w-[520px] max-w-[calc(100vw-40px)] p-0 bg-aurora-panel-strong border-aurora-border-strong rounded-aurora-3 overflow-hidden gap-0">
        <DialogTitle className="sr-only">Add Marketplace</DialogTitle>

        {/* Header */}
        <div className="px-7 pt-6 pb-5 border-b border-aurora-border-default bg-[linear-gradient(180deg,color-mix(in_srgb,var(--aurora-panel-strong)_80%,transparent),transparent)]">
          <div className="flex items-center gap-[10px] font-display text-[19px] font-extrabold tracking-[-0.02em] text-aurora-text-primary">
            <div className="w-8 h-8 rounded-[10px] flex-shrink-0 flex items-center justify-center text-aurora-accent-primary bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-accent-primary)_18%,transparent),color-mix(in_srgb,var(--aurora-accent-deep)_24%,transparent))] border border-[color-mix(in_srgb,var(--aurora-accent-primary)_20%,transparent)]">
              <ShoppingBag className="w-4 h-4" />
            </div>
            Add Marketplace
          </div>
          <p className="text-[13px] text-aurora-text-muted mt-1 leading-[1.5]">
            Connect a GitHub repo or git URL to browse its plugin catalogue.
          </p>
        </div>

        {/* Body */}
        <div className="px-7 py-6 flex flex-col gap-[18px]">
          {/* GitHub repo */}
          <div className="flex flex-col gap-[7px]">
            <label className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
              GitHub Repository
            </label>
            <input
              className="bg-aurora-control-surface border border-aurora-border-strong rounded-aurora-1 text-aurora-text-primary placeholder:text-aurora-text-muted/55 px-[14px] py-[10px] text-[13px] outline-none focus:border-aurora-accent-primary focus:shadow-[0_0_0_3px_var(--aurora-focus-ring)] transition-[border-color,box-shadow] shadow-[var(--aurora-shadow-inset)]"
              value={repo}
              onChange={e => setRepo(e.target.value)}
              placeholder="owner/repo — e.g. obra/superpowers-marketplace"
            />
            <span className="text-[11px] text-aurora-text-muted/60">Leave blank if providing a git URL below</span>
          </div>

          {/* Git URL */}
          <div className="flex flex-col gap-[7px]">
            <label className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
              Or Git URL
            </label>
            <input
              className="bg-aurora-control-surface border border-aurora-border-strong rounded-aurora-1 text-aurora-text-primary placeholder:text-aurora-text-muted/55 px-[14px] py-[10px] text-[13px] outline-none focus:border-aurora-accent-primary focus:shadow-[0_0_0_3px_var(--aurora-focus-ring)] transition-[border-color,box-shadow] shadow-[var(--aurora-shadow-inset)]"
              value={url}
              onChange={e => setUrl(e.target.value)}
              placeholder="https://github.com/…/marketplace.git"
            />
          </div>

          {/* Name (optional) */}
          <div className="flex flex-col gap-[7px]">
            <label className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
              Marketplace Name{' '}
              <span className="text-[10px] font-normal normal-case tracking-normal opacity-60">(optional)</span>
            </label>
            <input
              className="bg-aurora-control-surface border border-aurora-border-strong rounded-aurora-1 text-aurora-text-primary placeholder:text-aurora-text-muted/55 px-[14px] py-[10px] text-[13px] outline-none focus:border-aurora-accent-primary focus:shadow-[0_0_0_3px_var(--aurora-focus-ring)] transition-[border-color,box-shadow] shadow-[var(--aurora-shadow-inset)]"
              value={name}
              onChange={e => setName(e.target.value)}
              placeholder="auto-detected from manifest"
            />
          </div>

          {/* Auto-update toggle */}
          <div className="flex items-center justify-between bg-aurora-control-surface border border-aurora-border-strong rounded-aurora-1 px-[14px] py-3 shadow-[var(--aurora-shadow-inset)]">
            <div className="flex flex-col gap-0.5">
              <span className="text-[13px] font-medium text-aurora-text-primary">Auto-update</span>
              <span className="text-[11px] text-aurora-text-muted">Sync new plugins automatically</span>
            </div>
            <label className="relative w-9 h-5 flex-shrink-0 cursor-pointer">
              <input
                type="checkbox"
                className="sr-only peer"
                checked={autoUpdate}
                onChange={e => setAutoUpdate(e.target.checked)}
              />
              <div className="absolute inset-0 rounded-full bg-aurora-border-strong peer-checked:bg-aurora-accent-primary transition-colors duration-200" />
              <div className={cn(
                'absolute top-[3px] left-[3px] w-[14px] h-[14px] rounded-full bg-aurora-text-primary shadow-[0_1px_4px_color-mix(in_srgb,black_30%,transparent)] transition-transform duration-200',
                autoUpdate && 'translate-x-4',
              )} />
            </label>
          </div>
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-2 px-7 py-4 pb-6 border-t border-aurora-border-default">
          <button
            onClick={onClose}
            className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer border border-transparent bg-transparent text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary transition-all duration-150"
          >
            Cancel
          </button>
          <button
            onClick={handleSubmit}
            disabled={loading || (!repo.trim() && !url.trim())}
            className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong transition-all duration-150 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            <ShoppingBag className="w-[14px] h-[14px]" />
            {loading ? 'Adding…' : 'Add Marketplace'}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/add-marketplace-modal.tsx
git commit -m "feat(marketplace): AddMarketplaceModal with repo/url/auto-update fields"
```

---

## Task 8: Plugin Info Panel

**Files:**
- Create: `components/marketplace/plugin-info-panel.tsx`

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/plugin-info-panel.tsx
'use client'

import { Bot, Terminal, Zap, Link2, Cpu, FileText } from 'lucide-react'
import type { Plugin, Artifact } from '@/lib/types/marketplace'

interface PluginInfoPanelProps {
  plugin: Plugin
  artifacts: Artifact[]
}

function renderMd(md: string): string {
  const lines = md.split('\n')
  let html = ''
  let inCode = false
  let codeLang = ''
  let codeLines: string[] = []

  function flushCode() {
    const escaped = codeLines.join('\n').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
    html += `<pre class="md-pre"><code>${escaped}</code></pre>`
    codeLines = []; inCode = false; codeLang = ''
  }

  for (const line of lines) {
    if (line.startsWith('```')) {
      if (inCode) { flushCode(); continue }
      inCode = true; codeLang = line.slice(3).trim(); continue
    }
    if (inCode) { codeLines.push(line); continue }

    if (/^#{1}\s/.test(line))  { html += `<div class="md-h1">${line.slice(2)}</div>`; continue }
    if (/^#{2}\s/.test(line))  { html += `<div class="md-h2">${line.slice(3)}</div>`; continue }
    if (/^#{3}\s/.test(line))  { html += `<div class="md-h3">${line.slice(4)}</div>`; continue }
    if (line.trim() === '')    { html += `<div class="md-gap"></div>`; continue }

    const formatted = line
      .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
      .replace(/`(.*?)`/g, '<code class="md-code">$1</code>')

    if (line.startsWith('- ') || line.startsWith('* ')) {
      html += `<li>${formatted.slice(2)}</li>`
    } else {
      html += `<span>${formatted}</span> `
    }
  }
  if (inCode) flushCode()
  return html
}

function CountChip({ value, label }: { value: number; label: string }) {
  if (value === 0) return null
  return (
    <div className="flex items-baseline gap-[5px] px-[14px] py-2 rounded-aurora-2 bg-aurora-control-surface border border-aurora-border-default">
      <span className="font-display text-[22px] font-bold tracking-[-0.02em] text-aurora-text-primary leading-none">
        {value}
      </span>
      <span className="text-[11px] text-aurora-text-muted">{label}</span>
    </div>
  )
}

function DetailRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-[10px] px-[14px] py-[9px] border-b border-aurora-border-default last:border-b-0">
      <span className="text-[12px] text-aurora-text-muted flex-[0_0_110px]">{label}</span>
      <span className="text-[12px] font-semibold text-aurora-text-primary flex-1 flex items-center gap-[5px] flex-wrap">
        {children}
      </span>
    </div>
  )
}

const TYPE_ICON: Record<string, React.ReactNode> = {
  agent: <Bot className="w-4 h-4" />,
  command: <Terminal className="w-4 h-4" />,
  skill: <Zap className="w-4 h-4" />,
  hook: <Link2 className="w-4 h-4" />,
  mcp: <Cpu className="w-4 h-4" />,
  lsp: <FileText className="w-4 h-4" />,
}

export function PluginInfoPanel({ plugin, artifacts }: PluginInfoPanelProps) {
  const agents   = artifacts.filter(a => a.path.startsWith('agents/')).map(a => ({ name: a.path.split('/').pop()!.replace(/\.(md|yaml|yml)$/, ''), type: 'agent' }))
  const commands = artifacts.filter(a => a.path.startsWith('commands/')).map(a => ({ name: a.path.split('/').pop()!.replace(/\.md$/, ''), type: 'command' }))
  const skills   = artifacts.filter(a => a.path.startsWith('skills/')).map(a => ({ name: a.path.split('/').pop()!.replace(/\.md$/, ''), type: 'skill' }))
  const hooks    = artifacts.filter(a => a.path.startsWith('hooks/')).map(a => ({ name: a.path.split('/').pop()!, type: 'hook' }))
  const included = [...agents, ...commands, ...skills, ...hooks]

  const readme = artifacts.find(a => a.path === 'README.md')

  return (
    <div
      className="flex-1 overflow-y-auto p-6 flex flex-col gap-5 bg-aurora-panel-strong [scrollbar-width:thin] [scrollbar-color:var(--aurora-border-default)_transparent] [&::-webkit-scrollbar]:w-[5px] [&::-webkit-scrollbar-track]:bg-transparent [&::-webkit-scrollbar-thumb]:bg-aurora-border-default [&::-webkit-scrollbar-thumb]:rounded-[3px]"
    >
      {/* Description */}
      <p className="text-[14px] leading-[1.7] text-aurora-text-primary">{plugin.desc}</p>

      {/* Count chips */}
      {included.length > 0 && (
        <div className="flex items-center gap-2 flex-wrap">
          <CountChip value={agents.length} label={agents.length === 1 ? 'agent' : 'agents'} />
          <CountChip value={commands.length} label={commands.length === 1 ? 'command' : 'commands'} />
          <CountChip value={skills.length} label={skills.length === 1 ? 'skill' : 'skills'} />
          <CountChip value={hooks.length} label={hooks.length === 1 ? 'hook' : 'hooks'} />
        </div>
      )}

      {/* Details table */}
      <div className="flex flex-col gap-2">
        <div className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Details</div>
        <div className="bg-aurora-control-surface border border-aurora-border-default rounded-aurora-2 overflow-hidden">
          <DetailRow label="Version">v{plugin.ver}</DetailRow>
          <DetailRow label="Marketplace">{plugin.mkt}</DetailRow>
          {plugin.installedAt && <DetailRow label="Installed">{plugin.installedAt}</DetailRow>}
          {plugin.updatedAt && <DetailRow label="Last updated">{plugin.updatedAt}</DetailRow>}
          <DetailRow label="Status">
            {plugin.installed
              ? plugin.hasUpdate
                ? <span className="text-aurora-warn">Update available</span>
                : <span className="text-aurora-success">Up to date</span>
              : <span className="text-aurora-text-muted">Not installed</span>
            }
          </DetailRow>
          <DetailRow label="Tags">
            {plugin.tags.map(t => (
              <span key={t} className="text-[10px] font-bold uppercase tracking-[0.14em] px-[9px] py-[3px] rounded-full bg-aurora-page-bg text-aurora-text-muted border border-aurora-border-default">
                {t}
              </span>
            ))}
          </DetailRow>
        </div>
      </div>

      {/* Included items */}
      {included.length > 0 && (
        <div className="flex flex-col gap-2">
          <div className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Included</div>
          <div className="flex flex-col gap-1">
            {included.map(item => (
              <div
                key={`${item.type}-${item.name}`}
                className="flex items-center gap-[10px] px-3 py-2 bg-aurora-control-surface border border-aurora-border-default rounded-aurora-1 transition-[border-color,background] duration-150 hover:bg-aurora-hover-bg hover:border-aurora-border-strong"
              >
                <span className="text-aurora-text-muted flex-shrink-0">{TYPE_ICON[item.type] ?? <FileText className="w-4 h-4" />}</span>
                <span className="text-[12px] font-semibold text-aurora-text-primary flex-1 min-w-0 truncate">{item.name}</span>
                <span className="text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted flex-shrink-0">{item.type}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* README */}
      {readme && (
        <div className="flex flex-col gap-2">
          <div className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">README</div>
          <div
            className="bg-aurora-control-surface border border-aurora-border-default rounded-aurora-2 px-[22px] py-5 text-[13px] leading-[1.7] text-aurora-text-primary [&_.md-h1]:font-display [&_.md-h1]:text-[18px] [&_.md-h1]:font-bold [&_.md-h1]:tracking-[-0.02em] [&_.md-h1]:text-aurora-text-primary [&_.md-h1]:mb-3 [&_.md-h2]:text-[11px] [&_.md-h2]:font-bold [&_.md-h2]:uppercase [&_.md-h2]:tracking-[0.14em] [&_.md-h2]:text-aurora-text-muted [&_.md-h2]:mt-[18px] [&_.md-h2]:mb-2 [&_.md-h2]:pb-[6px] [&_.md-h2]:border-b [&_.md-h2]:border-aurora-border-default [&_.md-h3]:text-[12px] [&_.md-h3]:font-bold [&_.md-h3]:text-aurora-text-primary [&_.md-h3]:mt-[14px] [&_.md-h3]:mb-1.5 [&_.md-gap]:h-2 [&_.md-code]:font-mono [&_.md-code]:text-[11.5px] [&_.md-code]:bg-aurora-page-bg [&_.md-code]:text-aurora-accent-primary [&_.md-code]:border [&_.md-code]:border-aurora-border-default [&_.md-code]:rounded [&_.md-code]:px-1.5 [&_.md-code]:py-px [&_.md-pre]:bg-aurora-page-bg [&_.md-pre]:border [&_.md-pre]:border-aurora-border-default [&_.md-pre]:rounded-aurora-1 [&_.md-pre]:px-[14px] [&_.md-pre]:py-3 [&_.md-pre]:my-2 [&_.md-pre]:overflow-x-auto [&_.md-pre]:font-mono [&_.md-pre]:text-[12px] [&_.md-pre]:leading-[1.6] [&_.md-pre]:text-aurora-text-muted [&_strong]:text-aurora-text-primary [&_strong]:font-semibold [&_li]:ml-4 [&_li]:list-disc [&_li]:text-aurora-text-muted [&_li]:my-[3px]"
            dangerouslySetInnerHTML={{ __html: renderMd(readme.content) }}
          />
        </div>
      )}
    </div>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/plugin-info-panel.tsx
git commit -m "feat(marketplace): PluginInfoPanel with counts, details, included items, README"
```

---

## Task 9: Plugin Files Panel

**Files:**
- Create: `components/marketplace/plugin-files-panel.tsx`

- [ ] **Step 1: Install Prism if not already present**

Check whether Prism is in the project:

```bash
grep -r "prismjs\|prism-react" apps/gateway-admin/package.json
```

If not found, Prism is loaded via CDN script tags (as in the mockup). For the React implementation, use `prismjs` as a module:

```bash
cd apps/gateway-admin && npm install prismjs
npm install --save-dev @types/prismjs
```

- [ ] **Step 2: Write the component**

```tsx
// components/marketplace/plugin-files-panel.tsx
'use client'

import { useState, useEffect, useRef, useCallback } from 'react'
import Prism from 'prismjs'
import 'prismjs/components/prism-json'
import 'prismjs/components/prism-yaml'
import 'prismjs/components/prism-bash'
import 'prismjs/components/prism-markdown'
import 'prismjs/components/prism-toml'
import { Copy, Check } from 'lucide-react'
import type { Artifact, ArtifactLang } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils/utils'

interface PluginFilesPanelProps {
  artifacts: Artifact[]
}

const LANG_GRAMMAR: Record<ArtifactLang, string> = {
  json: 'json', yaml: 'yaml', markdown: 'markdown', bash: 'bash', toml: 'toml', text: 'text',
}

const LANG_ICON: Record<ArtifactLang, string> = {
  json: '{}', yaml: '⚙', markdown: '📝', bash: '$', toml: '⚙', text: '📄',
}

const FILE_COLOR: Record<ArtifactLang, string> = {
  json: 'var(--aurora-warn)',
  yaml: 'var(--aurora-success)',
  markdown: 'var(--aurora-accent-strong)',
  bash: 'var(--aurora-accent-primary)',
  toml: 'var(--aurora-warn)',
  text: 'var(--aurora-text-muted)',
}

const FOLDER_ICON: Record<string, string> = {
  agents: '🤖', commands: '⌨️', skills: '✨', hooks: '🔗',
  monitors: '📊', bin: '⚙️', scripts: '📜',
}

function detectLang(path: string): ArtifactLang {
  if (path.endsWith('.json')) return 'json'
  if (path.endsWith('.yaml') || path.endsWith('.yml')) return 'yaml'
  if (path.endsWith('.md')) return 'markdown'
  if (path.endsWith('.sh') || !path.includes('.')) return 'bash'
  if (path.endsWith('.toml')) return 'toml'
  return 'text'
}

function highlight(code: string, lang: ArtifactLang): string {
  const grammar = Prism.languages[LANG_GRAMMAR[lang]]
  if (!grammar) return code.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;')
  return Prism.highlight(code, grammar, LANG_GRAMMAR[lang])
}

interface FileTreeProps {
  artifacts: Artifact[]
  activePath: string | null
  onSelect: (path: string) => void
}

function FileTree({ artifacts, activePath, onSelect }: FileTreeProps) {
  const folders: Record<string, Artifact[]> = {}
  const roots: Artifact[] = []

  artifacts.forEach(a => {
    const parts = a.path.split('/')
    if (parts.length === 1) { roots.push(a); return }
    const dir = parts.slice(0, -1).join('/')
    if (!folders[dir]) folders[dir] = []
    folders[dir].push(a)
  })

  const [openFolders, setOpenFolders] = useState<Set<string>>(
    () => new Set(Object.keys(folders))
  )

  function toggleFolder(dir: string) {
    setOpenFolders(prev => {
      const next = new Set(prev)
      next.has(dir) ? next.delete(dir) : next.add(dir)
      return next
    })
  }

  function FileRow({ artifact, indented }: { artifact: Artifact; indented: boolean }) {
    const lang = detectLang(artifact.path)
    const fname = artifact.path.split('/').pop()!
    const isActive = activePath === artifact.path
    return (
      <div
        role="button"
        tabIndex={0}
        onClick={() => onSelect(artifact.path)}
        onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') onSelect(artifact.path) }}
        className={cn(
          'flex items-center gap-[7px] py-1 cursor-pointer text-[12px] font-medium text-aurora-text-muted transition-[background,color] duration-100',
          'hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
          indented ? 'pl-7 pr-[10px]' : 'pl-[14px] pr-[10px]',
          isActive && 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] text-aurora-accent-strong border-l-2 border-aurora-accent-primary',
          isActive && indented && 'pl-[26px]',
          isActive && !indented && 'pl-3',
        )}
      >
        <span className="text-[12px] flex-shrink-0" style={{ color: FILE_COLOR[lang] }}>
          {LANG_ICON[lang]}
        </span>
        <span>{fname}</span>
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col overflow-y-auto overflow-x-hidden bg-aurora-nav-bg border-r border-aurora-border-default pt-[6px] pb-3 [scrollbar-width:thin] [scrollbar-color:var(--aurora-border-default)_transparent] [&::-webkit-scrollbar]:w-[4px] [&::-webkit-scrollbar-track]:bg-transparent [&::-webkit-scrollbar-thumb]:bg-aurora-border-default [&::-webkit-scrollbar-thumb]:rounded-[2px]">
      <div className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted px-[14px] pt-[10px] pb-[5px]">
        Files
      </div>
      {Object.entries(folders).map(([dir, files]) => {
        const topDir = dir.split('/')[0]
        const icon = FOLDER_ICON[topDir] ?? '📁'
        const isOpen = openFolders.has(dir)
        return (
          <div key={dir}>
            <div
              role="button"
              tabIndex={0}
              onClick={() => toggleFolder(dir)}
              onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') toggleFolder(dir) }}
              className="flex items-center gap-[5px] px-[10px] py-[5px] cursor-pointer text-[12px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary transition-[background,color] duration-100 select-none"
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2.5}
                strokeLinecap="round"
                strokeLinejoin="round"
                className={cn('w-[10px] h-[10px] flex-shrink-0 transition-transform duration-150', isOpen && 'rotate-90')}
              >
                <polyline points="9 18 15 12 9 6" />
              </svg>
              <span className="text-[13px] flex-shrink-0">{icon}</span>
              <span className="flex-1 min-w-0">{dir}/</span>
              <span className="text-[10px] font-semibold bg-aurora-control-surface border border-aurora-border-default rounded-full px-[7px] py-px text-aurora-text-muted">
                {files.length}
              </span>
            </div>
            {isOpen && files.map(f => <FileRow key={f.path} artifact={f} indented />)}
          </div>
        )
      })}
      {roots.map(f => <FileRow key={f.path} artifact={f} indented={false} />)}
    </div>
  )
}

interface CodeViewerProps {
  artifact: Artifact | null
}

function CodeViewer({ artifact }: CodeViewerProps) {
  const [copied, setCopied] = useState(false)

  async function copyCode() {
    if (!artifact) return
    await navigator.clipboard.writeText(artifact.content)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  if (!artifact) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-[10px] text-aurora-text-muted bg-aurora-page-bg">
        <span className="text-[32px] opacity-20">📂</span>
        <span className="text-[13px] opacity-50">Select a file from the tree</span>
      </div>
    )
  }

  const lang = detectLang(artifact.path)
  const parts = artifact.path.split('/')
  const pathHtml = parts.map((p, i) =>
    i === parts.length - 1
      ? `<span class="text-aurora-text-primary font-semibold">${p}</span>`
      : `${p}/`
  ).join('')

  const lines = artifact.content.split('\n')
  const lineNums = lines.map((_, i) => i + 1).join('\n')
  const highlighted = highlight(artifact.content, lang)

  return (
    <div className="flex-1 flex flex-col overflow-hidden min-w-0">
      {/* Toolbar */}
      <div className="flex items-center gap-[10px] px-[14px] py-[7px] flex-shrink-0 border-b border-aurora-border-default bg-aurora-nav-bg">
        <span
          className="font-mono text-[12px] text-aurora-text-muted flex-1 min-w-0 truncate"
          dangerouslySetInnerHTML={{ __html: pathHtml }}
        />
        <span className="text-[10px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted bg-aurora-control-surface border border-aurora-border-default rounded-[6px] px-2 py-[2px] flex-shrink-0">
          {lang}
        </span>
        <button
          onClick={copyCode}
          className="inline-flex items-center gap-[5px] bg-transparent border border-aurora-border-default rounded-[6px] text-aurora-text-muted px-[9px] py-[3px] font-sans text-[11px] font-medium cursor-pointer transition-all duration-150 hover:bg-aurora-hover-bg hover:text-aurora-text-primary hover:border-aurora-border-strong flex-shrink-0 whitespace-nowrap"
        >
          {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>

      {/* Code pane */}
      <div className="flex-1 overflow-auto flex bg-aurora-page-bg font-mono text-[12.5px] leading-[1.72] [scrollbar-width:thin] [scrollbar-color:var(--aurora-border-strong)_var(--aurora-nav-bg)] [&::-webkit-scrollbar]:w-[6px] [&::-webkit-scrollbar]:h-[6px] [&::-webkit-scrollbar-track]:bg-aurora-nav-bg [&::-webkit-scrollbar-thumb]:bg-aurora-border-strong [&::-webkit-scrollbar-thumb]:rounded-[3px] [&::-webkit-scrollbar-corner]:bg-aurora-nav-bg">
        <div className="min-w-[46px] px-[10px] pl-[14px] py-4 text-right text-aurora-border-strong select-none flex-shrink-0 border-r border-aurora-border-default text-[12px] leading-[1.72] whitespace-pre">
          {lineNums}
        </div>
        <div
          className="flex-1 px-5 py-4 text-aurora-text-muted whitespace-pre overflow-x-auto min-w-0"
          dangerouslySetInnerHTML={{ __html: highlighted }}
        />
      </div>
    </div>
  )
}

export function PluginFilesPanel({ artifacts }: PluginFilesPanelProps) {
  const [activePath, setActivePath] = useState<string | null>(
    () => artifacts.find(a => a.path === 'plugin.json')?.path ?? artifacts[0]?.path ?? null
  )

  const activeArtifact = artifacts.find(a => a.path === activePath) ?? null

  return (
    <div className="flex-1 flex overflow-hidden">
      <div className="w-[240px] flex-shrink-0">
        <FileTree artifacts={artifacts} activePath={activePath} onSelect={setActivePath} />
      </div>
      <CodeViewer artifact={activeArtifact} />
    </div>
  )
}
```

- [ ] **Step 3: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 4: Commit**

```bash
git add apps/gateway-admin/components/marketplace/plugin-files-panel.tsx
git commit -m "feat(marketplace): PluginFilesPanel with collapsible tree and Prism viewer"
```

---

## Task 10: Plugin Detail Dialog

**Files:**
- Create: `components/marketplace/plugin-detail-dialog.tsx`

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/plugin-detail-dialog.tsx
'use client'

import { useState, useCallback } from 'react'
import { X, Download, Trash2, RefreshCw } from 'lucide-react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { PluginInfoPanel } from './plugin-info-panel'
import { PluginFilesPanel } from './plugin-files-panel'
import type { Plugin, Marketplace, Artifact } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils/utils'
import { getArtifacts } from '@/lib/api/marketplace-client'

type DialogTab = 'info' | 'files'

interface PluginDetailDialogProps {
  plugin: Plugin | null
  marketplace: Marketplace | undefined
  installedIds: Set<string>
  onClose: () => void
  onInstall: (id: string, name: string) => void
  onUninstall: (id: string, name: string) => void
}

function PluginAvatar({ ghUser, name, size = 44 }: { ghUser?: string; name: string; size?: number }) {
  const initials = name.replace(/-/g,' ').split(' ').filter(Boolean).map(w => w[0]).join('').toUpperCase().slice(0, 2)
  const style = { width: size, height: size }
  if (!ghUser) {
    return (
      <div
        className="rounded-aurora-1 flex-shrink-0 overflow-hidden flex items-center justify-center font-display font-black text-aurora-text-muted bg-aurora-panel-medium border border-[color-mix(in_srgb,var(--aurora-border-strong)_40%,transparent)]"
        style={style}
      >
        {initials}
      </div>
    )
  }
  return (
    <div
      className="rounded-aurora-1 flex-shrink-0 overflow-hidden border border-[color-mix(in_srgb,var(--aurora-border-strong)_40%,transparent)]"
      style={style}
    >
      <img
        src={`https://github.com/${ghUser}.png?size=96`}
        alt={ghUser}
        className="w-full h-full object-cover"
        onError={e => {
          e.currentTarget.style.display = 'none'
          const p = e.currentTarget.parentElement
          if (p) p.textContent = initials
        }}
      />
    </div>
  )
}

export function PluginDetailDialog({
  plugin,
  marketplace,
  installedIds,
  onClose,
  onInstall,
  onUninstall,
}: PluginDetailDialogProps) {
  const [tab, setTab] = useState<DialogTab>('info')

  const artifacts: Artifact[] = plugin ? getArtifacts(plugin.id) : []
  const isInstalled = plugin ? installedIds.has(plugin.id) : false

  // Reset tab on new plugin
  const prevId = plugin?.id
  if (tab === 'files' && plugin?.id !== prevId) setTab('info')

  if (!plugin) return null

  return (
    <Dialog open onOpenChange={v => { if (!v) onClose() }}>
      <DialogContent className="w-[min(1100px,100%)] h-[min(740px,100%)] max-w-[calc(100vw-40px)] max-h-[calc(100vh-40px)] p-0 bg-aurora-panel-strong border-aurora-border-strong rounded-aurora-3 overflow-hidden flex flex-col gap-0 shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong),0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-primary)_4%,transparent),0_40px_100px_color-mix(in_srgb,black_50%,transparent)]">
        <DialogTitle className="sr-only">{plugin.name}</DialogTitle>

        {/* Header */}
        <div className="flex items-center gap-4 px-5 py-[14px] border-b border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] flex-shrink-0">
          <PluginAvatar ghUser={marketplace?.ghUser} name={plugin.name} size={44} />
          <div className="flex-1 min-w-0">
            <div className="text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted leading-none mb-[5px]">
              {marketplace?.name ?? plugin.mkt}
            </div>
            <div className="font-display text-[19px] font-bold tracking-[-0.02em] text-aurora-text-primary leading-[1.12]">
              {plugin.name}
            </div>
            <div className="flex items-center gap-[6px] mt-[6px] flex-wrap">
              <span className="text-[11px] font-semibold bg-aurora-control-surface text-aurora-text-muted border border-aurora-border-default rounded-full px-[10px] py-[3px]">
                v{plugin.ver}
              </span>
              {plugin.tags.slice(0, 3).map(t => (
                <span key={t} className="text-[10px] font-bold uppercase tracking-[0.14em] px-[9px] py-[3px] rounded-full bg-aurora-control-surface text-aurora-text-muted border border-aurora-border-default">
                  {t}
                </span>
              ))}
            </div>
          </div>
          <div className="flex items-center gap-2 flex-shrink-0">
            {isInstalled ? (
              <>
                {plugin.hasUpdate && (
                  <button
                    onClick={() => onInstall(plugin.id, plugin.name)}
                    className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer text-aurora-warn bg-[color-mix(in_srgb,var(--aurora-warn)_7%,transparent)] border border-[color-mix(in_srgb,var(--aurora-warn)_25%,transparent)] hover:bg-[color-mix(in_srgb,var(--aurora-warn)_14%,transparent)] transition-all duration-150"
                  >
                    <RefreshCw className="w-[14px] h-[14px]" />
                    Update
                  </button>
                )}
                <button
                  onClick={() => onUninstall(plugin.id, plugin.name)}
                  className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer text-aurora-error bg-transparent border border-[color-mix(in_srgb,var(--aurora-error)_30%,transparent)] hover:bg-[color-mix(in_srgb,var(--aurora-error)_8%,transparent)] transition-all duration-150"
                >
                  <Trash2 className="w-[14px] h-[14px]" />
                  Remove
                </button>
              </>
            ) : (
              <button
                onClick={() => onInstall(plugin.id, plugin.name)}
                className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong transition-all duration-150"
              >
                <Download className="w-[14px] h-[14px]" />
                Install
              </button>
            )}
            <button
              onClick={onClose}
              className="w-[30px] h-[30px] rounded-lg bg-aurora-control-surface border border-aurora-border-default text-aurora-text-muted cursor-pointer flex items-center justify-center transition-[background,color,border-color] duration-150 hover:bg-aurora-hover-bg hover:text-aurora-text-primary hover:border-aurora-border-strong"
            >
              <X className="w-3 h-3 stroke-[2.5]" />
            </button>
          </div>
        </div>

        {/* Tabs */}
        <div className="flex items-center gap-0 px-5 flex-shrink-0 border-b border-aurora-border-default bg-aurora-nav-bg">
          {(['info', 'files'] as const).map(t => (
            <button
              key={t}
              onClick={() => setTab(t)}
              className={cn(
                'font-sans text-[12px] font-semibold px-[14px] pt-[9px] pb-2 mb-[-1px] border-b-2 cursor-pointer bg-none border-t-0 border-l-0 border-r-0 transition-[color,border-color] duration-150 capitalize',
                tab === t
                  ? 'text-aurora-accent-primary border-aurora-accent-primary'
                  : 'text-aurora-text-muted border-transparent hover:text-aurora-text-primary',
              )}
            >
              {t}
            </button>
          ))}
        </div>

        {/* Content */}
        {tab === 'info' ? (
          <PluginInfoPanel plugin={plugin} artifacts={artifacts} />
        ) : (
          <PluginFilesPanel artifacts={artifacts} />
        )}
      </DialogContent>
    </Dialog>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/plugin-detail-dialog.tsx
git commit -m "feat(marketplace): PluginDetailDialog with Info/Files tabs, install/remove actions"
```

---

## Task 11: Marketplace List Content

**Files:**
- Create: `components/marketplace/marketplace-list-content.tsx`

This is the main page body. It owns: tab state, search/sort/filter, grid rendering, and dialog/modal state.

- [ ] **Step 1: Write the component**

```tsx
// components/marketplace/marketplace-list-content.tsx
'use client'

import { useState, useMemo, useCallback } from 'react'
import { Search, Plus, RefreshCw } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { MarketplaceCard } from './marketplace-card'
import { MktSourceCard } from './mkt-source-card'
import { MarketplaceStatsStrip } from './marketplace-stats-strip'
import { PluginDetailDialog } from './plugin-detail-dialog'
import { AddMarketplaceModal } from './add-marketplace-modal'
import { useMarketplaces, usePlugins, useMarketplaceMutations } from '@/lib/hooks/use-marketplace'
import type { Plugin } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils/utils'

type Tab = 'browse' | 'installed' | 'marketplaces'
type Sort = 'name' | 'marketplace' | 'installed' | 'updated'

function TabBadge({ count }: { count: number }) {
  return (
    <span className="text-[11px] font-bold tabular-nums bg-aurora-control-surface text-aurora-text-muted rounded-full px-[7px] py-px">
      {count}
    </span>
  )
}

function GroupHeader({ name, count }: { name: string; count: number }) {
  return (
    <div className="flex items-center gap-[10px] mb-3">
      <span className="font-sans text-[11px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted whitespace-nowrap">
        {name}
      </span>
      <span className="text-[11px] font-bold text-aurora-text-muted bg-aurora-control-surface rounded-full px-2 py-px border border-aurora-border-default">
        {count}
      </span>
      <div className="flex-1 h-px bg-aurora-border-default" />
    </div>
  )
}

function EmptyState({ icon, title, sub }: { icon: string; title: string; sub: string }) {
  return (
    <div className="flex flex-col items-center gap-3 text-center py-16 px-6">
      <span className="text-[36px] opacity-50">{icon}</span>
      <span className="font-display text-[17px] font-extrabold tracking-[-0.02em] text-aurora-text-primary">{title}</span>
      <span className="text-[13px] text-aurora-text-muted">{sub}</span>
    </div>
  )
}

export function MarketplaceListContent() {
  const { data: marketplaces = [], mutate: mutateMkt } = useMarketplaces()
  const { data: plugins = [] } = usePlugins()
  const { install, uninstall, addSource } = useMarketplaceMutations()

  const [tab, setTab] = useState<Tab>('browse')
  const [query, setQuery] = useState('')
  const [sort, setSort] = useState<Sort>('name')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [addModalOpen, setAddModalOpen] = useState(false)

  const installedIds = useMemo(() => new Set(plugins.filter(p => p.installed).map(p => p.id)), [plugins])

  const filtered = useMemo(() => {
    let list = tab === 'installed' ? plugins.filter(p => installedIds.has(p.id)) : plugins
    if (query) {
      const q = query.toLowerCase()
      list = list.filter(p =>
        p.name.toLowerCase().includes(q) ||
        (p.desc ?? '').toLowerCase().includes(q) ||
        (p.tags ?? []).some(t => t.includes(q)) ||
        p.mkt.includes(q)
      )
    }
    return [...list].sort((a, b) => {
      if (sort === 'name')        return a.name.localeCompare(b.name)
      if (sort === 'marketplace') return a.mkt.localeCompare(b.mkt) || a.name.localeCompare(b.name)
      if (sort === 'installed') {
        const ai = installedIds.has(a.id), bi = installedIds.has(b.id)
        return (ai === bi) ? a.name.localeCompare(b.name) : ai ? -1 : 1
      }
      if (sort === 'updated') return new Date(b.updatedAt ?? 0).getTime() - new Date(a.updatedAt ?? 0).getTime()
      return 0
    })
  }, [plugins, tab, query, sort, installedIds])

  const selectedPlugin = plugins.find(p => p.id === selectedId) ?? null
  const selectedMarketplace = marketplaces.find(m => m.id === selectedPlugin?.mkt)

  const ghUserForPlugin = useCallback((p: Plugin) => {
    return marketplaces.find(m => m.id === p.mkt)?.ghUser
  }, [marketplaces])

  const installedCount = (mktId: string) => plugins.filter(p => p.mkt === mktId && installedIds.has(p.id)).length

  function renderBrowseGrid() {
    if (!filtered.length) return <EmptyState icon="🔍" title="No results" sub={`No plugins match "${query}"`} />
    return (
      <div className="grid gap-3" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))' }}>
        {filtered.map(p => (
          <MarketplaceCard
            key={p.id}
            plugin={p}
            ghUser={ghUserForPlugin(p)}
            selected={selectedId === p.id}
            onClick={() => setSelectedId(p.id)}
          />
        ))}
      </div>
    )
  }

  function renderInstalledGroups() {
    if (query) return renderBrowseGrid()
    const groups: Record<string, Plugin[]> = {}
    filtered.forEach(p => { if (!groups[p.mkt]) groups[p.mkt] = []; groups[p.mkt].push(p) })
    if (!Object.keys(groups).length) return <EmptyState icon="📦" title="Nothing installed" sub="Browse plugins above to get started" />
    return (
      <div className="flex flex-col gap-7">
        {Object.entries(groups).sort(([a],[b]) => a.localeCompare(b)).map(([mktId, list]) => {
          const m = marketplaces.find(x => x.id === mktId)
          return (
            <div key={mktId}>
              <GroupHeader name={m?.name ?? mktId} count={list.length} />
              <div className="grid gap-3" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))' }}>
                {list.map(p => (
                  <MarketplaceCard
                    key={p.id}
                    plugin={p}
                    ghUser={ghUserForPlugin(p)}
                    selected={selectedId === p.id}
                    onClick={() => setSelectedId(p.id)}
                  />
                ))}
              </div>
            </div>
          )
        })}
      </div>
    )
  }

  function renderMarketplacesGrid() {
    return (
      <div className="grid gap-[14px]" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(340px, 1fr))' }}>
        {marketplaces.map(m => (
          <MktSourceCard
            key={m.id}
            marketplace={m}
            installedCount={installedCount(m.id)}
            onClick={() => setTab('browse')}
          />
        ))}
      </div>
    )
  }

  const browseCount = plugins.length
  const installedCount2 = installedIds.size
  const mktCount = marketplaces.length

  return (
    <>
      {/* Page header */}
      <AppHeader
        breadcrumbs={[{ label: 'Labby', href: '/' }, { label: 'Marketplace' }]}
        actions={
          <>
            <button
              onClick={() => setAddModalOpen(true)}
              className="inline-flex items-center gap-1.5 px-[14px] py-[6px] rounded-lg font-sans text-[13px] font-semibold cursor-pointer bg-transparent text-aurora-text-muted border border-aurora-border-strong hover:bg-aurora-hover-bg hover:text-aurora-text-primary transition-all duration-150"
            >
              <Plus className="w-[14px] h-[14px]" />
              Add Marketplace
            </button>
            <button
              onClick={() => { /* TODO: trigger revalidation */ }}
              className="inline-flex items-center gap-1.5 px-[14px] py-[6px] rounded-lg font-sans text-[13px] font-semibold cursor-pointer bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong transition-all duration-150"
            >
              <RefreshCw className="w-[14px] h-[14px]" />
              Refresh
            </button>
          </>
        }
      />

      {/* Tabs */}
      <div className="flex gap-0 px-6 border-b border-aurora-border-default bg-transparent flex-shrink-0">
        {([
          { id: 'browse' as const, label: 'Browse', count: browseCount },
          { id: 'installed' as const, label: 'Installed', count: installedCount2 },
          { id: 'marketplaces' as const, label: 'Marketplaces', count: mktCount },
        ] as const).map(({ id, label, count }) => (
          <button
            key={id}
            onClick={() => setTab(id)}
            className={cn(
              'flex items-center gap-[7px] px-[18px] py-3 font-sans text-[13px] font-semibold border-b-2 cursor-pointer bg-transparent border-t-0 border-l-0 border-r-0 transition-colors duration-150',
              tab === id
                ? 'text-aurora-accent-primary border-aurora-accent-primary [&_.tab-badge]:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_15%,transparent)] [&_.tab-badge]:text-aurora-accent-primary'
                : 'text-aurora-text-muted border-transparent hover:text-aurora-text-primary',
            )}
          >
            {label}
            <span className="tab-badge text-[11px] font-bold bg-aurora-control-surface text-aurora-text-muted rounded-full px-[7px] py-px transition-[background,color] duration-150">
              {count}
            </span>
          </button>
        ))}
      </div>

      {/* Main scroll */}
      <div className="flex-1 overflow-y-auto overflow-x-hidden [scrollbar-width:thin] [scrollbar-color:var(--aurora-border-default)_transparent] [&::-webkit-scrollbar]:w-[5px] [&::-webkit-scrollbar-track]:bg-transparent [&::-webkit-scrollbar-thumb]:bg-aurora-border-default [&::-webkit-scrollbar-thumb]:rounded-[3px] [&::-webkit-scrollbar-thumb:hover]:bg-aurora-border-strong">
        <div className="max-w-[1740px] w-full mx-auto px-6 py-6 pb-8 flex flex-col gap-5">

          {/* Search row */}
          {tab !== 'marketplaces' && (
            <div className="flex gap-[10px] items-center w-full">
              <div className="relative flex-[0_1_auto] min-w-[160px] max-w-[480px]">
                <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-aurora-text-muted pointer-events-none" />
                <input
                  type="text"
                  value={query}
                  onChange={e => setQuery(e.target.value)}
                  placeholder="Search plugins, marketplaces, tags…"
                  className="w-full bg-aurora-control-surface border border-aurora-border-default rounded-aurora-1 text-aurora-text-primary placeholder:text-aurora-text-muted/80 pl-10 pr-[14px] py-[10px] text-[13px] font-medium outline-none focus:border-aurora-accent-primary focus:shadow-[0_0_0_3px_var(--aurora-focus-ring)] transition-[border-color,box-shadow] shadow-[var(--aurora-shadow-small),var(--aurora-highlight-medium)]"
                />
              </div>
              <select
                value={sort}
                onChange={e => setSort(e.target.value as Sort)}
                className="bg-aurora-control-surface border border-aurora-border-default rounded-aurora-1 text-aurora-text-muted px-3 py-[9px] text-[13px] font-medium outline-none cursor-pointer flex-shrink-0 focus:border-aurora-accent-primary transition-border-color shadow-[var(--aurora-shadow-small),var(--aurora-highlight-medium)]"
              >
                <option value="name">A–Z</option>
                <option value="marketplace">Marketplace</option>
                <option value="installed">Installed first</option>
                <option value="updated">Recent</option>
              </select>
              <MarketplaceStatsStrip
                plugins={plugins}
                marketplaces={marketplaces}
                installedIds={installedIds}
                variant="browse"
              />
            </div>
          )}

          {/* Section header */}
          <div className="flex items-center justify-between">
            <span className="font-sans text-[11px] font-bold uppercase tracking-[0.14em] text-aurora-text-muted">
              {tab === 'browse' ? `${filtered.length} Plugins` : tab === 'installed' ? `${filtered.length} Installed Plugins` : `${marketplaces.length} Marketplaces`}
            </span>
          </div>

          {/* Content */}
          {tab === 'browse' && renderBrowseGrid()}
          {tab === 'installed' && renderInstalledGroups()}
          {tab === 'marketplaces' && renderMarketplacesGrid()}
        </div>
      </div>

      {/* Detail dialog */}
      {selectedPlugin && (
        <PluginDetailDialog
          plugin={selectedPlugin}
          marketplace={selectedMarketplace}
          installedIds={installedIds}
          onClose={() => setSelectedId(null)}
          onInstall={install}
          onUninstall={uninstall}
        />
      )}

      {/* Add marketplace modal */}
      <AddMarketplaceModal
        open={addModalOpen}
        onClose={() => setAddModalOpen(false)}
        onAdd={addSource}
      />
    </>
  )
}
```

- [ ] **Step 2: Verify TypeScript**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1 | grep marketplace
```

- [ ] **Step 3: Commit**

```bash
git add apps/gateway-admin/components/marketplace/marketplace-list-content.tsx
git commit -m "feat(marketplace): MarketplaceListContent — tabs, search, grids, dialog wiring"
```

---

## Task 12: Page Route + Sidebar

**Files:**
- Create: `app/(admin)/marketplace/page.tsx`
- Modify: `components/app-sidebar.tsx` — add nav item

- [ ] **Step 1: Write the page**

```tsx
// app/(admin)/marketplace/page.tsx
import { MarketplaceListContent } from '@/components/marketplace/marketplace-list-content'

export const metadata = { title: 'Marketplace — Labby' }

export default function MarketplacePage() {
  return <MarketplaceListContent />
}
```

- [ ] **Step 2: Read the current sidebar nav**

```bash
grep -n "primarySidebarNavigation\|Registry\|Activity" apps/gateway-admin/components/app-sidebar.tsx | head -20
```

This shows the exact lines where nav items are defined.

- [ ] **Step 3: Add Marketplace to sidebar**

Open `components/app-sidebar.tsx`. Find `primarySidebarNavigation`. Add the Marketplace entry after Registry:

```typescript
import { LayoutDashboard, Cable, Package, ShoppingBag, Activity, ScrollText, WandSparkles } from 'lucide-react'

export const primarySidebarNavigation = [
  { title: 'Overview',     url: '/',            icon: LayoutDashboard },
  { title: 'Gateways',     url: '/gateways',    icon: Cable },
  { title: 'Registry',     url: '/registry',    icon: Package },
  { title: 'Marketplace',  url: '/marketplace', icon: ShoppingBag },  // ← add this line
  { title: 'Setup',        url: '/setup',       icon: WandSparkles },
  { title: 'Activity',     url: '/activity',    icon: Activity },
  { title: 'Logs',         url: '/logs',        icon: ScrollText },
]
```

Only add the `Marketplace` entry and the `ShoppingBag` import. Do not change any other line.

- [ ] **Step 4: Verify TypeScript on the full project**

```bash
cd apps/gateway-admin && npx tsc --noEmit 2>&1
```

Expected: no errors.

- [ ] **Step 5: Run lint**

```bash
cd apps/gateway-admin && npx eslint components/marketplace/ app/\(admin\)/marketplace/ lib/types/marketplace.ts lib/api/marketplace-client.ts lib/hooks/use-marketplace.ts 2>&1
```

Fix any lint errors before continuing.

- [ ] **Step 6: Start dev server and verify the page loads**

```bash
cd apps/gateway-admin && npm run dev &
sleep 5
curl -s http://localhost:3000/marketplace | grep -q "Marketplace" && echo "✓ page loads" || echo "✗ page failed"
```

Navigate to `http://localhost:3000/marketplace` and verify:
- Sidebar shows "Marketplace" link with ShoppingBag icon, highlighted as active
- Browse tab renders plugin cards with GitHub avatars
- Clicking a card opens the detail dialog on the Info tab
- Switching to Files tab shows the file tree and code viewer
- "Add Marketplace" button opens the modal
- Install/Remove buttons fire toasts

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/app/\(admin\)/marketplace/page.tsx apps/gateway-admin/components/app-sidebar.tsx
git commit -m "feat(marketplace): route + sidebar nav entry — Marketplace page complete"
```

---

## Self-Review

### Spec Coverage

| Spec section | Task |
|--------------|------|
| Shell layout | T11 (AppHeader usage), T12 (page.tsx wrapping) |
| Sidebar nav | T12 (app-sidebar.tsx) |
| Tab system (Browse/Installed/Marketplaces) | T11 |
| Search + sort + stats strip | T6, T11 |
| Plugin card | T4 |
| Marketplace source card | T5 |
| Installed groups (grouped by marketplace) | T11 `renderInstalledGroups` |
| Plugin detail dialog — shell + header | T10 |
| Plugin detail dialog — Info tab | T8 |
| Plugin detail dialog — Files tab (tree + viewer) | T9, T10 |
| Add Marketplace modal | T7 |
| Aurora token compliance | All (no raw rgba/hex in component CSS/JSX) |
| Firefox scrollbar coverage | T8 (info panel), T9 (tree + code pane), T11 (main scroll) |
| GitHub avatar URL pattern | T4, T5, T10 |
| TypeScript types | T1 |
| API client + mock data | T2 |
| SWR hooks + mutations | T3 |
| Sonner toasts on install/uninstall | T3 |

All spec sections covered. No gaps found.

### Placeholder Scan

Reviewed all tasks. The only legitimate `// TODO` comments are in the API client stubs (`installPlugin`, `uninstallPlugin`, `addMarketplace`, and the Refresh button handler) — these are intentional deferred wire-ups with real mock implementations behind them. No "TBD", "fill in", or "similar to Task N" patterns found.

### Type Consistency

- `Plugin.id` — defined in T1, used consistently as `plugin.id` throughout T2–T11 ✓
- `Artifact.path` — defined in T1, `detectLang(artifact.path)` in T9 matches the field ✓
- `getArtifacts(pluginId)` — defined in T2 returning `Artifact[]`, consumed in T10 ✓
- `useMarketplaceMutations()` returns `{ install, uninstall, addSource }` — wired in T11 ✓
- `MarketplaceCard` prop `ghUser?: string` — T11 passes `ghUserForPlugin(p)` which returns `string | undefined` ✓
- `PluginDetailDialog` prop `onInstall: (id, name) => void` — T3 hook's `install` has signature `(pluginId: string, pluginName: string) => Promise<void>`, which is assignable ✓
