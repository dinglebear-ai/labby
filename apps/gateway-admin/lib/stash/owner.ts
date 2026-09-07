import type { AuthoritySnapshot } from '@/lib/auth/authority'

/** Owner selection File Stash accepts: a Team stash or the principal's own stash. */
export type StashOwnerSelection = { kind: 'team' | 'personal'; id: string }

export type StashOwnerResolution =
  /** `owner` is undefined when the session carries no authority projection; the server then applies its own default. */
  | { ok: true; owner: StashOwnerSelection | undefined }
  | { ok: false; reason: 'installation' | 'project_without_team' }

/**
 * The one place that maps the active workspace onto a File Stash owner.
 * Installation workspaces and Project workspaces without a bound Team have no
 * stash of their own; they must surface as an explicit unsupported-workspace
 * state instead of silently reading the principal's personal stash.
 */
export function resolveStashOwner(authority: AuthoritySnapshot | undefined): StashOwnerResolution {
  if (!authority) return { ok: true, owner: undefined }
  switch (authority.activeOwner.kind) {
    case 'team':
      return { ok: true, owner: { kind: 'team', id: authority.activeOwner.id } }
    case 'project':
      return authority.activeTeamId
        ? { ok: true, owner: { kind: 'team', id: authority.activeTeamId } }
        : { ok: false, reason: 'project_without_team' }
    case 'personal':
      return { ok: true, owner: { kind: 'personal', id: authority.principalId } }
    case 'installation':
      return { ok: false, reason: 'installation' }
  }
}

export function stashOwnerUnsupportedMessage(reason: Extract<StashOwnerResolution, { ok: false }>['reason']): string {
  return reason === 'installation'
    ? 'File Stash is not available in the installation workspace. Switch to a Personal or Team workspace.'
    : 'This Project is not bound to a Team, so it has no File Stash. Switch to a Personal or Team workspace.'
}
