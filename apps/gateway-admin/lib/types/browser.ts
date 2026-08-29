export interface BrowserIdentity {
  id: string
  display_name: string
  extension_id: string
  paired_at: number
  last_seen_at: number | null
  revoked_at: number | null
  connected: boolean
}

export interface BrowserPairing {
  id: string
  display_name: string
  extension_id: string
  status: 'pending' | 'approved' | 'rejected' | 'expired'
  expires_at: number
  browser_id: string | null
}

export interface BrowserToolDescriptor {
  name: string
  description: string
  input_schema: unknown
  annotations: unknown
}

export interface BrowserSession {
  id: string
  browser_id: string
  tab_id: number
  document_id: string
  origin: string
  sanitized_path: string
  page_title: string
  catalog_revision: number
  catalog_fingerprint: string
  tools: BrowserToolDescriptor[]
  enabled: boolean
  status: 'active' | 'replaced' | 'closed'
  last_seen_at: number
}

export interface BrowserStatusResponse {
  available: boolean
  database: string
  connected_browser_ids: string[]
}

export interface BrowserListResponse { browsers: BrowserIdentity[] }
export interface BrowserPairingListResponse { pairings: BrowserPairing[] }
export interface BrowserSessionListResponse { sessions: BrowserSession[] }
