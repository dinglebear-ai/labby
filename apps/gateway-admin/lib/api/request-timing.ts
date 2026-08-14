export type RequestTimingReport = {
  name: string
  outcome: 'success' | 'error'
  elapsedMs: number
}

type TimingReporter = (report: RequestTimingReport) => void

function reportToBrowser(report: RequestTimingReport) {
  console.info('[labby.performance]', report)
}

export async function withRequestTiming<T>(
  name: string,
  request: () => Promise<T>,
  report: TimingReporter = reportToBrowser,
): Promise<T> {
  const startedAt = performance.now()
  let outcome: RequestTimingReport['outcome'] = 'success'
  try {
    return await request()
  } catch (error) {
    outcome = 'error'
    throw error
  } finally {
    report({
      name,
      outcome,
      elapsedMs: Math.round((performance.now() - startedAt) * 10) / 10,
    })
  }
}
