import { Skeleton } from '@/components/ui/skeleton'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { cn } from '@/lib/utils'
import { AURORA_STRONG_PANEL } from '@/components/aurora/tokens'

interface TableSkeletonProps {
  rows?: number
  presentation?: 'table' | 'list' | 'cards'
}

export function GatewayTableSkeleton({ rows = 5, presentation = 'table' }: TableSkeletonProps) {
  if (presentation !== 'table') return (
    <div role="status" aria-label="Loading servers" className={cn('space-y-2', presentation === 'cards' && 'min-[1101px]:grid min-[1101px]:grid-cols-2 min-[1101px]:gap-3 min-[1101px]:space-y-0')}>
      {Array.from({ length: rows }).map((_, index) => <div key={index} className={cn(AURORA_STRONG_PANEL, 'space-y-3 p-4')}>
        <div className="flex items-center justify-between gap-3"><Skeleton className="h-5 w-2/3 max-w-52" /><Skeleton className="size-11 rounded-full" /></div>
        <Skeleton className="h-4 w-full max-w-72" />
        {presentation === 'cards' ? <div className="grid grid-cols-2 gap-2"><Skeleton className="h-12 w-full" /><Skeleton className="h-12 w-full" /></div> : null}
      </div>)}
    </div>
  )
  return (
    <div role="status" aria-label="Loading servers" className={cn(AURORA_STRONG_PANEL, 'aurora-scrollbar min-w-0 overflow-x-auto')}>
      <Table>
        <TableHeader>
          <TableRow className="border-b border-aurora-border-strong bg-[rgba(7,17,26,0.48)] hover:bg-[rgba(7,17,26,0.48)]">
            <TableHead className="w-[200px] text-aurora-text-muted">Name</TableHead>
            <TableHead className="w-[100px] text-aurora-text-muted">Transport</TableHead>
            <TableHead className="w-[100px] text-aurora-text-muted">Status</TableHead>
            <TableHead className="w-[140px] text-aurora-text-muted">Tools</TableHead>
            <TableHead className="w-[100px] text-aurora-text-muted">Warnings</TableHead>
            <TableHead className="w-[120px]" />
          </TableRow>
        </TableHeader>
        <TableBody>
          {Array.from({ length: rows }).map((_, i) => (
            <TableRow key={i} className="border-t border-aurora-border-strong/70">
              <TableCell>
                <Skeleton className="h-5 w-32 bg-aurora-panel-strong-top" />
              </TableCell>
              <TableCell>
                <Skeleton className="h-5 w-16 bg-aurora-panel-strong-top" />
              </TableCell>
              <TableCell>
                <Skeleton className="h-5 w-20 bg-aurora-panel-strong-top" />
              </TableCell>
              <TableCell>
                <div className="flex gap-2">
                  <Skeleton className="h-5 w-12 bg-aurora-panel-strong-top" />
                  <Skeleton className="h-5 w-12 bg-aurora-panel-strong-top" />
                </div>
              </TableCell>
              <TableCell>
                <Skeleton className="h-5 w-8 bg-aurora-panel-strong-top" />
              </TableCell>
              <TableCell>
                <div className="flex justify-end gap-1">
                  <Skeleton className="size-8 rounded-md bg-aurora-panel-strong-top" />
                  <Skeleton className="size-8 rounded-md bg-aurora-panel-strong-top" />
                </div>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
