import type { TransportType } from '@/lib/types/gateway'
import type { GatewayAuthMode } from '@/lib/gateway-protected-route'
import type { OAuthConnectState } from '@/lib/types/upstream-oauth'

export function shouldAutoConnectOauth({
  open,
  isEditing,
  transport,
  authMode,
  oauthDiscovered,
  upstream,
}: {
  open: boolean
  isEditing: boolean
  transport: TransportType
  authMode: GatewayAuthMode
  oauthDiscovered: boolean
  upstream?: string
}) {
  return open
    && !isEditing
    && transport === 'http'
    && authMode === 'none'
    && oauthDiscovered
    && !!upstream?.trim()
}

export function oauthConnectButtonLabel(state: OAuthConnectState) {
  if (state.kind === 'probing') return 'Detecting OAuth...'
  if (state.kind === 'authorizing') return 'Waiting...'
  if (state.kind === 'blocked') return 'Click to authorize'
  return 'Connect via OAuth'
}
