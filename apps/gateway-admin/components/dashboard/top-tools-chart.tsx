'use client'

import type { ToolUsageEntry } from '@/lib/types/metrics'

export function TopToolsChart({ tools, onSelect }: { tools: ToolUsageEntry[]; onSelect?: (tool: string) => void }) {
  const maximum = Math.max(0, ...tools.map(tool => tool.calls))
  return <ol aria-label="Top tools by supplied order" className="flex min-w-0 flex-col gap-2.5">
    {tools.map((tool, index) => {
      const label = tool.label ?? tool.name
      const content = <>
        <span className="mb-1 flex min-w-0 items-baseline gap-2">
          <span className="w-[18px] shrink-0 text-[10px] font-semibold tabular-nums text-aurora-text-muted">{index + 1}</span>
          <span title={label} className="min-w-0 flex-1 truncate text-[11.5px] font-semibold text-aurora-text-primary">{label}</span>
          {tool.failed > 0 ? <span className="shrink-0 text-[10px] font-semibold tabular-nums text-aurora-error">{tool.failed} failed</span> : null}
          <span className="shrink-0 text-[11px] font-semibold tabular-nums text-aurora-text-muted">{tool.calls.toLocaleString('en-US')}</span>
        </span>
        <span aria-hidden="true" className="ml-[26px] block h-1 overflow-hidden rounded-full bg-aurora-control-surface"><span className="block h-full rounded-full bg-gradient-to-r from-aurora-accent-deep to-aurora-accent-primary" style={{ width: `${maximum > 0 ? Math.max(0, tool.calls) / maximum * 100 : 0}%` }}/></span>
      </>
      return <li key={tool.id ?? tool.name} className="min-w-0">{onSelect
        ? <button type="button" onClick={() => onSelect(tool.name)} aria-label={`Inspect ${label}: ${tool.calls} calls, ${tool.failed} failed`} className="-mx-2 block w-[calc(100%+16px)] min-w-0 rounded-lg px-2 py-1 text-left hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{content}</button>
        : <div className="min-w-0 py-1">{content}</div>}</li>
    })}
  </ol>
}
