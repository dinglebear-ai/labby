export function updateLibraryUrl(values: { artifact?: string | null; kind?: string; q?: string }) {
  const url = new URL(window.location.href)
  for (const [key, value] of Object.entries(values)) {
    if (value && value !== 'all') url.searchParams.set(key, value)
    else url.searchParams.delete(key)
  }
  if (url.href === window.location.href) return
  // These filters are client state. Next synchronizes native history updates
  // with useSearchParams without fetching a static-export route again.
  window.history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`)
}
