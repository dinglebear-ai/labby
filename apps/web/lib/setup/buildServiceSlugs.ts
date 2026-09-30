// Static catalog of every service slug that should pre-render under
// /settings/services/[service]/. Required by Next.js `output: 'export'`
// — `generateStaticParams()` must enumerate every slug at build time.
//
// Keep this static export list aligned with the service configuration UI, then
// run `pnpm build` after changing it. Keep entries alphabetical so diffs read
// cleanly.

export const SERVICE_SLUGS = [
  'adguard',
  'apprise',
  'arcane',
  'beads',
  'bytestash',
  'deploy',
  'dozzle',
  'freshrss',
  'fs',
  'glances',
  'gotify',
  'immich',
  'linkding',
  'loggifly',
  'memos',
  'navidrome',
  'neo4j',
  'notebooklm',
  'openacp',
  'openai',
  'pihole',
  'qdrant',
  'scrutiny',
  'tailscale',
  'tei',
  'unifi',
  'unraid',
  'uptime_kuma',
] as const

export type ServiceSlug = (typeof SERVICE_SLUGS)[number]

export function isKnownService(slug: string): slug is ServiceSlug {
  return (SERVICE_SLUGS as readonly string[]).includes(slug)
}
