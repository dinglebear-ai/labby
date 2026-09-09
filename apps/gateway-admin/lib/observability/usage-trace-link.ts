/** Match the reference's upstream-scoped Usage -> Traces navigation. */
export function usageTraceHref(target: string): string {
  const separator = target.indexOf('::')
  const upstream = separator < 0 ? target : target.slice(0, separator)
  return `/traces/?search=${encodeURIComponent(upstream)}`
}
