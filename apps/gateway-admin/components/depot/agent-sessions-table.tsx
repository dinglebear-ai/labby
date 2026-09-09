'use client'

import type { ReactNode } from 'react'
import { ArrowUpDown } from 'lucide-react'

export function AgentSessionsTable({ rows, onSelect, onSort, renderMark }: { rows: string[][]; onSelect: (row: string[]) => void; onSort: (column: number) => void; renderMark: (name: string) => ReactNode }) {
  return <div className="aurora-scrollbar overflow-x-auto"><table className="w-full min-w-[640px] table-fixed text-left">
    <colgroup><col className="w-[110px]"/><col/><col className="w-[138px]"/><col className="w-[128px] max-[1180px]:hidden"/><col className="w-[102px] max-[1400px]:hidden"/><col className="w-[82px]"/></colgroup>
    <thead className="border-b border-aurora-border-subtle bg-[var(--gw0-0_26)]"><tr>{['Status', 'Session', 'Loadout', 'Container', 'Harness', 'Elapsed'].map((label, index) => <th key={label} scope="col" className={`py-2 pr-2.5 ${index === 0 ? 'pl-4' : ''} ${index === 3 ? 'max-[1180px]:hidden' : ''} ${index === 4 ? 'max-[1400px]:hidden' : ''}`}><button type="button" onClick={() => onSort(index)} className={`flex items-center gap-1 text-[9.5px] font-bold uppercase tracking-[.1em] text-[#97b6c9] ${index === 5 ? 'ml-auto' : ''}`}>{label}<ArrowUpDown aria-hidden="true" className="size-2.5 opacity-45"/></button></th>)}</tr></thead>
    <tbody>{rows.map((row, index) => <tr key={row[1]} tabIndex={0} onClick={() => onSelect(row)} onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onSelect(row) } }} className={`cursor-pointer border-t border-aurora-border-subtle hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary ${index % 2 ? 'bg-[var(--gw2-0_55)]' : 'bg-[var(--gw1-0_62)]'}`}>
      <td className="py-[9px] pl-4 pr-2.5"><span className={`inline-flex h-5 items-center gap-1.5 rounded-full border border-current/25 bg-current/5 px-2 text-[9.5px] font-[650] ${row[0] === 'Running' ? 'text-aurora-success' : row[0] === 'Failed' ? 'text-aurora-error' : 'text-aurora-text-muted'}`}><span className="size-1.5 shrink-0 rounded-full bg-current"/>{row[0]}</span></td>
      <td className="py-[9px] pr-2.5"><span className="block truncate text-[12.5px] font-[650] text-aurora-text-primary">{row[1]}</span></td>
      <td className="py-[9px] pr-2.5"><span className="inline-flex h-[18px] max-w-full items-center rounded-full border border-aurora-border-subtle bg-aurora-control-surface px-2 text-[9.5px] text-aurora-accent-strong"><span className="truncate">{row[2]}</span></span></td>
      {[3, 4].map(column => <td key={column} className={`py-[9px] pr-2.5 ${column === 3 ? 'max-[1180px]:hidden' : 'max-[1400px]:hidden'}`}><span className="flex min-w-0 items-center gap-1.5 text-[10.5px] text-aurora-text-muted"><span className="shrink-0">{renderMark(row[column])}</span><span className="truncate">{row[column]}</span></span></td>)}
      <td className="py-[9px] pr-4 text-right text-[11px] text-aurora-text-muted">{row[5]}</td>
    </tr>)}</tbody>
  </table>{!rows.length ? <p className="p-6 text-center text-xs text-aurora-text-muted">No matching sessions.</p> : null}</div>
}
