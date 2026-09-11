/** Usage -> Traces navigation is scoped to the upstream: the `::tool` suffix is dropped and Traces is searched by upstream name. */
export function usageTraceHref(target: string): string {
  const separator = target.indexOf('::')
  const upstream = separator < 0 ? target : target.slice(0, separator)
  return `/traces/?search=${encodeURIComponent(upstream)}`
}
