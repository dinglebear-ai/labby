import type { OAuthConnectState } from '@/lib/types/upstream-oauth'

export interface OAuthProbeResult {
  oauth_discovered: boolean
  upstream: string
  issuer?: string
  scopes?: string[]
  registration_strategy?: string
}

export interface GatewayFormUiState {
  jsonDrawerOpen: boolean
  isSaving: boolean
  isTesting: boolean
  saveError: string | null
  errors: Record<string, string>
  oauthState: OAuthConnectState
  oauthProbed: OAuthProbeResult | null
  isProbing: boolean
}

export const initialGatewayFormUiState: GatewayFormUiState = {
  jsonDrawerOpen: false,
  isSaving: false,
  isTesting: false,
  saveError: null,
  errors: {},
  oauthState: { kind: 'idle' },
  oauthProbed: null,
  isProbing: false,
}

export interface GatewayFormUiAction {
  type: 'set'
  key: keyof GatewayFormUiState
  value: GatewayFormUiState[keyof GatewayFormUiState]
}

export function gatewayFormUiReducer(
  state: GatewayFormUiState,
  action: GatewayFormUiAction,
): GatewayFormUiState {
  const current = state[action.key]
  if (Object.is(current, action.value) || shallowEqual(current, action.value)) return state
  return { ...state, [action.key]: action.value }
}

function shallowEqual(left: unknown, right: unknown) {
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false
  const leftEntries = Object.entries(left)
  const rightEntries = Object.entries(right)
  const rightRecord = Object.fromEntries(rightEntries)
  return leftEntries.length === rightEntries.length
    && leftEntries.every(([key, value]) => Object.is(value, rightRecord[key]))
}
