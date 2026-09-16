import { accessActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'
import type { OrganizationBootstrapCreateResponse } from './setup-client.ts'

export class AccessApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'AccessApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

export interface TeamSnapshot {
  team_id: string
  name: string
  status: string
  role: 'owner' | 'admin' | 'member' | null
  policy_epoch: number
  membership_epoch: number
  global_revision: number
}

export interface TeamInvitationAcceptOutcome {
  team_id: string
  principal_id: string
  role: 'owner' | 'admin' | 'member'
  status: string
  membership_epoch: number
  organization_profile?: OrganizationBootstrapCreateResponse
}

export interface TeamInvitationCreateOutcome {
  team_id: string
  role: 'owner' | 'admin' | 'member'
  status: string
  team_membership_epoch: number
  expires_at: number
  token: string
}

function accessAction<T>(action: string, params: Record<string, unknown>, signal?: AbortSignal): Promise<T> {
  return performServiceAction<T, AccessApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Access',
    url: accessActionUrl(),
    createError: (message, status, code, param) => new AccessApiError(message, status, code, param),
  })
}

export const accessApi = {
  listTeams(signal?: AbortSignal): Promise<{ teams: TeamSnapshot[] }> {
    return accessAction<{ teams: TeamSnapshot[] }>('access.team.list', {}, signal)
  },

  acceptTeamInvitation(token: string, signal?: AbortSignal): Promise<TeamInvitationAcceptOutcome> {
    return accessAction<TeamInvitationAcceptOutcome>('access.team_invitation.accept', { token: token.trim() }, signal)
  },

  createTeamInvitation(
    teamId: string,
    email: string,
    role: TeamInvitationCreateOutcome['role'],
    ttlSeconds = 7 * 24 * 60 * 60,
    signal?: AbortSignal,
  ): Promise<TeamInvitationCreateOutcome> {
    return accessAction<TeamInvitationCreateOutcome>(
      'access.team_invitation.create',
      { team_id: teamId, email: email.trim(), role, ttl_seconds: ttlSeconds },
      signal,
    )
  },
}
