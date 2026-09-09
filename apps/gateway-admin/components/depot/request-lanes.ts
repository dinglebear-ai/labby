export type RequestLane = 'list' | 'detail' | 'import'

export class RequestLanes {
  private generations: Record<RequestLane, number> = { list: 0, detail: 0, import: 0 }

  invalidateContext(): void {
    this.invalidate('list')
    this.invalidate('detail')
    this.invalidate('import')
  }

  begin(lane: RequestLane): number { return ++this.generations[lane] }
  invalidate(lane: RequestLane): void { this.generations[lane] += 1 }
  isCurrent(lane: RequestLane, generation: number): boolean { return this.generations[lane] === generation }
}
