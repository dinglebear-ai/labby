'use client'

import { Fragment, useState } from 'react'
import { Clock3, ScrollText } from 'lucide-react'
import { useMediaQuery } from '@/lib/hooks/use-media-query'

export function ScheduledTasks({ rows, onToggle, onSelect }: { rows: string[][]; onToggle: (row: string[]) => void; onSelect: (row: string[]) => void }) {
  const [filter, setFilter] = useState('All')
  const [expanded, setExpanded] = useState<string | null>(null)
  const hideLoadout = useMediaQuery('(width < 1180px)')
  const hideLastRun = useMediaQuery('(width < 1400px)')
  const visibleColumns = 7 - Number(hideLoadout) - Number(hideLastRun)
  const shown = rows.filter(row => filter === 'All' || row[0] === filter)
  return <section aria-label="Scheduled tasks" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
    <div className="flex items-center gap-2 border-b border-aurora-border-subtle bg-[var(--gw0-0_38)] px-[15px] py-2.5">
      <h2 className="mr-auto text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">Scheduled</h2>
      {['All', 'Armed', 'Paused'].map(item => <button key={item} type="button" aria-pressed={filter === item} onClick={() => setFilter(item)} className="h-6 rounded-full border border-aurora-border-subtle px-2.5 text-[10.5px] font-[650] text-aurora-text-muted hover:text-aurora-text-primary aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-[#06202e]">{item}</button>)}
    </div>
    <div className="aurora-scrollbar overflow-x-auto">
      <table className="w-full min-w-[720px] table-fixed text-left">
        <colgroup><col className="w-[68px]"/><col/><col className="w-[142px]"/><col className="w-[160px] max-[1180px]:hidden"/><col className="w-[106px] max-[1400px]:hidden"/><col className="w-[96px]"/><col className="w-11"/></colgroup>
        <thead className="border-b border-aurora-border-subtle bg-[var(--gw0-0_26)]"><tr>{['On', 'Task', 'Schedule', 'Loadout', 'Last Run', 'Next', ''].map((label, index) => <th key={index} scope="col" className={`py-2 pr-2.5 text-[9.5px] font-bold uppercase tracking-[.1em] text-[#97b6c9] ${index === 0 ? 'pl-4' : ''} ${index === 3 ? 'max-[1180px]:hidden' : ''} ${index === 4 ? 'max-[1400px]:hidden' : ''} ${index === 5 ? 'text-right' : ''}`}><span className={index === 6 ? 'sr-only' : ''}>{label || 'Actions'}</span></th>)}</tr></thead>
        <tbody>{shown.map((row, index) => <Fragment key={row[1]}><tr className={`border-t border-aurora-border-subtle hover:bg-aurora-hover-bg ${index % 2 ? 'bg-[var(--gw2-0_55)]' : 'bg-[var(--gw1-0_62)]'}`}>
          <td className="py-[9px] pl-4 pr-2.5"><button type="button" role="switch" aria-checked={row[0] === 'Armed'} aria-label={`${row[0] === 'Armed' ? 'Pause' : 'Arm'} ${row[1]}`} onClick={() => onToggle(row)} className="relative block h-[18px] w-8 rounded-full border border-aurora-border-strong bg-aurora-control-surface aria-checked:border-aurora-accent-primary aria-checked:bg-aurora-selected-bg"><span className="absolute left-0.5 top-0.5 size-3 rounded-full bg-aurora-text-muted transition-transform [[aria-checked=true]_&]:translate-x-[14px] [[aria-checked=true]_&]:bg-aurora-accent-strong"/></button></td>
          <td className="py-[9px] pr-2.5"><button type="button" aria-expanded={expanded === row[1]} onClick={() => setExpanded(current => current === row[1] ? null : row[1])} className="block w-full min-w-0 text-left"><span className="block truncate text-[12.5px] font-[650] text-aurora-text-primary">{row[1]}</span><span className="block truncate text-[10.5px] text-aurora-text-muted">{row[5]}</span></button></td>
          <td className="py-[9px] pr-2.5"><span className="flex items-center gap-1.5"><Clock3 aria-hidden="true" className="size-[11px] shrink-0 text-aurora-text-muted"/><span className="truncate text-[11px] font-semibold text-aurora-text-primary">{row[2]}</span></span></td>
          <td className="py-[9px] pr-2.5 max-[1180px]:hidden"><span className="inline-flex h-[18px] max-w-full items-center rounded-full border border-aurora-border-subtle bg-aurora-control-surface px-2 text-[9.5px] text-aurora-accent-strong"><span className="truncate">#{row[3]}</span></span></td>
          <td className="py-[9px] pr-2.5 max-[1400px]:hidden"><span className={`inline-flex items-center gap-1.5 text-[10px] font-semibold ${row[6] === 'failed' ? 'text-aurora-error' : row[6] === 'partial' ? 'text-aurora-warn' : row[6] === 'passed' ? 'text-aurora-success' : 'text-aurora-text-muted'}`}><span className="size-1.5 rounded-full bg-current"/>{row[6]}</span></td>
          <td className="py-[9px] pr-2.5 text-right text-[11px] text-aurora-text-muted">{row[4]}</td>
          <td className="py-[9px] pr-4"><a href="/logs" aria-label={`View logs for ${row[1]}`} className="grid size-7 place-items-center rounded-lg text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-accent-strong"><ScrollText className="size-3.5"/></a></td>
        </tr>{expanded === row[1] ? <tr><td colSpan={visibleColumns} className="p-0"><TaskInlineDetail row={row} onEdit={() => onSelect(row)} onToggle={() => onToggle(row)} /></td></tr> : null}</Fragment>)}</tbody>
      </table>
      {!shown.length ? <p className="p-6 text-center text-xs text-aurora-text-muted">No {filter === 'All' ? '' : filter.toLowerCase() + ' '}tasks.</p> : null}
    </div>
  </section>
}

function TaskInlineDetail({ row, onEdit, onToggle }: { row: string[]; onEdit: () => void; onToggle: () => void }) {
  const label = 'mt-2.5 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted'
  return <div className="grid grid-cols-[minmax(0,1.3fr)_minmax(220px,.7fr)] gap-3.5 border-t border-aurora-border-subtle bg-[color-mix(in_srgb,var(--aurora-page-bg)_25%,transparent)] pb-4 pl-[68px] pr-4 pt-1">
    <section aria-label={`Recent runs for ${row[1]}`} className="flex min-w-0 flex-col gap-2"><h3 className={label}>Recent runs</h3><p className="rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-2.5 py-[7px] text-[11.5px] text-aurora-text-muted">Run history is unavailable in this preview.</p></section>
    <section aria-label={`Definition of ${row[1]}`} className="flex min-w-0 flex-col gap-2"><h3 className={label}>Definition</h3><dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-[11px] py-[9px] text-[11.5px]">{[['State', row[0]], ['Schedule', row[2]], ['Loadout', row[3]], ['Task', row[5]]].map(([key, value]) => <Fragment key={key}><dt className="text-aurora-text-muted">{key}</dt><dd title={value} className="truncate text-aurora-text-primary">{value}</dd></Fragment>)}</dl><div className="flex flex-wrap gap-1.5">{[[row[0] === 'Armed' ? 'Pause task' : 'Arm task', onToggle], ['Edit task', onEdit]].map(([text, action]) => <button key={String(text)} type="button" onClick={action as () => void} className="h-7 rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-[11px] text-[11.5px] font-[650] text-aurora-accent-strong hover:bg-aurora-hover-bg">{text as string}</button>)}</div></section>
  </div>
}
