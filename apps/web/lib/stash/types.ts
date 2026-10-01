export interface StashFile {
  file_id: string
  uri: string
  folder?: string
  content_type?: string
  display_name: string
  size_bytes: number
  created_at: number
  updated_at: number
  owned: boolean
}

export interface StashPage {
  files: StashFile[]
  next_cursor: string | null
}

export interface StashStats {
  owned_file_count: number
  owned_shared_file_count: number
  owned_committed_bytes: number
  owned_reserved_bytes: number
}

export interface StashGrant {
  grant_id: string
  file_id: string
  grantee_principal_id: string
  created_at: number
}

export interface GrantPage {
  grants: StashGrant[]
  next_cursor: string | null
}

export interface StashFolder { folder: string; file_count: number }
export interface StashFolderPage { folders: StashFolder[]; next_cursor: string | null }
