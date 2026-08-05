export interface UpstreamEntry {
  name: string
}

export interface GoogleCredentialBrokerStatus {
  account_selector_configured: boolean
  provider_generation?: number
  client_bound: boolean
  required_scopes: string[]
  granted_scopes: string[]
  missing_scopes: string[]
}

export interface UpstreamOauthStatus {
  authenticated: boolean
  upstream: string
  credential_source: 'dedicated' | 'google_provider'
  google_credential_broker?: GoogleCredentialBrokerStatus
  expires_within_5m: boolean
  state?: 'connected' | 'expiring' | 'expired' | 'refresh_failed' | 'scope_upgrade_required' | 'discovery_failed' | 'disconnected'
  access_token_expires_at?: number
  seconds_until_expiry?: number
  refresh_token_present?: boolean
  refresh_attempted?: boolean
  refreshed?: boolean
  refresh_error_kind?: string
  refresh_error?: string
  discovery_checked?: boolean
  discovered_tool_count?: number
  exposed_tool_count?: number
  discovery_error?: string
}

export interface StartResponse {
  authorization_url: string
}

export interface ProbeResponse {
  upstream: string
  url: string
  oauth_discovered: boolean
  issuer?: string
  scopes?: string[]
  registration_strategy?: 'dynamic' | 'preregistered' | 'client_metadata_document'
}

export type OAuthConnectState =
  | { kind: 'idle' }
  | { kind: 'probing' }
  | { kind: 'discovered'; upstream: string; issuer?: string; scopes?: string[] }
  | { kind: 'blocked'; upstream: string; issuer?: string; scopes?: string[] }
  | { kind: 'authorizing'; upstream: string }
  | { kind: 'connected'; upstream: string; registration_strategy: string; scopes?: string[] }
  | { kind: 'error'; message: string }
