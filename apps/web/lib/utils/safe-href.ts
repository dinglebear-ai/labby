/**
 * Sanitize an external URL for use in href attributes.
 * Only https: and http: schemes are permitted.
 * Returns undefined for null, undefined, unparseable, or non-http(s) URLs.
 */
export function safeHref(url: string | undefined | null): string | undefined {
  if (!url) return undefined
  try {
    const parsed = new URL(url)
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') return undefined
    return url
  } catch {
    return undefined
  }
}
