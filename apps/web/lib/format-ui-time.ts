const DATE_FORMATTER = new Intl.DateTimeFormat('en-US', {
  year: 'numeric',
  month: 'short',
  day: 'numeric',
  timeZone: 'UTC',
  timeZoneName: 'short',
})

const DATE_TIME_FORMATTER = new Intl.DateTimeFormat('en-US', {
  year: 'numeric',
  month: 'short',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
  hour12: true,
  timeZone: 'UTC',
  timeZoneName: 'short',
})

const TIME_FORMATTER = new Intl.DateTimeFormat('en-US', {
  hour: 'numeric',
  minute: '2-digit',
  hour12: true,
  timeZone: 'UTC',
  timeZoneName: 'short',
})

const RELATIVE_TIME_FORMATTER = new Intl.RelativeTimeFormat('en-US', {
  numeric: 'auto',
})

const RELATIVE_THRESHOLDS: Array<{ unit: Intl.RelativeTimeFormatUnit; ms: number }> = [
  { unit: 'year', ms: 365 * 24 * 60 * 60 * 1000 },
  { unit: 'month', ms: 30 * 24 * 60 * 60 * 1000 },
  { unit: 'day', ms: 24 * 60 * 60 * 1000 },
  { unit: 'hour', ms: 60 * 60 * 1000 },
  { unit: 'minute', ms: 60 * 1000 },
  { unit: 'second', ms: 1000 },
]

function parseDate(value: string | number | Date | null | undefined): Date | null {
  if (value == null) return null
  const parsed = value instanceof Date ? value : new Date(value)
  return Number.isNaN(parsed.getTime()) ? null : parsed
}

export function formatUiDate(value: string | number | Date | null | undefined): string {
  const parsed = parseDate(value)
  return parsed ? DATE_FORMATTER.format(parsed) : 'Unknown'
}

export function formatUiDateTime(value: string | number | Date | null | undefined): string {
  const parsed = parseDate(value)
  return parsed ? DATE_TIME_FORMATTER.format(parsed) : 'Unknown'
}

export function formatUiTime(value: string | number | Date | null | undefined): string {
  const parsed = parseDate(value)
  return parsed ? TIME_FORMATTER.format(parsed) : 'Unknown'
}

export function formatUiRelativeTime(
  value: string | number | Date | null | undefined,
  now: number = Date.now(),
): string {
  const parsed = parseDate(value)
  if (!parsed) return 'Unknown'
  const diffMs = parsed.getTime() - now
  const absMs = Math.abs(diffMs)
  for (const { unit, ms } of RELATIVE_THRESHOLDS) {
    if (absMs >= ms || unit === 'second') {
      const value = Math.round(diffMs / ms)
      return RELATIVE_TIME_FORMATTER.format(value, unit)
    }
  }
  return RELATIVE_TIME_FORMATTER.format(0, 'second')
}
