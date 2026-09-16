export const TEAM_INVITATION_TOKEN_SESSION_KEY = 'labby.team-invitation.token'

const INVITATION_TOKEN_RE = /^[0-9a-f]{64}$/i

export function normalizeTeamInvitationToken(value: string | null | undefined): string | undefined {
  const token = value?.trim()
  return token && INVITATION_TOKEN_RE.test(token) ? token.toLowerCase() : undefined
}

export function teamInvitationTokenFromHash(hash: string): string | undefined {
  if (!hash.startsWith('#')) return undefined
  const params = new URLSearchParams(hash.slice(1))
  return normalizeTeamInvitationToken(params.get('invite'))
}

export function teamInvitationHref(origin: string, token: string): string {
  const normalized = normalizeTeamInvitationToken(token)
  if (!normalized) throw new Error('Invitation token must be exactly 32 bytes of hexadecimal data.')
  return origin.replace(/\/$/, '') + '/#invite=' + normalized
}
