import { Layers3, createLucideIcon, type LucideIcon } from 'lucide-react'

// Fixed, reviewed glyph geometry from the approved Gateway Console export.
// Keep the Lucide rendering/accessibility contract; never load SVG from catalog data.
export const DiscoverPublisherVerifiedIcon = createLucideIcon('DiscoverPublisherVerified', [
  ['path', { d: 'm12 15 2 2 4-4', key: 'check' }],
  ['path', { d: 'M21 12a9 9 0 1 1-6.2-8.5', key: 'circle' }],
])
const Server = createLucideIcon('DiscoverServer', [
  ['rect', { x: '3', y: '4', width: '18', height: '7', rx: '2', key: 'upper' }],
  ['rect', { x: '3', y: '13', width: '18', height: '7', rx: '2', key: 'lower' }],
  ['path', { d: 'M7 7.5h.01M7 16.5h.01', key: 'lights' }],
])
const Activity = createLucideIcon('DiscoverActivity', [
  ['path', { d: 'M3 12h4l3-8 4 16 3-8h4', key: 'signal' }],
])
const Shield = createLucideIcon('DiscoverShield', [
  ['path', { d: 'M12 2 4 6v6c0 5 3.4 8.6 8 10 4.6-1.4 8-5 8-10V6z', key: 'shield' }],
])
const Terminal = createLucideIcon('DiscoverTerminal', [
  ['path', { d: 'm5 8 4 4-4 4M13 16h6', key: 'command' }],
])
const SquareCode = createLucideIcon('DiscoverCode', [
  ['rect', { x: '3', y: '3', width: '18', height: '18', rx: '2', key: 'frame' }],
  ['path', { d: 'm9 10-2 2 2 2m6-4 2 2-2 2', key: 'brackets' }],
])
const Bot = createLucideIcon('DiscoverBot', [
  ['rect', { x: '4', y: '7', width: '16', height: '12', rx: '3', key: 'body' }],
  ['path', { d: 'M12 3v4M9 13h.01M15 13h.01', key: 'face' }],
])
const MessageSquareText = createLucideIcon('DiscoverPrompt', [
  ['path', { d: 'M21 15a2 2 0 0 1-2 2H8l-5 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2zM8 9h8M8 13h5', key: 'prompt' }],
])
const Plug = createLucideIcon('DiscoverPlug', [
  ['path', { d: 'M9 3v4M15 3v4', key: 'pins' }],
  ['rect', { x: '5', y: '7', width: '14', height: '9', rx: '2', key: 'body' }],
  ['path', { d: 'M9 16v3a3 3 0 0 0 6 0v-3', key: 'loop' }],
])
const Loadout = createLucideIcon('DiscoverLoadout', [
  ['path', { d: 'm12 2 9 5-9 5-9-5zM3 12l9 5 9-5M3 17l9 5 9-5', key: 'layers' }],
])
const Extension = createLucideIcon('DiscoverExtension', [
  ['rect', { x: '3', y: '3', width: '8', height: '8', rx: '2', key: 'source' }],
  ['rect', { x: '13', y: '13', width: '8', height: '8', rx: '2', key: 'target' }],
  ['path', { d: 'M11 7h4a2 2 0 0 1 2 2v4', key: 'connection' }],
])
const Hook = createLucideIcon('DiscoverHook', [
  ['path', { d: 'M7 3v10a5 5 0 0 0 10 0V8M17 8V5M14 5h6', key: 'hook' }],
])

const kinds: Record<string, { family: string; icon: LucideIcon }> = {
  mcp: { family: 'Protocol', icon: Server },
  acp: { family: 'Protocol', icon: Activity },
  skill: { family: 'Capability', icon: Shield },
  command: { family: 'Capability', icon: Terminal },
  snippet: { family: 'Capability', icon: SquareCode },
  agent: { family: 'Authored', icon: Bot },
  prompt: { family: 'Authored', icon: MessageSquareText },
  plugin: { family: 'Bundle', icon: Plug },
  extension: { family: 'Bundle', icon: Extension },
  loadout: { family: 'Bundle', icon: Loadout },
  hook: { family: 'Guard', icon: Hook },
}

const colors: Record<string, string> = {
  mcp: 'var(--aurora-protocol)', acp: 'var(--aurora-protocol)',
  skill: 'var(--aurora-accent-primary)', command: 'var(--aurora-success)',
  snippet: 'var(--aurora-accent-primary)', agent: 'var(--aurora-accent-pink-deep)',
  prompt: 'var(--aurora-accent-pink-deep)', plugin: 'var(--aurora-success)',
  extension: 'var(--aurora-success)', loadout: 'var(--aurora-success)', hook: 'var(--aurora-warn)',
}

const foregrounds: Record<string, string> = {
  mcp: 'var(--aurora-protocol-strong)', acp: 'var(--aurora-protocol-strong)',
  skill: 'var(--aurora-accent-strong)', snippet: 'var(--aurora-accent-strong)',
  agent: 'var(--aurora-accent-pink)', prompt: 'var(--aurora-accent-pink-strong)',
}

/** Presentation taxonomy only; this never implies runtime support or permissions. */
export function discoverKindPresentation(kind: string) {
  const key = kind.toLowerCase()
  const tone = Object.hasOwn(colors, key) ? colors[key] : 'var(--aurora-text-muted)'
  const color = Object.hasOwn(foregrounds, key) ? foregrounds[key] : tone
  return {
    ...(Object.hasOwn(kinds, key) ? kinds[key] : { family: null, icon: Layers3 }),
    color,
    tone,
    iconStyle: {
      color,
      backgroundColor: `color-mix(in srgb, ${tone} 10%, transparent)`,
      borderColor: `color-mix(in srgb, ${tone} 30%, transparent)`,
    },
  }
}
