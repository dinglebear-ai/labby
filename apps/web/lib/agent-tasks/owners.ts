import { assertGatewayAuthorityCurrent, captureGatewayAuthority } from '@/lib/api/gateway-request'
import type { AuthoritySnapshot } from '@/lib/auth/authority'
import type { OwnerKind } from './client'

export type AgentOwnerChoice = { kind: OwnerKind; id: string; label: string; key: string }

export function agentOwnerChoices(
  authority: AuthoritySnapshot | undefined,
  teamNames: ReadonlyMap<string, string>,
  projectNames: ReadonlyMap<string, string>,
): AgentOwnerChoice[] {
  if (!authority?.capabilities.includes('scope.create')) return []
  return [
    { kind: 'personal', id: authority.principalId, label: 'Personal workspace', key: 'personal' },
    ...authority.teams.map((team) => ({
      kind: 'team' as const,
      id: team.id,
      label: teamNames.get(team.id) ?? `Team ${team.id}`,
      key: `team:${team.id}`,
    })),
    ...authority.projects.filter((project) => project.role !== 'viewer').map((project) => ({
      kind: 'project' as const,
      id: project.id,
      label: projectNames.get(project.id) ?? project.name ?? `Project ${project.id}`,
      key: `project:${project.id}`,
    })),
  ]
}

export async function listTeamNames(signal?: AbortSignal): Promise<Map<string, string>> {
  const authority = captureGatewayAuthority(signal)
  try {
    const response = await fetch('/v1/access/admin/', {
      method: 'POST',
      credentials: 'include',
      cache: 'no-store',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ action: 'access.team.list', params: {} }),
      signal: authority.signal,
    })
    if (!response.ok) throw new Error(`Could not load team names (${response.status}).`)
    const value = await response.json() as { teams?: Array<{ team_id?: unknown; name?: unknown }> }
    if (!Array.isArray(value.teams)) throw new Error('Team list response is invalid.')
    assertGatewayAuthorityCurrent(authority.generation)
    return new Map(value.teams.filter((team) => typeof team.team_id === 'string' && typeof team.name === 'string')
      .map((team) => [team.team_id as string, team.name as string]))
  } finally {
    authority.finish()
  }
}
