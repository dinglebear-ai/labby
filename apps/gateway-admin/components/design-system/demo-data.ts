
export const auroraColorTokens = [
  {
    label: 'Aurora page background',
    value: '#07131c',
    className: 'bg-aurora-page-bg',
    textClassName: 'text-aurora-text-primary',
  },
  {
    label: 'Aurora panel medium',
    value: '#102330',
    className: 'bg-aurora-panel-medium',
    textClassName: 'text-aurora-text-primary',
  },
  {
    label: 'Aurora control surface',
    value: '#0c1a24',
    className: 'bg-aurora-control-surface',
    textClassName: 'text-aurora-text-primary',
  },
  {
    label: 'Aurora accent',
    value: '#29b6f6',
    className: 'bg-aurora-accent-primary',
    textClassName: 'text-aurora-page-bg',
  },
  {
    label: 'Aurora warn',
    value: '#c6a36b',
    className: 'bg-aurora-warn',
    textClassName: 'text-aurora-page-bg',
  },
  {
    label: 'Aurora error',
    value: '#c78490',
    className: 'bg-aurora-error',
    textClassName: 'text-aurora-page-bg',
  },
  {
    label: 'Aurora success',
    value: '#7dd3c7',
    className: 'bg-aurora-success',
    textClassName: 'text-aurora-page-bg',
  },
] as const

export const typeRamp = [
  {
    label: 'Display 1',
    className: 'font-display text-4xl font-semibold tracking-[-0.05em] text-aurora-text-primary',
    preview: '72',
  },
  {
    label: 'Display 2',
    className: 'font-display text-3xl font-semibold tracking-[-0.04em] text-aurora-text-primary',
    preview: 'Gateway Fleet',
  },
  {
    label: 'Metric Display',
    className: 'font-display text-2xl font-semibold tracking-[-0.05em] text-aurora-accent-strong tabular-nums',
    preview: '189',
  },
  {
    label: 'Body',
    className: 'text-base text-aurora-text-primary',
    preview: 'Default interface copy tuned for readable admin panels.',
  },
  {
    label: 'Control',
    className: 'text-sm font-medium text-aurora-text-primary',
    preview: 'Primary action',
  },
  {
    label: 'Dense Data',
    className: 'font-mono text-xs leading-6 text-aurora-text-muted',
    preview: 'gateway.request.start',
  },
  {
    label: 'Eyebrow',
    className: 'text-[11px] font-medium uppercase tracking-[0.18em] text-aurora-text-muted',
    preview: 'Sandbox heading',
  },
] as const

export const spacingScale = [
  { label: 'Space 2', value: '0.5rem' },
  { label: 'Space 4', value: '1rem' },
  { label: 'Space 6', value: '1.5rem' },
  { label: 'Space 8', value: '2rem' },
] as const

export const radiusScale = [
  { label: 'Radius 1', className: 'rounded-aurora-1', value: '14px' },
  { label: 'Radius 2', className: 'rounded-aurora-2', value: '18px' },
  { label: 'Radius 3', className: 'rounded-aurora-3', value: '22px' },
] as const

export const elevationTiers = [
  {
    label: 'Tier 1 lift',
    description: 'Default panels and cards with medium emphasis.',
    className: 'shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]',
    emphasized: false,
  },
  {
    label: 'Tier 2 lift',
    description: 'Pinned inspectors and hero surfaces with stronger separation.',
    className: 'shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]',
    emphasized: true,
  },
] as const

export const environmentOptions = [
  { value: 'preview', label: 'Preview cluster' },
  { value: 'prod', label: 'Production' },
  { value: 'edge', label: 'Edge canary' },
] as const

export const pillFilterOptions = [
  'Healthy only',
  'Warnings',
  'Protected routes',
  'Paused rollouts',
] as const

export const accessModeOptions = [
  { value: 'session', label: 'Session auth' },
  { value: 'token', label: 'Bearer token' },
  { value: 'mock', label: 'Mock data' },
] as const

export const feedbackModes = [
  'loading',
  'empty',
  'success',
  'warning',
  'error',
] as const

export const navigationTabs = [
  'Overview',
  'Policies',
  'Logs',
] as const

export const sidebarPreviewItems = [
  { label: 'Overview', active: false },
  { label: 'Gateways', active: true },
  { label: 'Logs', active: false },
] as const

export const metricCards = [
  { label: 'Connected gateways', value: '18', detail: 'Across preview and prod clusters' },
  { label: 'Warnings', value: '4', detail: 'Need operator review before rollout' },
  { label: 'Published tools', value: '73', detail: 'Visible to downstream clients' },
] as const

export const denseRows = [
  {
    gateway: 'plex-prod',
    transport: 'streamable_http',
    tools: '18 exposed',
    health: 'Healthy',
  },
  {
    gateway: 'edge-lab',
    transport: 'stdio',
    tools: '9 exposed',
    health: 'Warning',
  },
  {
    gateway: 'preview-ssh',
    transport: 'sse',
    tools: '24 exposed',
    health: 'Healthy',
  },
  {
    gateway: 'radarr-main',
    transport: 'streamable_http',
    tools: '12 exposed',
    health: 'Healthy',
  },
  {
    gateway: 'sonarr-main',
    transport: 'streamable_http',
    tools: '11 exposed',
    health: 'Healthy',
  },
  {
    gateway: 'prowlarr-edge',
    transport: 'sse',
    tools: '7 exposed',
    health: 'Error',
  },
  {
    gateway: 'qbit-seed',
    transport: 'stdio',
    tools: '5 exposed',
    health: 'Warning',
  },
  {
    gateway: 'tailscale-gate',
    transport: 'streamable_http',
    tools: '22 exposed',
    health: 'Healthy',
  },
] as const

export const keyValueBlocks = [
  { label: 'Gateway ID', value: 'gw_01HZZP7D4JQY' },
  { label: 'Last deploy', value: '2026-04-17 22:14 EDT' },
  { label: 'Operator', value: 'labby-admin@example.com' },
  { label: 'Auth mode', value: 'Session auth' },
] as const


export const toolbarSummaryChips = [
  { label: '18 configured', tone: 'default' },
  { label: '14 healthy', tone: 'success' },
  { label: '4 warnings', tone: 'warning' },
  { label: '73 tools', tone: 'info' },
] as const
