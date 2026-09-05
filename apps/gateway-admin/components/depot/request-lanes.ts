export type RequestLane = 'list' | 'detail'

export class RequestLanes {
  private generations: Record<RequestLane, number> = { list: 0, detail: 0 }

  begin(lane: RequestLane): number { return ++this.generations[lane] }
  invalidate(lane: RequestLane): void { this.generations[lane] += 1 }
  isCurrent(lane: RequestLane, generation: number): boolean { return this.generations[lane] === generation }
}
