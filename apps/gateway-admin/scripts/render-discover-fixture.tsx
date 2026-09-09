import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { mkdtempSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { DiscoverArtifactCard } from '../components/depot/discover-artifact-card'
import { DiscoverReadme } from '../components/depot/discover-readme'
import { DiscoverFeaturedRail } from '../components/depot/discover-featured-rail'
import type { FederatedArtifact } from '../lib/api/depot-client'

// Visual-only evidence: no gateway credentials, network requests, or mutations.
// Run from gateway-admin after building: pnpm exec tsx scripts/render-discover-fixture.tsx
const exportedPage = readFileSync(new URL('../out/depot/index.html', import.meta.url), 'utf8')
const styles = [...new Set(exportedPage.match(/\/_next\/static\/[^"\s]+\.css/g) ?? [])]
if (!styles.length) throw new Error('Build the static export before rendering the fixture.')
const output = mkdtempSync('/private/tmp/labby-discover-visual-')
// Keep the actual exported fonts and CSS same-origin without duplicating assets.
symlinkSync(new URL('../out/_next', import.meta.url), join(output, '_next'), 'dir')
const fontClasses = exportedPage.match(/<html[^>]*class="([^"]*)"/)?.[1] ?? ''
const now = Date.parse('2026-09-08T12:00:00Z')
const artifacts: FederatedArtifact[] = [
  { providerId: 'fixture-catalog', artifactId: 'fixture-skill', kind: 'skill', title: 'Repository research', namespace: 'example-team', description: 'Read a repository and return a concise, sourced implementation map.', currentRevisionId: 'fixture-revision', currentRevision: { fileCount: 4, authoredAt: '2026-09-07T12:00:00Z' } },
  { providerId: 'fixture-catalog', artifactId: 'fixture-agent', kind: 'agent', title: 'A deliberately long Artifact title to check card truncation', namespace: 'example-publisher-with-a-long-name', description: 'An intentionally longer description tests wrapping across two lines without pushing adjacent card actions out of alignment.', currentRevisionId: 'fixture-revision', currentRevision: { fileCount: 12 } },
  { providerId: 'fixture-secondary', artifactId: 'fixture-unknown', title: 'Incomplete source metadata' },
]
for (const theme of ['dark', 'light']) {
  const markup = renderToStaticMarkup(<html lang="en" className={`${fontClasses} ${theme}`}><head><meta charSet="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>{`Discover visual fixture — ${theme}`}</title>
    {styles.map(path => <link key={path} rel="stylesheet" href={path} />)}
  </head><body><main className="min-h-screen bg-aurora-bg-base p-6 text-aurora-text-primary">
    <h1>Discover card visual fixture — {theme}</h1>
    <p>Sample data only. Controls are inert; this is not live workflow evidence.</p>
    <div className="my-6 max-w-4xl space-y-5">
      <DiscoverFeaturedRail title="Popular This Week" subtitle="fixture installs, last 7 days" feed={{ state: 'ready', items: artifacts.map((artifact, index) => ({ artifact, installs: [58000, 42000, 27000][index] })) }} artifactHref={() => '#'} />
      <DiscoverFeaturedRail title="New From Your Team" subtitle="fixture team" feed={{ state: 'ready', items: artifacts.map(artifact => ({ artifact })) }} artifactHref={() => '#'} />
      <DiscoverFeaturedRail title="Pairs With Your Loadouts" subtitle="fixture pairings — not compatibility evidence" feed={{ state: 'ready', items: artifacts.map(artifact => ({ artifact })) }} artifactHref={() => '#'} />
      <DiscoverFeaturedRail title="Unavailable feed" subtitle="explicit unavailable state" feed={{ state: 'unavailable', message: 'This source does not report the required activity.' }} artifactHref={() => '#'} />
    </div>
    <div className="my-6 max-w-3xl"><DiscoverReadme path="README.md" content={'# Repository research\n\nSample documentation for visual review only.\n\n## Usage\n\nReview an exact revision before adding it to your library.\n\n- Inspect the source\n- Confirm the selected revision\n\n```text\nThis is sample source text, not an install command.\n```'} /></div>
    {(['default', 'compact', 'comfortable'] as const).map(density => <section key={density} className="my-6 space-y-4"><h2>{density}</h2>
      <div className={density === 'compact' ? 'grid gap-4' : 'grid gap-4 md:grid-cols-2 xl:grid-cols-3'}>
        {artifacts.map((artifact, index) => <DiscoverArtifactCard key={artifact.artifactId} artifact={artifact} compact={density === 'compact'} density={density} selected={index === 1} href="#" now={now} inLibrary={index === 0} onImport={async () => {}} onSend={() => {}} onFork={() => {}} />)}
      </div>
    </section>)}
  </main></body></html>)
  writeFileSync(join(output, `${theme}.html`), `<!doctype html>${markup}`)
}
console.log(output)
