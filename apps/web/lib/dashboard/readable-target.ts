/** Presentation only; raw target identifiers remain the action/detail keys. */
export function readableTarget(identifier: string): string {
  const [target, ...qualifiers] = identifier.split(' · ')
  const split = target.indexOf('::')
  const server = split < 0 ? '' : target.slice(0, split)
  const raw = split < 0 ? target : target.slice(split + 2)
  // Resource identifiers can contain nested upstream URIs. The final path
  // segment is the useful display name; callers retain the full ID in details.
  const name = raw.includes('://') ? raw.split(/[?#]/, 1)[0].split('/').filter(Boolean).at(-1) ?? raw : raw
  const words = name.replace(/([a-z\d])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ').replace(/\s+/g, ' ').trim()
  const label = words ? words[0].toUpperCase() + words.slice(1) : raw
  const operations: Record<string, string> = {
    'resource.read': 'Read resource',
    'resources.list': 'List resources',
    'prompt.get': 'Get prompt',
    'prompts.list': 'List prompts',
  }
  return [label, server, ...qualifiers.map(qualifier => operations[qualifier] ?? qualifier)].filter(Boolean).join(' · ')
}
