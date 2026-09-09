import { Plug, Scan, SquareTerminal } from 'lucide-react'
import type { ComponentType, SVGProps } from 'react'
import { Button } from '@/components/ui/button'
import { ClaudeMark } from './discover-format-mark'
import { DISCOVER_SOURCE_ORIGINS } from './discover-source-badge'
import type { DepotProviderOption } from '@/lib/api/depot-client'
import { discoverSourceSupport } from './discover-source-support'

// Brand paths vendored from Simple Icons (CC0), not runtime image requests:
// https://github.com/simple-icons/simple-icons/blob/develop/icons/github.svg
// https://github.com/simple-icons/simple-icons/blob/develop/icons/googlegemini.svg
function GitHubMark(props: SVGProps<SVGSVGElement>) {
  return <svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor" {...props}><path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12" /></svg>
}

function GeminiMark(props: SVGProps<SVGSVGElement>) {
  return <svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor" {...props}><path d="M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81" /></svg>
}

type FilterOrigin = keyof typeof DISCOVER_SOURCE_ORIGINS
type Source = { label: string; icon: ComponentType<SVGProps<SVGSVGElement>>; color: string; origin?: FilterOrigin }

/** Mock order. An absent origin means provenance filtering is not implemented, not zero results. */
const sources: readonly Source[] = [
  { ...DISCOVER_SOURCE_ORIGINS['mcp-registry'], origin: 'mcp-registry' },
  { ...DISCOVER_SOURCE_ORIGINS['acp-registry'], origin: 'acp-registry' },
  { label: 'skills.sh', icon: SquareTerminal, color: 'var(--aurora-accent-strong)' },
  { ...DISCOVER_SOURCE_ORIGINS.ard, origin: 'ard' },
  { label: 'GitHub', icon: GitHubMark, color: 'var(--aurora-text-primary)' },
  { label: 'Claude', icon: ClaudeMark, color: 'var(--aurora-text-primary)' },
  { label: 'Gemini', icon: GeminiMark, color: 'var(--aurora-text-primary)' },
  { label: 'Agent Plugins', icon: Plug, color: 'var(--aurora-warn)' },
  { label: 'Web Crawl', icon: Scan, color: 'var(--aurora-error)' },
]

export function DiscoverSourceStrip({ selectedOrigin, onSelect, providers, selectedProvider }: { selectedOrigin?: string; onSelect: (origin?: FilterOrigin) => void; providers: readonly DepotProviderOption[]; selectedProvider: string }) {
  return <div role="group" aria-label="Quick source filters" className="aurora-scrollbar hidden max-w-[50%] shrink-0 items-center gap-[3px] overflow-x-auto border-l border-aurora-border-default pl-2 sm:flex">
    {sources.map(source => {
      const support = source.origin ? discoverSourceSupport(providers, selectedProvider, source.origin) : undefined
      const available = support?.state === 'available'
      return <Button key={source.label} size="icon-sm" variant={source.origin && selectedOrigin === source.origin ? 'secondary' : 'ghost'}
      aria-label={source.origin ? `Filter source: ${source.label}` : `${source.label}: source filter unavailable`}
      aria-disabled={!available || undefined} aria-pressed={source.origin ? selectedOrigin === source.origin : undefined}
      title={source.origin ? `${source.label} — ${support?.reason}` : `${source.label} — recorded source provenance is not available for filtering yet`}
      onClick={() => { if (available && source.origin) onSelect(selectedOrigin === source.origin ? undefined : source.origin) }}
      className="size-[26px] shrink-0 rounded-[7px] border border-aurora-border-subtle aria-disabled:cursor-not-allowed aria-disabled:opacity-60" style={{ color: source.color }}>
      <source.icon aria-hidden="true" className="size-3.5" />
    </Button>})}
  </div>
}
