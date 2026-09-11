import { Badge } from '@/components/ui/badge'

export function DiscoverFileCount({ count }: { count: number | undefined }) {
  return count === undefined ? null : <Badge variant="outline">{`${count} ${count === 1 ? 'file' : 'files'}`}</Badge>
}
