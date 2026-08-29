import { Copy, Download, Pause, Search, Wifi } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { MockSurfaceBadge } from '@/components/console/mock-surface-badge'

const lines = [
  ['14:53:13','INFO','axon','oauth token refreshed · sub:ab12cd'],
  ['14:52:12','ERROR','mcpsh','upstream_connect_error · connection refused · retry in 8s'],
  ['14:51:11','WARN','ytdl','slow upstream · ytdl::list_items took 594ms (p99 breach)'],
  ['14:50:10','INFO','tailscale','client initialize · claude-code v2.0.14 · session 01J705'],
  ['14:49:09','DEBUG','gateway','reconcile tick · 16 servers probed · catalog unchanged'],
  ['14:48:08','INFO','gotify','tools/call routed · gotify::list_items · 200 in 591ms'],
  ['14:47:11','DEBUG','gateway','notifications/tools/list_changed forwarded to 3 clients'],
  ['14:46:10','INFO','tailscale','oauth token refreshed · sub:ab12cd'],
  ['14:45:09','ERROR','unraid','upstream_connect_error · connection refused · retry in 8s'],
  ['14:44:08','WARN','gotify','slow upstream · gotify::list_items took 531ms (p99 breach)'],
  ['14:43:07','INFO','apprise','client initialize · claude-code v2.0.14 · session 01J642'],
  ['14:42:06','DEBUG','gateway','reconcile tick · 16 servers probed · catalog unchanged'],
] as const

const control = 'inline-flex h-8 items-center gap-1.5 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3 text-[11px] font-semibold text-aurora-text-muted disabled:cursor-not-allowed disabled:opacity-60'

export function MockLogsPage() {
  return <><AppHeader breadcrumbs={[{ label: 'Logs' }]} /><section data-screen-label="Logs" data-mock-region="logs" aria-label="Logs mock data" className="flex flex-col gap-[14px]">
    <header className="flex items-end justify-between gap-4"><div><div className="flex items-center gap-2.5"><span className="text-[10.5px] font-bold uppercase tracking-[.16em] text-aurora-text-muted">Observability</span><span className="inline-flex items-center gap-1.5 text-[10.5px] font-semibold text-aurora-success"><span className="size-1.5 rounded-full bg-aurora-success" />Streaming · all sources</span><MockSurfaceBadge /></div><h1 className="mt-2 font-display text-[30px] font-extrabold text-aurora-text-primary">Logs</h1></div></header>
    <section role="region" aria-label="Log stream" className="overflow-hidden rounded-aurora-2 border border-aurora-border-default/55 bg-[var(--gw4-0_62)] shadow-[var(--aurora-shadow-strong),inset_0_1px_0_rgba(255,255,255,0.04)]">
      <div className="flex flex-wrap items-center gap-2 border-b border-aurora-border-default/60 bg-[var(--gw0-0_48)] p-3"><label className="flex h-8 min-w-[220px] flex-1 items-center gap-2 rounded-[9px] border border-aurora-border-default bg-aurora-control-surface px-3"><Search className="size-3.5 text-aurora-text-muted" /><input disabled aria-label="Filter log lines" placeholder="Filter lines…" className="w-full bg-transparent text-[11px] outline-none placeholder:text-aurora-text-muted" /></label><span className="text-[10px] font-bold uppercase text-aurora-text-muted">Source</span><button disabled className={control}>Gateway (all)</button>{[['ERROR','7'],['WARN','6'],['INFO','18'],['DEBUG','11']].map(([level,count])=><button key={level} disabled className={control}>{level}<b>{count}</b></button>)}<button disabled className={control}><Pause className="size-3" />Follow</button><button disabled className={control}><Download className="size-3" />Download</button><MockSurfaceBadge /></div>
      <div className="hidden grid-cols-[90px_70px_110px_minmax(360px,1fr)_42px] gap-3 border-b border-aurora-border-strong bg-[var(--gw0-0_38)] px-4 py-2 text-[10px] font-bold uppercase tracking-[.13em] text-aurora-text-muted md:grid"><span>Time</span><span>Level</span><span>Source</span><span>Message</span><span /></div>
      <div className="aurora-scrollbar max-h-[560px] overflow-auto font-mono">{lines.map(([time,level,source,message])=><article key={`${time}-${source}`} data-mock-region="log-line" className="grid grid-cols-[auto_1fr_auto] items-center gap-x-3 gap-y-1.5 border-t border-aurora-border-default/35 px-4 py-3 text-[10.5px] first:border-t-0 hover:bg-aurora-hover-bg md:grid-cols-[90px_70px_110px_minmax(360px,1fr)_42px] md:gap-3 md:py-2.5"><span className="text-aurora-text-muted">{time}</span><span className={level==='ERROR'?'text-aurora-error':level==='WARN'?'text-aurora-warn':level==='INFO'?'text-aurora-success':'text-aurora-accent-strong'}>{level}</span><span className="text-right text-aurora-accent-pink md:text-left">{source}</span><span className="col-span-3 text-aurora-text-primary md:col-span-1">{message}</span><button disabled aria-label="Copy line" className={`${control} hidden md:inline-flex`}><Copy className="size-3" /></button></article>)}</div>
      <div className="flex flex-wrap items-center gap-2 border-t border-aurora-border-default/60 bg-[var(--gw0-0_48)] px-4 py-2 text-[10.5px] leading-[1.4] text-aurora-text-muted"><Wifi className="size-3 shrink-0 text-aurora-warn" />Paused — new lines buffered · gateway + 15 upstream servers · retained 72h<span className="w-full sm:ml-auto sm:w-auto">11 lines/s · 42 of 42 lines · Mock</span></div>
    </section>
    <div className="flex flex-col items-start gap-2.5 rounded-aurora-1 border border-aurora-warn/30 bg-[color-mix(in_srgb,var(--aurora-warn)_8%,var(--aurora-panel-strong))] px-4 py-3 text-[12px] leading-[1.55] text-aurora-text-muted sm:flex-row sm:gap-3"><MockSurfaceBadge /><p>This Logs page reproduces the approved mock. Every line, count, source, status, and stream control is illustrative; no controls call a Labby service.</p></div>
  </section></>
}
